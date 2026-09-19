<#
  Bring the Arc Deck window to the foreground, maximised, then screenshot it.

  Why this exists: a full-screen screenshot captures whatever is on top, and this
  machine sometimes has a game running full-screen. The earlier captures silently
  grabbed the wrong window, which is worse than failing -- it looks like the app
  regressed. Foreground the target first, then capture.

  Usage:
    powershell.exe -File Focus.ps1 -Out shot.png
    powershell.exe -File Focus.ps1 -Max -Out shot.png
#>
param(
  [string]$Out,
  [switch]$Max,
  [int]$SettleMs = 1200
)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class Fg {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();

  public static IntPtr Find() {
    foreach (var p in System.Diagnostics.Process.GetProcesses()) {
      if (p.ProcessName != "ApplicationFrameHost") continue;
      var hh = p.MainWindowHandle; if (hh == IntPtr.Zero) continue;
      var t = new System.Text.StringBuilder(512); GetWindowTextW(hh, t, 512);
      if (t.ToString().Contains("Arc Deck")) return hh;
    }
    return IntPtr.Zero;
  }

  public static string Title(IntPtr h) {
    var t = new System.Text.StringBuilder(512); GetWindowTextW(h, t, 512);
    return t.ToString();
  }

  // SetForegroundWindow is refused when the calling process is not the foreground
  // process. Attaching to the current foreground thread's input queue lifts that
  // restriction -- but NOT when a full-screen game holds the foreground: those
  // swallow the request. When that happens, minimise the current foreground window
  // first; that hands the foreground to someone else and unblocks us.
  public static string Focus() {
    IntPtr h = Find();
    if (h == IntPtr.Zero) return "NO_WINDOW";

    var fgBefore = Title(GetForegroundWindow());

    if (IsIconic(h)) ShowWindow(h, 9);   // SW_RESTORE
    TryTake(h);

    if (GetForegroundWindow() != h) {
      IntPtr blocker = GetForegroundWindow();
      if (blocker != IntPtr.Zero && blocker != h) {
        ShowWindow(blocker, 6);          // SW_MINIMIZE
        System.Threading.Thread.Sleep(500);
      }
      TryTake(h);
    }

    var fgAfter = Title(GetForegroundWindow());
    return "before=[" + fgBefore + "] after=[" + fgAfter + "]";
  }

  static void TryTake(IntPtr h) {
    uint fgPid; uint fgThread = GetWindowThreadProcessId(GetForegroundWindow(), out fgPid);
    uint me = GetCurrentThreadId();
    AttachThreadInput(me, fgThread, true);
    SetForegroundWindow(h);
    AttachThreadInput(me, fgThread, false);
  }
}
"@

if ($Max) {
  [Fg]::Focus() | Out-Null
  [Fg]::ShowWindow([Fg]::Find(), 3) | Out-Null   # SW_MAXIMIZE
  Start-Sleep -Milliseconds 600
}

$result = [Fg]::Focus()
"focus: $result"
Start-Sleep -Milliseconds $SettleMs

if ($Out) {
  Add-Type -AssemblyName System.Drawing
  Add-Type -AssemblyName System.Windows.Forms
  $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
  $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()

  $fgTitle = [Fg]::Title([Fg]::GetForegroundWindow())
  if ($fgTitle -notlike "*Arc Deck*") {
    "WARNING: foreground is [$fgTitle], not Arc Deck -- the capture is of the wrong window"
  } else {
    "saved $Out (foreground confirmed: $fgTitle)"
  }
}
