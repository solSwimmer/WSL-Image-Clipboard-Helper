use anyhow::{bail, Result};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LMENU, VK_MENU, VK_RMENU, VK_SHIFT, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, PostMessageW, GUITHREADINFO,
    GUI_INMENUMODE,
};

use tracing::{info, warn};

use crate::config::ImeProtection;

/// HKL type alias (Win32 HKL is an isize).
pub type HKL = isize;

/// WM_INPUTLANGCHANGEREQUEST
const WM_INPUTLANGCHANGEREQUEST: u32 = 0x0050;

/// WM_IME_CONTROL and its sub-commands, used to close / restore the foreground
/// window's IME input state **without changing the keyboard layout**.
const WM_IME_CONTROL: u32 = 0x0283;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IMC_SETOPENSTATUS: usize = 0x0006;

/// LoadKeyboardLayoutW flags.
///
/// **Intentionally omit KLF_ACTIVATE (0x1)**: it would activate the English
/// layout on the current thread, and LoadKeyboardLayoutW already registers the
/// layout in the system IME list. Together those two flags are the direct cause
/// of the extra `ENG` entry in Win+Space after startup (issue #10).
/// KLF_NOTELLSHELL (0x80) stops the Shell from receiving HSHELL_LANGUAGE,
/// so the taskbar IME indicator is not woken up.
const KLF_NOTELLSHELL: u32 = 0x0080;

/// SendMessageTimeout flag: return immediately if the target thread is hung,
/// instead of stalling the hotkey thread.
const SMTO_ABORTIFHUNG: u32 = 0x0002;
/// Timeout (ms) for talking to the foreground window IME. The paste path is
/// latency-sensitive: drop protection rather than stall.
const IME_CONTROL_TIMEOUT_MS: u32 = 80;

/// English primary language ID (LANG_ENGLISH).
const LANG_ENGLISH: u16 = 0x09;
/// Full en-US language ID.
const LANGID_EN_US: u16 = 0x0409;

/// English layout this process loaded via LoadKeyboardLayoutW.
/// 0 means it was never loaded. Unload on exit so ENG is not left in the
/// user's system IME list.
static SELF_LOADED_ENGLISH_HKL: AtomicIsize = AtomicIsize::new(0);

#[link(name = "user32")]
extern "system" {
    fn LoadKeyboardLayoutW(pwszklid: *const u16, flags: u32) -> HKL;
    fn UnloadKeyboardLayout(hkl: HKL) -> i32;
    fn GetKeyboardLayout(id_thread: u32) -> HKL;
    fn GetKeyboardLayoutList(n_buff: i32, lp_list: *mut HKL) -> i32;
    fn SendMessageTimeoutW(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
        flags: u32,
        timeout: u32,
        result: *mut usize,
    ) -> isize;
}

#[link(name = "imm32")]
extern "system" {
    fn ImmGetDefaultIMEWnd(hwnd: isize) -> isize;
}

/// Input language ID from the low 16 bits of HKL (high 16 bits are the device handle).
fn langid_of(hkl: HKL) -> u16 {
    (hkl as usize & 0xFFFF) as u16
}

/// Primary language ID (low 10 bits of LANGID).
fn primary_lang_of(hkl: HKL) -> u16 {
    langid_of(hkl) & 0x3FF
}

/// Find an English layout among layouts **already loaded** by the system.
/// Query only; does not add any layout, so it has no side effects.
fn find_loaded_english_layout() -> Option<HKL> {
    unsafe {
        let count = GetKeyboardLayoutList(0, std::ptr::null_mut());
        if count <= 0 {
            return None;
        }

        let mut list: Vec<HKL> = vec![0; count as usize];
        let filled = GetKeyboardLayoutList(count, list.as_mut_ptr());
        if filled <= 0 {
            return None;
        }
        list.truncate(filled as usize);

        // Prefer an exact en-US match, then any English variant (en-GB etc. are also letter layouts).
        list.iter()
            .copied()
            .find(|hkl| langid_of(*hkl) == LANGID_EN_US)
            .or_else(|| {
                list.iter()
                    .copied()
                    .find(|hkl| primary_lang_of(*hkl) == LANG_ENGLISH)
            })
    }
}

