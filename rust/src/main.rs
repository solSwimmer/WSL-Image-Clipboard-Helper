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

/// Application runtime state (modifiable via tray commands)
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

    // Load configuration
    let app_config = config::AppConfig::load().unwrap_or_default();
    info!(
        "Loaded config: hotkey={}, mode={:?}, IME guard={}",
        app_config.hotkey,
        app_config.runtime_mode,
        app_config.ime_protection.as_str()
    );

    // Temp images and config live under %LOCALAPPDATA%\WSLImageClipboardHelper,
    // not next to the executable (so they can't end up in a git repo or shared folder).
    let temp_dir = paths::ensure_app_dirs()?;
    info!("Temp directory: {}", temp_dir.display());

    // Note (issue #10): the English keyboard layout is **no longer** preloaded here.
    // The old implementation unconditionally called LoadKeyboardLayoutW("00000409", KLF_ACTIVATE) at startup,
    // registering the English layout in the system input-method list and adding a spurious
    // `ENG / English (United States)` entry to Win+Space. The English layout is now resolved lazily and only when needed,
    // and by default only an existing system layout is reused; see config::ImeProtection.

    // Create the clipboard manager
    let clipboard_manager = ClipboardManager::new(temp_dir.clone());

    // Runtime state
    let state = Arc::new(Mutex::new(AppState {
        runtime_mode: app_config.runtime_mode.clone(),
        path_style: app_config.path_style,
        ime_protection: app_config.ime_protection,
        allow_non_image_files: app_config.allow_non_image_files,
    }));

    // Start the tray (including the hotkey manager)
    let std_tray_rx = tray::TrayController::start(app_config, temp_dir.clone())?;

    // Bridge the std mpsc channel to a tokio mpsc channel for use in select!
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

    // Start the hotkey bridge
    let mut hotkey_rx = hotkey::start_hotkey_bridge();

    // Periodic cleanup task
    let temp_dir_for_cleanup = temp_dir.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(900));
        loop {
            interval.tick().await;
            if let Err(e) = cleanup::cleanup_old_files(&temp_dir_for_cleanup) {
                warn!("Failed to clean up temp files: {}", e);
            }
        }
    });

    info!("WSL Clipboard Helper started");

    // Main event loop
    loop {
        tokio::select! {
            // Hotkey fired
            Some(_hotkey_id) = hotkey_rx.recv() => {
                let (mode, path_style, ime_protection, allow_non_image_files) = {
                    let s = state.lock().await;
                    (s.runtime_mode.clone(), s.path_style, s.ime_protection, s.allow_non_image_files)
                };
                match handle_paste(&clipboard_manager, &mode, path_style, ime_protection, allow_non_image_files).await {
                    Ok(_) => {}
                    Err(e) => {
                        error!("Paste failed: {}", e);
                    }
                }
            }
            // Tray command
            Some(cmd) = tray_rx.recv() => {
                match cmd {
                    TrayCommand::SwitchHotkey(ht) => {
                        info!("Main loop: hotkey switched to {}", ht.display_name());
                    }
                    TrayCommand::SwitchMode(mode) => {
                        info!("Main loop: mode switched to {:?}", mode);
                        let mut s = state.lock().await;
                        s.runtime_mode = mode;
                    }
                    TrayCommand::SwitchPathStyle(style) => {
                        info!("Main loop: path style switched to {}", style.display_name());
                        let mut s = state.lock().await;
                        s.path_style = style;
                    }
                    TrayCommand::SwitchImeProtection(policy) => {
                        info!("Main loop: IME guard switched to {}", policy.display_name());
                        let mut s = state.lock().await;
                        s.ime_protection = policy;
                    }
                    TrayCommand::SwitchAllowNonImageFiles(allow) => {
                        info!("Main loop: non-image file path conversion={}", allow);
                        let mut s = state.lock().await;
                        s.allow_non_image_files = allow;
                    }
                    TrayCommand::OpenFolder => {
                        if let Err(e) = tray::open_temp_folder() {
                            error!("Failed to open folder: {}", e);
                        }
                    }
                    TrayCommand::Exit => {
                        info!("Received exit command");
                        break;
                    }
                }
            }
            else => {
                // All channels closed
                break;
            }
        }
    }

    // Delete all PNG files in the temp directory before exiting
    if let Err(e) = cleanup::cleanup_temp_png(&temp_dir) {
        warn!("Failed to clean up temp files on exit: {}", e);
    }

    // If this process lazily loaded an English keyboard layout (ime_protection = "layout-force"),
    // unload it on exit so nothing is left in the user's input-method list (issue #10)
    paste::unload_self_loaded_layout();

    info!("WSL Clipboard Helper exited");
    std::process::exit(0);
}

/// Handle a paste request
async fn handle_paste(
    clipboard_manager: &ClipboardManager,
    mode: &RuntimeMode,
    path_style: PathStyle,
    ime_protection: ImeProtection,
    allow_non_image_files: bool,
) -> Result<()> {
    // 0. Inject the menu-mask key immediately, before the physical Alt is released,
    //    so the target window doesn't treat "Alt down -> up" as a lone Alt tap that activates the menu bar
    paste::mask_alt_tap();

    // 1. Check whether the clipboard holds an image
    if !clipboard_manager.has_image() {
        if clipboard_manager.has_file_list() {
            if let Some(wsl_paths) =
                clipboard_manager.read_file_list_for_paste(allow_non_image_files)
            {
                let paste_text = path_style.format_paths(&wsl_paths);
                if paste_text.is_empty() {
                    info!("All file paths were filtered out, skipping paste");
                    return Ok(());
                }

                let _ime_guard = match mode {
                    RuntimeMode::Safe => Some(paste::ImeGuard::new(ime_protection)),
                    RuntimeMode::Fast => None,
                };

                info!("Pasting file paths: {}", paste_text);
                paste::paste_text(&paste_text)?;
                return Ok(());
            }

            // The clipboard holds a file list but nothing is allowed to be pasted (e.g. a copied .env).
            // Do not fall back to Ctrl+V, or the raw file drop would be pasted into the current window.
            info!("Clipboard file list rejected (non-image or unsafe path), skipping paste");
            return Ok(());
        }

        info!("No image on the clipboard, doing a normal paste");
        paste::send_ctrl_v()?;
        return Ok(());
    }

    info!("Clipboard image detected");

    // 2. Read the image (cached)
    let (win_path, wsl_path, png_data) = clipboard_manager
        .read_image_for_paste()
        .ok_or_else(|| anyhow::anyhow!("Failed to read the clipboard image"))?;

    // 3. Save first, then paste: the moment the CLI receives the path it checks that the file exists,
    //    to decide between rendering [Image #n] and leaving the raw path text (issue #4)
    info!("Saving image: {} bytes -> {}", png_data.len(), win_path.display());
    image_saver::ensure_saved(&win_path, &png_data).await?;

    // 4. IME guard (safe mode only; the exact measure is set by the ime_protection policy)
    let _ime_guard = match mode {
        RuntimeMode::Safe => Some(paste::ImeGuard::new(ime_protection)),
        RuntimeMode::Fast => None,
    };

    // 5. Paste the WSL path
    let paste_text = path_style.format_paths(std::slice::from_ref(&wsl_path));
    info!("Pasting path: {}", paste_text);
    paste::paste_text(&paste_text)?;

    // 6. ImeGuard is dropped here, restoring the IME after 120 ms

    Ok(())
}
