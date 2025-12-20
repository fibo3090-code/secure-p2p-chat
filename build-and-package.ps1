<#
  build-and-package.ps1
  Build and package script for Encrypted P2P Messenger
  Usage example:
    .\build-and-package.ps1 -Version "1.2.0" -PfxPath "C:\keys\mycert.pfx" -PfxPassword "YourPassword"
#>

param(
    [string]$Version = "",
    [string]$Configuration = "release",
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$InnoPath = "", # Will attempt auto-detection if empty
    [string]$IconSource = "encodeur_rsa_icon.ico",        # Relative to repo root or absolute path
    [string]$PfxPath = "",                               # Optional: path to .pfx for signing
    [System.Security.SecureString]$PfxPassword = $null,   # Optional: password for .pfx (secure)
    [string]$SignToolPath = ""                            # Will attempt auto-detection if empty
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Helper to print colored status messages
function Write-Status {
    param([string]$Message, [ConsoleColor]$Color = "Cyan")
    Write-Host "`n[$([DateTime]::Now.ToString('HH:mm:ss'))] $Message" -ForegroundColor $Color
}

function Write-ErrorMsg {
    param([string]$Message)
    Write-Host "`n[ERROR] $Message" -ForegroundColor Red
}

try {
    # Determine repository root based on script location
    $RepoRoot = (Get-Item $PSScriptRoot).FullName
    Set-Location $RepoRoot

    Write-Status "Starting build and package process..." "Green"

    # --- Pre-flight Checks ---

    if (-not (Get-Command "cargo" -ErrorAction SilentlyContinue)) {
        throw "Rust 'cargo' command not found. Please install Rust from https://rustup.rs/"
    }

    # Resolve version from Cargo.toml if not provided or set to 'auto'
    if ([string]::IsNullOrWhiteSpace($Version) -or $Version -eq 'auto') {
        $cargoToml = Join-Path $RepoRoot 'Cargo.toml'
        if (Test-Path $cargoToml) {
            $verLine = (Get-Content $cargoToml | Where-Object { $_ -match '^version\s*=\s*"([^"]+)"' } | Select-Object -First 1)
            if ($verLine -and ($verLine -match '"([^"]+)"')) { $Version = $Matches[1] }
        }
        if ([string]::IsNullOrWhiteSpace($Version)) { $Version = '0.0.0' }
    }

    Write-Host "Version:       $Version"
    Write-Host "Configuration: $Configuration"
    Write-Host "Target:        $Target"

    # Nom du binaire produit par cargo (adapté à ton projet)
    $BinaryName = "encodeur_rsa_rust.exe"
    $BinaryBase = [System.IO.Path]::GetFileNameWithoutExtension($BinaryName)

    # 1) Build release
    Write-Status "Building project (cargo build --release)..."
    $buildArgs = @("build", "--release", "--target", $Target)
    
    # Run cargo and check exit code
    $process = Start-Process -FilePath "cargo" -ArgumentList $buildArgs -Wait -NoNewWindow -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Cargo build failed with exit code $($process.ExitCode)."
    }

    # 2) Prepare dist folder
    Write-Status "Preparing 'dist' directory..."
    $Dist = Join-Path -Path $RepoRoot -ChildPath "dist"
    if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
    New-Item -ItemType Directory -Path $Dist | Out-Null

    $BuiltBinary = Join-Path -Path $RepoRoot -ChildPath "target/$Target/release/$BinaryName"
    if (-not (Test-Path $BuiltBinary)) {
        throw "Built binary not found at $BuiltBinary. Check target and binary name."
    }

    Write-Host "Copying binary to dist..."
    Copy-Item $BuiltBinary -Destination (Join-Path $Dist $BinaryName)

    # Ensure documentation and LICENSE.md end up in dist
    Write-Host "Copying documentation..."
    if (Test-Path (Join-Path $RepoRoot "README.md")) {
        Copy-Item (Join-Path $RepoRoot "README.md") -Destination (Join-Path $Dist "README.md") -Force
    }
    elseif (Test-Path (Join-Path $RepoRoot "docs\Community\README.md")) {
        Copy-Item (Join-Path $RepoRoot "docs\Community\README.md") -Destination (Join-Path $Dist "README.md") -Force
    }
    else {
        # Create a minimal README
        $placeholder = @(
            "Encrypted P2P Messenger",
            "",
            "This distribution was packaged from the project repository. See the docs/ directory in the source for details."
        ) -join "`n"
        Set-Content -Path (Join-Path $Dist "README.md") -Value $placeholder -Encoding UTF8
    }

    if (Test-Path (Join-Path $RepoRoot "LICENSE.md")) {
        Copy-Item (Join-Path $RepoRoot "LICENSE.md") -Destination (Join-Path $Dist "LICENSE.md") -Force
    }
    elseif (Test-Path (Join-Path $RepoRoot "docs\Community\LICENSE.md")) {
        Copy-Item (Join-Path $RepoRoot "docs\Community\LICENSE.md") -Destination (Join-Path $Dist "LICENSE.md") -Force
    }
    else {
        Write-Warning "No LICENSE.md file found. Installing without license file."
    }

    # 3) Copy icon into dist
    Write-Status "Locating application icon..."
    $IconPathCandidates = @(
        (Join-Path $RepoRoot $IconSource),
        (Join-Path (Join-Path $RepoRoot "dist") $IconSource),
        $IconSource
    )
    $FoundIcon = $null
    foreach ($p in $IconPathCandidates) {
        if (-not ([string]::IsNullOrWhiteSpace($p)) -and (Test-Path $p)) {
            $FoundIcon = (Resolve-Path $p).Path
            break
        }
    }
    if ($FoundIcon) {
        Write-Host "Copying icon from $FoundIcon to $Dist"
        Copy-Item $FoundIcon -Destination (Join-Path $Dist (Split-Path $FoundIcon -Leaf)) -Force
    }
    else {
        Write-Warning "No icon found. The installer might fail if it expects an icon."
    }

    # 4) Create zip artifact
    Write-Status "Creating portable ZIP archive..."
    $ReleaseDir = Join-Path $RepoRoot "release"
    if (-not (Test-Path $ReleaseDir)) { New-Item -ItemType Directory -Path $ReleaseDir | Out-Null }
    $zipOut = Join-Path -Path $ReleaseDir -ChildPath ("$BinaryBase-$Version-windows-x64.zip")
    if (Test-Path $zipOut) { Remove-Item $zipOut -Force }
    
    Compress-Archive -Path (Join-Path $Dist '*') -DestinationPath $zipOut
    Write-Host "ZIP created: $zipOut"

    # 5) Call Inno Setup compiler (ISCC.exe)
    Write-Status "Compiling Installer (Inno Setup)..."
    
    # Auto-detect ISCC if not provided
    if ([string]::IsNullOrWhiteSpace($InnoPath) -or -not (Test-Path $InnoPath)) {
        $isccCmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
        if ($isccCmd) { 
            $InnoPath = $isccCmd.Source 
        }
        else {
            # Check common paths
            $commonPaths = @(
                'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
                'C:\Program Files\Inno Setup 6\ISCC.exe'
            )
            foreach ($path in $commonPaths) {
                if (Test-Path $path) { 
                    $InnoPath = $path; 
                    break 
                }
            }
        }
    }

    if (-not (Test-Path $InnoPath)) {
        throw "ISCC.exe (Inno Setup Compiler) not found. Please install Inno Setup 6 or provide -InnoPath."
    }

    $issPath = Join-Path $RepoRoot "setup.iss"
    if (-not (Test-Path $issPath)) {
        throw "Setup script 'setup.iss' not found in repository root."
    }

    $ExpectedSetupBaseName = "$BinaryBase-setup-$Version"
    $OutputDir = Join-Path $RepoRoot "Output"
    if (-not (Test-Path $OutputDir)) { New-Item -ItemType Directory -Path $OutputDir | Out-Null }
    
    # Construct arguments as a single string to ensure quotes are preserved correctly
    # ISCC needs: /O"Path With Spaces" and "Script Path With Spaces"
    $isccArgs = "/DMyAppVersion=`"$Version`" /O`"$OutputDir`" /F`"$ExpectedSetupBaseName`" `"$issPath`""
    
    Write-Host "Running: $InnoPath $isccArgs"
    
    $isccProcess = Start-Process -FilePath $InnoPath -ArgumentList $isccArgs -Wait -NoNewWindow -PassThru
    if ($isccProcess.ExitCode -ne 0) {
        throw "ISCC compilation failed with exit code $($isccProcess.ExitCode)."
    }

    $SetupPath = Join-Path $OutputDir "$ExpectedSetupBaseName.exe"

    # 6) Signing (optional)
    if ($PfxPath -and (Test-Path $PfxPath)) {
        Write-Status "Signing the installer..."
        
        # Auto-detect signtool
        $signtool = $null
        if (-not ([string]::IsNullOrWhiteSpace($SignToolPath)) -and (Test-Path $SignToolPath)) {
            $signtool = $SignToolPath
        }
        elseif (Get-Command "signtool.exe" -ErrorAction SilentlyContinue) {
            $signtool = (Get-Command "signtool.exe").Source
        }
        else {
            # Try some common Windows Kit paths (brute force search could be added but might be slow)
            $kitPaths = @(
                "C:\Program Files (x86)\Windows Kits\10\bin\x64\signtool.exe",
                "C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64\signtool.exe",
                "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe"
            )
            foreach ($path in $kitPaths) {
                if (Test-Path $path) { $signtool = $path; break }
            }
        }

        if (-not $signtool) {
            Write-Warning "signtool.exe not found. Install Windows SDK or check path. Skipping signing."
        }
        else {
            $signArgs = @('sign', '/f', $PfxPath)
            if ($PfxPassword -ne $null) {
                $plainPwd = ConvertFrom-SecureString -SecureString $PfxPassword -AsPlainText
                $signArgs += '/p'; $signArgs += $plainPwd
            }
            $signArgs += ('/tr', 'http://timestamp.digicert.com', '/td', 'sha256', '/fd', 'sha256', $SetupPath)

            Write-Host "Executing Signtool..."
            $signProc = Start-Process -FilePath $signtool -ArgumentList $signArgs -Wait -NoNewWindow -PassThru
            
            # Clean up password string from memory (best effort)
            if ($plainPwd) { $plainPwd = $null }

            if ($signProc.ExitCode -eq 0) {
                Write-Host "Successfully signed: $SetupPath" -ForegroundColor Green
            }
            else {
                Write-Warning "Signing failed with exit code $($signProc.ExitCode)"
            }
        }
    }
    else {
        Write-Host "Skipping signing (no PFX provided)."
    }

    Write-Status "Build and Package Complete!" "Green"
    Write-Host "Installer: $SetupPath"
    Write-Host "Zip Archive: $zipOut"

}
catch {
    Write-ErrorMsg $_.Exception.Message
    exit 1
}
