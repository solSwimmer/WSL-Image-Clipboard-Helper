use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Application config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Hotkey combo: "!v", "^!v", "!Enter"
    pub hotkey: String,

    /// Runtime mode: "safe" (IME guard), "fast" (low latency)
    pub runtime_mode: RuntimeMode,

    /// Paste format: "plain" (path), "attachment" (attachment)
    pub paste_format: PasteFormat,

    /// Path paste style: "plain" (raw path), "at" (`@` prefix), "quoted" (quoted)
    /// Older config files omit this field; default is plain.
    #[serde(default)]
    pub path_style: PathStyle,

    /// IME guard strategy (only applies when runtime_mode = "safe")
    /// Older config files omit this field; default is imm.
    #[serde(default)]
    pub ime_protection: ImeProtection,

    /// Also convert non-image files copied in Explorer to WSL paths.
    /// Default false: image extensions only, so Alt+V does not feed `.env` / secrets to the agent.
    #[serde(default)]
    pub allow_non_image_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    Safe,
    Fast,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PasteFormat {
    Plain,
    Attachment,
}

/// Path paste style.
/// - Plain: `/mnt/c/...`, which Claude Code / Codex / OpenCode can treat as an image
/// - At: `@/mnt/c/... `, `@file` syntax used by Kimi Code CLI / Gemini CLI / Qwen Code
/// - Quoted: `"/mnt/c/..."`, for paths with spaces or feeding a shell command
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathStyle {
    #[default]
    Plain,
    At,
    Quoted,
}

impl PathStyle {
    /// Format a set of WSL paths into the final paste text.
    pub fn format_paths(&self, paths: &[String]) -> String {
        let paths: Vec<String> = paths
            .iter()
            .filter_map(|p| crate::paths::sanitize_pasted_path(p))
            .collect();

        match self {
            PathStyle::Plain => paths.join("\n"),
            PathStyle::Quoted => paths
                .iter()
                .map(|p| crate::paths::quote_wsl_path(p))
                .collect::<Vec<_>>()
                .join("\n"),
            // `@` references are space-delimited; keep a trailing space so the CLI
            // can finish parsing the file reference and the user can keep typing.
            PathStyle::At => {
                let joined = paths
                    .iter()
                    .map(|p| format!("@{}", p))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{} ", joined)
            }
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            PathStyle::Plain => "Plain path (Claude/Codex/OpenCode)",
            PathStyle::At => "@ path (Kimi/Gemini/Qwen)",
            PathStyle::Quoted => "Quoted path (spaces)",
        }
    }
}

/// IME guard strategy (issue #10).
///
/// Background: early builds called `LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)`
/// **at startup** to preload the English layout. `LoadKeyboardLayoutW` registers
/// that layout with the **system** input list, so launching the helper added a
/// phantom `ENG / English (United States)` entry in `Win + Space` and brought
/// the taskbar IME icon back — even when the user had never added English.
///
/// Handling is now lazy and policy-driven, from "zero side effects" up to
/// "fully compatible with the old behaviour":
/// - `Off`: never touch IME or keyboard layouts
/// - `Imm`: close only the foreground window's IME open status (direct input); do not touch layouts — default
/// - `Layout`: switch to an English layout, but **only reuse one already loaded**; never add one
/// - `LayoutForce`: load English lazily if none is found (without KLF_ACTIVATE), and unload it on exit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImeProtection {
    /// Disable IME protection entirely.
    Off,
    /// Close only the foreground window's IME open status (zero side effects; does not add a layout).
    #[default]
    Imm,
    /// Switch to an English keyboard layout, but only reuse one already loaded.
    Layout,
    /// Switch to an English keyboard layout, loading it lazily if needed (unloaded on exit).
    LayoutForce,
}

impl ImeProtection {
    pub fn display_name(&self) -> &'static str {
        match self {
            ImeProtection::Off => "Off (do not touch IME)",
            ImeProtection::Imm => "Close IME input (recommended, no side effects)",
            ImeProtection::Layout => "English layout (reuse installed only)",
            ImeProtection::LayoutForce => "English layout (load ENG if needed)",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ImeProtection::Off => "off",
            ImeProtection::Imm => "imm",
            ImeProtection::Layout => "layout",
            ImeProtection::LayoutForce => "layout-force",
        }
    }

    /// Whether this policy may add an English layout via LoadKeyboardLayoutW.
    pub fn allows_loading_layout(&self) -> bool {
        matches!(self, ImeProtection::LayoutForce)
    }

    pub fn all() -> &'static [ImeProtection] {
        &[
            ImeProtection::Off,
            ImeProtection::Imm,
            ImeProtection::Layout,
            ImeProtection::LayoutForce,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::PathStyle;

    #[test]
    fn quoted_style_escapes_and_plain_drops_unsafe() {
        let paths = vec![
            "/mnt/c/foo bar.png".into(),
            "/mnt/c/bad\n/etc/passwd".into(),
        ];
        assert_eq!(
            PathStyle::Quoted.format_paths(&paths),
            "\"/mnt/c/foo bar.png\""
        );
        assert_eq!(PathStyle::Plain.format_paths(&paths), "/mnt/c/foo bar.png");
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: "!v".to_string(),
            runtime_mode: RuntimeMode::Fast,
            paste_format: PasteFormat::Plain,
            path_style: PathStyle::default(),
            ime_protection: ImeProtection::default(),
            allow_non_image_files: false,
        }
    }
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = crate::paths::config_path()?;

        if !config_path.exists() {
            if let Some(portable) = crate::paths::portable_config_path() {
                if portable.exists() {
                    let content = std::fs::read_to_string(&portable)
                        .context("failed to read legacy config file")?;
                    let config: Self = toml::from_str(&content)
                        .context("failed to parse legacy config file")?;
                    config.save()?;
                    return Ok(config);
                }
            }

            let default = Self::default();
            default.save()?;
            return Ok(default);
        }

        let content = std::fs::read_to_string(&config_path)
            .context("failed to read config file")?;

        let config: Self = toml::from_str(&content)
            .context("failed to parse config file")?;

        Ok(config)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = crate::paths::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("failed to create config directory")?;
        }
        let content = toml::to_string_pretty(self)
            .context("failed to serialize config")?;

        std::fs::write(&path, content)
            .context("failed to write config file")?;

        Ok(())
    }
}
