# WSL Image Clipboard Helper Refactor Notes (V3.0 / V4.0)

## 1. Purpose of this document

This document clarifies the real boundary between the two versions so the terminology doesn't get confused:

- `V3.0`: a rework of the hotkey (evolved within the AHK stack, not the Rust version)
- `V4.0`: the Rust rewrite with added stability

## 2. V3.0 (hotkey rework, AHK implementation)

### 2.1 Goals

V3.0 aimed to improve the hotkey experience and configurability, improving usability without changing the technology stack.

### 2.2 Key changes

1. The hotkey strategy went from hard-coded to switchable
2. Better stability when hotkeys conflict (fewer "pressed it and nothing happened" cases)
3. The tray menu interaction was refined for quick switching
4. The existing main flow was kept: save the image to disk + paste the `/mnt/...` path

### 2.3 Version boundary

V3.0 is still the AHK main implementation; the Rust main program had not replaced it yet.

## 3. V4.0 (Rust rewrite)

### 3.1 Goals

V4.0 is the version where the main flow moved to Rust. Its goals:

1. Combine hotkey, tray, clipboard, saving and cleanup into a single binary
2. Reduce the maintenance cost of the mixed AHK + PowerShell setup
3. Provide stronger engineering guarantees for performance and stability

### 3.2 Rust module layout

V4.0 splits modules by responsibility:

- `main.rs`: main event loop and task orchestration
- `hotkey.rs`: global hotkey registration and event bridging
- `tray.rs`: tray menu and command dispatch
- `clipboard.rs`: clipboard reading and DIB-to-PNG conversion
- `paste.rs`: writing text to the clipboard and triggering the paste
- `image_saver.rs`: image saving
- `config.rs`: `wsl_clipboard.toml` configuration management

### 3.3 V4.0 stability improvements

1. The DIB pixel offset is computed from the real header information instead of a fixed 40 bytes
2. A `GlobalSize` bounds check was added before copying clipboard memory
3. Hotkey switching has a rollback strategy so a failure never leaves no usable hotkey
4. The clipboard handle is guaranteed to be released on error paths

## 4. Compatibility strategy

When upgrading from V3.0 to V4.0, the user's habits stay unchanged:

1. The default hotkey behaviour is the same
2. Non-image content still falls back to a normal paste
3. The tray interaction for switching hotkey/mode continues as before
4. The pasted path is still in WSL format (`/mnt/...`)

## 5. Build and verification (V4.0 Rust)

### 5.1 Build command

```bash
cd rust
cargo build --release --target x86_64-pc-windows-msvc
```

### 5.2 Suggested regression checks

1. Ordinary screenshot paste (24/32bpp)
2. Paste from a `CF_DIBV5` source
3. Switch the hotkey to a conflicting combination and back
4. Non-image content falls back to a normal paste
5. Hotkey and clipboard resources are released after exit

## 6. Conclusion

The correct way to describe the versions is:

- `V3.0 = hotkey rework (AHK)`
- `V4.0 = Rust rewrite`
