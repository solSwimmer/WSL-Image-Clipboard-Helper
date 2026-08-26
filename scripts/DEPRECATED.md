# AutoHotkey helpers are deprecated

The v3 AutoHotkey + PowerShell path is **unsupported** in this fork.

Reasons:

- It launches hidden PowerShell with `-ExecutionPolicy Bypass`
- `exit-all.ps1` used to force-kill any `wsl_clipboard*.exe` on the machine
  and recursively delete whatever `-TempDir` pointed at
- Non-drive paths were converted by `cmd.exe /C wsl wslpath "..."`, which
  expands `%VAR%` even inside quotes

Use the Rust build in `../rust` instead.

These scripts remain only so old installs can still be read. They now:

- Refuse to write outside a `...\temp\*.png` path
- Kill only processes whose executable path is under this repo's parent
- Delete only `*.png` files, never a recursive tree
- No longer call `cmd.exe /C wsl wslpath`
