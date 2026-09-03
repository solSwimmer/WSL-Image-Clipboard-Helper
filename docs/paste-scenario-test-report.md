# Terminal image-paste scenario test report (v4.1)

This report records a systematic test of the v4.1 paste path (after PR #7 landed): every path-text shape the helper injects into a terminal was fed into a CLI agent's file-read pipeline, to check whether it is recognized as an image and to reproduce the failure modes from issues #2 / #4 / #5.

## Method and limits

- **Environment**: a headless Claude Code agent session (Linux container). Inside the container, a `/mnt/c/Users/<username>/wsl_clipboard/temp/` tree was built 1:1 with the real WSL mount output; test images used the helper's real naming (`clip_YYYYMMDD_HHMMSS_mmm.png`).
- **Verification layer**: Claude Code's file-read pipeline — the same layer that loads image bytes when an interactive terminal renders a path as an `[Image #n]` attachment. Each variant submitted the paste text **as-is** (including quotes, `@`, trailing spaces, and other decoration) as a path, and observed whether it rendered as an image.
- **Limits**: interactive TUI preprocessing of paste text (`@` file-reference parsing, quote stripping, path detection) cannot be fully reproduced in the agent environment. This test covers "path string → filesystem resolve → image render". How `@` and quoted styles are parsed in Kimi/Gemini/Qwen CLI input layers needs a separate real-machine check.

## Test matrix and results (18 cases)

| # | Scenario | Submitted text | Result | Notes |
|---|----------|----------------|--------|-------|
| S1 | plain style, file already on disk (v4.1 main path) | `/mnt/c/.../clip_..._001.png` | ✅ | Rendered as a 64×64 solid-red image |
| S2a | File missing (old race / re-paste after cleanup) | same, file never written | ❌ | Explicit error `File does not exist` |
| S2b | 0-byte file (created, content not flushed) | same, 0 bytes | ❌ | Explicit error `Image file is empty` |
| S2c | Truncated PNG (mid-write snapshot) | same, first half of the bytes only | ⚠️ | **Silent failure**: read does not error, but media is rejected by the API (`media removed — rejected by API`); no image and no error prompt |
| S3a | quoted style (quotes reach the file layer as-is) | `"/mnt/c/.../clip_..._001.png"` | ❌ | Quotes treated as path characters, `File does not exist` |
| S3b | at style (`@` reaches the file layer as-is) | `@/mnt/c/.../clip_..._001.png` | ❌ | `@` not stripped, lookup fails |
| S3c | at style + trailing space | `@/mnt/c/... ` | ❌ | Same as S3b |
| S3d | plain + trailing space | `/mnt/c/.../clip_..._001.png ` | ⚠️ | File is found, but **image-type detection is skipped**; raw PNG bytes are returned as text instead of a rendered image |
| S4 | Windows username contains a space | `/mnt/c/Users/John Doe/.../clip_..._005.png` | ✅ | Renders without quotes (solid green) |
| S5 | CJK username | `/mnt/c/Users/张三/.../clip_..._006.png` | ✅ | Renders normally (solid blue) |
| S6 | 4K screenshot (3840×2160) | plain path | ✅ | Downsampled to 2000×1125 then rendered |
| S7 | Large file (6.2 MB noise 1920×1080) | plain path | ✅ | Rendered directly; no size limit hit |
| S8a | Multi-file paste (CF_HDROP) line 1 | `/mnt/c/.../shot_a.png` | ✅ | Solid yellow |
| S8b | Multi-file paste line 2 | `/mnt/c/.../shot_b.png` | ✅ | Solid cyan |
| S8c | Multi-file paste including a non-image | `/mnt/c/.../notes.txt` | ✅ | Read as text, not misclassified as an image |
| S9a | Windows backslash path leak | `C:\Users\...\clip_..._001.png` | ❌ | WSL cannot resolve it (file actually lives at `/mnt/c/...`) |
| S9b | Windows forward-slash path | `C:/Users/.../clip_..._001.png` | ❌ | `C:` is not translated to `/mnt/c` |
| extra | non-Windows host `cargo check` / `clippy` | Linux host | ✅* | See below |

## Conclusions

1. **The v4.1 main path passed in full.** "Write to disk first → paste a plain WSL path" is recognized as an image in normal cases (paths with spaces, CJK paths, 4K, 6 MB files, multi-file lists). The file-read layer does **not** need quotes or escaping for spaces in the path (S4); quoted style is valuable for feeding a shell command, not for image paste.
2. **The cause of issue #4 is confirmed empirically**: a missing file (S2a) or an incomplete write (S2b/S2c) is not rendered as `[Image #n]`, matching FAQ Q1. v4.1 changed to inject paste only after `ensure_saved` has fully written the file, so the normal flow no longer enters those three states.
3. **Two newly found edge-degrade modes** (neither affects the v4.1 happy path; recorded for later):
   - **A truncated PNG is the most hidden failure** (S2c): the header is intact, read does not error, but the image is rejected downstream and the user sees no error. Trigger: the helper crashes mid-write and leaves a half file, then the user re-pastes the same path (a clipboard cache hit short-circuits `ensure_saved` via `try_exists` and **will not** rewrite/repair that file).
   - **A trailing space breaks image detection** (S3d): the path resolves but is returned as text. Plain style does not emit a trailing space, so it is safe; the trailing space on at-style is for the CLI input layer to terminate the token, and as long as the target CLI consumes `@` syntax correctly it never reaches the file layer — that is by design.
4. **Quoted / `@` / native Windows paths are not recognized at the file layer** (S3a–c, S9): correctness of those shapes depends entirely on the target CLI's input parser. Tray `Path style` must match the target CLI; the README FAQ Q3/Q4 guidance is necessary.
5. **`cargo check` / `cargo clippy` pass on a Linux host, with an undocumented prerequisite**: `rust/.cargo/config.toml` forces `target = "x86_64-pc-windows-msvc"`. A clean Linux host must first run `rustup target add x86_64-pc-windows-msvc`, or the first dependency fails with `E0463: can't find crate for core`. After the target is installed, `cargo check` passed in 19.5s; `cargo clippy` passed (8 style warnings, no errors). `cargo test` needs a Windows test binary and cannot run on a Linux host.

## Suggested follow-ups (recorded only; no code changes in that pass)

- `ensure_saved` could write a temp file then rename (atomic), and on a `try_exists` hit verify the file is non-empty, so an S2c half-file leftover can self-heal.
- The README build section already notes that non-Windows hosts need `rustup target add x86_64-pc-windows-msvc` (landed with this report).
