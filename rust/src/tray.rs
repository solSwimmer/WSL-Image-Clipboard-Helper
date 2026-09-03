use crate::cleanup;
use crate::config::{AppConfig, ImeProtection, PathStyle, RuntimeMode};
use crate::hotkey::{HotkeyManager, HotkeyType};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use tracing::{error, info, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Tray icon callback message.
const WM_TRAYICON: u32 = WM_APP + 100;
const APP_ICON_ID: u16 = 1;

/// Menu command IDs.
const CMD_HOTKEY_ALTV: u32 = 1001;
const CMD_HOTKEY_CTRLALTV: u32 = 1002;
const CMD_HOTKEY_ALTENTER: u32 = 1003;
const CMD_MODE_SAFE: u32 = 2001;
const CMD_MODE_FAST: u32 = 2002;
const CMD_OPEN_FOLDER: u32 = 3001;
const CMD_EXIT: u32 = 4001;
const CMD_STYLE_PLAIN: u32 = 5001;
const CMD_STYLE_AT: u32 = 5002;
const CMD_STYLE_QUOTED: u32 = 5003;
const CMD_IME_OFF: u32 = 6001;
const CMD_IME_IMM: u32 = 6002;
const CMD_IME_LAYOUT: u32 = 6003;
const CMD_IME_LAYOUT_FORCE: u32 = 6004;
const CMD_ALLOW_NON_IMAGE: u32 = 7001;

/// Commands sent from the tray to the main loop.
#[derive(Debug, Clone)]
pub enum TrayCommand {
    SwitchHotkey(HotkeyType),
    SwitchMode(RuntimeMode),
    SwitchPathStyle(PathStyle),
    SwitchImeProtection(ImeProtection),
    SwitchAllowNonImageFiles(bool),
    OpenFolder,
    Exit,
}

/// Thread-local state used from wnd_proc.
struct TrayState {
    nid: NOTIFYICONDATAW,
    config: AppConfig,
    hotkey_manager: HotkeyManager,
    cmd_tx: std_mpsc::Sender<TrayCommand>,
    temp_dir: PathBuf,
    session_end_cleanup_done: bool,
}

// Global state pointer (tray thread only).
static mut TRAY_STATE: *mut TrayState = std::ptr::null_mut();

/// Tray controller.
pub struct TrayController;

impl TrayController {
    /// Start the tray thread; returns the command receiver.
    /// The hotkey manager is created on the tray thread (needs a Win32 message loop).
    pub fn start(
        config: AppConfig,
        temp_dir: PathBuf,
    ) -> Result<std_mpsc::Receiver<TrayCommand>> {
        let (cmd_tx, cmd_rx) = std_mpsc::channel::<TrayCommand>();

        let config_clone = config.clone();
        let temp_dir_clone = temp_dir.clone();
        let tx = cmd_tx.clone();

        std::thread::Builder::new()
            .name("tray-thread".to_string())
            .spawn(move || {
                if let Err(e) = run_tray_thread(config_clone, temp_dir_clone, tx) {
                    error!("tray thread exited unexpectedly: {}", e);
                }
            })
            .context("failed to start tray thread")?;

        Ok(cmd_rx)
    }
}

unsafe fn load_app_icon(h_instance: HINSTANCE) -> windows::core::Result<HICON> {
    LoadIconW(h_instance, PCWSTR(APP_ICON_ID as usize as *const u16))
        .or_else(|_| LoadIconW(HINSTANCE::default(), IDI_APPLICATION))
}

/// Tray thread entry point.
fn run_tray_thread(
    config: AppConfig,
    temp_dir: PathBuf,
    cmd_tx: std_mpsc::Sender<TrayCommand>,
) -> Result<()> {
    unsafe {
        let h_instance = GetModuleHandleW(None)?;

        // Register the window class.
        let class_name_buf: Vec<u16> = "WSLClipboardTray\0".encode_utf16().collect();
        let class_name = PCWSTR::from_raw(class_name_buf.as_ptr());

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: Default::default(),
            lpfnWndProc: Some(tray_wnd_proc),
            hInstance: h_instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };

        RegisterClassExW(&wc);

        let window_name_buf: Vec<u16> = "WSL Clipboard Tray\0".encode_utf16().collect();

        // Create a hidden window.
        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            PCWSTR::from_raw(window_name_buf.as_ptr()),
            Default::default(),
            0, 0, 0, 0,
            None,
            None,
            h_instance,
            None,
        );

        if hwnd.0 == 0 {
            anyhow::bail!("failed to create hidden window");
        }

        // Create the tray icon.
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = load_app_icon(h_instance.into())?;

        // Set the tooltip.
        set_tooltip(&mut nid, &config);

        Shell_NotifyIconW(NIM_ADD, &nid);

        // Create the hotkey manager on the tray thread.
        let mut hotkey_manager = HotkeyManager::new()?;

        // Register the initial hotkey.
        let initial_hotkey = HotkeyType::from_config(&config.hotkey)
            .unwrap_or(HotkeyType::AltV);
        if let Err(e) = hotkey_manager.register(initial_hotkey) {
            warn!("failed to register initial hotkey: {}", e);
        }

        // Create state.
        let mut state = Box::new(TrayState {
            nid,
            config,
            hotkey_manager,
            cmd_tx,
            temp_dir,
            session_end_cleanup_done: false,
        });

        TRAY_STATE = &mut *state as *mut TrayState;

        // Message loop.
        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, HWND::default(), 0, 0);
            if ret.0 <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Cleanup.
        if let Err(e) = state.hotkey_manager.unregister() {
            warn!("failed to unregister hotkey on exit: {}", e);
        }
        Shell_NotifyIconW(NIM_DELETE, &state.nid);
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(class_name, h_instance);
        TRAY_STATE = std::ptr::null_mut();

        info!("tray thread exited");
    }

    Ok(())
}

