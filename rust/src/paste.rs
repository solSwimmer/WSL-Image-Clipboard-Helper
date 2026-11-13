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
    GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, PostMessageW, GUITHREADINFO,
    GUI_INMENUMODE,
};

use tracing::{info, warn};

use crate::config::ImeProtection;

/// HKL type alias (a Win32 HKL is just an isize)
pub type HKL = isize;

/// WM_INPUTLANGCHANGEREQUEST
const WM_INPUTLANGCHANGEREQUEST: u32 = 0x0050;

/// WM_IME_CONTROL and its sub-commands, used to turn the foreground window's IME input state
/// off and back on **without changing the keyboard layout**
const WM_IME_CONTROL: u32 = 0x0283;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IMC_SETOPENSTATUS: usize = 0x0006;

/// Flags for LoadKeyboardLayoutW.
///
/// **KLF_ACTIVATE (0x1) is deliberately not used here**: it activates the English layout on the current thread,
/// and LoadKeyboardLayoutW itself already registers the layout in the system input-method list,
/// and the two combined were the direct cause of issue #10, "Win+Space shows an extra ENG after startup".
/// KLF_NOTELLSHELL (0x80) stops the shell receiving the HSHELL_LANGUAGE notification,
/// so the taskbar IME indicator isn't brought back.
const KLF_NOTELLSHELL: u32 = 0x0080;

/// SendMessageTimeout flag: return immediately if the target thread is hung so the hotkey thread isn't held up
const SMTO_ABORTIFHUNG: u32 = 0x0002;
/// Timeout (ms) for talking to the foreground window's IME. Pasting is latency-sensitive: better to give up protection than to stall.
const IME_CONTROL_TIMEOUT_MS: u32 = 80;

/// English primary language ID (LANG_ENGLISH)
const LANG_ENGLISH: u16 = 0x09;
/// Full en-US language ID
const LANGID_EN_US: u16 = 0x0409;

/// The English layout this process loaded itself via LoadKeyboardLayoutW.
/// 0 means never loaded. It is unloaded on exit so no ENG entry is left in the user's input-method list.
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

/// Take the input language ID from the low 16 bits of an HKL (the high 16 bits are a device handle)
fn langid_of(hkl: HKL) -> u16 {
    (hkl as usize & 0xFFFF) as u16
}

/// Take the primary language ID (low 10 bits of a LANGID)
fn primary_lang_of(hkl: HKL) -> u16 {
    langid_of(hkl) & 0x3FF
}

/// Look for an English layout among the keyboard layouts the system has **already loaded**.
/// Query only: it never adds a layout to the system, so there are no side effects.
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

        // Prefer an exact en-US match, then fall back to any English variant (en-GB and the like are also plain Latin layouts)
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

/// Lazily obtain an English layout HKL.
///
/// Key difference from the old implementation: **it is not called at startup**, and by default it only reuses layouts the system already has.
/// It only actually loads a layout when `allow_load = true` (`ime_protection = "layout-force"`),
/// without KLF_ACTIVATE, and [`unload_self_loaded_layout`] unloads it on exit.
fn resolve_english_layout(allow_load: bool) -> Option<HKL> {
    // 1) Reuse an English layout already installed (the path most English/multilingual users take)
    if let Some(hkl) = find_loaded_english_layout() {
        return Some(hkl);
    }

    // 2) The user has no English layout: skip switching by default and never add an ENG on our own
    if !allow_load {
        return None;
    }

    // 3) Only load when explicitly allowed, and only once
    let cached = SELF_LOADED_ENGLISH_HKL.load(Ordering::Relaxed);
    if cached != 0 {
        return Some(cached);
    }

    unsafe {
        let klid: Vec<u16> = "00000409\0".encode_utf16().collect();
        let hkl = LoadKeyboardLayoutW(klid.as_ptr(), KLF_NOTELLSHELL);
        if hkl == 0 {
            warn!("Failed to load the English keyboard layout, skipping IME guard");
            return None;
        }
        info!("Lazily loaded English keyboard layout {:#x} (will be unloaded on exit)", hkl);
        SELF_LOADED_ENGLISH_HKL.store(hkl, Ordering::Relaxed);
        Some(hkl)
    }
}

