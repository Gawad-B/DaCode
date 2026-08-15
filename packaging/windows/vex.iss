; The Windows installer: vex-setup.exe.
;
; What a Windows user expects to download is something they double-click that installs the editor —
; not the editor's own executable, which looks like a broken download when it opens a text editor
; instead. `install.ps1` does the same work for anyone building from source; this is the artifact.
;
; Per-user by design (`PrivilegesRequired=lowest`): it installs under %LOCALAPPDATA%\Programs, adds
; only the *user's* PATH, and therefore never shows a UAC prompt. The same reasoning as the Linux
; installer defaulting to ~/.local — an editor is not something that needs to touch the machine.
;
; Build it with packaging\windows\build-installer.ps1, or:
;     iscc packaging\windows\vex.iss

#define AppName "vex"
#define AppPublisher "vex"
#define AppUrl "https://github.com/Gawad-B/Vex"
#define AppExe "vex.exe"

; Passed in as /DAppVersion=… by build-installer.ps1, which reads it out of crates/vex/Cargo.toml.
; Not read from the binary: it carries no version resource, and not hardcoded here alone, because two
; places that must agree on a version number are two places that will not.
#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif

[Setup]
; Never change this: it is how Windows recognises an existing install to upgrade rather than
; duplicate, and a new one would leave every previous version behind in Add/Remove Programs.
AppId={{7B2D9F3E-4C6A-4F52-9E2C-1A5B8D0C7A11}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}
DefaultDirName={autopf}\vex
DefaultGroupName=vex
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE
PrivilegesRequired=lowest
OutputDir=..\..\target\packages
OutputBaseFilename=vex-setup
SetupIconFile=vex.ico
UninstallDisplayIcon={app}\vex.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; 64-bit only, which is what the build produces.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "addtopath"; Description: "Add vex to my PATH (so `vex .` works in any terminal)"

[Files]
Source: "..\..\target\release\vex.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "vex.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\CHEATSHEET.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\examples\init.lua"; DestDir: "{app}\examples"; Flags: ignoreversion
; The README links into docs\ with relative paths, so the two ship together or the front page's
; every link is dead — and offline is when that page matters most.
Source: "..\..\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\vex"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\vex.ico"; Comment: "A modal code editor that never stalls on your keystroke"
Name: "{group}\Keyboard cheatsheet"; Filename: "{app}\CHEATSHEET.md"
Name: "{autodesktop}\vex"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\vex.ico"; Tasks: desktopicon

[Registry]
; The user's PATH, not the machine's, and only when the task is ticked. `NeedsAddPath` keeps a
; repeated install from appending the same directory a second time — that is how PATH variables end
; up a kilometre long.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

; "Open with vex" on a folder's context menu, which is how a folder gets opened on Windows.
Root: HKCU; Subkey: "Software\Classes\Directory\shell\vex"; ValueType: string; ValueName: ""; \
    ValueData: "Open with vex"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\vex"; ValueType: string; ValueName: "Icon"; \
    ValueData: "{app}\{#AppExe}"
Root: HKCU; Subkey: "Software\Classes\Directory\shell\vex\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#AppExe}"" ""%V"""
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\vex"; ValueType: string; ValueName: ""; \
    ValueData: "Open with vex"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\vex"; ValueType: string; ValueName: "Icon"; \
    ValueData: "{app}\{#AppExe}"
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\vex\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#AppExe}"" ""%V"""

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start vex"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Only what the installer put there. Settings under %APPDATA%\vex and sessions under
; %LOCALAPPDATA%\vex are the user's and survive an uninstall — deleting someone's configuration
; because they removed the program is not a decision an uninstaller gets to make.
Type: filesandordirs; Name: "{app}\examples"

[Code]
// Whether the install directory is missing from the user's PATH. Compared with semicolons around
// both sides so C:\...\vexes does not count as a match for C:\...\vex.
//
// Comments here are // and not the Pascal { }: a brace comment ends at the first closing brace, and
// the constants this script is full of are spelled {app} — so a comment that mentioned one silently
// ended halfway through itself and the rest of the sentence was compiled as code.
function NeedsAddPath(Directory: string): Boolean;
var
  Existing: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Existing) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Directory) + ';', ';' + Uppercase(Existing) + ';') = 0;
end;

// Take the directory back out of PATH on uninstall. Leaving it behind is the thing every installer
// that touches PATH gets wrong, and the leftovers accumulate silently.
procedure RemoveFromPath(Directory: string);
var
  Existing: string;
  At: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Existing) then
    exit;
  At := Pos(';' + Uppercase(Directory), ';' + Uppercase(Existing));
  if At = 0 then
    exit;
  Delete(Existing, At, Length(Directory) + 1);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Existing);
end;

procedure CurUninstallStepChanged(CurStep: TUninstallStep);
begin
  if CurStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