/// Set tooltip text.
fn set_tooltip(nid: &mut NOTIFYICONDATAW, config: &AppConfig) {
    let hotkey_display = HotkeyType::from_config(&config.hotkey)
        .map(|h| h.display_name())
        .unwrap_or("Alt+V");

    let mode_display = match &config.runtime_mode {
        RuntimeMode::Safe => "Compat",
        RuntimeMode::Fast => "Fast",
    };

    let tip = format!("WSL Clipboard ({} | {})", hotkey_display, mode_display);
    let tip_utf16: Vec<u16> = tip.encode_utf16().collect();
    let len = tip_utf16.len().min(nid.szTip.len() - 1);
    nid.szTip[..len].copy_from_slice(&tip_utf16[..len]);
    nid.szTip[len] = 0;
}

/// Window procedure.
unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAYICON {
        let mouse_msg = (lparam.0 & 0xFFFF) as u32;
        if mouse_msg == WM_RBUTTONUP || mouse_msg == WM_CONTEXTMENU {
            show_context_menu(hwnd);
        }
        return LRESULT(0);
    }

    if msg == WM_COMMAND {
        let cmd_id = (wparam.0 & 0xFFFF) as u32;
        handle_menu_command(cmd_id);
        return LRESULT(0);
    }

    if msg == WM_QUERYENDSESSION {
        return LRESULT(1);
    }

    if msg == WM_ENDSESSION {
        if wparam.0 != 0 {
            handle_session_end();
        }
        return LRESULT(0);
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn handle_session_end() {
    if TRAY_STATE.is_null() {
        return;
    }

    let state = &mut *TRAY_STATE;
    if state.session_end_cleanup_done {
        return;
    }

    state.session_end_cleanup_done = true;
    info!("Windows session is ending; cleaning up temp files");

    if let Err(e) = cleanup::cleanup_temp_png(&state.temp_dir) {
        warn!("session-end temp cleanup failed: {}", e);
    }

    let _ = state.cmd_tx.send(TrayCommand::Exit);
    PostQuitMessage(0);
}

/// Show the right-click menu.
unsafe fn show_context_menu(hwnd: HWND) {
    if TRAY_STATE.is_null() {
        return;
    }
    let state = &*TRAY_STATE;

    let h_menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };

    // ---- Hotkey submenu ----
    let h_hotkey_menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => { let _ = DestroyMenu(h_menu); return; }
    };

    let current_hotkey = state.hotkey_manager.current_type();
    for ht in HotkeyType::all() {
        let label = format!("{}\0", ht.display_name());
        let label_w: Vec<u16> = label.encode_utf16().collect();
        let cmd_id = match ht {
            HotkeyType::AltV => CMD_HOTKEY_ALTV,
            HotkeyType::CtrlAltV => CMD_HOTKEY_CTRLALTV,
            HotkeyType::AltEnter => CMD_HOTKEY_ALTENTER,
        };
        let mut flags = MF_STRING;
        if *ht == current_hotkey {
            flags |= MF_CHECKED;
        }
        let _ = AppendMenuW(h_hotkey_menu, flags, cmd_id as usize, PCWSTR::from_raw(label_w.as_ptr()));
    }

    let hotkey_label: Vec<u16> = "Hotkey\0".encode_utf16().collect();
    let _ = AppendMenuW(h_menu, MF_POPUP, h_hotkey_menu.0 as usize, PCWSTR::from_raw(hotkey_label.as_ptr()));

    // ---- Mode submenu ----
    let h_mode_menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => { let _ = DestroyMenu(h_menu); return; }
    };

    let is_safe = matches!(state.config.runtime_mode, RuntimeMode::Safe);

    let safe_label: Vec<u16> = "Compatibility mode (IME guard)\0".encode_utf16().collect();
    let safe_flags = MF_STRING | if is_safe { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(h_mode_menu, safe_flags, CMD_MODE_SAFE as usize, PCWSTR::from_raw(safe_label.as_ptr()));

    let fast_label: Vec<u16> = "Fast mode (low latency)\0".encode_utf16().collect();
    let fast_flags = MF_STRING | if !is_safe { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(h_mode_menu, fast_flags, CMD_MODE_FAST as usize, PCWSTR::from_raw(fast_label.as_ptr()));

    let mode_label: Vec<u16> = "Run mode\0".encode_utf16().collect();
    let _ = AppendMenuW(h_menu, MF_POPUP, h_mode_menu.0 as usize, PCWSTR::from_raw(mode_label.as_ptr()));

    // ---- Path style submenu ----
    let h_style_menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => { let _ = DestroyMenu(h_menu); return; }
    };

    let current_style = state.config.path_style;
    let style_items = [
        (PathStyle::Plain, CMD_STYLE_PLAIN),
        (PathStyle::At, CMD_STYLE_AT),
        (PathStyle::Quoted, CMD_STYLE_QUOTED),
    ];
    for (style, cmd_id) in style_items {
        let label = format!("{}\0", style.display_name());
        let label_w: Vec<u16> = label.encode_utf16().collect();
        let mut flags = MF_STRING;
        if style == current_style {
            flags |= MF_CHECKED;
        }
        let _ = AppendMenuW(h_style_menu, flags, cmd_id as usize, PCWSTR::from_raw(label_w.as_ptr()));
    }

    let style_label: Vec<u16> = "Path style\0".encode_utf16().collect();
    let _ = AppendMenuW(h_menu, MF_POPUP, h_style_menu.0 as usize, PCWSTR::from_raw(style_label.as_ptr()));

    // ---- IME guard submenu (issue #10) ----
    // How compatibility mode protects IME; can be turned off entirely here.
    let h_ime_menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => { let _ = DestroyMenu(h_menu); return; }
    };

    let current_ime = state.config.ime_protection;
    for policy in ImeProtection::all() {
        let label = format!("{}\0", policy.display_name());
        let label_w: Vec<u16> = label.encode_utf16().collect();
        let cmd_id = match policy {
            ImeProtection::Off => CMD_IME_OFF,
            ImeProtection::Imm => CMD_IME_IMM,
            ImeProtection::Layout => CMD_IME_LAYOUT,
            ImeProtection::LayoutForce => CMD_IME_LAYOUT_FORCE,
        };
        let mut flags = MF_STRING;
        if *policy == current_ime {
            flags |= MF_CHECKED;
        }
        // IME guard only applies in compatibility mode; gray it out in fast mode.
        if !is_safe {
            flags |= MF_GRAYED;
        }
        let _ = AppendMenuW(h_ime_menu, flags, cmd_id as usize, PCWSTR::from_raw(label_w.as_ptr()));
    }

    let ime_label: Vec<u16> = "IME guard\0".encode_utf16().collect();
    let _ = AppendMenuW(h_menu, MF_POPUP, h_ime_menu.0 as usize, PCWSTR::from_raw(ime_label.as_ptr()));

    // ---- Non-image files ----
    let mut file_flags = MF_STRING;
    if state.config.allow_non_image_files {
        file_flags |= MF_CHECKED;
    }
    let file_label: Vec<u16> = "Allow non-image file conversion\0".encode_utf16().collect();
    let _ = AppendMenuW(h_menu, file_flags, CMD_ALLOW_NON_IMAGE as usize, PCWSTR::from_raw(file_label.as_ptr()));

    // ---- Separator ----
    let _ = AppendMenuW(h_menu, MF_SEPARATOR, 0, PCWSTR::null());

    // ---- Open cache ----
    let folder_label: Vec<u16> = "Open temp image folder\0".encode_utf16().collect();
    let _ = AppendMenuW(h_menu, MF_STRING, CMD_OPEN_FOLDER as usize, PCWSTR::from_raw(folder_label.as_ptr()));

    // ---- Separator ----
    let _ = AppendMenuW(h_menu, MF_SEPARATOR, 0, PCWSTR::null());

    // ---- Exit ----
    let exit_label: Vec<u16> = "Exit and clean temporary images\0".encode_utf16().collect();
    let _ = AppendMenuW(h_menu, MF_STRING, CMD_EXIT as usize, PCWSTR::from_raw(exit_label.as_ptr()));

    // Show the menu.
    let mut pt = windows::Win32::Foundation::POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);

    TrackPopupMenu(h_menu, TPM_LEFTALIGN | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);

    let _ = DestroyMenu(h_menu);
}

