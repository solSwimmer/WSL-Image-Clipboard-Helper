# Architecture and workflow (WSL Clipboard Sync)

This add-on document explains the component layout, execution flow, and implementation details of the revamped WSL Clipboard Sync so maintainers can assess the value of the changes quickly.

### Component Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    wsl_clipboard.exe (AHK)                  │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  • Alt+V hotkey listener                             │   │
│  │  • Input method state capture & restore              │   │
│  │  • Path conversion (Windows → WSL)                   │   │
│  │  • Tray menu & status notifications                  │   │
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
│              │   persistence   │  │   cleanup │ │   cache │ │
│              │ • Silent errors │  │ • Temp    │ │         │ │
│              └─────────────────┘  │   purge   │  └─────────┘ │
│                                   │            │              │
│                                   └────────────┘              │
└─────────────────────────────────────────────────────────────┘
```

### Core Flow

#### Image Paste Sequence

```
Press Alt+V
    │
    ├─→ Save current input method handle
    │
    ├─→ Switch to the English layout
    │
    ├─→ Generate timestamped filename (yyyyMMdd_HHmmss.png)
    │
    ├─→ Convert Windows path to a WSL path
    │
    ├─→ Paste the WSL path immediately into the active window
    │
    ├─→ Trigger PowerShell asynchronously to write the image
    │
    └─→ Restore the original input method after a short delay
```

#### Path Conversion Strategy

```ahk
ConvertPathToWsl(winPath) {
    local p := Trim(winPath, '"')

    ; Handle common drive paths "C:\path\to\file"
    if RegExMatch(p, "^[A-Za-z]:\\") {
        local drive := SubStr(p, 1, 1)
        local rest := SubStr(p, 3)
        rest := StrReplace(rest, "\", "/")
        rest := RegExReplace(rest, "^/+", "")
        return "/mnt/" . StrLower(drive) . "/" . rest
    }

    ; Fallback to wsl wslpath command
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

#### Asynchronous Save Design

- Decouple path output from image persistence so the hotkey responds within roughly 10 ms.
- PowerShell is responsible for reading the clipboard and saving the file; failures are logged silently without blocking the main script.
- Commands run in hidden windows to avoid interrupting the user workflow.

#### Scheduled & Exit Cleanup

- An AutoHotkey timer scans `temp/` every two hours and deletes expired files to keep the cache light.
- Selecting `Exit` from the tray invokes `exit-all.ps1`, which terminates helper processes and clears the cache directory.

### Technical Highlights

- **Input Method Protection**: The v3 AHK build preloads the English layout (HKL 0x00000409) via `LoadKeyboardLayoutW` with `KLF_ACTIVATE` at startup and switches with `PostMessage(0x50, ...)`. ⚠️ That registers the layout with the **system** input list, which makes an extra `ENG / English (United States)` appear in `Win+Space` ([issue #10](https://github.com/cpulxb/WSL-Image-Clipboard-Helper/issues/10)). Rust v4.1.3 replaced it with lazy resolution and an IMM-based default (see `rust/src/paste.rs` and `config::ImeProtection`); the AHK build still has the old behaviour.
- **Encoding Strategy**: Save PowerShell files that contain non-ASCII (e.g. CJK or emoji) as UTF-8 with BOM, as PowerShell's parser relies on BOM to correctly identify UTF-8 encoding; ASCII-only files can remain without BOM.
- **Error Handling**: Async scripts capture exceptions and suppress pop-ups; optional logging or tray notifications can be layered in later.
- **Path Conversion Priority**: Use built-in regex matching first (fast), fallback to `wsl wslpath` command (compatibility).

### Performance Comparison

- **Previous Flow**: Performed SHA256 deduplication, waited for image persistence, and used `Send {Raw}` to emit characters one by one—total latency around three seconds.
- **Current Flow**: Runs path pasting and disk writes in parallel, avoids upfront validation and synchronous I/O, and calls `SendText` for a single-shot output—paths are typically available in under one second.
- Compatibility note: If deduplication is required again, it can be reintroduced inside the PowerShell script, keeping in mind it will likely reintroduce the earlier delay.

---

## Known limitations and possible extensions

- Currently only outputs `/mnt/...` paths; if Windows native paths are needed, extend the AHK script to provide mode switching.
- Current cleanup strategy uses time thresholds; consider adding capacity limits or custom policies.
- If multiple hotkey modes are needed in the future, add a configuration entry in the tray menu or introduce GUI options.
