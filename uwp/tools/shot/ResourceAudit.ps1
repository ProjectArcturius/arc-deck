<#
  Validate the resource keys and SymbolIcon enum values used in the project's XAML.

  Two classes of silent failure this catches:

  1. `{ThemeResource KEY}` / `{StaticResource KEY}` whose key is defined nowhere.
     UWP does NOT error on this -- the target property keeps its inherited or default
     value. So an invented name shows up as "wrong colour" or "invisible text", never
     as a build failure. (Real case: TextFillColorSecondaryBrush and
     TextFillColorTertiaryBrush are **WinUI 3** brushes and exist in neither the UWP
     platform resources nor WinUI 2; 20 usages were silently falling back, and
     BodyStrongTextBlockStyle likewise.)

  2. `<SymbolIcon Symbol="X"/>` where X is not a member of the Windows.UI.Xaml
     Symbol enum.

  Keys defined inside the project's own Page.Resources / App.xaml count as defined.

  Usage:
    powershell.exe -File ResourceAudit.ps1
#>
param(
  [string]$ProjectRoot = "C:\Users\mc158\Documents\Projects\arc-deck\uwp\src"
)

$platform = "C:\Program Files (x86)\Windows Kits\10\DesignTime\CommonConfiguration\Neutral\UAP\10.0.22621.0\Generic\themeresources.xaml"
$winui2   = "$env:USERPROFILE\.nuget\packages\microsoft.ui.xaml\2.8.6\lib\uap10.0\Microsoft.UI.Xaml\Themes\Generic.xaml"
$platformGeneric = "C:\Program Files (x86)\Windows Kits\10\DesignTime\CommonConfiguration\Neutral\UAP\10.0.22621.0\Generic\generic.xaml"

function Get-Keys([string]$path) {
  if (-not (Test-Path $path)) { return @() }
  Select-String -Path $path -Pattern 'x:Key="([^"]+)"' -AllMatches |
    ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value }
}

$known = New-Object System.Collections.Generic.HashSet[string]
foreach ($p in @($platform, $winui2, $platformGeneric)) {
  foreach ($k in (Get-Keys $p)) { [void]$known.Add($k) }
}

# Keys the XAML framework injects at runtime rather than declaring in a file.
# Evidence: SystemColorButtonTextColor appears 152 times in the SDK's themeresources.xaml
# as a {ThemeResource} *reference* and 0 times as a definition -- yet it is the
# documented way to reach the user's high-contrast system colours.
foreach ($k in @("SystemColorButtonFaceColor","SystemColorButtonTextColor",
                 "SystemColorGrayTextColor","SystemColorHighlightColor",
                 "SystemColorHighlightTextColor","SystemColorHotlightColor",
                 "SystemColorWindowColor","SystemColorWindowTextColor")) {
  [void]$known.Add($k)
}
"external keys: $($known.Count)"

$files = Get-ChildItem $ProjectRoot -Recurse -Filter "*.xaml" -File

$local = New-Object System.Collections.Generic.HashSet[string]
foreach ($f in $files) {
  foreach ($k in (Get-Keys $f.FullName)) { [void]$local.Add($k) }
}
foreach ($k in @("ControlCornerRadius", "OverlayCornerRadius", "SymbolThemeFontFamily")) {
  [void]$local.Add($k)
}
"project-defined keys: $($local.Count)"

$used = @{}
foreach ($f in $files) {
  foreach ($h in (Select-String -Path $f.FullName -Pattern '\{(?:Theme|Static)Resource\s+([^}]+)\}' -AllMatches)) {
    foreach ($m in $h.Matches) {
      $key = $m.Groups[1].Value.Trim()
      if (-not $used.ContainsKey($key)) { $used[$key] = @() }
      $used[$key] += ("{0}:{1}" -f $f.Name, $h.LineNumber)
    }
  }
}

$missing = @($used.Keys | Where-Object { -not $known.Contains($_) -and -not $local.Contains($_) } | Sort-Object)

"referenced keys: $($used.Count)"
""
if ($missing.Count -eq 0) {
  "OK: every referenced resource key resolves."
} else {
  "!!! UNDEFINED RESOURCE KEYS ($($missing.Count)) -- these silently fall back:"
  foreach ($k in $missing) {
    "  {0}   x{1}" -f $k, $used[$k].Count
    foreach ($s in ($used[$k] | Select-Object -First 3)) { "      $s" }
  }
}

