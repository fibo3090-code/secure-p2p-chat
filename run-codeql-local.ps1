<#
.SYNOPSIS
    Runs CodeQL analysis locally for the project.

.DESCRIPTION
    This script checks if the CodeQL CLI is installed, creates a CodeQL database,
    and runs the analysis. It outputs the results to 'codeql-results.sarif'.

.NOTES
    Prerequisite: You must have the CodeQL CLI installed and in your PATH.
    Download: https://github.com/github/codeql-cli-binaries/releases
#>

$ErrorActionPreference = "Stop"

function Check-CodeQL {
    if (-not (Get-Command "codeql" -ErrorAction SilentlyContinue)) {
        Write-Warning "CodeQL CLI not found in PATH."
        Write-Host "Please install the CodeQL CLI manually:" -ForegroundColor Yellow
        Write-Host "1. Download the latest release from: https://github.com/github/codeql-cli-binaries/releases"
        Write-Host "2. Extract the zip file"
        Write-Host "3. Add the extracted 'codeql' directory to your system PATH environment variable"
        Write-Host "4. Restart your terminal and run this script again."
        exit 1
    }
    Write-Host "Found CodeQL CLI." -ForegroundColor Green
}

function Run-Analysis {
    $dbDir = "codeql-db"
    $outputFile = "codeql-results.sarif"

    Write-Host "Creating CodeQL database in '$dbDir'..." -ForegroundColor Cyan
    # --overwrite allows rebuilding if the folder exists
    codeql database create $dbDir --language=rust --overwrite --command="cargo build"

    if ($LASTEXITCODE -eq 0) {
        Write-Host "Database created." -ForegroundColor Green
    } else {
        Write-Error "Failed to create CodeQL database."
        exit $LASTEXITCODE
    }

    Write-Host "Analyzing database..." -ForegroundColor Cyan
    # Using 'codeql-suites/rust-security-and-quality.qls' if available, or default queries
    # Since Rust support is newer, we let codeql pick the default suite for the language
    codeql database analyze $dbDir --format=sarif-latest --output=$outputFile --download

    if ($LASTEXITCODE -eq 0) {
        Write-Host "Analysis complete. Results saved to '$outputFile'" -ForegroundColor Green
        Write-Host "You can view this file using the SARIF Viewer extension in VS Code." -ForegroundColor Gray
    } else {
        Write-Error "Analysis failed."
        exit $LASTEXITCODE
    }
}

Check-CodeQL
Run-Analysis