/// Handle a menu command.
unsafe fn handle_menu_command(cmd_id: u32) {
    if TRAY_STATE.is_null() {
        return;
    }
    let state = &mut *TRAY_STATE;

    match cmd_id {
        CMD_HOTKEY_ALTV => switch_hotkey(state, HotkeyType::AltV),
        CMD_HOTKEY_CTRLALTV => switch_hotkey(state, HotkeyType::CtrlAltV),
        CMD_HOTKEY_ALTENTER => switch_hotkey(state, HotkeyType::AltEnter),
        CMD_MODE_SAFE => switch_mode(state, RuntimeMode::Safe),
        CMD_MODE_FAST => switch_mode(state, RuntimeMode::Fast),
        CMD_STYLE_PLAIN => switch_path_style(state, PathStyle::Plain),
        CMD_STYLE_AT => switch_path_style(state, PathStyle::At),
        CMD_STYLE_QUOTED => switch_path_style(state, PathStyle::Quoted),
        CMD_IME_OFF => switch_ime_protection(state, ImeProtection::Off),
        CMD_IME_IMM => switch_ime_protection(state, ImeProtection::Imm),
        CMD_IME_LAYOUT => switch_ime_protection(state, ImeProtection::Layout),
        CMD_IME_LAYOUT_FORCE => switch_ime_protection(state, ImeProtection::LayoutForce),
        CMD_ALLOW_NON_IMAGE => toggle_allow_non_image_files(state),
        CMD_OPEN_FOLDER => {
            let _ = state.cmd_tx.send(TrayCommand::OpenFolder);
        }
        CMD_EXIT => {
            let _ = state.cmd_tx.send(TrayCommand::Exit);
            PostQuitMessage(0);
        }
        _ => {}
    }
}

