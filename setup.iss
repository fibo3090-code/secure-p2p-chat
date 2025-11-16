; setup.iss - Inno Setup script for Encrypted P2P Messenger
; Call: ISCC.exe /DMyAppVersion="1.3.0" setup.iss

#define MyAppName "Encrypted P2P Messenger"
#define MyAppExe "encodeur_rsa_rust.exe"

#ifndef MyAppVersion
  #define MyAppVersion "1.3.0"
#endif
; Define icon only if it exists in dist
#if FileExists("dist\encodeur_rsa_icon.ico")
  #define MyAppIcon "dist\\encodeur_rsa_icon.ico"
#endif

[Setup]
AppName={#MyAppName}
AppVersion={#MyAppVersion}
DefaultDirName={commonpf}\{#MyAppName}
DefaultGroupName={#MyAppName}
; OutputBaseFilename is now provided by the build script via /F parameter
Compression=lzma
SolidCompression=yes
WizardStyle=modern
DisableWelcomePage=no
DisableFinishedPage=no
UninstallDisplayIcon={app}\{#MyAppExe}
; Use installer icon only if present
#if defined MyAppIcon
  SetupIconFile={#MyAppIcon}
#endif
; PrivilegesRequired=admin   ; décommente si tu veux forcer l'install système

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "dist\encodeur_rsa_icon.ico"; DestDir: "{app}"; DestName: "encodeur_rsa_icon.ico"; Flags: ignoreversion skipifsourcedoesntexist
Source: "dist\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\README.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "dist\LICENSE.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

; Si tu as d'autres fichiers à inclure, ajoute-les ici
; Source: "dist\data\*"; DestDir: "{app}\data"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
#if defined MyAppIcon
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; IconFilename: "{app}\encodeur_rsa_icon.ico"
Name: "{commondesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon; IconFilename: "{app}\encodeur_rsa_icon.ico"
#else
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"
Name: "{commondesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon
#endif

[Tasks]
Name: "desktopicon"; Description: "Créer un raccourci sur le bureau"; GroupDescription: "Tâches optionnelles :"

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "Lancer {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\data"
