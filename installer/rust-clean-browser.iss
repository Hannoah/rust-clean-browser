; Inno Setup 6 script for the Windows installer.
;   ISCC.exe /DAppVersion=0.1.0 installer\rust-clean-browser.iss   ->  dist\RustCleanBrowser-Setup-<ver>-x64.exe
; Expects a release build in target\release (rust-browser.exe + any *.dll next to it).

#define AppName "Rust Clean Browser"
#define AppExe "rust-browser.exe"
#define AppUrl "https://github.com/Hannoah/rust-clean-browser"
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

[Setup]
AppId={{8B4F3C2A-5D7E-4A91-B6C3-2E9F1A7D4C58}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Hannoah
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=RustCleanBrowser-Setup-{#AppVersion}-x64
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; Per-user install by default (no admin prompt); the user may choose "all users" instead.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
CloseApplications=yes
ChangesAssociations=yes

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
korean.RegisterBrowser=Windows 기본 브라우저 후보로 등록 (설정 > 기본 앱에서 선택 가능)
english.RegisterBrowser=Register as a browser option (Settings > Default apps)
korean.BrowserTasks=브라우저 통합:
english.BrowserTasks=Browser integration:

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "registerbrowser"; Description: "{cm:RegisterBrowser}"; GroupDescription: "{cm:BrowserTasks}"

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Browser capabilities (lets Windows offer it under Settings > Default apps; never forces it).
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser"; ValueType: string; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"""; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Ultra-lightweight pure-Rust web browser"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\{#AppExe},0"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities\URLAssociations"; ValueType: string; ValueName: "http"; ValueData: "RustCleanBrowserURL"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities\URLAssociations"; ValueType: string; ValueName: "https"; ValueData: "RustCleanBrowserURL"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities\FileAssociations"; ValueType: string; ValueName: ".htm"; ValueData: "RustCleanBrowserHTML"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities\FileAssociations"; ValueType: string; ValueName: ".html"; ValueData: "RustCleanBrowserHTML"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#AppName}"; ValueData: "Software\Clients\StartMenuInternet\RustCleanBrowser\Capabilities"; Flags: uninsdeletevalue; Tasks: registerbrowser
; ProgIDs used by the associations above.
Root: HKA; Subkey: "Software\Classes\RustCleanBrowserURL"; ValueType: string; ValueData: "{#AppName} URL"; Flags: uninsdeletekey; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Classes\RustCleanBrowserURL"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Classes\RustCleanBrowserURL\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Classes\RustCleanBrowserURL\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Classes\RustCleanBrowserHTML"; ValueType: string; ValueData: "{#AppName} HTML Document"; Flags: uninsdeletekey; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Classes\RustCleanBrowserHTML\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: registerbrowser
Root: HKA; Subkey: "Software\Classes\RustCleanBrowserHTML\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: registerbrowser

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

; User data (%LOCALAPPDATA%\rust-browser: bookmarks, logins, cache) is kept on uninstall,
; like other browsers, so reinstalling restores everything.
