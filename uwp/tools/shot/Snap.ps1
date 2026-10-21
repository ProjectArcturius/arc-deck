<#
  Resize the Saiku Deck window with the Windows snap shortcuts.

  Why not SetWindowPos: the window belongs to ApplicationFrameHost and SetWindowPos
  reports success while the size does not change -- verified on this machine, both
  SW_MAXIMIZE and explicit geometry. The snap shortcuts go through the shell's own
  resize path, which works.

  Why not activate first: a full-screen game may hold the foreground and refuse to
  yield. The shortcuts only need the window to be the *active* window for the shell
  to target it, so we attach to the current foreground thread's input queue, post the
  keys, then detach. If the game still keeps focus, the keys go to the game, so this
  script verifies the resulting rectangle and says so instead of pretending.

  Usage:
    powershell.exe -File Snap.ps1 -Snap Left
    powershell.exe -File Snap.ps1 -Snap Maximize
#>
param(
  [ValidateSet("Left", "Right", "Maximize")]
  [string]$Snap = "Left"
)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class Snap {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int m);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }

  public const byte VK_LWIN = 0x5B;
  const uint KEYUP = 0x0002;

  public static IntPtr Find() {
    foreach (var p in System.Diagnostics.Process.GetProcesses()) {
      if (p.ProcessName != "ApplicationFrameHost") continue;
      var hh = p.MainWindowHandle; if (hh == IntPtr.Zero) continue;
      var t = new System.Text.StringBuilder(512); GetWindowTextW(hh, t, 512);
      if (t.ToString().Contains("Saiku Deck")) return hh;
    }
    return IntPtr.Zero;
  }

  public static string Rect(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    return (r.R - r.L) + "x" + (r.B - r.T) + " at " + r.L + "," + r.T;
  }

  public static string Focus() {
    IntPtr h = Find();
    if (h == IntPtr.Zero) return "NO_WINDOW";
    uint pid; uint fgThread = GetWindowThreadProcessId(GetForegroundWindow(), out pid);
    uint me = GetCurrentThreadId();
    AttachThreadInput(me, fgThread, true);
    SetForegroundWindow(h);
    AttachThreadInput(me, fgThread, false);
    var t = new System.Text.StringBuilder(512); GetWindowTextW(GetForegroundWindow(), t, 512);
    return t.ToString();
  }

  /// direction: 'L' left arrow, 'R' right arrow, 'U' up arrow
  public static void Do(char direction) {
    byte vk = direction == 'L' ? (byte)0x25 : direction == 'R' ? (byte)0x27 : (byte)0x26;
    keybd_event(VK_LWIN, 0, 0, IntPtr.Zero);
    System.Threading.Thread.Sleep(60);
    keybd_event(vk, 0, 0, IntPtr.Zero);
    System.Threading.Thread.Sleep(60);
    keybd_event(vk, 0, KEYUP, IntPtr.Zero);
    System.Threading.Thread.Sleep(60);
    keybd_event(VK_LWIN, 0, KEYUP, IntPtr.Zero);
  }
}
"@

$h = [Snap]::Find()
if ($h -eq [IntPtr]::Zero) { Write-Error "Saiku Deck window not found"; exit 1 }

"before: $([Snap]::Rect($h))"
$fg = [Snap]::Focus()
"foreground after focus attempt: [$fg]"

if ($fg -notlike "*Saiku Deck*") {
  Write-Error "cannot focus Saiku Deck (foreground is [$fg]) -- another app is holding it; snap keys would go to the wrong window"
  exit 2
}

switch ($Snap) {
  "Left"     { [Snap]::Do('L'); Start-Sleep -Milliseconds 300; [Snap]::Do('U') }
  "Right"    { [Snap]::Do('R'); Start-Sleep -Milliseconds 300; [Snap]::Do('U') }
  "Maximize" { [Snap]::Do('U') }
}

Start-Sleep -Milliseconds 900
"after:  $([Snap]::Rect($h))"
