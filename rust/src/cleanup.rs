use anyhow::Result;
use chrono::{DateTime, Local};
use std::fs;
use std::path::Path;
use tracing::{info, warn};

/// Delete all PNG files in the temp directory on exit.
pub fn cleanup_temp_png(temp_dir: &Path) -> Result<()> {
    if !temp_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(temp_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !is_temp_image(&path) {
            continue;
        }

        if let Err(e) = fs::remove_file(&path) {
            warn!("Failed to delete temp file {}: {}", path.display(), e);
        } else {
            info!("Exit cleanup: {}", path.display());
        }
    }

    Ok(())
}

/// Delete temporary PNG files older than 30 minutes.
pub fn cleanup_old_files(temp_dir: &Path) -> Result<()> {
    let now = Local::now();

    if !temp_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(temp_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !is_temp_image(&path) {
            continue;
        }

        if let Ok(metadata) = entry.metadata() {
            if let Ok(modified) = metadata.modified() {
                let modified_time = DateTime::<Local>::from(modified);
                let minutes_old = now.signed_duration_since(modified_time).num_minutes();

                if minutes_old >= 30 {
                    if let Err(e) = fs::remove_file(&path) {
                        warn!("Failed to delete expired file {}: {}", path.display(), e);
                    } else {
                        info!("Deleted expired file: {}", path.display());
                    }
                }
            }
        }
    }

    Ok(())
}

fn is_temp_image(path: &Path) -> bool {
    path.extension()
        .map(|e| {
            let e = e.to_string_lossy();
            e.eq_ignore_ascii_case("png") || e.eq_ignore_ascii_case("partial")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::cleanup_temp_png;

    #[test]
    fn cleanup_temp_png_deletes_png_and_partial_files_only() {
        let temp_root = std::env::temp_dir().join(format!(
            "wsl_clipboard_cleanup_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).unwrap();

        let png_path = temp_root.join("clip_test.png");
        let upper_png_path = temp_root.join("clip_upper.PNG");
        let partial_path = temp_root.join("clip_test.png.partial");
        let txt_path = temp_root.join("keep.txt");
        std::fs::write(&png_path, b"png").unwrap();
        std::fs::write(&upper_png_path, b"png").unwrap();
        std::fs::write(&partial_path, b"partial").unwrap();
        std::fs::write(&txt_path, b"text").unwrap();

        cleanup_temp_png(&temp_root).unwrap();

        assert!(!png_path.exists());
        assert!(!upper_png_path.exists());
        assert!(!partial_path.exists());
        assert!(txt_path.exists());

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}
