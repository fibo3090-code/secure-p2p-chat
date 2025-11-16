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
  [string]$InnoPath = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
  [string]$IconSource = "encodeur_rsa_icon.ico",        # relatif au repo root ou chemin absolu
  [string]$PfxPath = "",                               # optionnel: chemin vers .pfx pour signer
  [System.Security.SecureString]$PfxPassword = $null,   # optionnel: mot de passe du .pfx (secure)
  [string]$SignToolPath = "C:\Program Files (x86)\Windows Kits\10\bin\x64\signtool.exe"
)

$ErrorActionPreference = 'Stop'

# Determine repository root based on script location
$RepoRoot = (Get-Item $PSScriptRoot).FullName
Set-Location $RepoRoot

# Resolve version from Cargo.toml if not provided or set to 'auto'
if ([string]::IsNullOrWhiteSpace($Version) -or $Version -eq 'auto') {
    $cargoToml = Join-Path $RepoRoot 'Cargo.toml'
    if (Test-Path $cargoToml) {
        $verLine = (Get-Content $cargoToml | Where-Object { $_ -match '^version\s*=\s*"([^"]+)"' } | Select-Object -First 1)
        if ($verLine -and ($verLine -match '"([^"]+)"')) { $Version = $Matches[1] }
    }
    if ([string]::IsNullOrWhiteSpace($Version)) { $Version = '0.0.0' }
}

Write-Host "Version: $Version"
Write-Host "Configuration: $Configuration"
Write-Host "Target: $Target"

# Nom du binaire produit par cargo (adapté à ton projet)
$BinaryName = "encodeur_rsa_rust.exe"
$BinaryBase = [System.IO.Path]::GetFileNameWithoutExtension($BinaryName)

# 1) Build release
Write-Host "`n=== cargo build --release ==="
cargo build --release --target $Target

# 2) Prepare dist folder
$Dist = Join-Path -Path $RepoRoot -ChildPath "dist"
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Path $Dist | Out-Null

$BuiltBinary = Join-Path -Path $RepoRoot -ChildPath "target/$Target/release/$BinaryName"
if (-not (Test-Path $BuiltBinary)) {
    Write-Error "Built binary not found at $BuiltBinary. Check target and binary name."
    exit 1
}

Copy-Item $BuiltBinary -Destination (Join-Path $Dist $BinaryName)

# Ensure documentation and LICENSE.md end up in dist. Prefer repo root README/LICENSE.md.
if (Test-Path (Join-Path $RepoRoot "README.md")) {
    Copy-Item (Join-Path $RepoRoot "README.md") -Destination (Join-Path $Dist "README.md") -Force
} elseif (Test-Path (Join-Path $RepoRoot "docs\Community\README.md")) {
    Copy-Item (Join-Path $RepoRoot "docs\Community\README.md") -Destination (Join-Path $Dist "README.md") -Force
} else {
    # Create a minimal README so installer build does not fail
    $placeholder = @(
        "Encrypted P2P Messenger",
        "",
        "This distribution was packaged from the project repository. See the docs/ directory in the source for details."
    ) -join "`n"
    $placeholderPath = Join-Path $Dist "README.md"
    Set-Content -Path $placeholderPath -Value $placeholder -Encoding UTF8
}

if (Test-Path (Join-Path $RepoRoot "LICENSE.md")) {
    Copy-Item (Join-Path $RepoRoot "LICENSE.md") -Destination (Join-Path $Dist "LICENSE.md") -Force
} elseif (Test-Path (Join-Path $RepoRoot "docs\Community\LICENSE.md")) {
    Copy-Item (Join-Path $RepoRoot "docs\Community\LICENSE.md") -Destination (Join-Path $Dist "LICENSE.md") -Force
} else {
    # No LICENSE.md file available in repo root or docs; do nothing (installer will proceed)
    Write-Host "NOTICE: No LICENSE.md file found in repo root or docs/Community. Installer will be built without a LICENSE.md file."
}

# 3) Copy icon into dist if present near repo root or provided path
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
} else {
    Write-Host "No icon found at candidates: $($IconPathCandidates -join ' ; ') - continuing without icon."
}

