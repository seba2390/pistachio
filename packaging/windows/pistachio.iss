; The Windows installer (Inno Setup 6), built by the release workflow:
;
;   ISCC.exe /DAppVersion=0.6.1 /DSourceDir=<folder with pistachio.exe> /O<out> /F<name> pistachio.iss
;
; It installs for the current user (no administrator rights; Windows' own "Install for all users"
; choice is offered too) into %LOCALAPPDATA%\Programs\Pistachio, where the app can update itself,
; adds Pistachio to the Start menu (and, if chosen, the desktop) and to Settings -> Apps, where it
; uninstalls. Paths are relative to this file.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif

[Setup]
; Never change the AppId: Windows recognizes the installed app (to upgrade or remove it) by it.
AppId={{738898AB-ED76-47B9-B506-5006A0D97529}
AppName=Pistachio
AppVersion={#AppVersion}
AppVerName=Pistachio {#AppVersion}
AppPublisher=Pistachio
AppPublisherURL=https://github.com/seba2390/pistachio
AppSupportURL=https://github.com/seba2390/pistachio/issues
AppUpdatesURL=https://github.com/seba2390/pistachio/releases
AppComments=A fast, native desktop app for the pi coding agent
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Pistachio
VersionInfoDescription=Pistachio Setup
DefaultDirName={autopf}\Pistachio
DefaultGroupName=Pistachio
DisableProgramGroupPage=yes
DisableWelcomePage=no
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SetupIconFile=..\..\crates\app\assets\app-icon\icon.ico
UninstallDisplayIcon={app}\pistachio.exe
UninstallDisplayName=Pistachio
WizardStyle=modern
WizardImageFile=wizard-large-100.bmp,wizard-large-200.bmp
WizardSmallImageFile=wizard-small-100.bmp,wizard-small-200.bmp
; A running Pistachio is closed (and asked to save nothing: it keeps no unsaved state) before
; its files are replaced.
CloseApplications=yes
RestartApplications=no
Compression=lzma2/max
SolidCompression=yes
OutputBaseFilename=Pistachio-v{#AppVersion}-windows-x86_64-setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\pistachio.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-LICENSES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Pistachio"; Filename: "{app}\pistachio.exe"; Comment: "The pi coding agent, in a native app"
Name: "{autodesktop}\Pistachio"; Filename: "{app}\pistachio.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\pistachio.exe"; Description: "{cm:LaunchProgram,Pistachio}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Left by the app's own updates (see crates/app/src/updater.rs).
Type: files; Name: "{app}\pistachio.exe.old"
Type: files; Name: "{app}\pistachio.exe.new"
