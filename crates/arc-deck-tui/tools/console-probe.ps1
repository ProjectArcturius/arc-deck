# TUI 真终端探针：起一个隐藏控制台、把按键写进它的输入缓冲区、读它的屏幕缓冲区。
#
# 为什么需要它：TUI 的单测走 ratatui 的 TestBackend，证明不了「在真终端里能起、
# 能打字、能提交」。这个脚本用 AttachConsole + ReadConsoleOutputCharacter +
# WriteConsoleInputW 驱动真的 `arcdeck` 进程，把「可用」变成几条可复现的断言。
#
# 用法（Windows PowerShell 5.1；脚本含中文，必须带 BOM）：
#   powershell -File console-probe.ps1 -Exe D:\arcdeck\target-main\release\arcdeck.exe
#   powershell -File console-probe.ps1 -Exe ... -Submit     # 同一会话真发两轮，消耗两次小配额
#   powershell -File console-probe.ps1 -Exe ... -Cancel     # 发一条长的再 Esc，消耗一次小配额
#   powershell -File console-probe.ps1 -Exe ... -Resume     # 新进程里打开最近的会话（先跑过 -Cancel），不耗配额
#
# 三个实测出来的坑：
# 1. 屏幕缓冲区里宽字符占两个 cell，第二个 cell 是 NUL —— 不先去掉，`Contains("文件引用")`
#    会假阴性（开发时就踩过，功能是好的）。
# 2. 提交后的第一秒屏幕还显示着草稿（`say OK`），用它当「已回复」的判据会假阳性；
#    要等 notice 变成「运行已结束」。
# 3. Ctrl+C 之后监工要收尾（可能十几秒），等进程退出再断言 quit。
# 4. 多轮的判据要是「答案不在题面里」：第一轮问 7×6（答 42），第二轮只说「再加 1」（答 43）——
#    第二轮答对，只可能是同一个会话记得上一轮。

