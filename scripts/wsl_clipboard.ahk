#Requires AutoHotkey v2.0
#SingleInstance Force
; DEPRECATED: this AutoHotkey helper is unsupported. Use the Rust build in ../rust.
; See scripts/DEPRECATED.md.

global gScriptDir := A_ScriptDir
global gTempDir := GetFullPath(gScriptDir "\..\temp")
global gPsScript := gScriptDir "\save-clipboard-image.ps1"
global gSetAttachmentClipboardScript := gScriptDir "\set-clipboard-attachment.ps1"
global gSaveAndSetAttachmentScript := gScriptDir "\save-and-set-attachment.ps1"
global gExitScript := gScriptDir "\exit-all.ps1"
global gConfigFile := gScriptDir "\wsl_clipboard.ini"
global gCleanupDone := false
global gLastNotifyAt := 0
global gCurrentHotkey := ""
global gHotkeyMenu := ""
global gSpeedModeMenu := ""
global gSpeedMode := "safe"
global gPasteFormatMenu := ""
global gPasteFormat := "plain"
global gPendingRestoreHKL := 0
global gWslTempDir := ""
global gLastImageClipboardSeq := 0
global gLastImageWinPath := ""
global gLastImageWslPath := ""
global gPendingSaveWinPath := ""
global gPendingSaveStartedAt := 0

; Cache the English input method HKL (en-US)
global gEngHKL := DllCall("user32.dll\LoadKeyboardLayoutW", "WStr", "00000409", "UInt", 0x1, "UPtr")

SetupTrayMenu()
InitializeHelperScripts()
InitializeSpeedMode()
InitializePasteFormat()
InitializeHotkey()
OnExit(HandleExit)

#HotIf gCurrentHotkey = "!v"
!v::HandlePasteHotkey()
#HotIf gCurrentHotkey = "^!v"
^!v::HandlePasteHotkey()
#HotIf gCurrentHotkey = "!Enter"
!Enter::HandlePasteHotkey()
#HotIf

