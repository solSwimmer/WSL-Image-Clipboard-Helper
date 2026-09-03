use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Mutex;
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_DIBV5, CF_HDROP};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use tracing::{info, warn};

#[link(name = "user32")]
extern "system" {
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
}

/// Cached image data.
struct ImageCache {
    /// Clipboard sequence number.
    seq: u32,
    /// PNG bytes.
    png_data: Vec<u8>,
    /// Windows path.
    win_path: PathBuf,
    /// WSL path.
    wsl_path: String,
}

/// Clipboard manager.
pub struct ClipboardManager {
    temp_dir: PathBuf,
    wsl_temp_dir: String,
    cache: Mutex<Option<ImageCache>>,
}

impl ClipboardManager {
    pub fn new(temp_dir: PathBuf) -> Self {
        // Precompute the WSL path (matches the AHK gWslTempDir optimization).
        let wsl_temp_dir = convert_path_to_wsl(&temp_dir.to_string_lossy());

        info!("WSL temp directory: {}", wsl_temp_dir);

        Self {
            temp_dir,
            wsl_temp_dir,
            cache: Mutex::new(None),
        }
    }

    /// Check whether the clipboard has an image (does not open the clipboard).
    pub fn has_image(&self) -> bool {
        unsafe {
            IsClipboardFormatAvailable(CF_BITMAP.0 as u32) != 0
                || IsClipboardFormatAvailable(CF_DIB.0 as u32) != 0
                || IsClipboardFormatAvailable(CF_DIBV5.0 as u32) != 0
        }
    }

