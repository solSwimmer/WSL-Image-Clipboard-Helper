# Architecture and workflow (WSL Clipboard Sync)

This document describes component layout, paste flow, and implementation notes for WSL Clipboard Sync.

Current mainline is the Rust binary (`v4.2`). The original AHK + PowerShell design is preserved at the end as historical context, fully translated from the Chinese write-up that this file started as.

Related notes: [V3.0 / V4.0 version boundary](rust-refactor-v3-v4.md), [terminal Ctrl+V interception](terminal-ctrl-v-interception.md).

---

## Current architecture (Rust v4.2)

The helper is a single Windows GUI process (`wsl_clipboard.exe`). It registers a global hotkey, reads the Windows clipboard, writes a PNG under `%LOCALAPPDATA%\WSLImageClipboardHelper\temp`, and pastes a WSL path (`/mnt/...`) into the focused window. There is no Windows service and no PowerShell/AHK in the supported path.

### Component responsibilities

```
┌─────────────────────────────────────────────────────────────┐
│                 wsl_clipboard.exe (Rust)                    │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  main.rs        event loop, paste orchestration      │   │
│  │  hotkey.rs      global hotkey register / event bridge│   │
│  │  tray.rs        tray menu and command dispatch       │   │
│  │  clipboard.rs   CF_DIB / CF_DIBV5 / CF_HDROP, PNG    │   │
│  │  paste.rs       clipboard text + SendInput Ctrl+V    │   │
│  │  image_saver.rs write PNG to disk before paste       │   │
│  │  paths.rs       AppData dirs, ACL, path sanitizing   │   │
│  │  config.rs      wsl_clipboard.toml                   │   │
│  │  cleanup.rs     periodic + exit temp PNG purge       │   │
│  └──────────────────────────────────────────────────────┘   │
│                           │                                  │
│                           ├─────────────┬───────────────┐    │
│                           ▼             ▼               ▼    │
│              ┌─────────────────┐  ┌──────────┐  ┌─────────┐ │
│              │ AppData config  │  │ IME guard│  │  temp/  │ │
│              │ wsl_clipboard.  │  │ (safe    │  │  *.png  │ │
│              │ toml            │  │  mode)   │  │         │ │
│              │                 │  │          │  │ • 30 min│ │
│              │ • hotkey/mode   │  │ • imm /  │  │   TTL   │ │
│              │ • path style    │  │   layout │  │ • ACL   │ │
│              └─────────────────┘  └──────────┘  └─────────┘ │
└─────────────────────────────────────────────────────────────┘
```