; ------------------ Paste hotkey: path first + IME protection ------------------
HandlePasteHotkey(*) {
    global gTempDir, gPsScript, gEngHKL
    local withImeGuard := IsImeGuardEnabled()
    local attachmentMode := IsAttachmentPasteMode()
    local prevHKL := 0
    local clipSeq := GetClipboardSequence()

    RefreshPendingSaveState()

    ; With no image on the clipboard, fall back to a normal paste to avoid emitting an invalid path
    if !ClipboardHasImage() {
        NormalizeModifierStateBeforeSend()
        Send("^v")
        return
    }

    ; Safe mode enables IME protection; fast mode skips this step to cut hotkey latency
    if (withImeGuard) {
        ; 1) Get the active window and thread ID
        local hwnd := WinActive("A")
        local threadId := hwnd ? DllCall("user32.dll\GetWindowThreadProcessId", "UInt", hwnd, "UInt", 0, "UInt") : 0

        ; 2) Save the current keyboard layout
        prevHKL := threadId ? DllCall("user32.dll\GetKeyboardLayout", "UInt", threadId, "UPtr") : 0

        ; 3) Switch to the English input method (only when the layout differs, to reduce switching overhead)
        if (gEngHKL != 0 && prevHKL != gEngHKL) {
            PostMessage(0x50, 0, gEngHKL, , "A")
            Sleep 60
        }
    }

    ; 4) In attachment mode, if the clipboard is unchanged and the cached file still exists, reuse it instead of writing again
    if attachmentMode && TryPasteCachedAttachment(clipSeq) {
        if (withImeGuard) {
            ScheduleKeyboardLayoutRestore(prevHKL, 120)
        }
        return
    }
    if !attachmentMode && TryPasteCachedPlainPath(clipSeq) {
        if (withImeGuard) {
            ScheduleKeyboardLayoutRestore(prevHKL, 120)
        }
        return
    }

    ; 5) Generate a high-precision file name so triggers within the same second don't collide
    local fileName := FormatTime(, "yyyyMMdd_HHmmss") "_" Format("{:03}", A_MSec + 0) ".png"
    local winPath := gTempDir "\" fileName

    ; 6) Attachment channel (experimental): no @ needed, preferring client-side attachment rendering
    if attachmentMode {
        if TryPasteAsAttachment(winPath) {
            SaveImageCache(winPath)
            ; The attachment channel finished; no need to send text or save asynchronously afterwards
            if (withImeGuard) {
                ScheduleKeyboardLayoutRestore(prevHKL, 120)
            }
            return
        }
        ; If the attachment channel fails, fall back to a plain path (compatible) so nothing is left unresponsive
        NotifyUser("Attachment mode fallback", "Attachment channel failed, fell back to a plain path.", 2500, false)
    }

    ; 7) Only compute the WSL path when a text path is needed (less overhead on the attachment success path)
    local wslPath := BuildWslPath(winPath, fileName)
    if (wslPath = "") {
        if (withImeGuard) {
            RestoreKeyboardLayout(prevHKL)
        }
        NormalizeModifierStateBeforeSend()
        Send("^v")
        return
    }

    ; 8) Back up the clipboard (including the image), write the text path, paste it instantly with Ctrl+V, then restore the clipboard
    local pasteText := BuildPasteText(wslPath)
    SavePlainPathCache(clipSeq, winPath, wslPath)

    local savedClip := ClipboardAll()
    A_Clipboard := pasteText
    NormalizeModifierStateBeforeSend()
    Send("^v")
    Sleep 80
    A_Clipboard := savedClip
    savedClip := ""

    ; 9) Save the image asynchronously via PowerShell (non-blocking; the clipboard is already restored)
    if !TriggerAsyncSaveIfNeeded(winPath) {
        ; The path was already pasted; if saving fails, give the user a gentle notice
        NotifyUser("Image save failed", "The path was pasted, but the image was not saved.", 3000, true)
    }

    ; 10) In safe mode, restore the input method after a delay without blocking the main hotkey thread
    if (withImeGuard) {
        ScheduleKeyboardLayoutRestore(prevHKL, 120)
    }
}

BuildSaveCommand(winPath) {
    global gPsScript
    return 'powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "' gPsScript '" -FilePath "' winPath '"'
}

BuildSetAttachmentClipboardCommand(winPath) {
    global gSetAttachmentClipboardScript
    return 'powershell -NoLogo -NoProfile -NonInteractive -STA -ExecutionPolicy Bypass -WindowStyle Hidden -File "' gSetAttachmentClipboardScript '" -FilePath "' winPath '"'
}

BuildSaveAndSetAttachmentCommand(winPath) {
    global gSaveAndSetAttachmentScript
    return 'powershell -NoLogo -NoProfile -NonInteractive -STA -ExecutionPolicy Bypass -WindowStyle Hidden -File "' gSaveAndSetAttachmentScript '" -FilePath "' winPath '"'
}

TryPasteAsAttachment(winPath) {
    ; Attachment channel optimisation: one process does "save the image + set the attachment clipboard"
    if !SaveAndSetAttachmentSync(winPath) {
        return false
    }

    NormalizeModifierStateBeforeSend()
    Send("^v")
    return true
}

TryPasteCachedAttachment(clipSeq) {
    global gLastImageClipboardSeq, gLastImageWinPath
    if (clipSeq = 0 || clipSeq != gLastImageClipboardSeq) {
        return false
    }
    if (gLastImageWinPath = "" || !FileExist(gLastImageWinPath)) {
        return false
    }

    if !SetClipboardAttachmentSync(gLastImageWinPath) {
        return false
    }

    RefreshImageCacheSequence()
    NormalizeModifierStateBeforeSend()
    Send("^v")
    return true
}