    pub fn has_file_list(&self) -> bool {
        unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) != 0 }
    }

    pub fn read_file_list_for_paste(&self, allow_non_image_files: bool) -> Option<Vec<String>> {
        let paths = self.get_file_paths()?;
        let wsl_paths: Vec<String> = paths
            .iter()
            .filter(|path| allow_non_image_files || crate::paths::is_image_path(path))
            .map(|path| convert_path_to_wsl(path))
            .filter_map(|path| crate::paths::sanitize_pasted_path(&path))
            .collect();

        if wsl_paths.is_empty() {
            None
        } else {
            Some(wsl_paths)
        }
    }

    /// Current clipboard sequence number.
    fn get_sequence(&self) -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    /// Read the image and prepare paste data (with cache).
    /// Returns (win_path, wsl_path, png_data).
    pub fn read_image_for_paste(&self) -> Option<(PathBuf, String, Vec<u8>)> {
        let seq = self.get_sequence();

        // Check cache.
        if let Ok(cache) = self.cache.lock() {
            if let Some(ref cached) = *cache {
                if cached.seq == seq && seq != 0 {
                    info!("using cached image data (seq={})", seq);
                    return Some((
                        cached.win_path.clone(),
                        cached.wsl_path.clone(),
                        cached.png_data.clone(),
                    ));
                }
            }
        }

        // Read fresh data.
        let png_data = self.get_image_data()?;

        // Build filename and paths.
        let now = chrono::Local::now();
        let timestamp = now.format("%Y%m%d_%H%M%S_%3f").to_string();
        let filename = format!("clip_{}_{}.png", timestamp, std::process::id());

        let win_path = self.temp_dir.join(&filename);
        let wsl_path = if !self.wsl_temp_dir.is_empty() {
            format!("{}/{}", self.wsl_temp_dir, filename)
        } else {
            convert_path_to_wsl(&win_path.to_string_lossy())
        };

        // Update cache.
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(ImageCache {
                seq,
                png_data: png_data.clone(),
                win_path: win_path.clone(),
                wsl_path: wsl_path.clone(),
            });
        }

        Some((win_path, wsl_path, png_data))
    }

    fn get_file_paths(&self) -> Option<Vec<String>> {
        unsafe {
            if OpenClipboard(None).is_err() {
                return None;
            }

            let paths = if let Ok(h_data) = GetClipboardData(CF_HDROP.0 as u32) {
                Self::read_hdrop_paths(HDROP(h_data.0 as isize))
            } else {
                Vec::new()
            };

            CloseClipboard().ok();

            if paths.is_empty() {
                None
            } else {
                Some(paths)
            }
        }
    }

    /// Read image data and convert it to PNG.
    fn get_image_data(&self) -> Option<Vec<u8>> {
        unsafe {
            if OpenClipboard(None).is_err() {
                return None;
            }

            // Try to get DIB data.
            let dib_data = if let Ok(h_data) = GetClipboardData(CF_DIB.0 as u32) {
                Some(Self::read_dib_data(h_data))
            } else if let Ok(h_data) = GetClipboardData(CF_DIBV5.0 as u32) {
                Some(Self::read_dib_data(h_data))
            } else {
                CloseClipboard().ok();
                return None;
            };

            CloseClipboard().ok();

            if let Some(dib) = dib_data {
                if dib.is_empty() {
                    return None;
                }
                return Self::convert_dib_to_png(&dib);
            }

            None
        }
    }

    unsafe fn read_hdrop_paths(hdrop: HDROP) -> Vec<String> {
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut paths = Vec::new();

        for index in 0..count {
            let len = DragQueryFileW(hdrop, index, None);
            if len == 0 {
                continue;
            }

            let mut buffer = vec![0u16; len as usize + 1];
            let copied = DragQueryFileW(hdrop, index, Some(&mut buffer));
            if copied == 0 {
                warn!("failed to read clipboard file path: index={}", index);
                continue;
            }

            paths.push(String::from_utf16_lossy(&buffer[..copied as usize]));
        }

        paths
    }

    /// Read DIB bytes from the clipboard.
    unsafe fn read_dib_data(h_data: HANDLE) -> Vec<u8> {
        const MAX_DIB_SIZE: usize = 100 * 1024 * 1024;

        let h_global = HGLOBAL(h_data.0 as *mut std::ffi::c_void);
        let ptr = GlobalLock(h_global);

        if ptr.is_null() {
            return Vec::new();
        }

        let data = (|| {
            let global_size = GlobalSize(h_global);
            if global_size < BITMAPINFOHEADER_SIZE {
                return Vec::new();
            }

            let header = std::slice::from_raw_parts(ptr as *const u8, BITMAPINFOHEADER_SIZE);
            let info = read_bitmap_info(header);

            // Copy packed-struct fields to locals (avoid alignment issues).
            let bi_width = info.bi_width.unsigned_abs() as usize;
            let bi_height = info.bi_height.unsigned_abs() as usize;
            let bi_bit_count = info.bi_bit_count;
            let bi_size = info.bi_size;
            let bi_compression = info.bi_compression;
            let bi_clr_used = info.bi_clr_used;

            let expected_size = calculate_dib_copy_size(
                bi_size,
                bi_bit_count,
                bi_compression,
                bi_clr_used,
                bi_width,
                bi_height,
            )
            .unwrap_or(global_size);

            // Read size is capped by estimate, actual allocation, and the hard limit.
            let read_size = expected_size.min(global_size).min(MAX_DIB_SIZE);
            if read_size == 0 {
                return Vec::new();
            }

            let mut data = vec![0u8; read_size];
            std::ptr::copy_nonoverlapping(ptr as *const u8, data.as_mut_ptr(), read_size);
            data
        })();

        let _ = GlobalUnlock(h_global);

        data
    }

    /// Convert DIB bytes to PNG.
    fn convert_dib_to_png(dib_data: &[u8]) -> Option<Vec<u8>> {
        if dib_data.len() < std::mem::size_of::<BITMAPINFOHEADER>() {
            return None;
        }

        let info = read_bitmap_info(dib_data);

        // Copy packed-struct fields to locals (avoid alignment issues).
        let bi_width = info.bi_width;
        let bi_height = info.bi_height;
        let bi_bit_count = info.bi_bit_count;
        let bi_size = info.bi_size;
        let bi_compression = info.bi_compression;
        let bi_clr_used = info.bi_clr_used;

        // Only 24-bit and 32-bit DIB are supported.
        if bi_bit_count != 24 && bi_bit_count != 32 {
            tracing::warn!("unsupported bit depth: {} bits", bi_bit_count);
            return None;
        }

        if bi_width == 0 || bi_height == 0 {
            return None;
        }

        let width = bi_width.unsigned_abs() as usize;
        let height = bi_height.unsigned_abs() as usize;
        let bottom_up = bi_height > 0;

        let pixel_offset =
            calculate_dib_pixel_offset(bi_size, bi_bit_count, bi_compression, bi_clr_used)?;

        let channels = if bi_bit_count == 32 { 4 } else { 3 };
        let row_size = calculate_row_size(width, bi_bit_count)?;
        let image_size = row_size.checked_mul(height)?;
        let pixel_end = pixel_offset.checked_add(image_size)?;

        if dib_data.len() < pixel_end {
            return None;
        }

        let pixel_data = &dib_data[pixel_offset..pixel_end];
        let has_alpha = bi_bit_count == 32;

        let capacity = width.checked_mul(height)?.checked_mul(channels)?;
        let mut img_data = Vec::with_capacity(capacity);
        let mut alpha_any_nonzero = false;

        // Bottom-up DIB stores rows from bottom to top.
        for y in 0..height {
            let src_y = if bottom_up { height - 1 - y } else { y };
            let row_start = src_y * row_size;
            for x in 0..width {
                let pixel_start = row_start + x * channels;

                if pixel_start + channels <= pixel_data.len() {
                    // BGR(A) -> RGB(A)
                    let b = pixel_data[pixel_start];
                    let g = pixel_data[pixel_start + 1];
                    let r = pixel_data[pixel_start + 2];

                    img_data.push(r);
                    img_data.push(g);
                    img_data.push(b);

                    if has_alpha {
                        let a = pixel_data[pixel_start + 3];
                        if a != 0 {
                            alpha_any_nonzero = true;
                        }
                        img_data.push(a);
                    }
                }
            }
        }

        // The 4th byte of a 32-bit BI_RGB DIB is often reserved padding (always 0),
        // not real alpha. If every alpha is 0, treat the image as opaque (same
        // heuristic browsers use for clipboard DIB); otherwise the exported PNG
        // is fully transparent and AI tools see a "blank" image.
        if has_alpha && !alpha_any_nonzero {
            for a in img_data.iter_mut().skip(3).step_by(4) {
                *a = 255;
            }
        }

        // Encode PNG with the image crate.
        #[cfg(feature = "image-support")]
        {
            use image::{DynamicImage, ImageBuffer};

            let img: DynamicImage = if has_alpha {
                DynamicImage::ImageRgba8(
                    ImageBuffer::from_raw(width as u32, height as u32, img_data)?,
                )
            } else {
                DynamicImage::ImageRgb8(
                    ImageBuffer::from_raw(width as u32, height as u32, img_data)?,
                )
            };

            let mut buffer = Cursor::new(Vec::new());
            if img.write_to(&mut buffer, image::ImageFormat::Png).is_ok() {
                return Some(buffer.into_inner());
            }
        }

        #[cfg(not(feature = "image-support"))]
        {
            tracing::warn!("image-support feature is not enabled");
        }

        None
    }
}

