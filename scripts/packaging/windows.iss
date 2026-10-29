[Setup]
AppId=Alacrterm
AppName=Alacrterm
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Alacrterm
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2
SolidCompression=yes
SetupIconFile={#AppIcon}
UninstallDisplayIcon={app}\alacrterm.exe
[InstallDelete]
Type: files; Name: "{app}\app-icon.ico"
Type: files; Name: "{app}\conpty.dll"
Type: files; Name: "{app}\OpenConsole.exe"
Type: filesandordirs; Name: "{app}\plugins\windows"
[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{autoprograms}\Alacrterm"; Filename: "{app}\alacrterm.exe"
[Run]
Filename: "{app}\alacrterm.exe"; Description: "Launch Alacrterm"; Flags: nowait postinstall skipifsilent