TryPasteCachedPlainPath(clipSeq) {
    global gLastImageClipboardSeq, gLastImageWinPath, gLastImageWslPath
    if (clipSeq = 0 || clipSeq != gLastImageClipboardSeq) {
        return false
    }
    if (gLastImageWinPath = "") {
        return false
    }

    local wslPath := gLastImageWslPath
    if (wslPath = "") {
        local fileName := ""
        SplitPath(gLastImageWinPath, &fileName)
        wslPath := BuildWslPath(gLastImageWinPath, fileName)
    }
    if (wslPath = "") {
        return false
    }

    NormalizeModifierStateBeforeSend()
    SendText(wslPath)
    if !FileExist(gLastImageWinPath) {
        TriggerAsyncSaveIfNeeded(gLastImageWinPath)
    }
    return true
}

SaveImageCache(winPath) {
    global gLastImageClipboardSeq, gLastImageWinPath, gLastImageWslPath
    if FileExist(winPath) {
        gLastImageWinPath := winPath
        gLastImageWslPath := ""
        RefreshImageCacheSequence()
        return
    }
    gLastImageClipboardSeq := 0
    gLastImageWinPath := ""
    gLastImageWslPath := ""
}

SavePlainPathCache(clipSeq, winPath, wslPath) {
    global gLastImageClipboardSeq, gLastImageWinPath, gLastImageWslPath
    if (clipSeq = 0 || winPath = "" || wslPath = "") {
        return
    }
    gLastImageClipboardSeq := clipSeq
    gLastImageWinPath := winPath
    gLastImageWslPath := wslPath
}

GetClipboardSequence() {
    return DllCall("user32.dll\GetClipboardSequenceNumber", "UInt")
}

RefreshImageCacheSequence() {
    global gLastImageClipboardSeq
    local seq := GetClipboardSequence()
    gLastImageClipboardSeq := seq != 0 ? seq : 0
}

RefreshPendingSaveState() {
    global gPendingSaveWinPath, gPendingSaveStartedAt
    if (gPendingSaveWinPath = "") {
        return
    }

    ; Once the file is on disk, clear the "in progress" marker.
    if FileExist(gPendingSaveWinPath) {
        gPendingSaveWinPath := ""
        gPendingSaveStartedAt := 0
        return
    }

    ; If it still hasn't succeeded after 10 seconds, treat the previous async save as failed and allow a retry.
    if (gPendingSaveStartedAt != 0 && A_TickCount - gPendingSaveStartedAt > 10000) {
        gPendingSaveWinPath := ""
        gPendingSaveStartedAt := 0
    }
}

TriggerAsyncSaveIfNeeded(winPath) {
    global gPendingSaveWinPath, gPendingSaveStartedAt
    if (winPath = "") {
        return false
    }

    RefreshPendingSaveState()
    if FileExist(winPath) {
        return true
    }
    if (gPendingSaveWinPath = winPath) {
        return true
    }

    try {
        Run(BuildSaveCommand(winPath), "", "Hide")
        gPendingSaveWinPath := winPath
        gPendingSaveStartedAt := A_TickCount
        return true
    } catch {
        return false
    }
}

SaveAndSetAttachmentSync(winPath) {
    global gSaveAndSetAttachmentScript
    if !FileExist(gSaveAndSetAttachmentScript) {
        ; If the script is missing, degrade to the old flow to stay usable
        if !SaveClipboardImageSync(winPath) {
            return false
        }
        return SetClipboardAttachmentSync(winPath)
    }

    try {
        RunWait(BuildSaveAndSetAttachmentCommand(winPath), "", "Hide")
    } catch {
        return false
    }
    return FileExist(winPath) != ""
}

SaveClipboardImageSync(winPath) {
    try {
        RunWait(BuildSaveCommand(winPath), "", "Hide")
    } catch {
        return false
    }
    return FileExist(winPath) != ""
}

