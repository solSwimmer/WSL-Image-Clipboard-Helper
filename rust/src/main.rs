#![windows_subsystem = "windows"]

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

mod clipboard;
mod cleanup;
mod config;
mod hotkey;
mod image_saver;
mod paste;
mod paths;
mod tray;

use clipboard::ClipboardManager;
use config::{ImeProtection, PathStyle, RuntimeMode};
use tray::TrayCommand;

/// Runtime state (updated by tray commands).
struct AppState {
    runtime_mode: RuntimeMode,
    path_style: PathStyle,
    ime_protection: ImeProtection,
    allow_non_image_files: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!(
        "WSL Clipboard Helper v{} (Rust) starting...",
        env!("CARGO_PKG_VERSION")
    );

    // Load config.
    let app_config = config::AppConfig::load().unwrap_or_default();
    info!(
        "loaded config: hotkey={}, mode={:?}, ime_protection={}",
        app_config.hotkey,
        app_config.runtime_mode,
        app_config.ime_protection.as_str()
    );

    // Temp images and config live under %LOCALAPPDATA%\WSLImageClipboardHelper,
    // not next to the executable (so they do not land in a git repo or shared folder).
    let temp_dir = paths::ensure_app_dirs()?;
    info!("temp directory: {}", temp_dir.display());

    // Note (issue #10): do **not** preload an English keyboard layout here.
    // The old implementation called LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)
    // unconditionally at startup, which registered the English layout in the system
    // IME list and added a phantom `ENG / English (United States)` entry in
    // Win+Space. English layouts are now resolved lazily, and only reused from
    // layouts the system already has. See config::ImeProtection.

    // Create the clipboard manager.
    let clipboard_manager = ClipboardManager::new(temp_dir.clone());

    // Runtime state.
    let state = Arc::new(Mutex::new(AppState {
        runtime_mode: app_config.runtime_mode.clone(),
        path_style: app_config.path_style,
        ime_protection: app_config.ime_protection,
        allow_non_image_files: app_config.allow_non_image_files,
    }));

    // Start the tray (includes the hotkey manager).
    let std_tray_rx = tray::TrayController::start(app_config, temp_dir.clone())?;