Runtime files live in `%LOCALAPPDATA%\WSLImageClipboardHelper\` (config + `temp\`), not next to the executable. `paths::ensure_app_dirs` creates that tree and tightens the directory ACL to the current user and SYSTEM.

### Image paste flow

Since v4.1 the image is written to disk **before** the path is pasted. CLI agents check whether the pasted path exists at the moment it arrives; that decides `[Image #n]` vs a raw path ([issue #4](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/4)).

```
User presses Alt+V (or the tray-selected hotkey)
    │
    ├─→ Inject Alt menu-mask key (avoid target window menu bar)
    │
    ├─→ Clipboard has an image (CF_BITMAP / CF_DIB / CF_DIBV5)?
    │       │
    │       ├─ yes → read DIB, convert to PNG (cached by clipboard sequence)
    │       │         generate timestamped filename
    │       │         convert Windows path → WSL path
    │       │         write PNG to disk (await)
    │       │         optional IME guard (safe mode)
    │       │         paste styled WSL path via clipboard + Ctrl+V
    │       │         restore IME after ~120 ms
    │       │
    │       └─ no  → CF_HDROP file list?
    │                 ├─ yes, allowed files → paste converted WSL paths
    │                 ├─ yes, all rejected  → do nothing (do not Ctrl+V)
    │                 └─ no image, no files → fall back to a normal Ctrl+V
    │
    └─→ ImeGuard drops and restores the previous IME state
```

Path paste style (`path_style` in config / tray):

| Style | Paste text | Typical consumer |
| --- | --- | --- |
| `plain` (default) | `/mnt/c/...` | Claude Code, Codex, OpenCode |
| `at` | `@/mnt/c/... ` (trailing space) | Kimi Code CLI, Gemini CLI, Qwen Code |
| `quoted` | `"/mnt/c/..."` | paths with spaces / shell commands |

Explorer file conversion is **image extensions only** by default (`png/jpg/jpeg/gif/webp/bmp/tif/tiff/ico`). Non-images (`.env`, secrets) are ignored unless `allow_non_image_files` is enabled. Rejected file lists do **not** fall back to Ctrl+V, so the original files are not dropped into the window.

Pasted paths are filtered: control characters, newlines, backticks, and `$` are dropped so clipboard-derived text cannot inject extra shell syntax.

### Path conversion strategy

Drive-letter paths are converted in-process (same idea as the AHK helper). There is no `wsl wslpath` spawn on the hotkey path.

```text
C:\Users\me\clip.png  →  /mnt/c/Users/me/clip.png
D:/temp/file.png      →  /mnt/d/temp/file.png
```

Implementation (`clipboard::convert_path_to_wsl`): strip wrapping quotes, take the drive letter, lowercase it, replace `\` with `/`, prefix `/mnt/<drive>/`. Non-drive paths return empty.

### Save-before-paste (replaces the old async race)

- v3 / v4.0 pasted the path first and wrote the PNG in the background so the hotkey could return in ~10 ms. Under load the file could land after the CLI existence check.
- v4.1+ awaits `image_saver::ensure_saved` **then** pastes. Latency is still well under a second in the usual case; the `[Image #n]` race is gone.
- PNG bytes are cached against the clipboard sequence number so a repeated hotkey does not re-decode the DIB.

### Scheduled and exit cleanup

- A tokio interval (~15 minutes) scans `temp/` and deletes PNG / `.partial` files older than **30 minutes** (v4.2; earlier builds used two hours).
- Tray **Exit and clean temporary images** deletes every temp PNG in that folder, unloads a self-loaded English layout if `ime_protection = "layout-force"`, then exits.
- Cleanup never deletes outside the AppData temp directory.

### Technical highlights

- **IME guard** (safe mode only; `ime_protection` in config / tray):
  - `off` — do not touch IME or keyboard layouts
  - `imm` (default) — close the foreground window's IME open status via `WM_IME_CONTROL` / `SendMessageTimeoutW`, restore afterwards; never touches layouts
  - `layout` — switch to an English layout **already loaded** on the system; skip if none
  - `layout-force` — load `ENG` lazily without `KLF_ACTIVATE`, with `KLF_NOTELLSHELL`, and `UnloadKeyboardLayout` on exit
  - v3 AHK and Rust up to v4.1.2 called `LoadKeyboardLayoutW("00000409", KLF_ACTIVATE)` at startup. That registers the layout with the **system** input list and produced a phantom `ENG / English (United States)` in `Win+Space` ([issue #10](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/10)). v4.1.3 does no layout work at startup.
- **Clipboard parsing**: DIB pixel offset comes from the real header (not a fixed 40 bytes). `GlobalSize` is checked before copying clipboard memory. 32-bit DIB with all-zero alpha is treated as opaque (v4.1.1).
- **Hotkey switch rollback**: changing the hotkey from the tray rolls back if registration fails, so the previous combo stays live.
- **Key injection**: wait for physical modifiers to release, inject an Alt menu-mask key, dismiss menu mode with Esc if needed ([issue #2](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/2)). Do not run the helper elevated; UIPI blocking injected keys into an elevated window is expected.
- **Error handling**: paste failures are logged (`tracing`); no pop-up dialogs on the hotkey path.

### Performance comparison

- **v1 / early AHK**: SHA256 dedup, wait for the image to hit disk, `Send {Raw}` character-by-character — about three seconds.
- **v2 / v3 AHK and v4.0 Rust**: path paste and image write in parallel, skip upfront validation and synchronous I/O, `SendText` / one-shot clipboard paste — path often available under one second, but racy against CLI existence checks.
- **v4.1+ Rust**: same in-process clipboard + `SendInput` path, but disk write is completed first. Typical paste is still under one second, without the `[Image #n]` race.

---

## Known limitations and possible extensions

- Default paste is a WSL `/mnt/...` path. Native Windows paths are not a tray mode; add one if that is needed.
- Cleanup is time-based (30 minutes), not capacity-based. A size cap could be added later.
- Hotkey, runtime mode, path style, IME policy, and non-image Explorer conversion are already tray-configurable. Further GUI beyond the tray is optional.
- The helper cannot type into elevated windows unless it is also elevated; **do not elevate it**. Use a non-elevated terminal instead.

---

## Historical: AHK v3 architecture

The remainder is the original architecture write-up for the AutoHotkey helper (component split, paste-first async save, two-hour cleanup). It is the English translation of the Chinese section this file used to lead with. It does **not** describe the supported Rust binary.

### Component responsibilities

```
┌─────────────────────────────────────────────────────────────┐
│                    wsl_clipboard.exe (AHK)                  │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  • Alt+V hotkey listener                             │   │
│  │  • Input method state save and restore               │   │
│  │  • Path conversion (Windows → WSL)                   │   │
│  │  • Tray menu and status notifications                │   │
│  │  • Scheduled cleanup dispatcher                      │   │
│  └──────────────────────────────────────────────────────┘   │
│                           │                                  │
│                           ├─────────────┬───────────────┐    │
│                           ▼             ▼               ▼    │
│              ┌─────────────────┐  ┌──────────┐  ┌─────────┐ │
│              │ save-clipboard- │  │ exit-    │  │  temp/  │ │
│              │   image.ps1     │  │ all.ps1  │  │  *.png  │ │
│              │                 │  │          │  │         │ │
│              │ • Async image   │  │ • Exit   │  │ • Image │ │
│              │   save          │  │   cleanup│  │   cache │ │
│              │ • Silent errors │  │ • Purge  │  │         │ │
│              └─────────────────┘  │   temp   │  └─────────┘ │
│                                   │   files  │              │
│                                   └──────────┘              │
└─────────────────────────────────────────────────────────────┘
```

### Image paste flow

Path paste and image save were decoupled: the path was pasted immediately so the hotkey could return in about 10 ms, then PowerShell wrote the PNG in the background.

```
User presses Alt+V
    │
    ├─→ Save current input method state
    │
    ├─→ Switch to the English input method
    │
    ├─→ Generate timestamped filename (yyyyMMdd_HHmmss.png)
    │
    ├─→ Convert the Windows path to a WSL path
    │
    ├─→ Paste the WSL path immediately into the active window
    │
    ├─→ Call PowerShell asynchronously to save the image
    │
    └─→ Restore the original input method after a short delay
```

### Path conversion strategy

Prefer a built-in drive-letter regex (fast). If the path is not a drive path, fall back to `wsl wslpath` for compatibility. Errors from that spawn are ignored.

```ahk
ConvertPathToWsl(winPath) {
    local p := Trim(winPath, '"')

    ; Common drive path "C:\path\to\file"
    if RegExMatch(p, "^[A-Za-z]:\\") {
        local drive := SubStr(p, 1, 1)
        local rest := SubStr(p, 3)
        rest := StrReplace(rest, "\", "/")
        rest := RegExReplace(rest, "^/+", "")
        return "/mnt/" . StrLower(drive) . "/" . rest
    }

    ; Not a drive path: try `wsl wslpath`
    try {
        local out := Trim(RunGetStdOut('wsl wslpath -a -u "' p '"'))
        if (out != "") {
            return out
        }
    } catch {
        ; Ignore errors
    }

    return ""
}
```

### Asynchronous save design

- Path paste completes on its own and does not wait for the file. That keeps hotkey response around 10 ms.
- PowerShell reads the clipboard and writes the image. On failure it writes a log and does not block the main script.
- The save command runs in a hidden window so it does not interrupt the user.

### Scheduled and exit cleanup

- An AutoHotkey timer scans `temp/` every two hours and deletes images past the age threshold, to keep the cache small.
- Tray `Exit` runs `exit-all.ps1`, which kills leftover helper processes and clears the cache directory.

### Technical highlights

- **Input method protection**: the v3 AHK build called `LoadKeyboardLayoutW` with `KLF_ACTIVATE` at startup to preload the English layout (HKL `0x00000409`), then switched with `PostMessage(0x50, ...)`. That registers the layout with the **system** input list, so `Win+Space` gained an extra `ENG / English (United States)` ([issue #10](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/10)). Rust v4.1.3 replaced this with lazy resolution and an IMM default (see `rust/src/paste.rs` and `config::ImeProtection`). The AHK build still has the old behaviour.
- **Encoding**: PowerShell files that contain CJK or emoji must be UTF-8 **with BOM**; the PowerShell parser uses the BOM to detect UTF-8. ASCII-only files can stay without a BOM.
- **Error tolerance**: async scripts catch exceptions and suppress pop-ups. Logging or tray notifications can be layered on later.
- **Path conversion priority**: built-in regex first (fast), `wsl wslpath` as fallback (compatibility).

### Performance comparison (AHK-era)

- **Previous flow**: SHA256 deduplication, wait for image save, `Send {Raw}` one character at a time — about three seconds total.
- **Then-new flow**: path paste and image write in parallel, skip upfront validation and synchronous I/O, `SendText` in one shot — path typically available within one second.
- If deduplication is required again, it can be added inside the async PowerShell script, at the cost of reintroducing that delay.

### AHK-era limitations (as originally documented)

- Only `/mnt/...` paths were emitted. A Windows-native path mode would have required an AHK change.
- Cleanup used a time threshold only; a capacity cap was a possible extension.
- Extra hotkey modes would have needed a tray or GUI settings entry.
