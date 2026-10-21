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
AppName=Saiku
AppVersion={#Version}
AppPublisher=Project Arcturius
AppPublisherURL=https://github.com/ProjectArcturius/saiku
DefaultDirName={localappdata}\Programs\Saiku
DefaultGroupName=Saiku
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=saiku-{#Version}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
LicenseFile={#SourceDir}\LICENSE
UninstallDisplayIcon={app}\saiku.exe
DisableProgramGroupPage=yes

[Tasks]
Name: "addtopath"; Description: "Add Saiku to your user PATH (recommended)"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Excludes: "install.ps1,install.cmd,uninstall.ps1,uninstall.cmd"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Getting started"; Filename: "{app}\INSTALL.md"
Name: "{group}\Uninstall Saiku"; Filename: "{uninstallexe}"

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
      RegWriteStringValue(HKCU, 'Software\Saiku', 'OriginalPath', Value);
      if Value <> '' then Value := Entry + ';' + Value else Value := Entry;
      RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Value);
      RegWriteStringValue(HKCU, 'Software\Saiku', 'WrittenPath', Value);
      RegWriteStringValue(HKCU, 'Software\Saiku', 'AddedPath', Entry);
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var Value, Entry, Item, Kept, Original, Written: String; Sep: Integer;
begin
  if (CurUninstallStep = usPostUninstall) and
      RegQueryStringValue(HKCU, 'Software\Saiku', 'AddedPath', Entry) then begin
    RegQueryStringValue(HKCU, 'Environment', 'Path', Value);
    if RegQueryStringValue(HKCU, 'Software\Saiku', 'WrittenPath', Written) and
        (Value = Written) and RegQueryStringValue(HKCU, 'Software\Saiku', 'OriginalPath', Original) then begin
      RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Original);
      RegDeleteValue(HKCU, 'Software\Saiku', 'AddedPath');
      RegDeleteValue(HKCU, 'Software\Saiku', 'WrittenPath');
      RegDeleteValue(HKCU, 'Software\Saiku', 'OriginalPath');
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
    RegDeleteValue(HKCU, 'Software\Saiku', 'AddedPath');
    RegDeleteValue(HKCU, 'Software\Saiku', 'WrittenPath');
    RegDeleteValue(HKCU, 'Software\Saiku', 'OriginalPath');
  end;
end;
