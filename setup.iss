; setup.iss - Inno Setup script for Encrypted P2P Messenger
; Call: ISCC.exe /DMyAppVersion="1.8.0" setup.iss

#define MyAppName "Encrypted P2P Messenger"
#define MyAppPublisher "fibo3090"
#define MyAppURL "https://github.com/fibo3090/secure-p2p-chat"
#define MyAppExe "encodeur_rsa_rust.exe"

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
UninstallDisplayName={#MyAppName} {#MyAppVersion}

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
; Use .ico for Add/Remove Programs / Settings > Apps so the app logo displays correctly
UninstallDisplayIcon={app}\encodeur_rsa_icon.ico
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
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Optional tasks:"

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent

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
    // Prompt the user if they want to delete their data
    if MsgBox('Do you want to delete your conversation data and local keys (recommended for a complete uninstall)?', mbConfirmation, MB_YESNO) = IDYES then
    begin
        // Manually delete the application data directory.
        // This path MUST match the one used by `directories::ProjectDirs` in the application.
        // ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger") creates:
        // %LOCALAPPDATA%\chat-p2p\EncryptedMessenger on Windows
        DelTree(ExpandConstant('{localappdata}\chat-p2p\EncryptedMessenger'), True, True, True);
    end;
  end;
end;