/// Lazily resolve an English layout HKL.
///
/// Unlike the old implementation, this is **not called at startup**, and by
/// default it only reuses a layout the system already has. A real load happens
/// only when `allow_load = true` (`ime_protection = "layout-force"`), without
/// KLF_ACTIVATE, and [`unload_self_loaded_layout`] unloads it on exit.
fn resolve_english_layout(allow_load: bool) -> Option<HKL> {
    // 1) Reuse an English layout already installed (most English / multilingual users).
    if let Some(hkl) = find_loaded_english_layout() {
        return Some(hkl);
    }

    // 2) No English layout installed: skip the switch; never inject ENG by default.
    if !allow_load {
        return None;
    }

    // 3) Load only when explicitly allowed, and only once.
    let cached = SELF_LOADED_ENGLISH_HKL.load(Ordering::Relaxed);
    if cached != 0 {
        return Some(cached);
    }

    unsafe {
        let klid: Vec<u16> = "00000409\0".encode_utf16().collect();
        let hkl = LoadKeyboardLayoutW(klid.as_ptr(), KLF_NOTELLSHELL);
        if hkl == 0 {
            warn!("failed to load English keyboard layout; skipping IME guard");
            return None;
        }
        info!("lazily loaded English keyboard layout {:#x} (will unload on exit)", hkl);
        SELF_LOADED_ENGLISH_HKL.store(hkl, Ordering::Relaxed);
        Some(hkl)
    }
}

/// Unload an English layout this process loaded, so it is not left in the
/// user's IME switch list (issue #10).
pub fn unload_self_loaded_layout() {
    let hkl = SELF_LOADED_ENGLISH_HKL.swap(0, Ordering::Relaxed);
    if hkl == 0 {
        return;
    }
    unsafe {
        if UnloadKeyboardLayout(hkl) == 0 {
            warn!("failed to unload English keyboard layout {:#x}", hkl);
        } else {
            info!("unloaded English keyboard layout loaded by this process {:#x}", hkl);
        }
    }
}

/// Send WM_IME_CONTROL to the foreground window's default IME window.
/// Uses SendMessageTimeoutW so an unresponsive target cannot stall paste.
unsafe fn send_ime_control(ime_wnd: isize, sub_command: usize, lparam: isize) -> Option<usize> {
    let mut result: usize = 0;
    let ok = SendMessageTimeoutW(
        ime_wnd,
        WM_IME_CONTROL,
        sub_command,
        lparam,
        SMTO_ABORTIFHUNG,
        IME_CONTROL_TIMEOUT_MS,
        &mut result,
    );
    if ok == 0 {
        None
    } else {
        Some(result)
    }
}

/// Put text on the clipboard and trigger paste.
pub fn paste_text(text: &str) -> Result<()> {
    unsafe {
        // Open the clipboard, with a retry.
        if OpenClipboard(None).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Err(e) = OpenClipboard(None) {
                bail!("failed to open clipboard: {:?}", e);
            }
        }

        // Empty first, then set (must empty before set).
        if let Err(e) = EmptyClipboard() {
            CloseClipboard().ok();
            bail!("failed to empty clipboard: {:?}", e);
        }

        // Prepare UTF-16 data.
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = utf16.len() * std::mem::size_of::<u16>();

        // Allocate memory.
        let h_mem = match GlobalAlloc(GMEM_MOVEABLE, byte_len) {
            Ok(mem) => mem,
            Err(e) => {
                CloseClipboard().ok();
                bail!("failed to allocate clipboard memory: {:?}", e);
            }
        };

        let ptr = GlobalLock(h_mem);
        if ptr.is_null() {
            CloseClipboard().ok();
            bail!("failed to lock memory");
        }

        // Copy data.
        std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, byte_len);

        let _ = GlobalUnlock(h_mem);

        // Set clipboard data.
        if SetClipboardData(
            CF_UNICODETEXT.0 as u32,
            windows::Win32::Foundation::HANDLE(h_mem.0 as isize),
        )
        .is_err()
        {
            CloseClipboard().ok();
            bail!("failed to set clipboard data");
        }

        CloseClipboard().ok();
    }

    // Send the paste shortcut.
    send_ctrl_v()?;
    Ok(())
}

/// Menu-mask key, matching AHK's A_MenuMaskKey (vkFF).
/// When the hotkey fires, physical Alt is often still down; the target window
/// only sees Alt down (V is swallowed by the hotkey). Injecting Alt up next
/// makes some apps treat it as an Alt tap, open the menu bar, and steal focus,
/// so the following Ctrl+V pastes nothing. Inject a no-op key first to break
/// that detection.
const VK_MENU_MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xFF);

/// Whether a virtual key is currently physically held.
fn is_key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 }
}

