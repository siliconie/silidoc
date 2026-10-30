; Inno Setup script for the Alfadoc installer.
; Build with packaging\windows\build.ps1 (it passes /DAppVersion=... and /DExePath=...).

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef ExePath
  #define ExePath "..\..\target\release\alfadoc.exe"
#endif

[Setup]
AppId={{6F1C0E55-2B0B-4C77-9D52-A1FAD0C00001}
AppName=Alfadoc
AppVersion={#AppVersion}
AppPublisher=HALA Software
DefaultDirName={autopf}\Alfadoc
DefaultGroupName=Alfadoc
OutputDir=..\..\dist
OutputBaseFilename=Alfadoc-{#AppVersion}-setup
SetupIconFile=..\icon\alfadoc.ico
UninstallDisplayIcon={app}\alfadoc.exe
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes
WizardStyle=modern

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Alfadoc"; Filename: "{app}\alfadoc.exe"
Name: "{autodesktop}\Alfadoc"; Filename: "{app}\alfadoc.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"

; Double-clicking a .alfadoc file opens it in Alfadoc.
[Registry]
Root: HKA; Subkey: "Software\Classes\.alfadoc"; ValueType: string; ValueName: ""; ValueData: "Alfadoc.Document"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Alfadoc.Document"; ValueType: string; ValueName: ""; ValueData: "Alfadoc document"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Alfadoc.Document\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\alfadoc.exe,0"
Root: HKA; Subkey: "Software\Classes\Alfadoc.Document\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\alfadoc.exe"" ""%1"""

[Run]
Filename: "{app}\alfadoc.exe"; Description: "Start Alfadoc"; Flags: nowait postinstall skipifsilent
