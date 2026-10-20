#ifndef SourceDir
  #error SourceDir must name the staged portable directory
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
#ifndef Version
  #define Version "1.0.0"
#endif

[Setup]
AppId={{231F89C8-49B7-47AD-8817-47C41DE88684}
AppName=Arc Deck
AppVersion={#Version}
AppPublisher=Project Arcturius
AppPublisherURL=https://github.com/ProjectArcturius/arc-deck
DefaultDirName={localappdata}\Programs\Arc Deck
DefaultGroupName=Arc Deck
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=arcdeck-{#Version}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
LicenseFile={#SourceDir}\LICENSE
UninstallDisplayIcon={app}\arcdeck.exe
DisableProgramGroupPage=yes

[Tasks]
Name: "addtopath"; Description: "Add Arc Deck to your user PATH (recommended)"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Excludes: "install.ps1,install.cmd,uninstall.ps1,uninstall.cmd"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Getting started"; Filename: "{app}\INSTALL.md"
Name: "{group}\Uninstall Arc Deck"; Filename: "{uninstallexe}"

[Code]
function HasPath(Value, Entry: String): Boolean;
begin
  Result := Pos(';' + Lowercase(Entry) + ';', ';' + Lowercase(Value) + ';') > 0;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var Value, Entry: String;
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then begin
    Entry := ExpandConstant('{app}');
    RegQueryStringValue(HKCU, 'Environment', 'Path', Value);
    if not HasPath(Value, Entry) then begin
      RegWriteStringValue(HKCU, 'Software\Arc Deck', 'OriginalPath', Value);
      if Value <> '' then Value := Entry + ';' + Value else Value := Entry;
      RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Value);
      RegWriteStringValue(HKCU, 'Software\Arc Deck', 'WrittenPath', Value);
      RegWriteStringValue(HKCU, 'Software\Arc Deck', 'AddedPath', Entry);
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var Value, Entry, Item, Kept, Original, Written: String; Sep: Integer;
begin
  if (CurUninstallStep = usPostUninstall) and
      RegQueryStringValue(HKCU, 'Software\Arc Deck', 'AddedPath', Entry) then begin
    RegQueryStringValue(HKCU, 'Environment', 'Path', Value);
    if RegQueryStringValue(HKCU, 'Software\Arc Deck', 'WrittenPath', Written) and
        (Value = Written) and RegQueryStringValue(HKCU, 'Software\Arc Deck', 'OriginalPath', Original) then begin
      RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Original);
      RegDeleteValue(HKCU, 'Software\Arc Deck', 'AddedPath');
      RegDeleteValue(HKCU, 'Software\Arc Deck', 'WrittenPath');
      RegDeleteValue(HKCU, 'Software\Arc Deck', 'OriginalPath');
      exit;
    end;
    Kept := '';
    while Value <> '' do begin
      Sep := Pos(';', Value);
      if Sep = 0 then Sep := Length(Value) + 1;
      Item := Copy(Value, 1, Sep - 1);
      Delete(Value, 1, Sep);
      if CompareText(Item, Entry) <> 0 then begin
        if Kept <> '' then Kept := Kept + ';';
        Kept := Kept + Item;
      end;
    end;
    RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Kept);
    RegDeleteValue(HKCU, 'Software\Arc Deck', 'AddedPath');
    RegDeleteValue(HKCU, 'Software\Arc Deck', 'WrittenPath');
    RegDeleteValue(HKCU, 'Software\Arc Deck', 'OriginalPath');
  end;
end;
