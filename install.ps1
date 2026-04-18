# ZLayer Installer for Windows - Downloads from GitHub Releases
# Usage: irm https://raw.githubusercontent.com/BlackLeafDigital/ZLayer/main/install.ps1 | iex
#
# Options (set as environment variables before running):
#   $env:ZLAYER_VERSION = "0.9.6"         - Install specific version
#   $env:ZLAYER_INSTALL_DIR = "C:\path"   - Custom install directory

$ErrorActionPreference = 'Stop'

$Repo = "BlackLeafDigital/ZLayer"
$Binary = "zlayer"

# --- Detect architecture ---
$RawArch = $env:PROCESSOR_ARCHITECTURE
switch ($RawArch) {
    "AMD64" { $Arch = "amd64" }
    "ARM64" { $Arch = "arm64" }
    default {
        Write-Host "Error: Unsupported architecture: $RawArch" -ForegroundColor Red
        exit 1
    }
}

# --- Resolve version from GitHub ---
$Version = $env:ZLAYER_VERSION
if (-not $Version) {
    Write-Host "Fetching latest version from GitHub..."
    try {
        $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "ZLayer-Installer" }
        $Version = $Release.tag_name
    }
    catch {
        Write-Host "Error: Could not determine latest version. Set `$env:ZLAYER_VERSION and retry." -ForegroundColor Red
        exit 1
    }
    if (-not $Version) {
        Write-Host "Error: Could not determine latest version. Set `$env:ZLAYER_VERSION and retry." -ForegroundColor Red
        exit 1
    }
}

# Normalize: Tag has v prefix, VersionNum does not
if ($Version.StartsWith("v")) {
    $Tag = $Version
    $VersionNum = $Version.Substring(1)
}
else {
    $Tag = "v$Version"
    $VersionNum = $Version
}

# --- Resolve install directory ---
$InstallDir = $env:ZLAYER_INSTALL_DIR
if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "ZLayer\bin"
}
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# --- Download Windows binary from GitHub Releases ---
$Artifact = "$Binary-$VersionNum-windows-$Arch.zip"
$Url = "https://github.com/$Repo/releases/download/$Tag/$Artifact"

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) "zlayer-install-$([System.Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

