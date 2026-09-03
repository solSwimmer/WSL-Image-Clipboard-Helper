# WSL Image Clipboard Helper version notes (V3.0 / V4.0)

## 1. Purpose

This document clarifies the real boundary between the two versions, so the labels do not get mixed up:

- `V3.0`: hotkey revision (evolution inside the AHK stack, not the Rust build)
- `V4.0`: Rust rewrite and stability work

## 2. V3.0 (hotkey revision, AHK)

### 2.1 Goal

V3.0 focused on hotkey experience and configurability, improving usability without changing the tech stack.

### 2.2 Key changes

1. Hotkeys went from hard-coded to switchable
2. Better behaviour in conflict cases (fewer "pressed but nothing happened" misses)
3. Tray menu interaction improved for fast switching
4. Existing main flow kept: write the image to disk + paste a `/mnt/...` path

### 2.3 Version boundary

V3.0 is still an AHK main implementation; it did not replace the main program with Rust.

## 3. V4.0 (Rust rewrite)

### 3.1 Rewrite goals

V4.0 is the version that moved the main flow to Rust, with these goals:

1. Fold hotkey, tray, clipboard, save, and cleanup into a single binary
2. Cut the maintenance cost of mixed AHK + PowerShell
3. Provide stronger engineering guarantees for performance and stability

### 3.2 Rust module split

V4.0 is split by responsibility:

- `main.rs`: main event loop and task orchestration
- `hotkey.rs`: global hotkey registration and event bridging
- `tray.rs`: tray menu and command dispatch
- `clipboard.rs`: clipboard read and DIB-to-PNG
- `paste.rs`: write text to the clipboard and trigger paste
- `image_saver.rs`: async image save
- `config.rs`: `wsl_clipboard.toml` config

### 3.3 V4.0 stability work

1. DIB pixel offset is computed from the real header, not a fixed 40 bytes
2. `GlobalSize` bound-check before copying clipboard memory
3. Hotkey switch rollback, so a failure cannot leave you with no hotkey
4. Clipboard handle is released on exception paths

## 4. Compatibility

Upgrading from V3.0 to V4.0 keeps the same user habits:

1. Default hotkey logic stays the same
2. Non-image clipboard content still falls back to a normal paste
3. Tray switching of hotkey/mode is carried forward
4. Pasted paths still use WSL form (`/mnt/...`)

## 5. Build and verification (V4.0 Rust)

### 5.1 Build

```bash
cd rust
cargo build --release --target x86_64-pc-windows-msvc
```

### 5.2 Suggested regression checks

1. Ordinary screenshot paste (24/32bpp)
2. Paste from a `CF_DIBV5` source
3. Switch the hotkey to a conflicting combo, then switch back
4. Non-image content falls back to a normal paste
5. Hotkey and clipboard resources are released after exit

## 6. Conclusion

The correct version labels for this project are:

- `V3.0 = hotkey revision (AHK)`
- `V4.0 = Rust rewrite`
