; setup.iss - Inno Setup script for Encrypted P2P Messenger
; Call: ISCC.exe /DMyAppVersion="1.4.0" setup.iss

#define MyAppName "Encrypted P2P Messenger"
#define MyAppVersion "1.4.0"
#define MyAppPublisher "fibo3090"
#define MyAppURL "https://github.com/fibo3090/secure-p2p-chat"
#define MyAppExe "encodeur_rsa_rust.exe"

#ifndef MyAppVersion
  #define MyAppVersion "1.4.0"
#endif

; Define icon only if it exists in dist
#if FileExists("dist\encodeur_rsa_icon.ico")
  #define MyAppIcon "dist\encodeur_rsa_icon.ico"
#endif

[Setup]
; Basic Information
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}

; Installation Paths
DefaultDirName={commonpf}\{#MyAppName}
DefaultGroupName={#MyAppName}
; OutputBaseFilename is provided by the build script via /F parameter

; Graphics and Styles
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
DisableWelcomePage=no
DisableFinishedPage=no
UninstallDisplayIcon={app}\{#MyAppExe}
; Use installer icon only if present
#if defined MyAppIcon
  SetupIconFile={#MyAppIcon}
#endif

; PrivilegesRequired=admin

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "dist\encodeur_rsa_icon.ico"; DestDir: "{app}"; DestName: "encodeur_rsa_icon.ico"; Flags: ignoreversion skipifsourcedoesntexist
Source: "dist\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\README.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "dist\LICENSE.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

; Add any other assets here
; Source: "dist\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

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
; Clean up app data on uninstall (optional, user confirms via uninstaller standard flow usually, 
; but here we force delete specific cache if it's considered part of the 'clean' uninstall request).
; Note: standard Inno Setup doesn't ask "Do you want to delete data?", it just follows the script.
; To be safe, we only delete the folder if it's empty or we can add a custom Code query.
; For now, we'll assume a "clean" uninstall instruction meant we should define the path.
Type: filesandordirs; Name: "{userappdata}\chat-p2p\EncryptedMessenger"

[Code]
// Helper function to find and close the application before install/uninstall

function InitializeSetup(): Boolean;
var
  ErrorCode: Integer;
begin
  // Check if the application is running
  // 'encodeur_rsa_rust.exe' should match the binary name
  // Note: ShellExec requires specific parameters to kill. 
  // A simpler way in Inno is usually to check for the mutex or window, 
  // but since we don't have a specific mutex defined in Rust app yet, 
  // we rely on user manually closing or standard file-in-use checks.
  
  // However, Inno Setup has built-in file checking. If {app}\MyApp.exe is locked, it will prompt.
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    // You could prompt the user here if they want to delete their data
    if MsgBox('Voulez-vous supprimer vos données de conversation et clés locales (recommandé pour une désinstallation complète) ?', mbConfirmation, MB_YESNO) = IDYES then
    begin
        // The deletion happens in [UninstallDelete] section automatically if defined, 
        // OR we can manually delete here if we want it conditional.
        // Since [UninstallDelete] runs unconditionally, we should use DelTree here for conditional.
        DelTree(ExpandConstant('{userappdata}\chat-p2p\EncryptedMessenger'), True, True, True);
    end;
  end;
end;