try {
    Write-Host "Downloading $Binary $Tag (windows/$Arch)..."
    $ArchivePath = Join-Path $TempDir "archive.zip"
    try {
        Invoke-WebRequest -Uri $Url -OutFile $ArchivePath -UseBasicParsing
    }
    catch {
        Write-Host "Error: Download failed from $Url" -ForegroundColor Red
        Write-Host "  $_" -ForegroundColor Red
        exit 1
    }

    Write-Host "Extracting..."
    $ExtractDir = Join-Path $TempDir "extracted"
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force

    # Find zlayer.exe in extracted contents
    $ExePath = Get-ChildItem -Path $ExtractDir -Filter "$Binary.exe" -Recurse -File | Select-Object -First 1
    if (-not $ExePath) {
        Write-Host "Error: $Binary.exe not found in archive" -ForegroundColor Red
        exit 1
    }

    # --- Stop running zlayer before overwriting ---
    $TargetExe = Join-Path $InstallDir "$Binary.exe"
    if (Test-Path $TargetExe) {
        $RunningProcs = Get-Process -Name $Binary -ErrorAction SilentlyContinue
        if ($RunningProcs) {
            Write-Host "Stopping running zlayer process(es)..."
            $RunningProcs | ForEach-Object {
                try {
                    $_.CloseMainWindow() | Out-Null
                }
                catch { }
            }
            # Wait up to 5 seconds for graceful shutdown
            $Waited = 0
            while ($Waited -lt 50) {
                $Still = Get-Process -Name $Binary -ErrorAction SilentlyContinue
                if (-not $Still) { break }
                Start-Sleep -Milliseconds 100
                $Waited++
            }
            # Force kill if still running
            $Still = Get-Process -Name $Binary -ErrorAction SilentlyContinue
            if ($Still) {
                Write-Host "Force stopping zlayer..."
                $Still | Stop-Process -Force -ErrorAction SilentlyContinue
                Start-Sleep -Milliseconds 500
            }
        }
    }

    # --- Install binary ---
    Write-Host "Installing to $InstallDir..."
    Copy-Item -Path $ExePath.FullName -Destination $TargetExe -Force

    # --- Warn if an older zlayer.exe shadows the one we just installed ---
    # Get-Command walks PATH in order; a stale binary earlier on PATH will
    # silently be used instead of the one we installed, which produces
    # confusing build failures where the daemon can't resolve local images.
    $Resolved = Get-Command $Binary -ErrorAction SilentlyContinue
    if ($Resolved -and $Resolved.Source -and ($Resolved.Source -ne $TargetExe)) {
        Write-Host ""
        Write-Host "Warning: another '$Binary' binary appears earlier on PATH:" -ForegroundColor Yellow
        Write-Host "    $($Resolved.Source)" -ForegroundColor Yellow
        Write-Host "It will shadow the version just installed. Remove it with:" -ForegroundColor Yellow
        Write-Host "    Remove-Item '$($Resolved.Source)'" -ForegroundColor Yellow
        Write-Host "Then open a new terminal." -ForegroundColor Yellow
    }

    # --- Download WSL2 support files (optional) ---
    Write-Host ""
    Write-Host "Downloading Linux binary for WSL2 backend support..."
    $LinuxArtifact = "$Binary-$VersionNum-linux-amd64.tar.gz"
    $LinuxUrl = "https://github.com/$Repo/releases/download/$Tag/$LinuxArtifact"
    $LinuxArchivePath = Join-Path $TempDir "linux-archive.tar.gz"
    $LinuxTargetPath = Join-Path $InstallDir "zlayer-linux"

    try {
        Invoke-WebRequest -Uri $LinuxUrl -OutFile $LinuxArchivePath -UseBasicParsing

        # Extract tar.gz - use tar which is available on Windows 10+
        $LinuxExtractDir = Join-Path $TempDir "linux-extracted"
        New-Item -ItemType Directory -Path $LinuxExtractDir -Force | Out-Null
        tar -xzf $LinuxArchivePath -C $LinuxExtractDir 2>$null

        $LinuxBin = Get-ChildItem -Path $LinuxExtractDir -Filter $Binary -Recurse -File | Select-Object -First 1
        if ($LinuxBin) {
            Copy-Item -Path $LinuxBin.FullName -Destination $LinuxTargetPath -Force
            Write-Host "  Installed zlayer-linux for WSL2 backend support"
        }
        else {
            Write-Host "  Warning: Could not find linux binary in archive (WSL2 support skipped)" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "  Note: Linux binary download skipped (WSL2 backend support unavailable)" -ForegroundColor Yellow
        Write-Host "  This is optional and does not affect Windows functionality."
    }

    # --- Add to PATH if needed ---
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathEntries = $UserPath -split ";"
    $AlreadyInPath = $PathEntries | Where-Object { $_.TrimEnd("\") -eq $InstallDir.TrimEnd("\") }

    if (-not $AlreadyInPath) {
        $NewPath = "$InstallDir;$UserPath"
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        # Also update current session
        $env:Path = "$InstallDir;$env:Path"
        Write-Host ""
        Write-Host "Added $InstallDir to your user PATH."
        Write-Host "  Note: Open a new terminal for the PATH change to take effect in other sessions."
    }

    # --- Install PowerShell completions (fail-soft) ---
    # Drop a completion script next to $PROFILE and dot-source it from the
    # profile if not already referenced. Never abort the installer on failure.
    Write-Host ""
    Write-Host "Installing PowerShell completion..."
    try {
        $CompletionScript = & $TargetExe completions powershell 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $CompletionScript) {
            Write-Host "  Warning: '$Binary completions powershell' failed; skipping." -ForegroundColor Yellow
        }
        else {
            $CompletionDir = Join-Path $HOME "Documents\PowerShell"
            if (-not (Test-Path $CompletionDir)) {
                # Fall back to WindowsPowerShell if it already exists; else create PowerShell.
                $WinPSDir = Join-Path $HOME "Documents\WindowsPowerShell"
                if (Test-Path $WinPSDir) {
                    $CompletionDir = $WinPSDir
                }
                else {
                    New-Item -ItemType Directory -Path $CompletionDir -Force | Out-Null
                }
            }
            $CompletionPath = Join-Path $CompletionDir "zlayer-completions.ps1"
            # -Join the script lines with newlines (completions command emits a string array).
            if ($CompletionScript -is [array]) {
                $CompletionText = $CompletionScript -join "`n"
            }
            else {
                $CompletionText = [string]$CompletionScript
            }
            Set-Content -Path $CompletionPath -Value $CompletionText -Encoding UTF8 -Force
            Write-Host "  Installed zlayer PowerShell completion to $CompletionPath"

            # Append dot-source line to $PROFILE if not present.
            $ProfilePath = $PROFILE
            if ($ProfilePath) {
                $ProfileDir = Split-Path -Path $ProfilePath -Parent
                if ($ProfileDir -and -not (Test-Path $ProfileDir)) {
                    New-Item -ItemType Directory -Path $ProfileDir -Force | Out-Null
                }
                if (-not (Test-Path $ProfilePath)) {
                    New-Item -ItemType File -Path $ProfilePath -Force | Out-Null
                }
                $DotSourceLine = ". '$CompletionPath'"
                $ExistingProfile = Get-Content -Path $ProfilePath -Raw -ErrorAction SilentlyContinue
                if (-not $ExistingProfile) { $ExistingProfile = "" }
                if ($ExistingProfile -notlike "*$CompletionPath*") {
                    Add-Content -Path $ProfilePath -Value "`n# Added by ZLayer installer`n$DotSourceLine"
                    Write-Host "  Appended dot-source line to $ProfilePath"
                }
                else {
                    Write-Host "  Profile already references completion script; skipping profile edit."
                }
                Write-Host "  Open a new PowerShell session (or run '. `$PROFILE') to enable completions."
            }
        }
    }
    catch {
        Write-Host "  Warning: failed to install PowerShell completion: $_" -ForegroundColor Yellow
    }

    # --- Success ---
    Write-Host ""
    Write-Host "$Binary $Tag installed to $TargetExe" -ForegroundColor Green
    Write-Host ""
    Write-Host "Run '$Binary --help' to get started."
}
finally {
    # Clean up temp directory
    if (Test-Path $TempDir) {
        Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
