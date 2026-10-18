; Inno Setup script for the ScreenForge Windows installer (run by scripts/bundle-windows.sh).
; Per-user install (no admin prompt), Start menu entry, uninstaller, .screenforge association.
; Defines passed on the command line: AppVersion, SourceDir (the portable folder), OutDir.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\windows\ScreenForge"
#endif
#ifndef OutDir
  #define OutDir "..\target"
#endif

[Setup]
AppId={{7E0C1D6A-5C1B-4F0E-9C5B-2D4A8F1E6B03}
AppName=ScreenForge
AppVersion={#AppVersion}
AppVerName=ScreenForge {#AppVersion}
AppPublisher=Smartloop Inc.
AppPublisherURL=https://screenforge.studio
AppSupportURL=https://screenforge.studio
AppUpdatesURL=https://github.com/mehfuzh/screenforge/releases
DefaultDirName={localappdata}\Programs\ScreenForge
DefaultGroupName=ScreenForge
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutDir}
OutputBaseFilename=ScreenForge-{#AppVersion}-windows-x86_64-setup
SetupIconFile=..\apps\studio\assets\icon.ico
WizardImageFile=..\apps\studio\assets\installer\wizard-164.bmp,..\apps\studio\assets\installer\wizard-246.bmp,..\apps\studio\assets\installer\wizard-328.bmp
WizardSmallImageFile=..\apps\studio\assets\installer\small-55.bmp,..\apps\studio\assets\installer\small-83.bmp,..\apps\studio\assets\installer\small-110.bmp
WizardImageStretch=no
UninstallDisplayIcon={app}\screenforge.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes
LicenseFile={#SourceDir}\LICENSE.txt

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\ScreenForge"; Filename: "{app}\screenforge.exe"
Name: "{group}\Uninstall ScreenForge"; Filename: "{uninstallexe}"
Name: "{autodesktop}\ScreenForge"; Filename: "{app}\screenforge.exe"; Tasks: desktopicon

[Registry]
; .screenforge projects are folders, so the association only covers Explorer's "Open with"
Root: HKCU; Subkey: "Software\Classes\.screenforge"; ValueType: string; ValueName: ""; ValueData: "ScreenForge.Project"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\ScreenForge.Project"; ValueType: string; ValueName: ""; ValueData: "ScreenForge Project"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\ScreenForge.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\screenforge.exe,0"
Root: HKCU; Subkey: "Software\Classes\ScreenForge.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\screenforge.exe"" ""%1"""

[Run]
Filename: "{app}\screenforge.exe"; Description: "{cm:LaunchProgram,ScreenForge}"; Flags: nowait postinstall skipifsilent
