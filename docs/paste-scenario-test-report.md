# Terminal Image Paste Scenario Test Report (v4.1)

This report records systematic testing of the v4.1 paste pipeline (after PR #7 was merged). Every shape of path text the tool injects into a terminal was fed into a CLI agent's file-reading pipeline to check that it is recognised as an image, and to reproduce the failure modes behind issues #2 / #4 / #5.

## Method and limitations

- **Environment**: a headless Claude Code agent session (Linux container). The directory structure `/mnt/c/Users/<username>/wsl_clipboard/temp/` was built inside the container to mirror, 1:1, the real output paths on a WSL-mounted drive; test images were generated with the tool's real naming scheme (`clip_YYYYMMDD_HHMMSS_mmm.png`).
- **Layer under test**: Claude Code's file-reading pipeline, which is the same layer that loads image bytes when an interactive terminal renders a path as an `[Image #n]` attachment. Each variant submits the pasted text **verbatim** (including decorations such as quotes, `@` and trailing spaces) as a path and checks whether it renders as an image.
- **Limitation**: the pre-processing an interactive TUI input box applies to pasted text (`@` file-reference parsing, quote stripping, path detection) cannot be fully reproduced in an agent environment. This test covers the "path string -> filesystem resolution -> image rendering" layer; how Kimi/Gemini/Qwen and similar CLIs parse the `@` and quoted styles at their input layer needs to be confirmed separately on real machines.

## Test matrix and results (18 items)

| # | Scenario | Submitted text | Result | Observation |
|---|----------|----------------|--------|-------------|
| S1 | plain style, file already on disk (v4.1 main path) | `/mnt/c/.../clip_..._001.png` | ✅ | Rendered as a 64×64 solid red image |
| S2a | File does not exist (old race / re-pasting an old path after cleanup) | same, file never written | ❌ | Clear error `File does not exist` |
| S2b | 0-byte file (created, contents not yet flushed) | same, 0 bytes | ❌ | Clear error `Image file is empty` |
| S2c | Truncated PNG (half-written snapshot) | same, first half of the bytes only | ⚠️ | **Silent failure**: the read raises no error, but the media is rejected by the API (`media removed — rejected by API`), so there is neither an image nor an error message |
| S3a | quoted style (quotes arrive at the file layer verbatim) | `"/mnt/c/.../clip_..._001.png"` | ❌ | Quotes are treated as path characters, `File does not exist` |
| S3b | at style (`@` arrives at the file layer verbatim) | `@/mnt/c/.../clip_..._001.png` | ❌ | `@` is not stripped, lookup fails |
| S3c | at style + trailing space | `@/mnt/c/... ` | ❌ | Same as S3b |
| S3d | plain + trailing space | `/mnt/c/.../clip_..._001.png ` | ⚠️ | The file is found, but **image type detection is bypassed** and the raw PNG bytes are returned as text instead of a rendered image |
| S4 | Windows username containing a space | `/mnt/c/Users/John Doe/.../clip_..._005.png` | ✅ | Renders fine without quotes (solid green) |
| S5 | Non-ASCII (Chinese) username | `/mnt/c/Users/<non-ASCII name>/.../clip_..._006.png` | ✅ | Renders normally (solid blue) |
| S6 | 4K screenshot (3840×2160) | plain path | ✅ | Automatically downsampled to 2000×1125 before rendering |
| S7 | Large file (6.2 MB noise image, 1920×1080) | plain path | ✅ | Rendered directly, no size limit hit |
| S8a | Multi-file paste (CF_HDROP), line 1 | `/mnt/c/.../shot_a.png` | ✅ | Solid yellow |
| S8b | Multi-file paste, line 2 | `/mnt/c/.../shot_b.png` | ✅ | Solid cyan |
| S8c | Multi-file paste including a non-image file | `/mnt/c/.../notes.txt` | ✅ | Read as text, not mistaken for an image |
| S9a | Windows backslash path leaks through | `C:\Users\...\clip_..._001.png` | ❌ | Cannot be resolved on the WSL side (the file actually lives at `/mnt/c/...`) |
| S9b | Windows forward-slash path | `C:/Users/.../clip_..._001.png` | ❌ | The `C:` drive letter is not translated to `/mnt/c` |
| Extra | `cargo check` / `clippy` on a non-Windows host | Linux host | ✅* | See below |

## Conclusions

1. **The v4.1 main path passes everything.** "Save to disk first, then paste a plain WSL path" is recognised as an image in all normal scenarios (paths with spaces, non-ASCII paths, 4K images, 6 MB files, multi-file lists). The file-reading layer does **not** need quotes or escaping for spaces in the path (S4); the value of the quoted style is for cases such as feeding a shell command, not for image pasting.
2. **The cause of issue #4 is confirmed.** When the file does not exist (S2a) or is not fully written (S2b/S2c), the path is not rendered as `[Image #n]`, which matches the explanation in FAQ Q1. v4.1 now injects the paste only after `ensure_saved` has written the file completely, so normal use no longer enters these three states.
3. **Two newly found edge-case degradations** (neither affects the normal v4.1 path; recorded for reference):
   - **A truncated PNG is the most insidious failure** (S2c): the header is intact and the read raises no error, but the image is rejected downstream and the user sees no error. Trigger: the tool crashes mid-write and leaves a half-written file, and the user later re-pastes the same path (a clipboard cache hit short-circuits on `ensure_saved`'s `try_exists`, so the file is **not** rewritten and repaired).
   - **A trailing space breaks image detection** (S3d): the path resolves but is returned as text. The plain style produces no trailing space, so it is safe; the trailing space in the at style exists so the CLI input layer can end the token, and as long as the target CLI consumes the `@` syntax correctly it never reaches the file layer, which matches the design intent.
4. **Quoted / `@` / native Windows paths are not recognised by the file layer** (S3a–c, S9): whether they work depends entirely on how the target CLI's input layer parses them. The tray "Path style" setting must match the target CLI, so the guidance in README FAQ Q3/Q4 is necessary.
5. **`cargo check` / `cargo clippy` pass on a Linux host, with an unstated prerequisite**: `rust/.cargo/config.toml` forces `target = "x86_64-pc-windows-msvc"`, so a clean Linux host must first run `rustup target add x86_64-pc-windows-msvc`, otherwise compiling the first dependency fails with `E0463: can't find crate for core`. With the target installed, `cargo check` passes in 19.5s and `cargo clippy` passes (8 style warnings, no errors). `cargo test` needs to run Windows test binaries and cannot be executed on a Linux host.

## Suggested improvements (recorded only; no code changed here)

- `ensure_saved` could write to a temporary file and rename it for an atomic write, and verify the file is non-empty when `try_exists` hits, which removes the problem that a half-written file (S2c) can never heal itself.
- The README build section now notes that non-Windows hosts need `rustup target add x86_64-pc-windows-msvc` (committed with this report).
