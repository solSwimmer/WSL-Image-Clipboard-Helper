# Security policy

## What this program is

WSL Image Clipboard Helper is a **local input agent**. It registers a global
hotkey, reads the Windows clipboard, writes a PNG, and injects `Ctrl+V` into
the focused window. Treat it like AutoHotkey or PowerToys, not like a library
that only parses files.

## Build it yourself

This fork does **not** ship prebuilt `.exe` files in git. Prefer:

```
cd rust
cargo build --release --target x86_64-pc-windows-msvc
```

Release zips, if published, include SHA-256 checksums. They are still
unsigned. Do not run a binary you cannot rebuild from this source.

## Trust boundary

Pressing the hotkey (default `Alt+V`) can:

- Read whatever is on the clipboard (bitmap or Explorer file list)
- Write a PNG under `%LOCALAPPDATA%\WSLImageClipboardHelper\temp`
- Paste a `/mnt/c/...` path into the focused window

Many AI CLIs treat an existing file path as an image attachment **without**
asking for a file-read permission. That is why the helper writes the file
before pasting. Do not use the hotkey on password-manager screenshots,
`.env` files, or anything you would not paste into the agent yourself.

Explorer file conversion is **images only** by default. Non-image files are
ignored so a copied secrets file is not auto-attached. Turn on
`allow_non_image_files` in the tray menu only if you understand the risk.

## What this fork changed versus upstream

- No committed PE binaries or AutoHotkey runtimes
- Temp images and config live in `%LOCALAPPDATA%`, not next to the exe
- Directory ACL is tightened to the current user + SYSTEM
- Temp PNGs expire after 30 minutes (was 2 hours)
- Explorer paste is image-extensions only unless opted in
- Pasted paths with control characters / `` ` `` / `$` are dropped
- Quoted path style escapes inner quotes
- Manifest requests `asInvoker` (no silent elevation)
- Release workflow no longer publishes from branch pushes or clobbers tags
- AutoHotkey / PowerShell helpers are deprecated and path-confined

## Reporting a vulnerability

Please open a GitHub issue titled `SECURITY: ...` or email the maintainer on
the fork. Do not attach the contents of clipboard images.

## Running as Administrator

Do not. Windows UIPI blocking `SendInput` into an elevated terminal is
working as designed. Running this helper elevated lets it type into any
elevated window, including UAC-protected admin shells.