    // Bridge std mpsc to tokio mpsc so it can be used in select!.
    let (tray_tx_bridge, mut tray_rx) = mpsc::channel::<TrayCommand>(32);
    tokio::task::spawn_blocking(move || {
        loop {
            match std_tray_rx.recv() {
                Ok(cmd) => {
                    if tray_tx_bridge.blocking_send(cmd).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Start the hotkey bridge.
    let mut hotkey_rx = hotkey::start_hotkey_bridge();

    // Periodic cleanup.
    let temp_dir_for_cleanup = temp_dir.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(900));
        loop {
            interval.tick().await;
            if let Err(e) = cleanup::cleanup_old_files(&temp_dir_for_cleanup) {
                warn!("temp file cleanup failed: {}", e);
            }
        }
    });

    info!("WSL Clipboard Helper started");

    // Main event loop.
    loop {
        tokio::select! {
            // Hotkey fired.
            Some(_hotkey_id) = hotkey_rx.recv() => {
                let (mode, path_style, ime_protection, allow_non_image_files) = {
                    let s = state.lock().await;
                    (s.runtime_mode.clone(), s.path_style, s.ime_protection, s.allow_non_image_files)
                };
                match handle_paste(&clipboard_manager, &mode, path_style, ime_protection, allow_non_image_files).await {
                    Ok(_) => {}
                    Err(e) => {
                        error!("paste handling failed: {}", e);
                    }
                }
            }
            // Tray command.
            Some(cmd) = tray_rx.recv() => {
                match cmd {
                    TrayCommand::SwitchHotkey(ht) => {
                        info!("main loop: hotkey switched to {}", ht.display_name());
                    }
                    TrayCommand::SwitchMode(mode) => {
                        info!("main loop: mode switched to {:?}", mode);
                        let mut s = state.lock().await;
                        s.runtime_mode = mode;
                    }
                    TrayCommand::SwitchPathStyle(style) => {
                        info!("main loop: path style switched to {}", style.display_name());
                        let mut s = state.lock().await;
                        s.path_style = style;
                    }
                    TrayCommand::SwitchImeProtection(policy) => {
                        info!("main loop: IME guard switched to {}", policy.display_name());
                        let mut s = state.lock().await;
                        s.ime_protection = policy;
                    }
                    TrayCommand::SwitchAllowNonImageFiles(allow) => {
                        info!("main loop: non-image file path conversion={}", allow);
                        let mut s = state.lock().await;
                        s.allow_non_image_files = allow;
                    }
                    TrayCommand::OpenFolder => {
                        if let Err(e) = tray::open_temp_folder() {
                            error!("failed to open folder: {}", e);
                        }
                    }
                    TrayCommand::Exit => {
                        info!("exit command received");
                        break;
                    }
                }
            }
            else => {
                // All channels closed.
                break;
            }
        }
    }

    // Delete all PNGs under temp before exiting.
    if let Err(e) = cleanup::cleanup_temp_png(&temp_dir) {
        warn!("exit temp cleanup failed: {}", e);
    }

    // If this process lazily loaded an English keyboard layout
    // (ime_protection = "layout-force"), unload it so it is not left in the
    // user's IME switch list (issue #10).
    paste::unload_self_loaded_layout();

    info!("WSL Clipboard Helper exited");
    std::process::exit(0);
}

/// Handle a paste request.
async fn handle_paste(
    clipboard_manager: &ClipboardManager,
    mode: &RuntimeMode,
    path_style: PathStyle,
    ime_protection: ImeProtection,
    allow_non_image_files: bool,
) -> Result<()> {
    // 0. Inject the menu-mask key immediately, before physical Alt is released,
    //    so the target window does not treat "Alt down → up" as an Alt tap that
    //    opens the menu bar.
    paste::mask_alt_tap();

    // 1. Check whether the clipboard has an image.
    if !clipboard_manager.has_image() {
        if clipboard_manager.has_file_list() {
            if let Some(wsl_paths) =
                clipboard_manager.read_file_list_for_paste(allow_non_image_files)
            {
                let paste_text = path_style.format_paths(&wsl_paths);
                if paste_text.is_empty() {
                    info!("all file paths were filtered; skip paste");
                    return Ok(());
                }

                let _ime_guard = match mode {
                    RuntimeMode::Safe => Some(paste::ImeGuard::new(ime_protection)),
                    RuntimeMode::Fast => None,
                };

                info!("pasting file paths: {}", paste_text);
                paste::paste_text(&paste_text)?;
                return Ok(());
            }

            // Clipboard is a file list but nothing is allowed to paste (e.g. a copied .env).
            // Do not fall back to Ctrl+V, or the original files would be dropped into the window.
            info!("clipboard file list rejected (non-image or unsafe path); skip paste");
            return Ok(());
        }

        info!("no image on clipboard; falling back to normal paste");
        paste::send_ctrl_v()?;
        return Ok(());
    }

    info!("clipboard image detected");

    // 2. Read the image (with cache).
    let (win_path, wsl_path, png_data) = clipboard_manager
        .read_image_for_paste()
        .ok_or_else(|| anyhow::anyhow!("failed to read clipboard image"))?;

    // 3. Write to disk before pasting: the CLI checks file existence the instant
    //    it receives the path, and that decides [Image #n] vs a raw path (issue #4).
    info!("saving image: {} bytes → {}", png_data.len(), win_path.display());
    image_saver::ensure_saved(&win_path, &png_data).await?;

    // 4. IME guard (safe mode only; the exact method is chosen by ime_protection).
    let _ime_guard = match mode {
        RuntimeMode::Safe => Some(paste::ImeGuard::new(ime_protection)),
        RuntimeMode::Fast => None,
    };

    // 5. Paste the WSL path.
    let paste_text = path_style.format_paths(std::slice::from_ref(&wsl_path));
    info!("pasting path: {}", paste_text);
    paste::paste_text(&paste_text)?;

    // 6. ImeGuard drops here and restores IME after 120 ms.

    Ok(())
}