/// Wait up to timeout_ms for physical modifiers (Alt/Shift) that would pollute
/// Ctrl+V to be released. A held Alt is re-pressed by keyboard auto-repeat, so
/// injecting key-up events is not always enough — a common cause of
/// "intermittent empty paste".
fn wait_modifiers_released(timeout_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while is_key_down(VK_MENU)
        || is_key_down(VK_LMENU)
        || is_key_down(VK_RMENU)
        || is_key_down(VK_SHIFT)
    {
        if std::time::Instant::now() >= deadline {
            warn!("timed out waiting for modifiers; injecting paste keys anyway");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Call immediately after the hotkey fires: insert the mask key before the user
/// releases Alt. RegisterHotKey swallows V, so the target window only sees
/// Alt down/up; if the mask key lands before physical Alt up, the window will
/// not treat it as an Alt tap that opens the menu bar. Must run before
/// clipboard/file I/O — the earlier, the better.
pub fn mask_alt_tap() {
    let inputs = [
        make_key_input(VK_MENU_MASK, false),
        make_key_input(VK_MENU_MASK, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// If the foreground thread is already in menu mode (fast Alt release can beat
/// the mask key), send Esc to leave the menu first; otherwise injected Ctrl+V
/// is eaten by the menu and paste does nothing.
fn dismiss_menu_mode_if_active() {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return;
        }
        let thread_id = GetWindowThreadProcessId(hwnd, None);
        if thread_id == 0 {
            return;
        }

        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread_id, &mut info).is_err() {
            return;
        }

        if (info.flags & GUI_INMENUMODE).0 != 0 {
            info!("foreground window is in menu mode; sending Esc before paste");
            let esc = [
                make_key_input(VK_ESCAPE, false),
                make_key_input(VK_ESCAPE, true),
            ];
            SendInput(&esc, std::mem::size_of::<INPUT>() as i32);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
}

/// Release all modifiers (Alt/Ctrl/Shift), matching AHK's NormalizeModifierStateBeforeSend.
pub fn release_all_modifiers() {
    let inputs = [
        // Mask key first, then Alt up, so an Alt tap does not open the window menu.
        make_key_input(VK_MENU_MASK, false),
        make_key_input(VK_MENU_MASK, true),
        make_key_input(VK_MENU, true),     // Alt up
        make_key_input(VK_LMENU, true),    // Left Alt up
        make_key_input(VK_RMENU, true),    // Right Alt (AltGr) up
        make_key_input(VK_CONTROL, true),  // Ctrl up
        make_key_input(VK_SHIFT, true),    // Shift up
    ];

    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Send the Ctrl+V paste shortcut (SendInput instead of keybd_event).
pub fn send_ctrl_v() -> Result<()> {
    // Give the user time to release the hotkey (if Alt from Alt+V / Alt+Enter
    // is still down, injected Ctrl+V is seen as Ctrl+Alt+V and does nothing).
    wait_modifiers_released(250);
    release_all_modifiers();
    // If Alt is already up and the menu bar is open, Ctrl+V goes to the menu, not the input.
    dismiss_menu_mode_if_active();

    let inputs = [
        make_key_input(VK_CONTROL, false), // Ctrl down
        make_key_input(VK_V, false),       // V down
        make_key_input(VK_V, true),        // V up
        make_key_input(VK_CONTROL, true),  // Ctrl up
    ];

    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            bail!("SendInput failed, expected {} got {}", inputs.len(), sent);
        }
    }

    Ok(())
}

/// Build a keyboard INPUT struct.
fn make_key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    let mut flags = windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags = KEYEVENTF_KEYUP;
    }

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Restore delay (ms): must land after the injected Ctrl+V is processed by the
/// target window, or IME would be switched back before paste completes.
const RESTORE_DELAY_MS: u64 = 120;

/// Action [`ImeGuard`] must undo on drop.
/// Only recorded when state actually changed; a no-op never restores.
enum ImeRestore {
    /// Nothing changed; no restore needed.
    Nothing,
    /// Switch the keyboard layout back.
    Layout { hwnd: HWND, previous_hkl: HKL },
    /// Re-open IME input.
    OpenStatus { ime_wnd: isize },
}

/// IME guard.
///
/// Puts the foreground window into "direct English input" before paste, then
/// restores asynchronously afterwards. The exact method is chosen by
/// [`ImeProtection`]; see that enum and issue #10.
///
/// Design: IME protection is **best-effort**. Any failure quietly degrades to
/// "no protection". IME issues must never fail the paste itself, and must never
/// alter the system IME list on their own.
pub struct ImeGuard {
    restore: ImeRestore,
}

impl ImeGuard {
    /// Protect IME state according to the given policy.
    pub fn new(policy: ImeProtection) -> Self {
        let restore = match policy {
            ImeProtection::Off => ImeRestore::Nothing,
            ImeProtection::Imm => Self::close_ime_open_status(),
            ImeProtection::Layout | ImeProtection::LayoutForce => {
                Self::switch_to_english_layout(policy.allows_loading_layout())
            }
        };

        Self { restore }
    }

    /// Close the foreground window's IME open status (direct input / English mode).
    ///
    /// Default policy: talk only to the target window's IME window and
    /// **never touch the keyboard layout**, so it cannot add `ENG` to the
    /// system IME list the way the old implementation did (issue #10).
    fn close_ime_open_status() -> ImeRestore {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0 == 0 {
                return ImeRestore::Nothing;
            }

            let ime_wnd = ImmGetDefaultIMEWnd(hwnd.0);
            if ime_wnd == 0 {
                // No IME window on the foreground window (common in English-only setups).
                return ImeRestore::Nothing;
            }

            // Already closed (direct input): leave it alone, so nothing to restore.
            match send_ime_control(ime_wnd, IMC_GETOPENSTATUS, 0) {
                Some(0) => return ImeRestore::Nothing,
                None => {
                    warn!("ImeGuard: timed out querying IME open status; skipping IME guard");
                    return ImeRestore::Nothing;
                }
                Some(_) => {}
            }

            if send_ime_control(ime_wnd, IMC_SETOPENSTATUS, 0).is_none() {
                warn!("ImeGuard: timed out closing IME open status; skipping IME guard");
                return ImeRestore::Nothing;
            }

            info!("ImeGuard: closed foreground window IME open status");
            ImeRestore::OpenStatus { ime_wnd }
        }
    }

    /// Switch to an English keyboard layout (legacy-compatible behaviour).
    ///
    /// When `allow_load` is false, only reuse an English layout already
    /// installed; if none is found, skip — never call LoadKeyboardLayoutW.
    fn switch_to_english_layout(allow_load: bool) -> ImeRestore {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0 == 0 {
                return ImeRestore::Nothing;
            }

            let thread_id = GetWindowThreadProcessId(hwnd, None);
            if thread_id == 0 {
                return ImeRestore::Nothing;
            }

            let current_hkl = GetKeyboardLayout(thread_id);

            // Already an English layout; no switch needed.
            if primary_lang_of(current_hkl) == LANG_ENGLISH {
                return ImeRestore::Nothing;
            }

            let english_hkl = match resolve_english_layout(allow_load) {
                Some(hkl) => hkl,
                None => {
                    info!("ImeGuard: no English keyboard layout loaded; skipping switch (will not add ENG)");
                    return ImeRestore::Nothing;
                }
            };

            if current_hkl == english_hkl {
                return ImeRestore::Nothing;
            }

            info!(
                "ImeGuard: switching IME {:#x} -> {:#x}",
                current_hkl, english_hkl
            );
            let _ = PostMessageW(
                hwnd,
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(0),
                LPARAM(english_hkl),
            );
            std::thread::sleep(std::time::Duration::from_millis(60));

            ImeRestore::Layout {
                hwnd,
                previous_hkl: current_hkl,
            }
        }
    }
}

impl Drop for ImeGuard {
    /// Restore on a background thread after a delay so the hotkey thread is not
    /// blocked and restore happens after the injected Ctrl+V is processed.
    fn drop(&mut self) {
        match std::mem::replace(&mut self.restore, ImeRestore::Nothing) {
            ImeRestore::Nothing => {}

            ImeRestore::Layout { hwnd, previous_hkl } => {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(RESTORE_DELAY_MS));
                    unsafe {
                        if let Err(e) = PostMessageW(
                            hwnd,
                            WM_INPUTLANGCHANGEREQUEST,
                            WPARAM(0),
                            LPARAM(previous_hkl),
                        ) {
                            warn!("failed to restore IME: {:?}", e);
                        } else {
                            info!("ImeGuard: restored IME {:#x}", previous_hkl);
                        }
                    }
                });
            }

            ImeRestore::OpenStatus { ime_wnd } => {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(RESTORE_DELAY_MS));
                    unsafe {
                        if send_ime_control(ime_wnd, IMC_SETOPENSTATUS, 1).is_none() {
                            warn!("timed out restoring IME open status");
                        } else {
                            info!("ImeGuard: restored IME open status");
                        }
                    }
                });
            }
        }
    }
}
