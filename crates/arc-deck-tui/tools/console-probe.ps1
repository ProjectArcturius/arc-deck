# TUI 真终端探针：起一个隐藏控制台、把按键写进它的输入缓冲区、读它的屏幕缓冲区。
#
# 为什么需要它：TUI 的单测走 ratatui 的 TestBackend，证明不了「在真终端里能起、
# 能打字、能提交」。这个脚本用 AttachConsole + ReadConsoleOutputCharacter +
# WriteConsoleInputW 驱动真的 `arcdeck` 进程，把「可用」变成几条可复现的断言。
#
# 用法（Windows PowerShell 5.1；脚本含中文，必须带 BOM）：
#   powershell -File console-probe.ps1 -Exe D:\arcdeck\target-main\release\arcdeck.exe
#   powershell -File console-probe.ps1 -Exe ... -Submit     # 真发一条 "say OK"，消耗一次小配额
#
# 三个实测出来的坑：
# 1. 屏幕缓冲区里宽字符占两个 cell，第二个 cell 是 NUL —— 不先去掉，`Contains("文件引用")`
#    会假阴性（开发时就踩过，功能是好的）。
# 2. 提交后的第一秒屏幕还显示着草稿（`say OK`），用它当「已回复」的判据会假阳性；
#    要等 notice 变成「运行已结束」。
# 3. Ctrl+C 之后监工要收尾（可能十几秒），等进程退出再断言 quit。

param(
    [Parameter(Mandatory=$true)][string]$Exe,
    [string]$WorkDir = "",
    [string]$Log = (Join-Path $env:TEMP "arcdeck-console-probe.log"),
    [switch]$Submit,
    [switch]$Cancel
)

$ErrorActionPreference = "Stop"

# $PSScriptRoot is not available while parameter defaults are evaluated (Windows PowerShell 5.1).
if ([string]::IsNullOrEmpty($WorkDir)) {
    $WorkDir = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
}

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class Probe {
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sec, uint disp, uint flags, IntPtr tmpl);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CONSOLE_SCREEN_BUFFER_INFO info);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool ReadConsoleOutputCharacterW(IntPtr h, [Out] char[] buf, uint len, COORD coord, out uint read);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool WriteConsoleInputW(IntPtr h, INPUT_RECORD[] recs, uint len, out uint written);

    [StructLayout(LayoutKind.Sequential)] public struct COORD { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential)] public struct SMALL_RECT { public short Left; public short Top; public short Right; public short Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct CONSOLE_SCREEN_BUFFER_INFO {
        public COORD dwSize; public COORD dwCursorPosition; public ushort wAttributes;
        public SMALL_RECT srWindow; public COORD dwMaximumWindowSize;
    }
    [StructLayout(LayoutKind.Explicit)] public struct KEY_EVENT_RECORD {
        [FieldOffset(0)] public int bKeyDown;
        [FieldOffset(4)] public ushort wRepeatCount;
        [FieldOffset(6)] public ushort wVirtualKeyCode;
        [FieldOffset(8)] public ushort wVirtualScanCode;
        [FieldOffset(10)] public char UnicodeChar;
        [FieldOffset(12)] public uint dwControlKeyState;
    }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT_RECORD {
        public ushort EventType;
        public KEY_EVENT_RECORD KeyEvent;
    }

    public static string Read(IntPtr h) {
        CONSOLE_SCREEN_BUFFER_INFO info;
        if (!GetConsoleScreenBufferInfo(h, out info)) return "";
        int width = info.srWindow.Right - info.srWindow.Left + 1;
        int rows = info.srWindow.Bottom - info.srWindow.Top + 1;
        var sb = new StringBuilder();
        for (int y = 0; y < rows; y++) {
            var buf = new char[width];
            uint read;
            var coord = new COORD { X = info.srWindow.Left, Y = (short)(info.srWindow.Top + y) };
            if (!ReadConsoleOutputCharacterW(h, buf, (uint)width, coord, out read)) break;
            sb.Append(new string(buf, 0, (int)read).Replace("\0", "").TrimEnd());
            sb.Append("\n");
        }
        return sb.ToString();
    }

    public static bool Key(IntPtr h, ushort vk, char ch, uint state, bool down) {
        var record = new INPUT_RECORD();
        record.EventType = 1;
        record.KeyEvent.bKeyDown = down ? 1 : 0;
        record.KeyEvent.wRepeatCount = 1;
        record.KeyEvent.wVirtualKeyCode = vk;
        record.KeyEvent.wVirtualScanCode = 0;
        record.KeyEvent.UnicodeChar = ch;
        record.KeyEvent.dwControlKeyState = state;
        uint written;
        return WriteConsoleInputW(h, new[] { record }, 1, out written) && written == 1;
    }
}
"@