# 4) Create zip artifact
$ReleaseDir = Join-Path $RepoRoot "release"
if (-not (Test-Path $ReleaseDir)) { New-Item -ItemType Directory -Path $ReleaseDir | Out-Null }
$zipOut = Join-Path -Path $ReleaseDir -ChildPath ("$BinaryBase-$Version-windows-x64.zip")
if (Test-Path $zipOut) { Remove-Item $zipOut -Force }
Write-Host "`nCreating zip $zipOut ..."
Compress-Archive -Path (Join-Path $Dist '*') -DestinationPath $zipOut

# 5) Call Inno Setup compiler (ISCC.exe)
if (-not (Test-Path $InnoPath)) {
    $isccCmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($isccCmd) { $InnoPath = $isccCmd.Source }
    elseif (Test-Path 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe') { $InnoPath = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe' }
    elseif (Test-Path 'C:\Program Files\Inno Setup 6\ISCC.exe') { $InnoPath = 'C:\Program Files\Inno Setup 6\ISCC.exe' }
}
if (-not (Test-Path $InnoPath)) {
    Write-Warning "ISCC.exe not found at $InnoPath. Update -InnoPath parameter with the correct path to ISCC.exe."
    exit 1
}

$issPath = Join-Path $RepoRoot "setup.iss"
$ExpectedSetupName = "$BinaryBase-setup-$Version.exe"
$OutputDir = Join-Path $RepoRoot "Output"
if (-not (Test-Path $OutputDir)) { New-Item -ItemType Directory -Path $OutputDir | Out-Null }
$SetupPath = Join-Path $OutputDir $ExpectedSetupName
$isccArgs = @("/DMyAppVersion=$Version", "/O$OutputDir", "/F$ExpectedSetupName", $issPath)
Write-Host "`nRunning Inno Setup: $InnoPath $($isccArgs -join ' ')"
& $InnoPath @isccArgs

# 6) Signing (optional) - sign the resulting installer if PFX provided

if ($PfxPath -and (Test-Path $PfxPath)) {
    Write-Host "`nPFX provided - attempting to sign the installer..."
    # locate signtool
    $signtool = $null
    if ((Test-Path $SignToolPath) -and (Get-Command $SignToolPath -ErrorAction SilentlyContinue)) { $signtool = $SignToolPath }
    else {
        $found = Get-Command signtool.exe -ErrorAction SilentlyContinue
        if ($found) { $signtool = $found.Source }
    }
    if (-not $signtool) {
        Write-Warning "signtool.exe not found. Install Windows SDK or provide -SignToolPath. Skipping signing."
    } else {
        if (-not (Test-Path $SetupPath)) {
            Write-Warning "Installer not found in Output: $SetupPath. Skipping signing."
        } else {
            $signArgs = @('sign')
            $signArgs += '/f'; $signArgs += $PfxPath
            if ($PfxPassword -ne $null) {
                $plainPwd = ConvertFrom-SecureString -SecureString $PfxPassword -AsPlainText
                $signArgs += '/p'; $signArgs += $plainPwd
            }
            $signArgs += '/tr'; $signArgs += 'http://timestamp.digicert.com'
            $signArgs += '/td'; $signArgs += 'sha256'
            $signArgs += '/fd'; $signArgs += 'sha256'
            $signArgs += $SetupPath

            Write-Host "Signing with: $signtool $($signArgs -join ' ')"
            $proc = Start-Process -FilePath $signtool -ArgumentList $signArgs -Wait -NoNewWindow -PassThru
            if ($plainPwd) { Clear-Variable plainPwd } # Clear the plain text password from memory

            if ($proc.ExitCode -eq 0) {
                Write-Host "Signing successful: $SetupPath"
            } else { Write-Warning "signtool failed with exit code $($proc.ExitCode)" }
        }
    }
} else {
    Write-Host "`nNo PFX provided or PFX path not found - skipping signing."
}

Write-Host "`nDone. Check the Output folder for the installer and release/ for the zip."