/// Convert a Windows path to a WSL path.
pub fn convert_path_to_wsl(path_str: &str) -> String {
    let path_str = path_str.trim_matches('"');

    // Drive path "C:\path\to\file".
    if path_str.len() >= 3 && path_str.as_bytes()[1] == b':' && path_str.as_bytes()[2] == b'\\' {
        let drive = &path_str[0..1];
        let rest = path_str[3..].replace('\\', "/");
        let rest = rest.trim_start_matches('/');
        return format!("/mnt/{}/{}", drive.to_lowercase(), rest);
    }

    // Path without a backslash (e.g. D:\temp written as D:/temp).
    if path_str.len() >= 3 && path_str.as_bytes()[1] == b':' {
        let drive = &path_str[0..1];
        let rest = path_str[2..].replace('\\', "/");
        let rest = rest.trim_start_matches('/');
        return format!("/mnt/{}/{}", drive.to_lowercase(), rest);
    }

    String::new()
}

/// BITMAPINFOHEADER (subset of fields).
#[repr(C, packed)]
struct BITMAPINFOHEADER {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

const BITMAPINFOHEADER_SIZE: usize = std::mem::size_of::<BITMAPINFOHEADER>();
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;
const RGBQUAD_SIZE: usize = 4;

fn read_bitmap_info(data: &[u8]) -> BITMAPINFOHEADER {
    let header_size = BITMAPINFOHEADER_SIZE;
    let mut header = BITMAPINFOHEADER {
        bi_size: 0,
        bi_width: 0,
        bi_height: 0,
        bi_planes: 0,
        bi_bit_count: 0,
        bi_compression: 0,
        bi_size_image: 0,
        bi_x_pels_per_meter: 0,
        bi_y_pels_per_meter: 0,
        bi_clr_used: 0,
        bi_clr_important: 0,
    };

    if data.len() >= header_size {
        let bytes = &data[0..header_size.min(40)];
        let ptr = bytes.as_ptr() as *const BITMAPINFOHEADER;
        unsafe { std::ptr::copy_nonoverlapping(ptr, &mut header, 1) };
    }

    header
}

fn calculate_row_size(width: usize, bit_count: u16) -> Option<usize> {
    if width == 0 || bit_count == 0 {
        return None;
    }

    let row_bits = width.checked_mul(usize::from(bit_count))?;
    row_bits.checked_add(31)?.checked_div(32)?.checked_mul(4)
}

fn calculate_palette_size(bit_count: u16, clr_used: u32) -> Option<usize> {
    if bit_count > 8 {
        return Some(0);
    }

    let entries = if clr_used > 0 {
        usize::try_from(clr_used).ok()?
    } else {
        1usize.checked_shl(u32::from(bit_count))?
    };

    entries.checked_mul(RGBQUAD_SIZE)
}

fn calculate_dib_pixel_offset(
    bi_size: u32,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_clr_used: u32,
) -> Option<usize> {
    let header_size = usize::try_from(bi_size).ok()?;
    if header_size < BITMAPINFOHEADER_SIZE {
        return None;
    }

    // For BITMAPINFOHEADER + BI_BITFIELDS, masks sit between the header and pixels.
    let mask_size = if header_size == BITMAPINFOHEADER_SIZE
        && (bi_compression == BI_BITFIELDS || bi_compression == BI_ALPHABITFIELDS)
    {
        if bi_compression == BI_ALPHABITFIELDS {
            16
        } else {
            12
        }
    } else {
        0
    };

    let palette_size = calculate_palette_size(bi_bit_count, bi_clr_used)?;
    header_size.checked_add(mask_size)?.checked_add(palette_size)
}

fn calculate_dib_copy_size(
    bi_size: u32,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_clr_used: u32,
    width: usize,
    height: usize,
) -> Option<usize> {
    let pixel_offset = calculate_dib_pixel_offset(bi_size, bi_bit_count, bi_compression, bi_clr_used)?;
    let row_size = calculate_row_size(width, bi_bit_count)?;
    let image_size = row_size.checked_mul(height)?;
    pixel_offset.checked_add(image_size)
}

#[cfg(test)]
mod tests {
    use super::convert_path_to_wsl;

    #[test]
    fn converts_drive_paths() {
        assert_eq!(
            convert_path_to_wsl(r"C:\Users\me\clip.png"),
            "/mnt/c/Users/me/clip.png"
        );
        assert_eq!(
            convert_path_to_wsl(r#"D:\temp\file.png"#),
            "/mnt/d/temp/file.png"
        );
        assert_eq!(convert_path_to_wsl(r"C:/foo/bar"), "/mnt/c/foo/bar");
    }

    #[test]
    fn rejects_unc_and_empty() {
        assert_eq!(convert_path_to_wsl(r"\\server\share\file.png"), "");
        assert_eq!(convert_path_to_wsl(r"\\?\C:\foo.png"), "");
        assert_eq!(convert_path_to_wsl(""), "");
    }
}