/// Switch hotkey.
unsafe fn switch_hotkey(state: &mut TrayState, hotkey_type: HotkeyType) {
    if state.hotkey_manager.current_type() == hotkey_type {
        return;
    }

    if let Err(e) = state.hotkey_manager.register(hotkey_type) {
        error!("failed to switch hotkey: {}", e);
        return;
    }

    state.config.hotkey = hotkey_type.as_str().to_string();
    let _ = state.config.save();

    // Update tooltip.
    set_tooltip(&mut state.nid, &state.config);
    state.nid.uFlags = NIF_TIP;
    Shell_NotifyIconW(NIM_MODIFY, &state.nid);

    let _ = state.cmd_tx.send(TrayCommand::SwitchHotkey(hotkey_type));
    info!("switched hotkey: {}", hotkey_type.display_name());
}

/// Switch path style.
unsafe fn switch_path_style(state: &mut TrayState, style: PathStyle) {
    if state.config.path_style == style {
        return;
    }

    state.config.path_style = style;
    let _ = state.config.save();

    let _ = state.cmd_tx.send(TrayCommand::SwitchPathStyle(style));
    info!("switched path style: {}", style.display_name());
}

/// Switch IME guard strategy.
unsafe fn switch_ime_protection(state: &mut TrayState, policy: ImeProtection) {
    if state.config.ime_protection == policy {
        return;
    }

    state.config.ime_protection = policy;
    let _ = state.config.save();

    let _ = state.cmd_tx.send(TrayCommand::SwitchImeProtection(policy));
    info!("switched IME guard: {}", policy.display_name());
}

/// Switch runtime mode.
unsafe fn switch_mode(state: &mut TrayState, mode: RuntimeMode) {
    let mode_str = match &mode {
        RuntimeMode::Safe => "safe",
        RuntimeMode::Fast => "fast",
    };
    let current_str = match &state.config.runtime_mode {
        RuntimeMode::Safe => "safe",
        RuntimeMode::Fast => "fast",
    };

    if mode_str == current_str {
        return;
    }

    state.config.runtime_mode = mode.clone();
    let _ = state.config.save();

    // Update tooltip.
    set_tooltip(&mut state.nid, &state.config);
    state.nid.uFlags = NIF_TIP;
    Shell_NotifyIconW(NIM_MODIFY, &state.nid);

    let _ = state.cmd_tx.send(TrayCommand::SwitchMode(mode));
    info!("switched mode: {}", mode_str);
}

unsafe fn toggle_allow_non_image_files(state: &mut TrayState) {
    state.config.allow_non_image_files = !state.config.allow_non_image_files;
    let _ = state.config.save();
    let _ = state.cmd_tx.send(TrayCommand::SwitchAllowNonImageFiles(
        state.config.allow_non_image_files,
    ));
    info!(
        "non-image file path conversion: {}",
        state.config.allow_non_image_files
    );
}

/// Open the temp folder.
pub fn open_temp_folder() -> Result<()> {
    let temp_dir = crate::paths::temp_dir()?;

    if temp_dir.exists() {
        std::process::Command::new("explorer.exe")
            .arg(temp_dir.to_string_lossy().to_string())
            .spawn()
            .context("failed to open folder")?;
    }

    Ok(())
}
