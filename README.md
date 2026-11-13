<a id="wsl-image-clipboard-helper"></a>

<p align="center">
  <img src="img/logo.png" alt="WSL Image Clipboard Helper logo" width="128" height="128">
</p>

<h1 align="center">WSL Image Clipboard Helper</h1>

<p align="center">
  One hotkey to turn Windows clipboard images into WSL-ready paths for AI CLI tools.
</p>

<p align="center">
  <a href="#english-guide">English</a>
  ·
  <a href="SECURITY.md">Security</a>
</p>

<p align="center">
  Security-hardened fork of <a href="https://github.com/cpulxb/WSL-Image-Clipboard-Helper">cpulxb/WSL-Image-Clipboard-Helper</a>. Build from source; this tree does not ship <code>.exe</code> files.
</p>

---

## English Guide 🌐

### 📌 Overview

#### 🌍 Background
Many AI CLI agents (Codex, Amazon Q Developer CLI, OpenCode, Claude Code, etc.) are optimized for Linux/macOS workflows. On Windows, users usually rely on WSL2, but clipboard image handling is still a practical gap:

- Problem: WSL2 terminals cannot directly consume image bytes from Windows clipboard
- Impact: screenshot-to-agent flow is less direct than native Linux/macOS
- Workaround: save image to file and pass file path to the tool

#### ✅ Solution
This project automates that workaround with a global hotkey (default `Alt+V`): it captures clipboard image data, saves a PNG under `%LOCALAPPDATA%\WSLImageClipboardHelper\temp`, and pastes the WSL path (`/mnt/...`) into the active input control.

Current mainline is Rust (`v4.2`). **Build from source.** This fork does not ship `.exe` files in git.

> This is an input agent: global hotkey, clipboard access, `SendInput` into the focused window. Many AI CLIs skip the file-read permission prompt when the pasted path already exists. Do not hotkey password-manager screenshots, `.env` files, or secrets. See [SECURITY.md](SECURITY.md).

### ✨ Highlights

- 🚀 Fast path-first paste workflow
- ⚡ Async image persistence
- 🌐 IME guard in compatibility mode — closes the foreground IME before pasting and restores it afterwards, without altering system keyboard layouts (switchable, and fully disableable)
- 🖱️ Tray-based hotkey and mode switching
- 🧹 Automatic cleanup for temporary PNG files
- 🛡️ Safer clipboard parsing with memory-bound checks
- 🪟 Embedded multi-size app icons and a DPI-aware manifest
- 📁 Explorer file path conversion: **image files only** by default; non-images are ignored unless you enable `Allow non-image files` in the tray
- 🎯 Switchable path style: plain / `@`-prefixed (for Kimi Code CLI, Gemini CLI, Qwen Code) / quoted

![clip_20260217_184919_809](./img/clip_20260217_184919_809.png)

Using this image as an example, pasting works correctly in Codex, Claude Code, and OpenCode (you can also try other agent tools). During paste, many tools render it as `[Image #n]`; when that rendering path is unavailable, the input falls back to showing the file path in the text box.

`codex`

![image-20260217185539242](./img/image-20260217185539242.png)

`claude`

![image-20260217185656617](./img/Snipaste_2026-02-17_18-56-35.jpg)

`openCode`

![image-20260217185731979](./img/image-20260217185731979.png)

### 🧰 Requirements

- Windows 10/11 with WSL2
- PowerShell 5.1+
- Rust toolchain (required; this is the supported install path)
- AutoHotkey v2 (deprecated — do not rebuild the AHK helper)

### 🗂️ Directory Structure

```text
WSL-Image-Clipboard-Helper/
├── README.md
├── LICENSE
├── SECURITY.md
├── docs/
├── rust/                    # only supported implementation
└── scripts/                 # deprecated AHK / PowerShell (see scripts/DEPRECATED.md)
```

### 🚀 Usage (Rust version)

1. Clone this fork and build it (the only supported install):

   ```bash
   git clone git@github.com:solSwimmer/WSL-Image-Clipboard-Helper.git
   cd WSL-Image-Clipboard-Helper/rust
   cargo build --release --target x86_64-pc-windows-msvc
   ```

   Output: `rust/target/x86_64-pc-windows-msvc/release/wsl_clipboard.exe`

