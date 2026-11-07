#define AppVersion "0.1.0"
[Setup]
AppId={{BBCA71EA-6283-43CF-8A72-25A20D3D0772}
AppName=ClipVault
AppVersion={#AppVersion}
AppPublisher=WitherBlitz
AppPublisherURL=https://github.com/WitherBlitz/ClipVault
AppSupportURL=https://github.com/WitherBlitz/ClipVault/issues
DefaultDirName={localappdata}\Programs\ClipVault
DefaultGroupName=ClipVault
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=ClipVault-Setup-{#AppVersion}-x64
SetupIconFile=..\assets\clipvault.ico
UninstallDisplayIcon={app}\ClipVault.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
LicenseFile=..\LICENSE

[Files]
Source: "..\target\release\clipvault.exe"; DestDir: "{app}"; DestName: "ClipVault.exe"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Tasks]
Name: "startup"; Description: "Start ClipVault when I sign in"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Icons]
Name: "{group}\ClipVault"; Filename: "{app}\ClipVault.exe"; Parameters: "--show"
Name: "{autodesktop}\ClipVault"; Filename: "{app}\ClipVault.exe"; Parameters: "--show"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "ClipVault"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ClipVault"; ValueData: """{app}\ClipVault.exe"""; Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "{app}\ClipVault.exe"; Parameters: "--show"; Description: "Open ClipVault"; Flags: nowait postinstall skipifsilent