/// On exit, unload the English layout this process loaded itself,
/// so nothing is left in the user's input-method switch list (issue #10).
pub fn unload_self_loaded_layout() {
    let hkl = SELF_LOADED_ENGLISH_HKL.swap(0, Ordering::Relaxed);
    if hkl == 0 {
        return;
    }
    unsafe {
        if UnloadKeyboardLayout(hkl) == 0 {
            warn!("Failed to unload English keyboard layout {:#x}", hkl);
        } else {
            info!("Unloaded the English keyboard layout this process loaded {:#x}", hkl);
        }
    }
}

/// Send WM_IME_CONTROL to the foreground window's default IME window.
/// Uses SendMessageTimeoutW with a timeout so an unresponsive target process can't stall the paste.
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

/// Put text on the clipboard and perform the paste
pub fn paste_text(text: &str) -> Result<()> {
    unsafe {
        // Try to open the clipboard, with a retry
        if OpenClipboard(None).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Err(e) = OpenClipboard(None) {
                bail!("Cannot open the clipboard: {:?}", e);
            }
        }

        // Empty the clipboard (key fix: it must be emptied before setting)
        if let Err(e) = EmptyClipboard() {
            CloseClipboard().ok();
            bail!("Failed to empty the clipboard: {:?}", e);
        }

        // Prepare the UTF-16 data
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = utf16.len() * std::mem::size_of::<u16>();

        // Allocate memory
        let h_mem = match GlobalAlloc(GMEM_MOVEABLE, byte_len) {
            Ok(mem) => mem,
            Err(e) => {
                CloseClipboard().ok();
                bail!("Failed to allocate clipboard memory: {:?}", e);
            }
        };

        let ptr = GlobalLock(h_mem);
        if ptr.is_null() {
            CloseClipboard().ok();
            bail!("Failed to lock memory");
        }

        // Copy the data
        std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, byte_len);

        let _ = GlobalUnlock(h_mem);

        // Set the clipboard data
        if SetClipboardData(
            CF_UNICODETEXT.0 as u32,
            Some(windows::Win32::Foundation::HANDLE(h_mem.0)),
        )
        .is_err()
        {
            CloseClipboard().ok();
            bail!("Failed to set clipboard data");
        }

        CloseClipboard().ok();
    }

    // Send the paste shortcut
    send_ctrl_v()?;
    Ok(())
}

/// Menu mask key: matches AHK's A_MenuMaskKey (vkFF).
/// When the hotkey fires the physical Alt is usually still held, so the target window only sees Alt down (the V is swallowed by the hotkey).
/// If an Alt up is injected right after, some programs treat it as a lone "Alt tap", activate the menu bar and steal input focus,
/// so the following Ctrl+V pastes nothing. Injecting a harmless key first breaks that detection.
const VK_MENU_MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xFF);

/// Check whether a virtual key is currently physically held down
fn is_key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 }
}

