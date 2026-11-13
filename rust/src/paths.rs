use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const APP_DIR_NAME: &str = "WSLImageClipboardHelper";

/// Per-user data directory (`%LOCALAPPDATA%\WSLImageClipboardHelper` on Windows).
///
/// Temp PNGs and config used to live next to the executable, which meant
/// screenshots landed in whatever folder the user dropped the exe into
/// (project trees, git repos, shared drives). AppData is user-private by default.
pub fn app_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir().unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    });
    Ok(base.join(APP_DIR_NAME))
}

pub fn temp_dir() -> Result<PathBuf> {
    Ok(app_dir()?.join("temp"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(app_dir()?.join("wsl_clipboard.toml"))
}

/// Config next to the executable, used only to migrate older installs.
pub fn portable_config_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("wsl_clipboard.toml")))
}

pub fn ensure_app_dirs() -> Result<PathBuf> {
    let app = app_dir()?;
    let temp = app.join("temp");
    std::fs::create_dir_all(&temp).context("Failed to create app data directory")?;
    restrict_directory_acl(&app);
    Ok(temp)
}

/// Drop inherited ACLs so only the current user and SYSTEM can read temp images.
#[cfg(windows)]
pub fn restrict_directory_acl(dir: &Path) {
    let path = dir.to_string_lossy().to_string();
    let user = match std::env::var("USERNAME") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            warn!("Could not read USERNAME, skipping directory ACL hardening");
            return;
        }
    };
    let grant_user = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() && !domain.eq_ignore_ascii_case(&user) => {
            format!(r"{domain}\{user}:(OI)(CI)F")
        }
        _ => format!(r"{user}:(OI)(CI)F"),
    };

    let status = std::process::Command::new("icacls.exe")
        .args([
            &path,
            "/inheritance:r",
            "/grant:r",
            &grant_user,
            "/grant:r",
            r"SYSTEM:(OI)(CI)F",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => info!("Tightened directory ACL: {}", dir.display()),
        Ok(s) => warn!("icacls failed (status={s}), directory {}", dir.display()),
        Err(e) => warn!("Could not run icacls: {e}"),
    }
}

#[cfg(not(windows))]
pub fn restrict_directory_acl(_dir: &Path) {}

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico",
];

pub fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|ok| e.eq_ignore_ascii_case(ok))
        })
        .unwrap_or(false)
}

/// Drop clipboard-derived paths that could inject extra lines or shell metacharacters
/// into the focused CLI.
pub fn sanitize_pasted_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control() || c == '`' || c == '$') {
        return None;
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return None;
    }
    Some(trimmed.to_string())
}

pub fn quote_wsl_path(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::{is_image_path, quote_wsl_path, sanitize_pasted_path};

    #[test]
    fn image_extension_filter() {
        assert!(is_image_path(r"C:\tmp\clip.PNG"));
        assert!(is_image_path("/mnt/c/tmp/photo.webp"));
        assert!(!is_image_path(r"C:\Users\me\.env"));
        assert!(!is_image_path(r"C:\Users\me\id_rsa"));
        assert!(!is_image_path(r"C:\Users\me\notes.txt"));
    }

    #[test]
    fn sanitize_rejects_control_and_shell_chars() {
        assert_eq!(
            sanitize_pasted_path("/mnt/c/temp/clip.png"),
            Some("/mnt/c/temp/clip.png".into())
        );
        assert!(sanitize_pasted_path("/mnt/c/temp/clip.png\n/etc/passwd").is_none());
        assert!(sanitize_pasted_path("/mnt/c/temp/`whoami`.png").is_none());
        assert!(sanitize_pasted_path("/mnt/c/temp/$HOME/clip.png").is_none());
    }

    #[test]
    fn quoted_paths_escape_inner_quotes() {
        assert_eq!(quote_wsl_path("/mnt/c/foo bar.png"), "\"/mnt/c/foo bar.png\"");
        assert_eq!(
            quote_wsl_path(r#"/mnt/c/say"hi".png"#),
            r#""/mnt/c/say\"hi\".png""#
        );
    }
}
