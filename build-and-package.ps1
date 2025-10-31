<#
  build-and-package.ps1
  Build and package script for Encrypted P2P Messenger
  Usage example:
    .\build-and-package.ps1 -Version "1.2.0" -PfxPath "C:\keys\mycert.pfx" -PfxPassword "YourPassword"
#>

param(
  [string]$Version = "1.2.0",
  [string]$Configuration = "release",
  [string]$Target = "x86_64-pc-windows-msvc",
  [string]$InnoPath = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
  [string]$IconSource = "encodeur_rsa_icon.ico",        # relatif au repo root ou chemin absolu
  [string]$PfxPath = "",                               # optionnel: chemin vers .pfx pour signer
  [System.Security.SecureString]$PfxPassword = $null,   # optionnel: mot de passe du .pfx (secure)
  [string]$SignToolPath = "C:\Program Files (x86)\Windows Kits\10\bin\x64\signtool.exe"
)

$ErrorActionPreference = 'Stop'
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
$RepoRoot = (Get-Location)
$Dist = Join-Path -Path $RepoRoot -ChildPath "dist"
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Path $Dist | Out-Null

$BuiltBinary = Join-Path -Path (Join-Path -Path "target" -ChildPath $Target) -ChildPath (Join-Path -Path "release" -ChildPath $BinaryName)
if (-not (Test-Path $BuiltBinary)) {
    Write-Error "Built binary not found at $BuiltBinary. Check target and binary name."
    exit 1
}

Copy-Item $BuiltBinary -Destination (Join-Path $Dist $BinaryName)
if (Test-Path ".\README.md") { Copy-Item ".\README.md" -Destination $Dist }
if (Test-Path ".\LICENSE") { Copy-Item ".\LICENSE" -Destination $Dist }

# 3) Copy icon into dist if present near repo root or provided path
$IconPathCandidates = @(
  (Join-Path $RepoRoot $IconSource),
  (Join-Path (Join-Path $RepoRoot "dist") $IconSource),
  $IconSource
)
$FoundIcon = $null
foreach ($p in $IconPathCandidates) {
    if ([string]::IsNullOrWhiteSpace($p)) { continue }
    if (Test-Path $p) { $FoundIcon = (Resolve-Path $p).Path; break }
}
if ($FoundIcon) {
    Write-Host "Copying icon from $FoundIcon to $Dist"
    Copy-Item $FoundIcon -Destination (Join-Path $Dist (Split-Path $FoundIcon -Leaf)) -Force
} else {
    Write-Host "No icon found at candidates: $($IconPathCandidates -join ' ; ') - continuing without icon."
}

# 4) Create zip artifact (optional)
if (-not (Test-Path "release")) { New-Item -ItemType Directory -Path "release" | Out-Null }
$zipOut = Join-Path -Path "release" -ChildPath ("$BinaryBase-$Version-windows-x64.zip")
if (Test-Path $zipOut) { Remove-Item $zipOut -Force }
Write-Host "`nCreating zip $zipOut ..."
Compress-Archive -Path (Join-Path $Dist '*') -DestinationPath $zipOut

# 5) Call Inno Setup compiler (ISCC.exe)
if (-not (Test-Path $InnoPath)) {
    Write-Warning "ISCC.exe not found at $InnoPath. Update -InnoPath parameter with the correct path to ISCC.exe."
    exit 1
}

$issPath = Join-Path $RepoRoot "setup.iss"
$defines = "/DMyAppVersion=`"$Version`""
Write-Host "`nRunning Inno Setup: $InnoPath $defines $issPath"
& $InnoPath $defines $issPath

# 6) Signing (optional) - sign the resulting installer if PFX provided
$ExpectedSetupName = "$BinaryBase-setup-$Version.exe"
$OutputDir = Join-Path $RepoRoot "Output"
$SetupPath = Join-Path $OutputDir $ExpectedSetupName

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
            # try to locate last exe in Output
            $alt = Get-ChildItem -Path $OutputDir -Filter "*.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($alt) { $SetupPath = $alt.FullName }
        }
        if (-not (Test-Path $SetupPath)) {
            Write-Warning "Installer not found in Output: $SetupPath. Skipping signing."
        } else {
            $signArgs = @('sign')
            $signArgs += '/f'; $signArgs += $PfxPath
            if ($PfxPassword -ne $null) {
                $bstr = $null
                try {
                    $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($PfxPassword)
                    $plainPwd = [Runtime.InteropServices.Marshal]::PtrToStringAuto($bstr)
                    if (-not [string]::IsNullOrEmpty($plainPwd)) {
                        $signArgs += '/p'; $signArgs += $plainPwd
                    }
                } finally {
                    if ($bstr) { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr) }
                    Remove-Variable plainPwd -ErrorAction SilentlyContinue
                }
            }
            $signArgs += '/tr'; $signArgs += 'http://timestamp.digicert.com'
            $signArgs += '/td'; $signArgs += 'sha256'
            $signArgs += '/fd'; $signArgs += 'sha256'
            $signArgs += $SetupPath

            Write-Host "Signing with: $signtool $($signArgs -join ' ')"
            $proc = Start-Process -FilePath $signtool -ArgumentList $signArgs -Wait -NoNewWindow -PassThru
            if ($proc.ExitCode -eq 0) { Write-Host "Signing successful: $SetupPath" } else { Write-Warning "signtool failed with exit code $($proc.ExitCode)" }
        }
    }
} else {
    Write-Host "`nNo PFX provided or PFX path not found - skipping signing."
}

Write-Host "`nDone. Check the Output folder for the installer and release/ for the zip."