SetClipboardAttachmentSync(winPath) {
    global gSetAttachmentClipboardScript
    if !FileExist(gSetAttachmentClipboardScript) {
        return false
    }

    try {
        RunWait(BuildSetAttachmentClipboardCommand(winPath), "", "Hide")
        return true
    } catch {
        return false
    }
}

ClipboardHasImage() {
    ; CF_BITMAP=2, CF_DIB=8, CF_DIBV5=17
    return DllCall("user32.dll\IsClipboardFormatAvailable", "UInt", 2, "Int")
        || DllCall("user32.dll\IsClipboardFormatAvailable", "UInt", 8, "Int")
        || DllCall("user32.dll\IsClipboardFormatAvailable", "UInt", 17, "Int")
}

NormalizeModifierStateBeforeSend() {
    ; Release modifier key state so the input box doesn't swallow characters after an Alt combination
    SendInput("{Alt up}{Ctrl up}{Shift up}")
}

; ------------------ Helper to restore the keyboard layout ------------------
RestoreKeyboardLayout(hkl) {
    if (hkl != 0) {
        PostMessage(0x50, 0, hkl, , "A")
        Sleep 30
    }
}

ScheduleKeyboardLayoutRestore(hkl, delayMs := 120) {
    global gPendingRestoreHKL
    if (hkl = 0) {
        return
    }
    gPendingRestoreHKL := hkl
    SetTimer(RestoreKeyboardLayoutTask, -delayMs)
}

RestoreKeyboardLayoutTask() {
    global gPendingRestoreHKL
    if (gPendingRestoreHKL != 0) {
        RestoreKeyboardLayout(gPendingRestoreHKL)
        gPendingRestoreHKL := 0
    }
}

BuildWslPath(winPath, fileName) {
    global gWslTempDir
    if (gWslTempDir != "") {
        return gWslTempDir "/" fileName
    }
    return ConvertPathToWsl(winPath)
}

BuildPasteText(wslPath) {
    global gPasteFormat
    switch gPasteFormat {
        case "attachment":
            ; If the attachment channel fails, fall back to a plain path
            return wslPath
        default:
            return wslPath
    }
}

; ------------------ Cleanup logic on exit (calls exit-all.ps1) ------------------
HandleExit(ExitReason, ExitCode) {
    CleanupAndExit(False)
}

ExitFromTray(*) {
    CleanupAndExit(True)
}

GetFullPath(path) {
    bufSize := 260  ; MAX_PATH
    buf := Buffer(bufSize * 2)  ; 2 bytes per character (Unicode)
    DllCall("GetFullPathNameW", "Str", path, "UInt", bufSize, "Ptr", buf, "Ptr", 0)
    return StrGet(buf)
}


CleanupAndExit(shouldExit) {
    global gCleanupDone, gExitScript, gScriptDir, gTempDir
    if (!gCleanupDone && FileExist(gExitScript)) {
        gCleanupDone := true
        try {
            RunWait(Format('powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "{1}" -TempDir "{2}"', gExitScript, gTempDir), gScriptDir, "Hide")
        } catch {
            ; Silently ignore cleanup-script failures so the main program can still exit
        }
    }
    if (shouldExit) {
        ExitApp()
    }
}


ShowTempFolder(*) {
    global gTempDir
    local tempDir := DirExist(gTempDir) ? gTempDir : A_ScriptDir
    Run(Format('explorer "{1}"', tempDir))
}


InitializeHelperScripts() {
    global gTempDir, gWslTempDir
    if !DirExist(gTempDir) {
        try {
            DirCreate(gTempDir)
        } catch {
            ; Ignore creation failures
        }
    }

    ; Precompute the WSL path of the temp directory to cut the conversion cost on every hotkey press
    local convertedDir := ConvertPathToWsl(gTempDir)
    if (convertedDir != "") {
        gWslTempDir := RTrim(convertedDir, "/")
    }

    SetTimer(CleanupTempFolder, 2 * 60 * 60 * 1000)  ; Run once every two hours

}

