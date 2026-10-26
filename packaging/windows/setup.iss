#define AppVersion GetEnv("SPACEMOSAIC_VERSION")
#define BundleDir GetEnv("SPACEMOSAIC_BUNDLE_DIR")
#define OutputPath GetEnv("SPACEMOSAIC_OUTPUT_DIR")
#define IconPath GetEnv("SPACEMOSAIC_ICON")

[Setup]
AppId={{B01C9932-CC90-4C94-B525-C1B0F43DF495}
AppName=SpaceMosaic
AppVersion={#AppVersion}
AppPublisher=SpaceMosaic contributors
AppPublisherURL=https://github.com/0then0/spacemosaic
AppSupportURL=https://github.com/0then0/spacemosaic/issues
DefaultDirName={localappdata}\Programs\SpaceMosaic
DefaultGroupName=SpaceMosaic
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputPath}
OutputBaseFilename=spacemosaic-windows-amd64-setup
SetupIconFile={#IconPath}
UninstallDisplayIcon={app}\SpaceMosaic.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
DisableProgramGroupPage=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BundleDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\SpaceMosaic"; Filename: "{app}\spacemosaic.exe"; IconFilename: "{app}\SpaceMosaic.ico"
Name: "{autodesktop}\SpaceMosaic"; Filename: "{app}\spacemosaic.exe"; IconFilename: "{app}\SpaceMosaic.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\spacemosaic.exe"; Description: "{cm:LaunchProgram,SpaceMosaic}"; Flags: nowait postinstall skipifsilent
