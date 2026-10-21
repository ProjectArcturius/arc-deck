# TUI 真终端探针：起一个隐藏控制台、把按键写进它的输入缓冲区、读它的屏幕缓冲区。
#
# 为什么需要它：TUI 的单测走 ratatui 的 TestBackend，证明不了「在真终端里能起、
# 能打字、能提交」。这个脚本用 AttachConsole + ReadConsoleOutputCharacter +
# WriteConsoleInputW 驱动真的 `saiku` 进程，把「可用」变成几条可复现的断言。
#
# 用法（Windows PowerShell 5.1；脚本含中文，必须带 BOM）：
#   powershell -File console-probe.ps1 -Exe D:\arcdeck\target-main\release\saiku.exe
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
    [string]$Log = (Join-Path $env:TEMP "saiku-console-probe.log"),
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
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool GetConsoleCursorInfo(IntPtr h, out CONSOLE_CURSOR_INFO info);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool ReadConsoleOutputCharacterW(IntPtr h, [Out] char[] buf, uint len, COORD coord, out uint read);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool WriteConsoleInputW(IntPtr h, INPUT_RECORD[] recs, uint len, out uint written);

    [StructLayout(LayoutKind.Sequential)] public struct COORD { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential)] public struct SMALL_RECT { public short Left; public short Top; public short Right; public short Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct CONSOLE_CURSOR_INFO { public uint size; public bool visible; }
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

    public static bool CursorOnComposer(IntPtr h) {
        CONSOLE_CURSOR_INFO cursor;
        CONSOLE_SCREEN_BUFFER_INFO screen;
        if (!GetConsoleCursorInfo(h, out cursor) || !GetConsoleScreenBufferInfo(h, out screen)) return false;
        if (!cursor.visible) return true;
        var row = new char[screen.dwSize.X];
        uint read;
        if (!ReadConsoleOutputCharacterW(h, row, (uint)row.Length,
            new COORD { X = 0, Y = screen.dwCursorPosition.Y }, out read)) return false;
        var text = new string(row).Replace("\0", "");
        return text.Contains("输入下一条") || text.Contains("继续这个会话") || text.Contains("输入消息");
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

# 断言中途出错也要收掉 TUI：留一个隐藏的 saiku 在后台，它会一直占着交互会话目录的锁
#（实测踩过：下一次启动被「另一个 saiku 正在使用」拒绝）。
try {
$checks = [ordered]@{}
$screenHome = Snapshot "home"
$checks.home_has_brand = $screenHome.Contains("Saiku")

# 交互内核在后台起（几秒到几十秒）；就绪之前提交会排队，但探针要的是「就绪」这个事实。
# 「就绪」不再有通知（2026-10-20 去掉了常驻通知行）：页脚右边的「内核启动中」消失、
# 且没有出现「内核不可用」，就是就绪。
$ready = $false
for ($i = 0; $i -lt 60; $i++) {
    $now = Screen
    if ($now.Contains("内核不可用") -or $now.Contains("交互会话不可用")) { break }
    if ($now.Contains("Saiku") -and -not $now.Contains("内核启动中")) { $ready = $true; break }
    Start-Sleep -Seconds 1
}
$checks.kernel_ready = $ready
$readyScreen = Snapshot "ready"
# 页脚只有一行：右边是键位，不再是八个键位的说明书。
$checks.footer_is_one_line = $readyScreen.Contains("ctrl+p 命令") -and -not $readyScreen.Contains("PgUp 上滚")
$checks.placeholder_says_enter = $readyScreen.Contains("Enter 发送")

# `@` 打开文件补全。弹层没有标题了（照 OpenCode），所以断言候选本身：打 `@AGENTS` 要列出 AGENTS.md。
Send-Key 0x1B ([char]0x1B) 0
Send-Text "@AGENTS"
$at = Snapshot "at"
$checks.at_popup = $at.Contains("AGENTS.md")
Send-Key 0x1B ([char]0x1B) 0
Send-Key 0x1B ([char]0x1B) 0
# 清掉草稿里的 `@AGENTS`（Esc 只收弹层）。
for ($i = 0; $i -lt 8; $i++) { Send-Key 0x08 ([char]8) 0 }

# 暂存：有字时 Ctrl+X S 收起草稿；`/stash` 列出来，Enter 取回。不碰模型。
Send-Text "stashed draft"
Send-Key 0x58 ([char]0x18) 8
Send-Text "s"
$checks.stash_clears_composer = -not (Screen).Contains("stashed draft")
Send-Text "/stash"
Send-Key 0x0D ([char]0x0D) 0
$stash = Snapshot "stash"
$checks.stash_dialog = $stash.Contains("暂存的草稿") -and $stash.Contains("stashed draft")
Send-Key 0x0D ([char]0x0D) 0
$checks.stash_restores = (Screen).Contains("stashed draft")
for ($i = 0; $i -lt 14; $i++) { Send-Key 0x08 ([char]8) 0 }

# 补全里**一次** Enter 就执行（`/per` → `/permission` → 授权对话框）。用真的 Enter（0x0D），
# 不是 F6 —— 用户报的就是 Enter 要按两次（2026-10-20）。
Send-Text "/per"
Send-Key 0x0D ([char]0x0D) 0
$checks.one_enter_runs_completion = (Screen).Contains("授权级别")
[void](Snapshot "permission")
Send-Key 0x1B ([char]0x1B) 0

# Shift+Tab 换 Agent：输入框下面的名字跟着变（Echo → Build），再从 Agent 对话框选回 Echo。
Send-Key 0x09 ([char]9) 0x10
$checks.shift_tab_switches_agent = (Screen).Contains("Build ·")
Send-Text "/agents"
Send-Key 0x0D ([char]0x0D) 0
Send-Key 0x0D ([char]0x0D) 0
Start-Sleep -Seconds 1
$checks.agent_back_to_echo = (Screen).Contains("Echo ·")

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
Send-Key 0x0D ([char]0x0D) 0
$sessionsScreen = Snapshot "sessions"
$checks.sessions_dialog = $sessionsScreen.Contains("会话（当前工作区）")
Send-Key 0x1B ([char]0x1B) 0
Send-Text "/runs"
Send-Key 0x0D ([char]0x0D) 0
$runsScreen = Snapshot "runs"
$checks.runs_dialog = $runsScreen.Contains("工人运行")
Send-Key 0x1B ([char]0x1B) 0

# 内容检索走真内核的 `session/search`（本地 FTS，不碰模型）。命中几条取决于本机会话，
# 判据只是「检索结果弹层出来了」—— 没有命中也是一个合法的结果（「0 条命中」）。
Send-Text "/search OK"
Send-Key 0x0D ([char]0x0D) 0
$searched = $false
for ($i = 0; $i -lt 30; $i++) {
    $now = Screen
    if ($now.Contains("条命中")) { $searched = $true; break }
    Start-Sleep -Seconds 1
}
$checks.search_real_kernel = $searched
[void](Snapshot "search")
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
    Send-Key 0x0D ([char]0x0D) 0
    $checks.turn1_answer_42 = Wait-Screen "42" 120
    [void](Snapshot "turn1")
    Start-Sleep -Seconds 2
    Send-Text "再加 1 呢？只回复阿拉伯数字"
    Send-Key 0x0D ([char]0x0D) 0
    $checks.turn2_answer_43 = Wait-Screen "43" 120
    $final = Snapshot "turn2"
    $checks.submit_has_error = $final.Contains("模型失败") -or $final.Contains("错误：")
}

if ($Cancel) {
    # 必须等正文已开始，覆盖「Esc abort 后再发一轮」的用户现场。
    Send-Key 0x1B ([char]0x1B) 0
    Send-Key 0x1B ([char]0x1B) 0
    Send-Text "从 1 数到 5000，每行一个数字，不要省略"
    Send-Key 0x0D ([char]0x0D) 0
    # 等正文真的开始流式输出；启动后固定等 2 秒往往只取消了尚未开始的模型请求。
    $streaming = $false
    for ($i = 0; $i -lt 90; $i++) {
        # Markdown 把连续的纯数字行合并成一个段落，因此不能只匹配单独一行的 1。
        if ((Screen) -match '(?m)^\s+[0-9]+(\s+[0-9]+){2,}') { $streaming = $true; break }
        Start-Sleep -Milliseconds 500
    }
    $checks.streaming_before_cancel = $streaming
    # 跑着的时候页脚左边是扫描条 + `esc 中断`（OpenCode 的「还在工作」）。
    $running = Screen
    # 只认「中断」与 `■`：中文代码页的 conhost 把歧义宽度字符（`■`、`·`）画成两格，同一行后面的字会
    # 错位（实测 `esc 中断` 读回来是 `■中断`）。Windows Terminal 按一格画，没有这个问题。
    $checks.scanner_while_running = $running.Contains("中断") -and $running.Contains([string][char]0x25A0)
    $checks.cursor_stays_in_composer = $true
    for ($sample = 0; $sample -lt 500; $sample++) {
        if (-not [Probe]::CursorOnComposer($out)) { $checks.cursor_stays_in_composer = $false }
        Start-Sleep -Milliseconds 1
    }
    [void](Snapshot "running")
    Send-Key 0x1B ([char]0x1B) 0
    # 一轮的页脚是「▣ Echo · 模型 · 已取消」；conhost 里前面的 `·` 会挤掉一个字（见上），读回来是
    # 「· 取消」。只认这两种形状 —— 光认「取消」会被通知里的「Esc 取消这一轮」骗过。
    $checks.cancel_shown = $false
    for ($i = 0; $i -lt 60; $i++) {
        $now = Screen
        if ($now.Contains("已取消") -or $now.Contains("· 取消")) { $checks.cancel_shown = $true; break }
        Start-Sleep -Seconds 1
    }
    [void](Snapshot "cancel")
    # 取消后必须真的再发一轮；只检查能打字抓不到 1.0 前发现的后端挂死。
    Send-Text "只回复英文单词 RECOVERED"
    $checks.composer_usable_after_cancel = (Screen).Contains("RECOVERED")
    Send-Key 0x0D ([char]0x0D) 0
    $checks.next_turn_after_cancel = $false
    for ($i = 0; $i -lt 90; $i++) {
        $now = Screen
        # 用户题面里也有 RECOVERED，必须看到第二次，且扫描条已经停下。
        if ([regex]::Matches($now, "RECOVERED").Count -ge 2 -and -not $now.Contains("中断")) {
            $checks.next_turn_after_cancel = $true
            break
        }
        if ($now.Contains("错误：")) { break }
        Start-Sleep -Seconds 1
    }
    [void](Snapshot "after-cancel-turn")
}

if ($Resume) {
    # 跨进程恢复：会话列表的第一行是最近的那个（上一次 -Cancel 留下的），打开后历史要回来。
    Send-Key 0x1B ([char]0x1B) 0
    Send-Text "/sessions"
    Send-Key 0x0D ([char]0x0D) 0
    Start-Sleep -Seconds 2
    Send-Key 0x0D ([char]0x0D) 0
    # 最近的会话不一定是 -Cancel 留下的那个（用户自己的会话更新时就不是），所以判据是
    # 「打开后出现了一轮的页脚 `▣`」—— 任何有历史的会话都有。「已取消」只在同一次跑过 -Cancel 时才查。
    $checks.resume_history = Wait-Screen "▣ " 30
    if ($Cancel) {
        $resumed = Screen
        $checks.resume_shows_cancelled = $resumed.Contains("已取消") -or $resumed.Contains("· 取消")
    }
    [void](Snapshot "resume")
    # 子 agent 对话框：有子会话就列出来，没有就明说没有 —— 两种都算对话框起来了。
    Send-Text "/subagents"
    Send-Key 0x0D ([char]0x0D) 0
    Start-Sleep -Seconds 1
    $checks.subagents_dialog = (Screen).Contains("子 agent 会话")
    [void](Snapshot "subagents")
    Send-Key 0x1B ([char]0x1B) 0
}

# Ctrl+C quits (and cancels an active run); the supervisor may take a while to clean up.
Send-Key 0x43 ([char]3) 8
for ($i = 0; $i -lt 20 -and -not $process.HasExited; $i++) { Start-Sleep -Seconds 1 }
$checks.quit = $process.HasExited
# 交互内核是 TUI 拉起的 node 子进程：退出后不该留下（参 AGENTS 的 Windows 进程规则）。
Start-Sleep -Seconds 2
$kernels = @(Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains("--profile web") -and $_.CommandLine.Contains("saiku") })
$checks.no_kernel_left = ($kernels.Count -eq 0)

$json = $checks | ConvertTo-Json
$json | Out-File -FilePath $Log -Append -Encoding utf8
$json
if (@($checks.GetEnumerator() | Where-Object { $_.Value -is [bool] -and -not $_.Value -and $_.Key -ne 'submit_has_error' }).Count -gt 0 -or $checks.submit_has_error) {
    throw 'console probe failed; see log'
}
} finally {
    if (-not $process.HasExited) { & taskkill /PID $process.Id /T /F | Out-Null }
}

[void][Probe]::CloseHandle($out)
[void][Probe]::CloseHandle($in)
Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