SetupTrayMenu() {
    global gHotkeyMenu, gSpeedModeMenu, gPasteFormatMenu
    A_IconTip := "WSL CLI Clipboard Helper"
    try A_TrayMenu.Delete()
    gHotkeyMenu := Menu()
    gHotkeyMenu.Add("Alt+V", UseHotkeyAltV)
    gHotkeyMenu.Add("Ctrl+Alt+V", UseHotkeyCtrlAltV)
    gHotkeyMenu.Add("Alt+Enter", UseHotkeyAltEnter)
    gSpeedModeMenu := Menu()
    gSpeedModeMenu.Add("Safe mode (IME protection)", UseSpeedModeSafe)
    gSpeedModeMenu.Add("Fast mode (faster)", UseSpeedModeFast)
    gPasteFormatMenu := Menu()
    gPasteFormatMenu.Add("Attachment channel (no @, experimental)", UsePasteFormatAttachment)
    gPasteFormatMenu.Add("Plain path (compatible)", UsePasteFormatPlain)
    A_TrayMenu.Add("Switch hotkey", gHotkeyMenu)
    A_TrayMenu.Add("Mode", gSpeedModeMenu)
    A_TrayMenu.Add("Paste format", gPasteFormatMenu)
    A_TrayMenu.Add("Open image cache", ShowTempFolder)
    A_TrayMenu.Add()
    A_TrayMenu.Add("Exit", ExitFromTray)
}

InitializeSpeedMode() {
    global gSpeedMode
    local configuredMode := LoadSpeedModeConfig()
    if !IsSupportedSpeedMode(configuredMode) {
        configuredMode := "safe"
    }
    gSpeedMode := configuredMode
    SaveSpeedModeConfig(configuredMode)
    UpdateSpeedModeMenuState()
}

InitializePasteFormat() {
    global gPasteFormat
    local configuredFormat := LoadPasteFormatConfig()
    if !IsSupportedPasteFormat(configuredFormat) {
        configuredFormat := "plain"
    }
    gPasteFormat := configuredFormat
    SavePasteFormatConfig(configuredFormat)
    UpdatePasteFormatMenuState()
}

InitializeHotkey() {
    global gCurrentHotkey
    local configuredHotkey := LoadHotkeyConfig()
    if !IsSupportedHotkey(configuredHotkey) {
        configuredHotkey := "!v"
    }

    gCurrentHotkey := configuredHotkey
    SaveHotkeyConfig(configuredHotkey)
    UpdateHotkeyMenuState()
}

LoadSpeedModeConfig() {
    global gConfigFile
    try {
        return Trim(IniRead(gConfigFile, "Runtime", "Mode", "safe"))
    } catch {
        return "safe"
    }
}

SaveSpeedModeConfig(mode) {
    global gConfigFile
    try {
        IniWrite(mode, gConfigFile, "Runtime", "Mode")
    } catch {
        ; Silently ignore config write failures; the main flow is unaffected
    }
}

LoadPasteFormatConfig() {
    global gConfigFile
    try {
        return Trim(IniRead(gConfigFile, "Output", "Format", "plain"))
    } catch {
        return "plain"
    }
}

SavePasteFormatConfig(format) {
    global gConfigFile
    try {
        IniWrite(format, gConfigFile, "Output", "Format")
    } catch {
        ; Silently ignore config write failures; the main flow is unaffected
    }
}

LoadHotkeyConfig() {
    global gConfigFile
    try {
        return Trim(IniRead(gConfigFile, "Hotkey", "Paste", "!v"))
    } catch {
        return "!v"
    }
}

SaveHotkeyConfig(hotkey) {
    global gConfigFile
    try {
        IniWrite(hotkey, gConfigFile, "Hotkey", "Paste")
    } catch {
        ; Silently ignore config write failures; the main flow is unaffected
    }
}