2. Put that exe in a **fixed, non-project** folder and launch it. Do not drop it inside a git repo.

   Runtime files live in `%LOCALAPPDATA%\WSLImageClipboardHelper\` (`wsl_clipboard.toml` and `temp\`). The directory ACL is restricted to the current user and SYSTEM.

3. Press the hotkey (default `Alt+V`) in any editable field:
   - With an image: save under `%LOCALAPPDATA%\WSLImageClipboardHelper\temp` and paste the `/mnt/...` path.
   - With no image: fall back to a normal `Ctrl+V`.
   - In Explorer, copy **image files**, then press `Alt+V` to paste WSL paths. Non-images are ignored by default.

4. Use the tray menu for hotkey/mode/file-type switches, and `Exit and clean temporary images` when quitting.

### 🧹 Uninstall

1. Tray icon → `Exit and clean temporary images`.
2. Remove `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\wsl_clipboard.bat` if you added one.
3. Delete the folder that holds the exe.
4. Delete `%LOCALAPPDATA%\WSLImageClipboardHelper`.
5. If an old build added `ENG` to your language list, remove it under Settings → Time & language → Language & region.

### ⚠️ Notes

- The default hotkey is `Alt+V`; the tray menu can switch it to `Ctrl+Alt+V` or `Alt+Enter`.
- Runtime settings are stored in `%LOCALAPPDATA%\WSLImageClipboardHelper\wsl_clipboard.toml`.
- `Exit and clean temporary images` removes PNGs under that `temp/` folder (PNGs older than 30 minutes are also cleaned periodically).
- **Do not run this helper as Administrator.** UIPI blocking injected keys is a protection, not a bug.
- If IME state causes paste issues, switch back to `Compatibility mode (IME guard)`.
- To stop the helper from touching your IME at all, pick `Off (do not touch the IME)` under the tray `IME guard` submenu.
- If the tray icon is not visible, check the hidden icons area in the Windows taskbar.
- Plain paths are pasted by default; for CLIs that need `@` file references (e.g. Kimi Code CLI), switch `Path style` in the tray menu.

### ⌨️ IME guard strategies

Effective only in compatibility mode. Switch it in the tray `IME guard` submenu, or set `ime_protection` in `wsl_clipboard.toml`:

| Value | Behaviour | Touches the system IME list? |
| --- | --- | --- |
| `off` | Never touches the IME | No |
| `imm` (default) | Only closes the foreground window's IME open status, restoring it after the paste | No |
| `layout` | Switches to an English keyboard layout, but **only reuses** one already installed; skips otherwise | No |
| `layout-force` | Loads `ENG` on demand (without activating it) and unloads it on exit | Only this one, temporarily |

### ❓ FAQ

**Q1: Why do I sometimes get `[Image #1]` and sometimes a literal `/mnt/...` path?** ([issue #4](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/4))

CLI agents check whether the pasted path exists **at the moment the paste arrives**; if the file is there, it renders as an `[Image #n]` attachment, otherwise the raw path stays as text and the agent later needs file-read permission (and possibly a Windows-path retry when it runs outside WSL). Versions up to v4.0 pasted the path first and saved the PNG in the background, so under load the file could land after the check. Since v4.1 the image is written to disk **before** the path is pasted, removing that race.

**Q2: Hotkey fires, the PNG appears in `temp/`, IME switches — but no text is pasted anywhere.** ([issue #2](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/2))

Most likely causes, in order: (1) you are on the v3.0 AHK build, which restored the clipboard 80 ms after pasting — slow target windows read the restored image instead of the path text; upgrade to the Rust build. (2) The target window runs elevated while the helper does not — Windows UIPI silently drops injected keys. **Do not elevate this helper**; use a non-elevated terminal instead. (3) Hotkey modifiers still held — injected Ctrl+V turns into Ctrl+Alt+V; v4.1 waits for physical key release and adds an Alt menu-mask key. (4) Security software blocking `SendInput` — expected; do not disable AV for this tool.

**Q3: Kimi Code CLI does not turn the pasted path into an image.** ([issue #5](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/5))

Kimi Code CLI does not auto-detect plain path text, but it supports `@file` references. Switch the tray `Path style` to the `@` option and the hotkey pastes `@/mnt/c/... ` (with a trailing space), which Kimi Code CLI / Gemini CLI / Qwen Code consume as a file reference. Note Kimi's own Windows build binds `Alt+V` for clipboard images; this helper's global hotkey takes priority, so switch one of them (e.g. this helper to `Ctrl+Alt+V`) if you want both behaviors.

**Q4: Paths containing spaces get cut off in the shell.**

Switch the path style to the quoted option; every path is then wrapped in double quotes.

**Q5: After launching the helper, an extra `ENG / English (United States)` shows up in `Win + Space`.** ([issue #10](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/10))

Up to v4.1.2 the helper called `LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)` unconditionally **at startup**. `LoadKeyboardLayoutW` registers the layout with the **system** input list and `KLF_ACTIVATE` activates it right away, so `ENG` appeared — and the taskbar IME indicator came back — even for users who never added English to their language list.

Since v4.1.3 the helper performs no keyboard-layout work at startup, resolves the English layout lazily, and by default only reuses layouts the system already has. The new default strategy `imm` never touches keyboard layouts at all; `layout-force` no longer uses `KLF_ACTIVATE`, passes `KLF_NOTELLSHELL`, and calls `UnloadKeyboardLayout` on exit. Set `ime_protection = "off"` to disable the guard entirely.

If an earlier version already left English (United States) in your language list, remove it once under `Settings → Time & language → Language & region`; the new build will not add it back.

### 🛠️ Build (Rust)

```bash
cd rust
cargo build --release --target x86_64-pc-windows-msvc
```

Binary output:

```text
rust/target/x86_64-pc-windows-msvc/release/wsl_clipboard.exe
```

Clean build artifacts:

```bash
cd rust
cargo clean
```

### 🕒 Version Line

- `v4.2.0`: this fork — no committed binaries, AppData temp + ACL, image-only Explorer paste, tag-only releases
- `v4.1.3`: no more phantom `ENG` keyboard layout on startup — lazy English-layout resolution plus the new `ime_protection` setting (#10)
- `v4.1.2`: dismiss the target window's menu mode (fast Alt release) before injecting Ctrl+V so pastes are not swallowed
- `v4.1.1`: treat all-zero alpha in 32-bit clipboard DIBs as opaque so saved PNGs are not fully transparent
- `v4.1`: save-before-paste ordering fix (#4), switchable path style incl. `@` references for Kimi/Gemini/Qwen (#5), hardened key injection (#2)
- `v4.0`: Rust mainline release with embedded multi-size icons, DPI-aware manifest, and Explorer-to-WSL path paste
- `v3.0`: Hotkey-focused revision on AHK
- `v2.0`: AHK path-first optimization
- `v1.0`: AHK baseline

### 📚 Additional Resources

- [Architecture & Workflow Details](docs/architecture_by_codex.md)
- [V3.0/V4.0 Refactor Notes](docs/rust-refactor-v3-v4.md)
