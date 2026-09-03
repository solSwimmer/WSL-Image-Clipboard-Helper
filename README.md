<a id="wsl-image-clipboard-helper"></a>

<p align="center">
  <img src="img/logo.png" alt="WSL Image Clipboard Helper logo" width="128" height="128">
</p>

<h1 align="center">WSL Image Clipboard Helper</h1>

<p align="center">
  One hotkey to turn Windows clipboard images into WSL-ready paths for AI CLI tools.
</p>

<p align="center">
  <a href="SECURITY.md">Security</a>
</p>

<p align="center">
  Security-hardened fork of <a href="https://github.com/cpulxb/WSL-Image-Clipboard-Helper">cpulxb/WSL-Image-Clipboard-Helper</a>. Build from source; this tree does not ship <code>.exe</code> files.
</p>

---

### 📌 Overview

#### 🌍 Background
Many AI CLI agents (Codex, Amazon Q Developer CLI, OpenCode, Claude Code, etc.) are optimized for Linux/macOS workflows. On Windows, users usually rely on WSL2, but clipboard image handling is still a practical gap:

- Problem: WSL2 terminals cannot directly consume image bytes from the Windows clipboard
- Impact: screenshot-to-agent flow is less direct than native Linux/macOS
- Workaround: save the image to a file and pass the file path to the tool

#### ✅ Solution
This project automates that workaround with a global hotkey (default `Alt+V`): it captures clipboard image data, saves a PNG under `%LOCALAPPDATA%\WSLImageClipboardHelper\temp`, and pastes the WSL path (`/mnt/...`) into the active input control so the AI tool can consume the image file.

Current mainline is Rust (`v4.2`). **Build from source.** This fork does not ship `.exe` files in git.

> This is an input agent: it registers a global hotkey, reads the clipboard, and injects `Ctrl+V` into the focused window. Many AI CLIs skip the file-read permission prompt when the pasted path already exists. Do not hotkey password-manager screenshots, `.env` files, or secrets. See [SECURITY.md](SECURITY.md).

### ✨ Highlights

- 🚀 Fast path-first paste: the hotkey prefers pasting a `/mnt/...` path so you wait less
- ⚡ Image is written to disk before the path is pasted (Rust mainline; see FAQ Q1)
- 🌐 IME guard in compatibility mode — closes the foreground IME before pasting and restores it afterwards, without altering system keyboard layouts (switchable, and fully disableable)
- 🧹 Automatic cleanup: expired PNGs are cleaned periodically; temp images are removed on exit
- 🖱️ Tray-based hotkey, run-mode, and temp-folder management
- 🛡️ Safer clipboard parsing with DIB-header and memory-bound checks
- 🪟 Embedded multi-size app icons and a DPI-aware manifest
- 📁 Explorer file path conversion: **image files only** by default; non-images (`.env` / secrets) are ignored unless you enable `Allow non-image file conversion` in the tray
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
│   ├── Cargo.toml
│   ├── Cargo.lock
│   └── src/
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