SwitchHotkeyTo(targetHotkey) {
    global gCurrentHotkey
    if (targetHotkey = gCurrentHotkey) {
        return
    }

    gCurrentHotkey := targetHotkey
    SaveHotkeyConfig(targetHotkey)
    UpdateHotkeyMenuState()
    NotifyUser("Hotkey switched", "Now: " HotkeyToLabel(targetHotkey), 1800)
}

SwitchSpeedModeTo(targetMode) {
    global gSpeedMode
    if (targetMode = gSpeedMode) {
        return
    }

    gSpeedMode := targetMode
    SaveSpeedModeConfig(targetMode)
    UpdateSpeedModeMenuState()
    NotifyUser("Mode switched", "Now: " SpeedModeToLabel(targetMode), 2200)
}

UseHotkeyAltV(*) {
    SwitchHotkeyTo("!v")
}

UseHotkeyCtrlAltV(*) {
    SwitchHotkeyTo("^!v")
}

UseHotkeyAltEnter(*) {
    SwitchHotkeyTo("!Enter")
}

UseSpeedModeSafe(*) {
    SwitchSpeedModeTo("safe")
}

UseSpeedModeFast(*) {
    SwitchSpeedModeTo("fast")
}

UsePasteFormatPlain(*) {
    SwitchPasteFormatTo("plain")
}

UsePasteFormatAttachment(*) {
    SwitchPasteFormatTo("attachment")
}

SwitchPasteFormatTo(targetFormat) {
    global gPasteFormat
    if (targetFormat = gPasteFormat) {
        return
    }

    gPasteFormat := targetFormat
    SavePasteFormatConfig(targetFormat)
    UpdatePasteFormatMenuState()
    NotifyUser("Paste format switched", "Now: " PasteFormatToLabel(targetFormat), 2200)
}

UpdateHotkeyMenuState() {
    global gHotkeyMenu, gCurrentHotkey
    if !IsObject(gHotkeyMenu) {
        return
    }

    gHotkeyMenu.Uncheck("Alt+V")
    gHotkeyMenu.Uncheck("Ctrl+Alt+V")
    gHotkeyMenu.Uncheck("Alt+Enter")

    local currentLabel := HotkeyToLabel(gCurrentHotkey)
    if (currentLabel != "") {
        gHotkeyMenu.Check(currentLabel)
    }
    UpdateTrayIconTip()
}

UpdateSpeedModeMenuState() {
    global gSpeedModeMenu, gSpeedMode
    if !IsObject(gSpeedModeMenu) {
        return
    }

    gSpeedModeMenu.Uncheck("Safe mode (IME protection)")
    gSpeedModeMenu.Uncheck("Fast mode (faster)")

    local modeLabel := SpeedModeToLabel(gSpeedMode)
    if (modeLabel != "") {
        gSpeedModeMenu.Check(modeLabel)
    }
    UpdateTrayIconTip()
}

UpdatePasteFormatMenuState() {
    global gPasteFormatMenu, gPasteFormat
    if !IsObject(gPasteFormatMenu) {
        return
    }

    gPasteFormatMenu.Uncheck("Attachment channel (no @, experimental)")
    gPasteFormatMenu.Uncheck("Plain path (compatible)")

    local formatLabel := PasteFormatToLabel(gPasteFormat)
    if (formatLabel != "") {
        gPasteFormatMenu.Check(formatLabel)
    }
    UpdateTrayIconTip()
}

UpdateTrayIconTip() {
    global gCurrentHotkey, gSpeedMode, gPasteFormat
    local hotkeyLabel := HotkeyToLabel(gCurrentHotkey)
    local modeLabel := SpeedModeToShortLabel(gSpeedMode)
    local formatLabel := PasteFormatToShortLabel(gPasteFormat)

    local tipParts := ""
    if (hotkeyLabel != "") {
        tipParts := hotkeyLabel
    }
    if (modeLabel != "") {
        tipParts := tipParts = "" ? modeLabel : tipParts " | " modeLabel
    }
    if (formatLabel != "") {
        tipParts := tipParts = "" ? formatLabel : tipParts " | " formatLabel
    }

    if (tipParts = "") {
        A_IconTip := "WSL CLI Clipboard Helper"
        return
    }

    A_IconTip := "WSL CLI Clipboard Helper (" tipParts ")"
}

