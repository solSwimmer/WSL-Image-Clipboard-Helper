use anyhow::Result;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use tracing::{info, warn};

/// Available hotkeys
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyType {
    AltV,
    CtrlAltV,
    AltEnter,
}

impl HotkeyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyType::AltV => "!v",
            HotkeyType::CtrlAltV => "^!v",
            HotkeyType::AltEnter => "!Enter",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            HotkeyType::AltV => "Alt+V",
            HotkeyType::CtrlAltV => "Ctrl+Alt+V",
            HotkeyType::AltEnter => "Alt+Enter",
        }
    }

    pub fn from_config(s: &str) -> Option<Self> {
        match s {
            "!v" | "Alt+V" => Some(HotkeyType::AltV),
            "^!v" | "Ctrl+Alt+V" => Some(HotkeyType::CtrlAltV),
            "!Enter" | "Alt+Enter" => Some(HotkeyType::AltEnter),
            _ => None,
        }
    }

    pub fn all() -> &'static [HotkeyType] {
        &[HotkeyType::AltV, HotkeyType::CtrlAltV, HotkeyType::AltEnter]
    }
}

/// Simplified hotkey manager; must be created on a thread with a Win32 message loop
pub struct HotkeyManager {
    manager: GlobalHotKeyManager,
    current_hotkey: Option<HotKey>,
    current_type: HotkeyType,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            manager: GlobalHotKeyManager::new()?,
            current_hotkey: None,
            current_type: HotkeyType::AltV,
        })
    }

    /// Register a hotkey
    pub fn register(&mut self, hotkey_type: HotkeyType) -> Result<()> {
        if self.current_type == hotkey_type && self.current_hotkey.is_some() {
            return Ok(());
        }

        let previous_hotkey = self.current_hotkey.clone();
        let (mods, key) = parse_hotkey(hotkey_type.as_str())?;
        let new_hotkey = HotKey::new(Some(mods), key);

        // Register the new hotkey first so a failure doesn't lose the old binding
        self.manager.register(new_hotkey)?;

        // Unregister the old hotkey only after the new one registered; roll back the new one on failure
        if let Some(old_hotkey) = previous_hotkey {
            if let Err(e) = self.manager.unregister(old_hotkey) {
                if let Err(rollback_err) = self.manager.unregister(new_hotkey) {
                    warn!(
                        "Hotkey rollback failed: new hotkey={:?}, error={:?}",
                        hotkey_type,
                        rollback_err
                    );
                }
                return Err(e.into());
            }
        }

        self.current_hotkey = Some(new_hotkey);
        self.current_type = hotkey_type;

        info!("Registered hotkey: {}", hotkey_type.display_name());
        Ok(())
    }

    /// Unregister the current hotkey
    pub fn unregister(&mut self) -> Result<()> {
        if let Some(hotkey) = self.current_hotkey.take() {
            self.manager.unregister(hotkey)?;
            info!("Unregistered hotkey");
        }
        Ok(())
    }

    /// Get the current hotkey type
    pub fn current_type(&self) -> HotkeyType {
        self.current_type
    }
}

/// Start the hotkey bridge: listen for GlobalHotKeyEvent inside spawn_blocking,
/// and forward events to the main loop over a tokio mpsc channel
pub fn start_hotkey_bridge() -> tokio::sync::mpsc::Receiver<u32> {
    let (tx, rx) = tokio::sync::mpsc::channel::<u32>(32);

    tokio::task::spawn_blocking(move || {
        let receiver = GlobalHotKeyEvent::receiver();
        loop {
            match receiver.recv() {
                Ok(event) => {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        if tx.blocking_send(event.id).is_err() {
                            break; // receiver closed
                        }
                    }
                }
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    });

    rx
}

/// Parse a hotkey combination string
fn parse_hotkey(combo: &str) -> Result<(Modifiers, Code)> {
    let combo_lower = combo.to_lowercase();
    let mut mods = Modifiers::empty();

    if combo_lower.contains("ctrl") || combo_lower.contains("control") || combo.contains('^') {
        mods |= Modifiers::CONTROL;
    }
    if combo_lower.contains("alt") || combo.contains('!') {
        mods |= Modifiers::ALT;
    }
    if combo_lower.contains("shift") || combo.contains('+') {
        mods |= Modifiers::SHIFT;
    }

    // Extract the key
    let key_str = combo
        .replace("Ctrl+", "")
        .replace("Control+", "")
        .replace("Alt+", "")
        .replace("Shift+", "")
        .replace('!', "")
        .replace('^', "")
        .replace('+', "")
        .trim()
        .to_lowercase();

    let key = parse_key_code(&key_str)
        .ok_or_else(|| anyhow::anyhow!("Invalid key name: {}", key_str))?;

    Ok((mods, key))
}

fn parse_key_code(s: &str) -> Option<Code> {
    match s {
        "v" => Some(Code::KeyV),
        "enter" => Some(Code::Enter),
        "c" => Some(Code::KeyC),
        "a" => Some(Code::KeyA),
        "x" => Some(Code::KeyX),
        _ => None,
    }
}
