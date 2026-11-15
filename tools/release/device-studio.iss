; Inno Setup script for the Device Windows installer.
; Built by tools/release/package-windows.ps1, which passes AppVersion, SourceDir (the staged
; app folder) and OutputDir.

#ifndef AppVersion
  #error AppVersion is not defined
#endif

[Setup]
; A fixed id: later versions upgrade this install instead of adding a second one.
AppId={{6C1C7F5E-3B47-4E0B-9C1F-5D2B8F0A4E61}
AppName=Device
AppVersion={#AppVersion}
AppVerName=Device {#AppVersion}
DefaultDirName={autopf}\Device
DefaultGroupName=Device
UsePreviousGroup=no
UninstallDisplayIcon={app}\device-studio.exe
; The installer's own icon; the app's icon is built into device-studio.exe (crates/dstudio-app/build.rs).
SetupIconFile=..\..\crates\dstudio-app\icons\device.ico
OutputDir={#OutputDir}
OutputBaseFilename=Device-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Per-user install by default (no admin prompt); the user can choose all users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline dialog
#ifexist SourceDir + "\LICENSE"
LicenseFile={#SourceDir}\LICENSE
#endif

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\Device"; Filename: "{app}\device-studio.exe"
Name: "{group}\Uninstall Device"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Device"; Filename: "{app}\device-studio.exe"; Tasks: desktopicon

; Remove the previous default shortcuts when upgrading the same AppId. Existing
; installation directories are reused by Inno Setup; user data is not moved.
[InstallDelete]
Type: files; Name: "{autoprograms}\Device Studio\Device Studio.lnk"
Type: files; Name: "{autoprograms}\Device Studio\Uninstall Device Studio.lnk"
Type: dirifempty; Name: "{autoprograms}\Device Studio"
Type: files; Name: "{autodesktop}\Device Studio.lnk"

[Run]
Filename: "{app}\device-studio.exe"; Description: "{cm:LaunchProgram,Device}"; Flags: nowait postinstall skipifsilent