2. Put that exe in a **fixed, non-project** folder and double-click to launch. Do not drop it inside a git repo.

   Runtime files live in `%LOCALAPPDATA%\WSLImageClipboardHelper\` (`wsl_clipboard.toml` and `temp\`). The directory ACL is restricted to the current user and SYSTEM.

3. Press the hotkey (default `Alt+V`) in any editable field:
   - With an image: save under `%LOCALAPPDATA%\WSLImageClipboardHelper\temp` and paste the `/mnt/...` path
   - With no image: fall back to a normal `Ctrl+V`
   - In Explorer, copy **image files**, then press `Alt+V` to paste WSL paths. Non-images are ignored by default

4. Use the tray menu to switch hotkey, run mode, and whether non-image files are allowed.

5. Quit from the tray with `Exit and clean temporary images`.

### 🧹 Uninstall

1. Tray icon → `Exit and clean temporary images` (stops the process and deletes temp PNGs).
2. Remove `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\wsl_clipboard.bat` if you added one.
3. Delete the folder that holds the exe.
4. Delete `%LOCALAPPDATA%\WSLImageClipboardHelper`.
5. If an old build added `ENG` to your language list, remove it under Settings → Time & language → Language & region.

### ⚠️ Notes

- The default hotkey is `Alt+V`; the tray menu can switch it to `Ctrl+Alt+V` or `Alt+Enter`.
- Runtime settings are stored in `%LOCALAPPDATA%\WSLImageClipboardHelper\wsl_clipboard.toml`.
- `Exit and clean temporary images` removes PNGs under that `temp/` folder (PNGs older than 30 minutes are also cleaned periodically).
- **Do not run this helper as Administrator.** UIPI blocking injected keys is a protection, not a bug. Elevating it lets it type into any elevated window.
- If IME state causes paste issues, switch back to `Compatibility mode (IME guard)`.
- To stop the helper from touching your IME at all, pick `Off (do not touch IME)` under the tray `IME guard` submenu.
- If the tray icon is not visible, check the hidden icons area in the Windows taskbar.
- Plain paths are pasted by default; for CLIs that need `@` file references (e.g. Kimi Code CLI), switch `Path style` in the tray menu.

### ⌨️ IME guard strategies

Effective only when `Run mode` is compatibility mode. Switch it in the tray `IME guard` submenu, or set `ime_protection` in `wsl_clipboard.toml`:

| Value | Behaviour | Touches the system IME list? |
| --- | --- | --- |
| `off` | Never touches the IME | No |
| `imm` (default) | Only closes the foreground window's IME open status (direct input), restoring it after the paste | No |
| `layout` | Switches to an English keyboard layout, but **only reuses** one already installed; skips otherwise | No |
| `layout-force` | Loads `ENG` on demand (without activating it) and unloads it on exit | Only this one, temporarily |

> Since v4.1.3 the helper **no longer preloads an English keyboard layout at startup**. Older builds called
> `LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)` unconditionally at launch, which registered the English
> layout in the system IME list and added a phantom `ENG / English (United States)` entry in `Win + Space`
> ([issue #10](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/10)).

### ❓ FAQ

**Q1: Why do I sometimes get `[Image #1]` and sometimes a literal `/mnt/...` path?** ([issue #4](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/4))

CLI agents (Claude Code / Codex / OpenCode, etc.) check whether the pasted path exists **at the moment the paste arrives**. If the file is there, it renders as an `[Image #n]` attachment (no file-read permission prompt). If it is missing or unrecognized, the raw path stays as text and the agent later needs file-read permission (and possibly a Windows-path retry when it runs outside WSL — a "second prompt").

Versions up to v4.0 pasted the path first and saved the PNG in the background, so under load the file could land after the check. Since v4.1 the image is written to disk **before** the path is pasted, removing that race. If it still happens occasionally, confirm the terminal supports bracketed paste (Windows Terminal does by default) and that the agent is running inside WSL.

**Q2: Hotkey fires, the PNG appears in `temp/`, IME switches — but no text is pasted anywhere.** ([issue #2](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/2))

Most likely causes, in order:

1. **You are on the v3.0 AHK build (or a custom compile of it).** That build restored the clipboard 80 ms after pasting; a slow target window then reads the restored image instead of the path text, so nothing pastes as text. Upgrade to the v4.x Rust build (that race is gone).
2. **The target window is elevated** (e.g. an admin terminal) while the helper is not. Windows UIPI silently drops injected keys. **Do not elevate this helper**; use a non-elevated terminal instead.
3. **Hotkey modifiers still held.** If Alt is down, injected Ctrl+V becomes Ctrl+Alt+V and does nothing. v4.1 waits for physical key release and adds an Alt menu-mask key, which largely fixes this.
4. **Security software blocking `SendInput`.** Expected; do not disable AV for this tool. If you allow-list, allow only the exe you built yourself.

**Q3: Kimi Code CLI does not turn the pasted path into an image.** ([issue #5](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/5))

Kimi Code CLI does not auto-detect plain path text, but it supports `@file` references. Switch the tray `Path style` to `@ path (Kimi/Gemini/Qwen)` and the hotkey pastes `@/mnt/c/... ` (with a trailing space), which Kimi Code CLI / Gemini CLI / Qwen Code consume as a file reference.

Note Kimi's own Windows build binds `Alt+V` for clipboard images; this helper's global hotkey takes priority. Switch this helper to `Ctrl+Alt+V` if you want Kimi's native behaviour.

**Q4: Paths containing spaces get cut off in the shell.**

Switch `Path style` to `Quoted path`; every path is then wrapped in double quotes.

**Q5: After launching the helper, an extra `ENG / English (United States)` shows up in `Win + Space`.** ([issue #10](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/10))

Up to v4.1.2 the helper called `LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)` unconditionally **at startup**. `LoadKeyboardLayoutW` registers the layout with the **system** input list and `KLF_ACTIVATE` activates it right away, so `ENG` appeared — and the taskbar IME indicator came back — even for users who never added English to their language list.

Since v4.1.3:

- Startup does no keyboard-layout work
- The English layout is resolved **lazily**, and by default only reused from layouts the system already has
- The default strategy is `imm`: close only the foreground window's IME open status, never touch keyboard layouts
- When a load is actually needed (`ime_protection = "layout-force"`) it no longer uses `KLF_ACTIVATE`, passes `KLF_NOTELLSHELL` so the taskbar indicator is not woken up, and calls `UnloadKeyboardLayout` on exit
- `ime_protection = "off"` disables the guard entirely

If an earlier version already left English (United States) in your language list, remove it once under `Settings → Time & language → Language & region`; the new build will not add it back.

### 🛠️ Build (Rust)

From the repo root:

```bash
cd rust
cargo build --release --target x86_64-pc-windows-msvc
```

Binary output:

```text
rust/target/x86_64-pc-windows-msvc/release/wsl_clipboard.exe
```

Debug build:

```bash
cd rust
cargo build --target x86_64-pc-windows-msvc
```

Clean build artifacts:

```bash
cd rust
cargo clean
```

On a non-Windows host (Linux/macOS), install the cross-compile target before `cargo check` / `cargo clippy` (`rust/.cargo/config.toml` defaults to `x86_64-pc-windows-msvc`):

```bash
rustup target add x86_64-pc-windows-msvc
```

### 🔧 AHK compile (only if you are maintaining V3.0)

If you are maintaining the `v3.0` AHK branch, you can rebuild with Ahk2Exe:

1. Install AutoHotkey v2:
   [https://www.autohotkey.com/download/ahk-v2.exe](https://www.autohotkey.com/download/ahk-v2.exe)
2. Open `C:\Program Files\AutoHotkey\Compiler\Ahk2Exe.exe`
3. Source: `scripts/wsl_clipboard.ahk`
4. Destination: e.g. `scripts/wsl_clipboard.exe`
5. Base File: `AutoHotkey64.exe` is recommended

### 📚 Additional Resources

- [Architecture & Workflow Details](docs/architecture_by_codex.md)
- [V3.0/V4.0 Refactor Notes](docs/rust-refactor-v3-v4.md)
- [v4.1 terminal image-paste scenario test report](docs/paste-scenario-test-report.md)

### 🕒 Version History

#### v4.2.0 (current, this fork, Rust) ✅

- Repo no longer contains prebuilt `.exe` / AutoHotkey runtimes
- Temp images and config moved to `%LOCALAPPDATA%\WSLImageClipboardHelper`, with a tightened ACL
- Temp PNG retention dropped from 2 hours to 30 minutes
- Explorer file conversion is image-extensions only by default, so secrets are not fed to the agent
- Pasted paths drop control characters / `` ` `` / `$`; quoted style escapes inner quotes
- Manifest declares `asInvoker`; running as Administrator is no longer recommended
- Release workflow publishes only from `v*` tags; branch pushes cannot clobber assets
- AHK / PowerShell path marked deprecated, with write/delete confined

#### v4.1.3 (upstream, Rust) ✅

- Fixed a phantom `ENG / English (United States)` entry in the system IME list after startup: no longer calls `LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)` unconditionally at launch (#10)
- English layout is resolved lazily and by default only reused from layouts already installed; a needed load no longer uses `KLF_ACTIVATE` and is unloaded on exit
- New tray `IME guard` option and `ime_protection` config: `off` / `imm` (new default, zero side effects) / `layout` / `layout-force`
- Foreground IME communication uses `SendMessageTimeoutW` so an unresponsive window cannot stall paste

#### v4.1.2 (Rust) ✅

- Fixed paste being swallowed by the target window's menu bar on a fast Alt release: inject a mask key immediately after the hotkey, and send Esc first if the foreground is already in menu mode (#2)

#### v4.1.1 (Rust) ✅

- Fixed 32-bit clipboard DIB reserved bytes (all zeros) being treated as alpha, which produced a fully transparent PNG: treat all-zero alpha as opaque (same heuristic browsers use for clipboard DIB)

#### v4.1 (Rust) ✅

- Fixed intermittent "raw path instead of `[Image #n]`": write the image first, then paste the path, removing the existence race (#4)
- New tray `Path style`: plain / `@` prefix / quoted, for Kimi Code CLI, Gemini CLI, Qwen Code, etc. (#5)
- Hardened key injection: wait for physical modifiers, add an Alt menu-mask key, and release right Alt, reducing "paste with no content" (#2)
- `cargo check` / `cargo clippy` work from non-Windows hosts (resource embedding is skipped)
- README gained an FAQ / troubleshooting section

#### v4.0 (Rust) ✅

- Main flow moved to Rust for easier maintenance
- Fixed DIB pixel-offset parsing, improving image compatibility
- Clipboard memory bound-checks to avoid out-of-range reads
- Hotkey switch rollback so a failed switch cannot leave you with no hotkey
- Exception paths always release the clipboard
- Embedded multi-size app icons and a DPI-aware manifest
- Explorer-copied files can be converted and pasted as `/mnt/...` paths with `Alt+V`

#### v3.0 (hotkey revision, AHK) 🔁

- Still based on AHK
- Focused on hotkey experience and configurability
- Tray interaction improved further

#### v2.0 (AHK performance) ⚡

- Path-first async save, noticeably lower perceived latency
- IME guard and automatic cleanup

#### v1.0 🧱

- Baseline clipboard-image sync
- SHA256 dedup and cache management
