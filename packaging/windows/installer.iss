; Winter Terminal — Inno Setup installer script
; Build with: WINTER_VERSION=0.0.1 iscc packaging\windows\installer.iss
; Requires Inno Setup 6: https://jrsoftware.org/isinfo.php
;
; Version comes from the WINTER_VERSION env var rather than a /D command-line
; define: a Makefile recipe may run under sh.exe (MSYS/Git for Windows), which
; mangles "/D..." into a path before ISCC ever sees it.

#define MyAppName "Winter Terminal"
#define MyAppExeName "winter.exe"
#define MyAppPublisher "Quang Trung Ta"
#define MyAppURL "https://github.com/taquangtrung/winter-term"

#define MyAppVersion GetEnv("WINTER_VERSION")
#if MyAppVersion == ""
  #define MyAppVersion "0.0.1"
#endif

[Setup]
LicenseFile=..\..\LICENSE
AppId={{6F1E1C2A-6E6B-4C1D-9C3B-0D6E6F4A9B21}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
DefaultDirName={localappdata}\Programs\Winter Terminal
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\..\target
OutputBaseFilename=winter-terminal-{#MyAppVersion}-setup
Compression=lzma
SolidCompression=yes
; Per-user install: no admin/UAC prompt, since /VERYSILENT only silences
; Inno Setup's own UI and has no effect on the Windows elevation dialog.
PrivilegesRequired=lowest
ArchitecturesInstallIn64BitMode=x64compatible
; A running winter.exe holds a lock on the file this installer overwrites.
; CloseApplications uses Windows Restart Manager to close it automatically
; (silently, when combined with /FORCECLOSEAPPLICATIONS) instead of relying
; on whatever launched Setup to have closed it first. RestartApplications is
; off since a freshly-installed Winter should be launched manually, not
; auto-relaunched mid-install.
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\{#MyAppExeName}
SetupIconFile=..\..\assets\icons\winter-terminal.ico

[Files]
Source: "..\..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\samples\settings.kdl"; DestDir: "{app}\examples"; Flags: ignoreversion
Source: "..\..\samples\keybindings.kdl"; DestDir: "{app}\examples"; Flags: ignoreversion
; Shell integration for shells reachable on Windows (Git Bash, MSYS2, WSL
; mounts). Winter's own ConPTY sessions use whatever shell the user configures.
Source: "..\..\clients\shell-integration\winter.bash"; DestDir: "{app}\shell-integration"; Flags: ignoreversion
Source: "..\..\clients\shell-integration\winter.zsh"; DestDir: "{app}\shell-integration"; Flags: ignoreversion
Source: "..\..\clients\shell-integration\winter.fish"; DestDir: "{app}\shell-integration"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
; unins000.exe carries Inno Setup's own generic icon by default; point this
; shortcut at winter.exe's embedded icon instead so both entries match.
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"; IconFilename: "{app}\{#MyAppExeName}"

; Registered under App Paths so ShellExecute-style bare-name launches (e.g.
; RightKeys' `exec windows="winter.exe"`) resolve reliably: App Paths is read
; fresh from the registry on every call, unlike a PATH addition, which only
; takes effect for processes started after the change — a long-lived daemon
; already running at install time keeps its stale PATH indefinitely.
; HKCU (not HKLM): the install itself is per-user (see PrivilegesRequired
; above), so the App Paths entry and PATH addition below must be too.
[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#MyAppExeName}"; \
    ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName}"; Flags: uninsdeletekey

[Code]
const
  EnvKey = 'Environment';

procedure EnvAddPath(Path: string);
var
  Paths: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths) then
    Paths := '';

  if Pos(';' + Uppercase(Path) + ';', ';' + Uppercase(Paths) + ';') > 0 then
    exit;

  Paths := Paths + ';' + Path + ';';
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths);
end;

procedure EnvRemovePath(Path: string);
var
  Paths: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths) then
    exit;

  P := Pos(';' + Uppercase(Path) + ';', ';' + Uppercase(Paths) + ';');
  if P = 0 then
    exit;

  Delete(Paths, P - 1, Length(Path) + 1);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    EnvAddPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    EnvRemovePath(ExpandConstant('{app}'));
end;