/// Wait for the user to release physical modifier keys (Alt/Shift) that would corrupt Ctrl+V, for at most timeout_ms.
/// A physically held Alt is set back to down by keyboard auto-repeat, so injecting a key-up is not always enough;
/// this is a common source of occasional empty pastes.
fn wait_modifiers_released(timeout_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while is_key_down(VK_MENU)
        || is_key_down(VK_LMENU)
        || is_key_down(VK_RMENU)
        || is_key_down(VK_SHIFT)
    {
        if std::time::Instant::now() >= deadline {
            warn!("Timed out waiting for modifier release, injecting the paste keys anyway");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Call immediately after the hotkey fires: insert the mask key before the user releases Alt.
/// RegisterHotKey swallows the V key event, so the target window only sees Alt down/up;
/// if the mask key lands before the physical Alt release, the window won't treat it as a lone Alt tap and activate the menu bar.
/// It must run before slow work such as reading the clipboard or writing files, the earlier the better.
pub fn mask_alt_tap() {
    let inputs = [
        make_key_input(VK_MENU_MASK, false),
        make_key_input(VK_MENU_MASK, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// If the foreground thread has already entered menu mode (when Alt is released quickly the mask key loses the race and the menu bar is activated),
/// send Esc to leave the menu first, otherwise the injected Ctrl+V is eaten by the menu and the paste does nothing
fn dismiss_menu_mode_if_active() {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
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
            info!("Foreground window is in menu mode, sending Esc before pasting");
            let esc = [
                make_key_input(VK_ESCAPE, false),
                make_key_input(VK_ESCAPE, true),
            ];
            SendInput(&esc, std::mem::size_of::<INPUT>() as i32);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
}

/// Release all modifier keys (Alt/Ctrl/Shift); corresponds to AHK's NormalizeModifierStateBeforeSend
pub fn release_all_modifiers() {
    let inputs = [
        // Send the menu mask key first, then release Alt, so a lone Alt tap doesn't activate the window menu
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

/// Windows Terminal top-level window class name
const WINDOWS_TERMINAL_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

/// Whether the foreground window is Windows Terminal.
/// In a WSL terminal Ctrl+V is often rebound by the user or handled by a TUI app,
/// whereas Ctrl+Shift+V is the terminal's fixed "paste" shortcut, so injecting it reliably pastes the path into the input box.
fn foreground_is_windows_terminal() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut buf);
        if len <= 0 {
            return false;
        }
        String::from_utf16_lossy(&buf[..len as usize]) == WINDOWS_TERMINAL_CLASS
    }
}

/// Send the paste shortcut (uses SendInput instead of keybd_event).
/// Windows Terminal gets Ctrl+Shift+V; other windows get Ctrl+V.
pub fn send_ctrl_v() -> Result<()> {
    // Give the user time to release the hotkey first (if the Alt of Alt+V/Alt+Enter is still down,
    // the injected Ctrl+V is read as Ctrl+Alt+V and does nothing)
    wait_modifiers_released(250);
    let use_shift = foreground_is_windows_terminal();
    release_all_modifiers();
    // If Alt was released and the menu bar got activated, Ctrl+V goes to the menu instead of the input box
    dismiss_menu_mode_if_active();

    let mut inputs = vec![make_key_input(VK_CONTROL, false)];
    if use_shift {
        inputs.push(make_key_input(VK_SHIFT, false));
    }
    inputs.push(make_key_input(VK_V, false));
    inputs.push(make_key_input(VK_V, true));
    if use_shift {
        inputs.push(make_key_input(VK_SHIFT, true));
    }
    inputs.push(make_key_input(VK_CONTROL, true));

    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            bail!("SendInput failed, expected {} sent {}", inputs.len(), sent);
        }
    }

    Ok(())
}

/// Build a keyboard INPUT structure
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

/// Delay (ms) before restoring: it must be later than the moment the target window actually processes the injected Ctrl+V,
/// otherwise the IME would be switched back before the paste completes.
const RESTORE_DELAY_MS: u64 = 120;

/// The action [`ImeGuard`] must undo when dropped.
/// It is only recorded when state was actually changed; if nothing was done, nothing is ever restored.
enum ImeRestore {
    /// Nothing was changed, nothing to restore
    Nothing,
    /// The keyboard layout must be switched back to its original value
    Layout { hwnd: isize, previous_hkl: HKL },
    /// The IME open status must be turned back on
    OpenStatus { ime_wnd: isize },
}

/// IME guard
///
/// Puts the foreground window into a "direct English input" state before pasting and restores it asynchronously afterwards.
/// The exact measure is decided by the [`ImeProtection`] policy; see that enum's docs and issue #10.
///
/// Design principle: the IME guard is a **best-effort** aid. Any failing step quietly degrades to
/// "no protection"; an IME problem must never make the paste itself fail, and the system input-method list is never modified on our own.
pub struct ImeGuard {
    restore: ImeRestore,
}

impl ImeGuard {
    /// Protect the IME state according to the given policy
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

    /// Close the foreground window's IME open status (i.e. switch to direct input / English mode).
    ///
    /// This is the default policy: it only talks to the target window's IME window and **never touches keyboard layouts**,
    /// so unlike the old implementation it never adds an `ENG` entry to the system input-method list (issue #10).
    fn close_ime_open_status() -> ImeRestore {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return ImeRestore::Nothing;
            }

            let ime_wnd = ImmGetDefaultIMEWnd(hwnd.0 as isize);
            if ime_wnd == 0 {
                // The foreground window has no associated IME window (common in English-only setups), so no protection is needed
                return ImeRestore::Nothing;
            }

            // If it is already closed (direct input) change nothing, so there is nothing to restore
            match send_ime_control(ime_wnd, IMC_GETOPENSTATUS, 0) {
                Some(0) => return ImeRestore::Nothing,
                None => {
                    warn!("ImeGuard: timed out querying IME open status, skipping IME guard");
                    return ImeRestore::Nothing;
                }
                Some(_) => {}
            }

            if send_ime_control(ime_wnd, IMC_SETOPENSTATUS, 0).is_none() {
                warn!("ImeGuard: timed out closing IME open status, skipping IME guard");
                return ImeRestore::Nothing;
            }

            info!("ImeGuard: closed the foreground window IME open status");
            ImeRestore::OpenStatus { ime_wnd }
        }
    }

    /// Switch to an English keyboard layout (legacy-compatible behaviour).
    ///
    /// When `allow_load` is false only an English layout the system already has is reused,
    /// and if none is found it gives up without ever calling LoadKeyboardLayoutW to add a layout.
    fn switch_to_english_layout(allow_load: bool) -> ImeRestore {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return ImeRestore::Nothing;
            }

            let thread_id = GetWindowThreadProcessId(hwnd, None);
            if thread_id == 0 {
                return ImeRestore::Nothing;
            }

            let current_hkl = GetKeyboardLayout(thread_id);

            // Already an English layout, no switch needed
            if primary_lang_of(current_hkl) == LANG_ENGLISH {
                return ImeRestore::Nothing;
            }

            let english_hkl = match resolve_english_layout(allow_load) {
                Some(hkl) => hkl,
                None => {
                    info!("ImeGuard: no English keyboard layout loaded, skipping the switch (will not add ENG on its own)");
                    return ImeRestore::Nothing;
                }
            };

            if current_hkl == english_hkl {
                return ImeRestore::Nothing;
            }

            info!(
                "ImeGuard: switching input method {:#x} -> {:#x}",
                current_hkl, english_hkl
            );
            let _ = PostMessageW(
                Some(hwnd),
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(0),
                LPARAM(english_hkl),
            );
            std::thread::sleep(std::time::Duration::from_millis(60));

            ImeRestore::Layout {
                hwnd: hwnd.0 as isize,
                previous_hkl: current_hkl,
            }
        }
    }
}