HotkeyToLabel(hotkey) {
    switch hotkey {
        case "!v":
            return "Alt+V"
        case "^!v":
            return "Ctrl+Alt+V"
        case "!Enter":
            return "Alt+Enter"
        default:
            return ""
    }
}

IsSupportedHotkey(hotkey) {
    return hotkey = "!v" || hotkey = "^!v" || hotkey = "!Enter"
}

IsSupportedPasteFormat(format) {
    return format = "attachment" || format = "plain"
}

PasteFormatToLabel(format) {
    switch format {
        case "attachment":
            return "Attachment channel (no @, experimental)"
        case "plain":
            return "Plain path (compatible)"
        default:
            return ""
    }
}

PasteFormatToShortLabel(format) {
    switch format {
        case "attachment":
            return "Attachment"
        case "plain":
            return "Path"
        default:
            return ""
    }
}

SpeedModeToLabel(mode) {
    switch mode {
        case "safe":
            return "Safe mode (IME protection)"
        case "fast":
            return "Fast mode (faster)"
        default:
            return ""
    }
}

SpeedModeToShortLabel(mode) {
    switch mode {
        case "safe":
            return "Safe"
        case "fast":
            return "Fast"
        default:
            return ""
    }
}

IsSupportedSpeedMode(mode) {
    return mode = "safe" || mode = "fast"
}

IsImeGuardEnabled() {
    global gSpeedMode
    return gSpeedMode != "fast"
}

IsAttachmentPasteMode() {
    global gPasteFormat
    return gPasteFormat = "attachment"
}

; ------------------ Utility: convert a Windows path to a WSL path ------------------
ConvertPathToWsl(winPath) {
    local p := Trim(winPath, '"')

    ; Only drive paths are converted. No more fallback to `cmd.exe /C wsl wslpath`,
    ; avoiding cmd's %VAR% expansion and the command injection that quote concatenation brings.
    if RegExMatch(p, "^[A-Za-z]:\\") {
        local drive := SubStr(p, 1, 1)
        local rest := SubStr(p, 3)
        rest := StrReplace(rest, "\", "/")
        rest := RegExReplace(rest, "^/+", "")
        return "/mnt/" . StrLower(drive) . "/" . rest
    }

    return ""
}


; Generic notification helper: tray notice + optional beep, with simple debouncing to avoid spam
NotifyUser(title, msg, durationMs := 2500, beep := false) {
    global gLastNotifyAt
    now := A_TickCount
    if (now - gLastNotifyAt < 800)  ; 800 ms debounce
        return
    gLastNotifyAt := now

    TrayTip(title, msg, durationMs)
    if (beep) {
        SoundBeep(750, 120)  ; Subtle beep
    }
}


CleanupTempFolder(*) {
    global gTempDir, gLastImageWinPath, gLastImageClipboardSeq, gLastImageWslPath, gPendingSaveWinPath, gPendingSaveStartedAt
    try {
        if (gLastImageWinPath != "" && !FileExist(gLastImageWinPath)) {
            gLastImageWinPath := ""
            gLastImageClipboardSeq := 0
            gLastImageWslPath := ""
        }
        if (gPendingSaveWinPath != "" && FileExist(gPendingSaveWinPath)) {
            gPendingSaveWinPath := ""
            gPendingSaveStartedAt := 0
        }
        Loop Files gTempDir "\*.png", "F" {
            ; Newer time first, older time second, giving a positive number of seconds
            if (DateDiff(A_Now, A_LoopFileTimeModified, "Seconds") > 7200) {
                FileDelete(A_LoopFileFullPath)
            }
        }
    } catch {
        ; Ignore cleanup failures
    }
}