[void][Probe]::FreeConsole()
$process = Start-Process -FilePath $Exe -WorkingDirectory $WorkDir -WindowStyle Hidden -PassThru
Start-Sleep -Milliseconds 1500
if (-not [Probe]::AttachConsole([uint32]$process.Id)) {
    "attach failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    exit 1
}
$out = [Probe]::CreateFileW("CONOUT$", [uint32]3221225472, 3, [IntPtr]::Zero, 3, 0, [IntPtr]::Zero)
$in = [Probe]::CreateFileW("CONIN$", [uint32]3221225472, 3, [IntPtr]::Zero, 3, 0, [IntPtr]::Zero)
if ($out -eq [IntPtr]::Zero -or $in -eq [IntPtr]::Zero) { "open console failed"; exit 2 }

function Screen { return [Probe]::Read($out) }
function Snapshot($label) {
    $text = Screen
    "===== $label =====" | Out-File -FilePath $Log -Append -Encoding utf8
    $text | Out-File -FilePath $Log -Append -Encoding utf8
    return $text
}
function Send-Text($text) {
    foreach ($c in $text.ToCharArray()) {
        [void][Probe]::Key($in, 0, $c, 0, $true)
        [void][Probe]::Key($in, 0, $c, 0, $false)
        Start-Sleep -Milliseconds 25
    }
    Start-Sleep -Milliseconds 250
}
function Send-Key($vk, $ch, $ctrl) {
    [void][Probe]::Key($in, $vk, [char]$ch, $ctrl, $true)
    [void][Probe]::Key($in, $vk, [char]$ch, $ctrl, $false)
    Start-Sleep -Milliseconds 400
}

$checks = [ordered]@{}
$screenHome = Snapshot "home"
$checks.home_has_brand = $screenHome.Contains("Arc Deck")

# `@` alone at the start of an empty draft opens the file popup.
Send-Key 0x1B ([char]0x1B) 0
Send-Text "@"
$at = Snapshot "at"
$checks.at_popup = $at.Contains("文件引用")
Send-Key 0x1B ([char]0x1B) 0
Send-Key 0x1B ([char]0x1B) 0

# F1 opens help; Esc closes it; typing lands in the composer.
Send-Key 0x70 ([char]0) 0
$help = Snapshot "help"
$checks.help_opened = $help.Contains("帮助")
Send-Key 0x1B ([char]0x1B) 0
Send-Text "hi"
$typed = Snapshot "typed"
$checks.typed_shows_hi = $typed.Contains("hi")

if ($Submit -or $Cancel) {
    # `/sessions` opens the runs dialog without touching the model.
    Send-Key 0x1B ([char]0x1B) 0
    Send-Text "/sessions"
    Send-Key 0x75 ([char]0) 0
    $runsScreen = Snapshot "runs"
    $checks.runs_dialog = $runsScreen.Contains("运行历史")
    Send-Key 0x1B ([char]0x1B) 0
}

if ($Submit) {
    # 真发一条；只等「运行已结束」，不等「屏幕里出现 OK」——草稿里本来就有 OK。
    Send-Key 0x1B ([char]0x1B) 0
    Send-Text "say OK"
    Send-Key 0x75 ([char]0) 0
    $settled = $false
    for ($i = 0; $i -lt 120; $i++) {
        Start-Sleep -Seconds 2
        $now = Screen
        if ($now.Contains("运行已结束")) { $settled = $true; break }
        if ($now.Contains("运行退出") -or $now.Contains("失败")) { $settled = $true; break }
    }
    $final = Snapshot "submit"
    $checks.submit_settled = $settled
    $checks.submit_has_ok = $final.Contains("OK")
    $checks.submit_has_error = $final.Contains("运行退出") -or $final.Contains("失败")
}

if ($Cancel) {
    # Start a run, then Esc while it is still starting; the run should end cancelled.
    Send-Key 0x1B ([char]0x1B) 0
    Send-Text "say OK"
    Send-Key 0x75 ([char]0) 0
    Start-Sleep -Seconds 3
    Send-Key 0x1B ([char]0x1B) 0
    $cancelSettled = $false
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Seconds 2
        $now = Screen
        if ($now.Contains("运行已结束") -or $now.Contains("取消")) { $cancelSettled = $true; break }
    }
    $finalCancel = Snapshot "cancel"
    $checks.cancel_settled = $cancelSettled
    $checks.cancel_screen = $finalCancel.Contains("取消")
    $newest = Get-ChildItem (Join-Path $env:LOCALAPPDATA "arcdeck\\state\\runs") -Directory -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($newest) {
        $status = Get-Content (Join-Path $newest.FullName "status.json") -Raw -ErrorAction SilentlyContinue | ConvertFrom-Json
        if ($status) { $checks.cancel_run_state = $status.state }
    }
}

# Ctrl+C quits (and cancels an active run); the supervisor may take a while to clean up.
Send-Key 0x43 ([char]3) 8
for ($i = 0; $i -lt 20 -and -not $process.HasExited; $i++) { Start-Sleep -Seconds 1 }
$checks.quit = $process.HasExited

$checks | ConvertTo-Json | Tee-Object -FilePath $Log -Append

[void][Probe]::CloseHandle($out)
[void][Probe]::CloseHandle($in)
Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