param(
    [Parameter(Mandatory=$true)][string]$Exe,
    [string]$WorkDir = "",
    [string]$Log = (Join-Path $env:TEMP "arcdeck-console-probe.log"),
    [switch]$Submit,
    [switch]$Cancel,
    [switch]$Resume
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
    // CharSet.Unicode 是必需的：不写的话结构里的 char 按 ANSI 单字节封送，
    // 非 ASCII 字符（中文）转码失败，WriteConsoleInputW 报 0x8007007A —— 只打 ASCII 时看不出来。
    [StructLayout(LayoutKind.Explicit, CharSet=CharSet.Unicode)] public struct KEY_EVENT_RECORD {
        [FieldOffset(0)] public int bKeyDown;
        [FieldOffset(4)] public ushort wRepeatCount;
        [FieldOffset(6)] public ushort wVirtualKeyCode;
        [FieldOffset(8)] public ushort wVirtualScanCode;
        [FieldOffset(10)] public char UnicodeChar;
        [FieldOffset(12)] public uint dwControlKeyState;
    }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct INPUT_RECORD {
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

# 断言中途出错也要收掉 TUI：留一个隐藏的 arcdeck 在后台，它会一直占着交互会话目录的锁
#（实测踩过：下一次启动被「另一个 arcdeck 正在使用」拒绝）。
try {
$checks = [ordered]@{}
$screenHome = Snapshot "home"
$checks.home_has_brand = $screenHome.Contains("Arc Deck")

# 交互内核在后台起（几秒到几十秒）；就绪之前提交会排队，但探针要的是「就绪」这个事实。
$ready = $false
for ($i = 0; $i -lt 60; $i++) {
    $now = Screen
    if ($now.Contains("交互内核就绪")) { $ready = $true; break }
    if ($now.Contains("交互会话不可用")) { break }
    Start-Sleep -Seconds 1
}
$checks.kernel_ready = $ready
[void](Snapshot "ready")

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

# `/sessions` 是会话列表、`/runs` 是工人运行；两个都不碰模型。
Send-Key 0x1B ([char]0x1B) 0
Send-Text "/sessions"
Send-Key 0x75 ([char]0) 0
$sessionsScreen = Snapshot "sessions"
$checks.sessions_dialog = $sessionsScreen.Contains("会话（当前工作区）")
Send-Key 0x1B ([char]0x1B) 0
Send-Text "/runs"
Send-Key 0x75 ([char]0) 0
$runsScreen = Snapshot "runs"
$checks.runs_dialog = $runsScreen.Contains("工人运行")
Send-Key 0x1B ([char]0x1B) 0

function Wait-Screen($needle, $seconds) {
    for ($i = 0; $i -lt $seconds; $i++) {
        $now = Screen
        if ($now.Contains($needle)) { return $true }
        if ($now.Contains("模型失败") -or $now.Contains("错误：")) { return $false }
        Start-Sleep -Seconds 1
    }
    return $false
}

if ($Submit) {
    Send-Key 0x1B ([char]0x1B) 0
    Send-Text "7 乘以 6 等于多少？只回复阿拉伯数字"
    Send-Key 0x75 ([char]0) 0
    $checks.turn1_answer_42 = Wait-Screen "42" 120
    [void](Snapshot "turn1")
    Start-Sleep -Seconds 2
    Send-Text "再加 1 呢？只回复阿拉伯数字"
    Send-Key 0x75 ([char]0) 0
    $checks.turn2_answer_43 = Wait-Screen "43" 120
    $final = Snapshot "turn2"
    $checks.submit_has_error = $final.Contains("模型失败") -or $final.Contains("错误：")
}

if ($Cancel) {
    # 一条要写很久的；Esc 取消这一轮，界面上要看得到「已取消」，输入区还能用。
    Send-Key 0x1B ([char]0x1B) 0
    Send-Key 0x1B ([char]0x1B) 0
    # 要足够长：flash 6 秒内就能数完 300（实测），那时 Esc 已经没有进行中的一轮可取消。
    Send-Text "从 1 数到 5000，每行一个数字，不要省略"
    Send-Key 0x75 ([char]0) 0
    Start-Sleep -Seconds 2
    Send-Key 0x1B ([char]0x1B) 0
    $checks.cancel_shown = Wait-Screen "已取消" 60
    [void](Snapshot "cancel")
    Send-Text "still here"
    $checks.composer_usable_after_cancel = (Screen).Contains("still here")
}

if ($Resume) {
    # 跨进程恢复：会话列表的第一行是最近的那个（上一次 -Cancel 留下的），打开后历史要回来。
    Send-Key 0x1B ([char]0x1B) 0
    Send-Text "/sessions"
    Send-Key 0x75 ([char]0) 0
    Start-Sleep -Seconds 2
    Send-Key 0x0D ([char]0x0D) 0
    $checks.resume_history = Wait-Screen "数到 5000" 30
    $checks.resume_shows_cancelled = (Screen).Contains("已取消")
    [void](Snapshot "resume")
}

# Ctrl+C quits (and cancels an active run); the supervisor may take a while to clean up.
Send-Key 0x43 ([char]3) 8
for ($i = 0; $i -lt 20 -and -not $process.HasExited; $i++) { Start-Sleep -Seconds 1 }
$checks.quit = $process.HasExited
# 交互内核是 TUI 拉起的 node 子进程：退出后不该留下（参 AGENTS 的 Windows 进程规则）。
Start-Sleep -Seconds 2
$kernels = @(Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains("--profile web") -and $_.CommandLine.Contains("arcdeck") })
$checks.no_kernel_left = ($kernels.Count -eq 0)

$checks | ConvertTo-Json | Tee-Object -FilePath $Log -Append
} finally {
    if (-not $process.HasExited) { & taskkill /PID $process.Id /T /F | Out-Null }
}

[void][Probe]::CloseHandle($out)
[void][Probe]::CloseHandle($in)
Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
