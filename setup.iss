; setup.iss - Inno Setup script for Encrypted P2P Messenger
; Call: ISCC.exe /DMyAppVersion="1.2.0" setup.iss

#define MyAppName "Encrypted P2P Messenger"
#define MyAppExe "encodeur_rsa_rust.exe"

#ifndef MyAppVersion
  #define MyAppVersion "1.2.0"
#endif

[Setup]
AppName={#MyAppName}
AppVersion={#MyAppVersion}
DefaultDirName={commonpf}\{#MyAppName}
DefaultGroupName={#MyAppName}
OutputBaseFilename={#MyAppExe}-setup-{#MyAppVersion}
Compression=lzma
SolidCompression=yes
WizardStyle=modern
DisableWelcomePage=no
DisableFinishedPage=no
UninstallDisplayIcon={app}\{#MyAppExe}
SetupIconFile=dist\encodeur_rsa_icon.ico
; PrivilegesRequired=admin   ; décommente si tu veux forcer l'install système

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "dist\encodeur_rsa_icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

; Si tu as d'autres fichiers à inclure, ajoute-les ici
; Source: "dist\data\*"; DestDir: "{app}\data"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; IconFilename: "{app}\encodeur_rsa_icon.ico"
Name: "{commondesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon; IconFilename: "{app}\encodeur_rsa_icon.ico"

[Tasks]
Name: "desktopicon"; Description: "Créer un raccourci sur le bureau"; GroupDescription: "Tâches optionnelles :"

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "Lancer {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\data"
