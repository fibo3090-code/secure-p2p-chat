<#
.SYNOPSIS
    Runs CodeQL analysis locally for the project.

.DESCRIPTION
    This script checks if the CodeQL CLI is installed, creates a CodeQL database,
    and runs the analysis. It outputs the results to 'codeql-results.sarif'.
    
    It automatically handles paths with spaces (common on Windows) by mounting
    the current directory to a temporary virtual drive letter during execution.

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

function Get-FreeDriveLetter {
    # Check drive letters from Z to D
    for ($char = [byte][char]'Z'; $char -ge [byte][char]'D'; $char--) {
        $letter = [char]$char + ":"
        if (-not (Test-Path $letter)) {
            return $letter
        }
    }
    throw "No free drive letters found to mount the workspace."
}

function Run-Analysis {
    $originalLocation = Get-Location
    $dbName = "codeql-db"
    $outputFile = "codeql-results.sarif"
    $mountPoint = $null

    # Workaround for CodeQL on Windows with spaces in path:
    # We mount the current directory to a drive letter (e.g. X:)
    if ($originalLocation.Path.Contains(" ")) {
        Write-Host "Path contains spaces. Mounting to a virtual drive to satisfy CodeQL..." -ForegroundColor Cyan
        try {
            $mountPoint = Get-FreeDriveLetter
            Write-Host "Mounting '$originalLocation' to '$mountPoint'"
            subst $mountPoint "$originalLocation"
            Set-Location "$mountPoint\"
        }
        catch {
            Write-Error "Failed to mount virtual drive: $_"
            exit 1
        }
    }

    try {
        Write-Host "Creating CodeQL database in '$dbName'..." -ForegroundColor Cyan
        # --overwrite allows rebuilding if the folder exists
        codeql database create $dbName --language=rust --overwrite --command="cargo build"

        if ($LASTEXITCODE -eq 0) {
            Write-Host "Database created." -ForegroundColor Green
        }
        else {
            Write-Error "Failed to create CodeQL database."
            exit $LASTEXITCODE
        }

        Write-Host "Analyzing database..." -ForegroundColor Cyan
        codeql database analyze $dbName --format=sarif-latest --output=$outputFile --download

        if ($LASTEXITCODE -eq 0) {
            Write-Host "Analysis complete. Results saved to '$outputFile'" -ForegroundColor Green
            Write-Host "You can view this file using the SARIF Viewer extension in VS Code." -ForegroundColor Gray
            
            # Copy result back to original location if we are mounted
            if ($mountPoint) {
                Copy-Item $outputFile "$originalLocation\$outputFile" -Force
                Write-Host "Copied results to original directory."
            }
        }
        else {
            Write-Error "Analysis failed."
            exit $LASTEXITCODE
        }
    }
    finally {
        # Cleanup
        if ($mountPoint) {
            Set-Location $originalLocation
            Write-Host "Unmounting '$mountPoint'..."
            subst $mountPoint /d
        }
    }
}

Check-CodeQL
Run-Analysis