impl Drop for ImeGuard {
    /// Restore after a delay on a background thread when dropped, which neither blocks the hotkey thread
    /// nor lets the restore happen before the target window has processed the injected Ctrl+V
    fn drop(&mut self) {
        match std::mem::replace(&mut self.restore, ImeRestore::Nothing) {
            ImeRestore::Nothing => {}

            ImeRestore::Layout { hwnd, previous_hkl } => {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(RESTORE_DELAY_MS));
                    unsafe {
                        if let Err(e) = PostMessageW(
                            Some(HWND(hwnd as *mut _)),
                            WM_INPUTLANGCHANGEREQUEST,
                            WPARAM(0),
                            LPARAM(previous_hkl),
                        ) {
                            warn!("Failed to restore the input method: {:?}", e);
                        } else {
                            info!("ImeGuard: restored input method {:#x}", previous_hkl);
                        }
                    }
                });
            }

            ImeRestore::OpenStatus { ime_wnd } => {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(RESTORE_DELAY_MS));
                    unsafe {
                        if send_ime_control(ime_wnd, IMC_SETOPENSTATUS, 1).is_none() {
                            warn!("Timed out restoring IME open status");
                        } else {
                            info!("ImeGuard: restored IME open status");
                        }
                    }
                });
            }
        }
    }
}
