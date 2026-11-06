; Racoon Finder, per-user Windows installer (Inno Setup 6). Saved as UTF-8 with a BOM: Inno reads
; a script without one as ANSI and would garble the Korean text below.
;
; Build (see installer/windows/README.md):
;   ISCC.exe /DAppVersion=0.4.0 /DArch=x64 /DSource=C:\path\to\folder\with\exe racoon-finder.iss
;   AppVersion  plain x.y.z, the workspace version
;   Arch        x64 or arm64, which Windows the installer accepts
;   Source      folder that holds racoon-finder.exe

; Inno Setup 6.3 added the x64compatible architecture identifier and the official Korean translation
; (Languages\Korean.isl) that this script uses.
#if Ver < EncodeVer(6, 3, 0)
  #error Inno Setup 6.3 or newer is needed
#endif
#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z
#endif
#ifndef Arch
  #error Pass /DArch=x64 or /DArch=arm64
#endif
#ifndef Source
  #error Pass /DSource=<folder holding racoon-finder.exe>
#endif
#if (Arch != "x64") && (Arch != "arm64")
  #error Arch must be x64 or arm64
#endif

[Setup]
; AppId identifies this program to Windows and to later installers (upgrade in place, one entry
; in the Apps list). It was generated once and MUST NEVER CHANGE, or an upgrade installs a second copy.
AppId={{54915450-7901-40BC-8D23-4F3D018ABAEE}
AppName=Racoon Finder
AppVersion={#AppVersion}
AppVerName=Racoon Finder {#AppVersion}
AppPublisher=doguri25
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Racoon Finder
UninstallDisplayName=Racoon Finder
UninstallDisplayIcon={app}\racoon-finder.exe
; Per user: no administrator prompt, nothing outside the user's profile.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\RacoonFinder
DefaultGroupName=Racoon Finder
DisableProgramGroupPage=yes
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
; An upgrade closes the running app first and does not start it again by itself.
CloseApplications=yes
RestartApplications=no
OutputBaseFilename=RacoonFinder-setup-{#AppVersion}-windows-{#Arch}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Languages]
#if FileExists(CompilerPath + "Languages\Korean.isl")
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
#else
  #error Languages\Korean.isl is missing from the Inno Setup install (it ships with 6.3 and newer)
#endif

[Tasks]
Name: "desktopicon"; Description: "바탕 화면에 바로 가기 만들기"; Flags: unchecked

[Files]
; Only the program. No `portable` marker file: its absence is what makes the app keep its data
; in %APPDATA%\RacoonFinder and %LOCALAPPDATA%\RacoonFinder.
Source: "{#Source}\racoon-finder.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The AppUserModelID is the same as identity::AUMID, so notifications show the app's name and icon.
Name: "{autoprograms}\Racoon Finder"; Filename: "{app}\racoon-finder.exe"; AppUserModelID: "io.github.doguri25.RacoonFinder"
Name: "{autodesktop}\Racoon Finder"; Filename: "{app}\racoon-finder.exe"; AppUserModelID: "io.github.doguri25.RacoonFinder"; Tasks: desktopicon

[Run]
Filename: "{app}\racoon-finder.exe"; Description: "Racoon Finder 실행"; Flags: nowait postinstall skipifsilent

; No [UninstallRun]: `racoon-finder.exe --cleanup` is run from [Code] below, because its
; arguments depend on the question asked at the start of the uninstall and its exit code is reported.

[Code]
var
  BackupSettings: Boolean;

// Asked once, before anything is removed. No answer (silent uninstall) means no backup.
function InitializeUninstall(): Boolean;
begin
  Result := True;
  BackupSettings := False;
  if not UninstallSilent then
    BackupSettings := SuppressibleMsgBox(
      '삭제하기 전에 설정을 백업할까요?' + #13#10#13#10 +
      '"예"를 누르면 설정 파일을 문서 폴더의 RacoonFinder-settings-backup.toml로 복사합니다.',
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
end;

// Runs before the files are deleted, while racoon-finder.exe is still there. --cleanup asks a
// running app to quit, puts back what the app changed, and deletes its settings, cache and logs.
// A failure is reported but never stops the uninstall.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Exe, Params, Backup: String;
  Code: Integer;
begin
  if CurUninstallStep <> usUninstall then Exit;
  Exe := ExpandConstant('{app}\racoon-finder.exe');
  if not FileExists(Exe) then Exit;
  Params := '--cleanup';
  if BackupSettings then
  begin
    Backup := ExpandConstant('{userdocs}\RacoonFinder-settings-backup.toml');
    Params := Params + ' --backup-settings "' + Backup + '"';
  end;
  if not Exec(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, Code) then
  begin
    Log('could not start ' + Exe + ': ' + SysErrorMessage(Code));
    if not UninstallSilent then
      SuppressibleMsgBox('앱의 데이터를 정리하는 프로그램을 실행하지 못했습니다. 프로그램은 계속 삭제합니다.' + #13#10 +
        '%APPDATA%\RacoonFinder 와 %LOCALAPPDATA%\RacoonFinder 폴더가 남아 있으면 직접 지워 주세요.',
        mbError, MB_OK, IDOK);
  end
  else if Code <> 0 then
  begin
    Log('--cleanup exit code ' + IntToStr(Code));
    if not UninstallSilent then
    begin
      if Code = 4 then
        SuppressibleMsgBox('설정을 백업하지 못해 앱의 설정과 기록은 지우지 않았습니다 (종료 코드 4).' + #13#10 +
          '프로그램은 계속 삭제합니다. 설정은 그대로 남아 있습니다.',
          mbError, MB_OK, IDOK)
      else
        SuppressibleMsgBox('앱의 데이터를 모두 정리하지 못했습니다 (종료 코드 ' + IntToStr(Code) + ').' + #13#10 +
          '프로그램은 계속 삭제합니다. %APPDATA%\RacoonFinder 와 %LOCALAPPDATA%\RacoonFinder 폴더가 남아 있으면 직접 지워 주세요.',
          mbError, MB_OK, IDOK);
    end;
  end;
end;