""
# The Symbol enum lives in a WinRT type that Windows PowerShell 5.1 cannot load by
# name -- [Windows.UI.Xaml.Controls.Symbol] raises "Unable to find type". So the
# member list is carried here instead. It is the documented Windows.UI.Xaml.Symbol
# enum; extend it if a newer SDK adds members.
$enum = @(
  "Accept","Account","Add","AddFriend","Admin","AlignCenter","AlignLeft","AlignRight",
  "AllApps","Attach","AttachCamera","Audio","Back","BlockContact","Bold","Bookmarks",
  "BrowsePhotos","Bullets","Calculator","Calendar","CalendarDay","CalendarReply",
  "CalendarWeek","Camera","Cancel","Caption","CellPhone","Character","Clear",
  "ClearSelection","Clock","ClosedCaption","ClosePane","Comment","Contact","Contact2",
  "ContactInfo","ContactPresence","Copy","Crop","Cut","Delete","Directions","DisableUpdates",
  "DisconnectDrive","Dislike","DockBottom","DockLeft","DockRight","Document","Download",
  "Edit","Emoji","Emoji2","Favorite","Filter","Find","Flag","Folder","Font","FontColor",
  "FontDecrease","FontIncrease","FontSize","Forward","FourBars","FullScreen","Globe",
  "Go","GoToStart","GoToToday","HangUp","Help","HideBcc","Highlight","Home","Import",
  "ImportAll","Important","Italic","Keyboard","LeaveChat","Library","Like","LikeDislike",
  "Link","List","Mail","MailFilled","MailForward","MailReply","MailReplyAll","Map",
  "MapDrive","MapPin","Memo","Message","Microphone","More","MoveToFolder","MusicInfo",
  "Mute","NewFolder","NewWindow","Next","OneBar","OpenFile","OpenLocal","OpenPane",
  "OpenWith","Orientation","OtherUser","OutlineStar","Page","Page2","Paste","Pause",
  "People","Permissions","Phone","Pictures","Pin","Placeholder","Play","PostUpdate",
  "Preview","PreviewLink","Previous","Print","Priority","ProtectedDocument","Read",
  "Redo","Refresh","Remote","Remove","Rename","Repair","RepeatAll","RepeatOne","ReportHacked",
  "ReShare","Rotate","RotateCamera","Save","SaveLocal","Scan","SelectAll","Send",
  "SetLockScreen","SetTile","Setting","Share","Shop","ShowBcc","ShowResults","Shuffle",
  "SlideShow","SolidStar","Sort","Stop","StopSlideShow","Street","Switch","SwitchApps",
  "Sync","SyncFolder","Tag","Target","ThreeBars","TouchPointer","Trim","TwoBars",
  "TwoPage","Underline","Undo","UnFavorite","UnPin","UnSyncFolder","Up","Upload",
  "Video","VideoChat","View","ViewAll","Volume","WebCam","World","ZeroBars","Zoom",
  "ZoomIn","ZoomOut"
)
"Symbol enum members (carried list): $($enum.Count)"

$badSymbols = @{}
foreach ($f in $files) {
  foreach ($h in (Select-String -Path $f.FullName -Pattern 'Symbol="([^"]+)"' -AllMatches)) {
    foreach ($m in $h.Matches) {
      $s = $m.Groups[1].Value.Trim()
      if ($s.StartsWith("{")) { continue }
      if ($enum -notcontains $s) {
        if (-not $badSymbols.ContainsKey($s)) { $badSymbols[$s] = @() }
        $badSymbols[$s] += ("{0}:{1}" -f $f.Name, $h.LineNumber)
      }
    }
  }
}

if ($badSymbols.Count -eq 0) {
  "OK: every SymbolIcon Symbol value is a real enum member."
} else {
  "!!! NOT A Symbol ENUM MEMBER ($($badSymbols.Count)):"
  foreach ($s in ($badSymbols.Keys | Sort-Object)) {
    "  {0}   x{1}" -f $s, $badSymbols[$s].Count
    foreach ($site in ($badSymbols[$s] | Select-Object -First 3)) { "      $site" }
  }
}
