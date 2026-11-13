use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Hotkey combination: "!v", "^!v", "!Enter"
    pub hotkey: String,

    /// Runtime mode: "safe" (IME guard), "fast" (lowest latency)
    pub runtime_mode: RuntimeMode,

    /// Paste format: "plain" (path), "attachment"
    pub paste_format: PasteFormat,

    /// Path paste style: "plain" (bare path), "at" (@ prefix), "quoted" (wrapped in quotes)
    /// Older config files lack this field; it defaults to plain
    #[serde(default)]
    pub path_style: PathStyle,

    /// IME guard policy (only effective when runtime_mode = "safe")
    /// Older config files lack this field; it defaults to imm
    #[serde(default)]
    pub ime_protection: ImeProtection,

    /// Whether non-image files copied in Explorer are also converted to WSL paths.
    /// Default false: only image extensions are converted, so Alt+V can't feed .env / key files to an agent.
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

/// Path paste style
/// - Plain: `/mnt/c/...`, recognised as an image directly by Claude Code / Codex / OpenCode
/// - At: `@/mnt/c/... `, for the @ file-reference syntax of Kimi Code CLI / Gemini CLI / Qwen Code
/// - Quoted: `"/mnt/c/..."`, for paths with spaces or for feeding straight into a shell command
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathStyle {
    #[default]
    Plain,
    At,
    Quoted,
}

impl PathStyle {
    /// Format a set of WSL paths into the final paste text
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
            // @ references need a space to end the token: multiple paths are space-separated and a trailing space is kept,
            // so the CLI finishes parsing the file reference immediately and the user can keep typing
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
            PathStyle::Quoted => "Quoted path (paths with spaces)",
        }
    }
}

/// IME guard policy (issue #10)
///
/// Background: early versions unconditionally called `LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)` **at startup**
/// to preload the English layout. `LoadKeyboardLayoutW` registers that layout in the **system** input-method list,
/// so as soon as this tool started, `Win + Space` gained a spurious
/// `ENG / English (United States)` entry and the taskbar IME indicator reappeared,
/// even if the user had never added English to their language list.
///
/// It is now handled lazily per policy, ranging from "zero side effects" to "fully backwards compatible":
/// - `Off`: Never touches the IME or keyboard layouts
/// - `Imm`: Only closes the foreground window's IME open status (direct input) without touching keyboard layouts; the default
/// - `Layout`: Switches to an English keyboard layout, but **only reuses one already loaded** in the system and never adds one
/// - `LayoutForce`: Lazily loads an English layout only when none is found (without KLF_ACTIVATE) and unloads it on exit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImeProtection {
    /// IME guard fully disabled
    Off,
    /// Only closes the foreground window's IME open status (no side effects, never adds a keyboard layout)
    #[default]
    Imm,
    /// Switch to an English keyboard layout, reusing only already-loaded layouts
    Layout,
    /// Switch to an English keyboard layout, lazily loading one if needed (unloaded on exit)
    LayoutForce,
}

impl ImeProtection {
    pub fn display_name(&self) -> &'static str {
        match self {
            ImeProtection::Off => "Off (do not touch the IME)",
            ImeProtection::Imm => "Close IME open status (recommended, no side effects)",
            ImeProtection::Layout => "Switch to English layout (installed layouts only)",
            ImeProtection::LayoutForce => "Switch to English layout (load ENG if needed)",
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

    /// Whether this policy may add an English layout to the system via LoadKeyboardLayoutW
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
                        .context("Failed to read legacy config file")?;
                    let config: Self = toml::from_str(&content)
                        .context("Failed to parse legacy config file")?;
                    config.save()?;
                    return Ok(config);
                }
            }

            let default = Self::default();
            default.save()?;
            return Ok(default);
        }

        let content = std::fs::read_to_string(&config_path)
            .context("Failed to read config file")?;

        let config: Self = toml::from_str(&content)
            .context("Failed to parse config file")?;

        Ok(config)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = crate::paths::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create config directory")?;
        }
        let content = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;

        std::fs::write(&path, content)
            .context("Failed to write config file")?;

        Ok(())
    }
}
