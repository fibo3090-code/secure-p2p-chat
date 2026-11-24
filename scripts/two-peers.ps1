<#
.SYNOPSIS
  Launch two isolated P2PEM desktop instances ("alice" and "bob") on this
  machine to exercise a real host -> connect -> verify -> message round trip.

.DESCRIPTION
  Unit and e2e tests drive ChatManager directly; the Playwright smoke tests
  drive the UI against the in-browser mock. Neither runs the shipped app
  talking to itself, which is where "messages send but are never received"
  and "verify & trust does nothing" were found. This script is that check.

  Each instance gets its own P2PEM_DATA_DIR (identity, history, settings), so
  they are two different peers. The data lives under %TEMP%\p2pem-peers and is
  never your real profile.

.PARAMETER Build
  Rebuild the release binary first (npx tauri build --no-bundle). Implied when
  no binary exists.

.PARAMETER Fresh
  Wipe both peers' data dirs first (new identities, first-run password screen).

.PARAMETER Stop
  Close the instances a previous run started, then exit.

.EXAMPLE
  ./scripts/two-peers.ps1 -Fresh
  ./scripts/two-peers.ps1 -Build
  ./scripts/two-peers.ps1 -Stop
#>
[CmdletBinding()]
param(
    [switch]$Build,
    [switch]$Fresh,
    [switch]$Stop
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$root = Join-Path $env:TEMP 'p2pem-peers'
$pidFile = Join-Path $root 'pids.txt'
$peers = 'alice', 'bob'

function Stop-Peers {
    if (-not (Test-Path $pidFile)) { return }
    foreach ($id in Get-Content $pidFile) {
        $p = Get-Process -Id $id -ErrorAction SilentlyContinue
        if ($p -and $p.ProcessName -like 'p2pem-desktop*') {
            # CloseMainWindow lets the app flush history on window close.
            [void]$p.CloseMainWindow()
            if (-not $p.WaitForExit(5000)) { $p.Kill() }
        }
    }
    Remove-Item $pidFile -Force
}

if ($Stop) { Stop-Peers; Write-Host 'Stopped.'; return }

# Resolve the real target dir (CARGO_TARGET_DIR / .cargo/config may move it).
$target = (cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $repo 'Cargo.toml') |
    ConvertFrom-Json).target_directory
$exe = Join-Path $target 'release\p2pem-desktop.exe'

if ($Build -or -not (Test-Path $exe)) {
    Write-Host "Building release binary (frontend + Tauri, no installers)..."
    Push-Location (Join-Path $repo 'desktop')
    try {
        npx tauri build --no-bundle
        if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
} else {
    $age = (Get-Date) - (Get-Item $exe).LastWriteTime
    $newest = git -C $repo log -1 --format=%cI
    if ((Get-Item $exe).LastWriteTime -lt [datetime]$newest) {
        Write-Warning ("Binary is older than the last commit ({0:N0} h old). Pass -Build to test current code." -f $age.TotalHours)
    }
}

Stop-Peers
New-Item -ItemType Directory -Force $root | Out-Null

$saved = $env:P2PEM_DATA_DIR
$started = @()
try {
    foreach ($name in $peers) {
        $dir = Join-Path $root $name
        if ($Fresh -and (Test-Path $dir)) { Remove-Item -Recurse -Force $dir }
        New-Item -ItemType Directory -Force $dir | Out-Null
        # Child processes inherit the current environment.
        $env:P2PEM_DATA_DIR = $dir
        $p = Start-Process -FilePath $exe -PassThru
        $started += $p.Id
        Write-Host ("{0,-6} pid {1,-6} data {2}" -f $name, $p.Id, $dir)
        Start-Sleep -Milliseconds 800
    }
} finally {
    $env:P2PEM_DATA_DIR = $saved
}
$started | Set-Content $pidFile

Write-Host @"

Two peers are running. Check, in this order (each step is a past bug):
  1. Both: set/unlock a password (>= 12 chars). The two windows must show different identities.
  2. alice: Host (default port 12345).   bob: Connect to 127.0.0.1:12345.
  3. Both show the SAME 6-digit code + emoji. Click Verify & trust on both: the prompt must close and stay closed.
  4. Send a message each way. It must appear on the OTHER window, with a delivered tick on the sender.
  5. bob: send a small file. alice: accept it; it lands in alice's downloads.
  6. Close bob, reopen with this script (no -Fresh): history is still there, and alice shows bob offline/online correctly.

Stop both:  ./scripts/two-peers.ps1 -Stop
"@
