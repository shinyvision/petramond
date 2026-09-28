; Inno Setup script for the Windows installer. The release workflow compiles it
; with /DAppVersion, /DSourceDir (the staged bundle), /DOutputBase and /DArch
; (x64compatible or arm64).
;
; It installs per user (no administrator prompt), so the game lands in
; %LOCALAPPDATA%\Programs\Petramond. Worlds and settings live in the user data
; folder, never here, so reinstalling or uninstalling leaves them alone.

#define AppExe "petramond.exe"

[Setup]
AppId={{B4CA453E-A043-4D66-87E4-0B7BA7A24440}
AppName=Petramond
AppVersion={#AppVersion}
AppPublisher=Petramond
AppPublisherURL=https://github.com/shinyvision/petramond
DefaultDirName={autopf}\Petramond
DefaultGroupName=Petramond
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed={#Arch}
ArchitecturesInstallIn64BitMode={#Arch}
OutputDir=.
OutputBaseFilename={#OutputBase}
SetupIconFile=..\icons\petramond.ico
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[InstallDelete]
; An update replaces the shipped content outright, so files a newer version
; dropped don't linger. Mods the player installs live in the user data folder.
Type: filesandordirs; Name: "{app}\assets"
Type: filesandordirs; Name: "{app}\mods"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Petramond"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\Petramond"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,Petramond}"; Flags: nowait postinstall skipifsilent
