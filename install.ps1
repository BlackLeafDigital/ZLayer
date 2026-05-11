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

    # --- Pre-create data directory layout (secrets/) ---
    # zlayer 0.11.20+ requires `<data_dir>\secrets` to be a directory (the
    # encrypted-secrets sqlite store lives at `secrets\secrets.sqlite`). If a
    # prior version left `secrets` as a regular file, leave it alone — the
    # daemon's startup migration (also exposed via `zlayer daemon migrate`)
    # renames it into place. We only pre-create when the path is absent or
    # already a directory, mirroring the POSIX installer.
    #
    # Data dir on Windows is %ProgramData%\ZLayer (per zlayer-paths), falling
    # back to C:\ProgramData\ZLayer when the env var is unset.
    $DataDir = if ($env:PROGRAMDATA) {
        Join-Path $env:PROGRAMDATA 'ZLayer'
    }
    else {
        'C:\ProgramData\ZLayer'
    }
    $SecretsPath = Join-Path $DataDir 'secrets'
    try {
        if (-not (Test-Path -LiteralPath $SecretsPath) -or (Test-Path -LiteralPath $SecretsPath -PathType Container)) {
            New-Item -ItemType Directory -Force -Path $SecretsPath | Out-Null
        }
    }
    catch {
        # Non-fatal: the daemon will create/migrate this itself on first boot.
        Write-Host "  Warning: could not pre-create $SecretsPath ($_); daemon will handle it on first boot." -ForegroundColor Yellow
    }

    # --- Belt-and-suspenders: run the data-dir migration. ---
    # On platforms where the installer registers a service, `daemon install`
    # already runs this; the Windows installer leaves service registration to
    # the user, so we invoke `daemon migrate` directly. It's idempotent and
    # self-heals legacy `secrets` regular-files into the new layout. Never
    # block the install on migration failure.
    try {
        & $TargetExe daemon migrate 2>&1 | Out-Null
    }
    catch {
        # Intentionally swallow — never block install on migration.
    }

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

    # --- WSL2 bootstrap (required for Linux containers on Windows) ---
    # zlayer's Linux-workload path runs containers under `youki` inside a
    # WSL2 distro (Wsl2DelegateRuntime in zlayer-agent). WSL2 itself is a
    # Windows Optional Feature, so `wsl --install` has to happen once before
    # the runtime can bring up Linux workloads.
    #
    # We fire `wsl --install --no-distribution` unconditionally when WSL2
    # isn't already present — UAC is the consent gate. If the user declines
    # the elevation prompt, we print the manual command and move on without
    # failing the install; Windows-native containers (HCS) still work.
    Write-Host ""
    Write-Host "Checking for WSL2..."
    $WslAvailable = $false
    if (Get-Command wsl.exe -ErrorAction SilentlyContinue) {
        try {
            $null = wsl.exe --status 2>&1
            if ($LASTEXITCODE -eq 0) {
                $WslAvailable = $true
            }
        }
        catch {
            # wsl.exe exists but failed — treat as not-available.
        }
    }

    if ($WslAvailable) {
        Write-Host "  WSL2 detected." -ForegroundColor Green
    }
    else {
        Write-Host "  WSL2 not installed. Running 'wsl --install --no-distribution' (UAC prompt will appear)..."
        try {
            $Proc = Start-Process -FilePath "wsl.exe" `
                -ArgumentList "--install", "--no-distribution" `
                -Verb RunAs -Wait -PassThru -ErrorAction Stop
            switch ($Proc.ExitCode) {
                0 {
                    Write-Host "  WSL2 installed. A reboot may be required before first use." -ForegroundColor Green
                }
                3010 {
                    Write-Host "  WSL2 installed; reboot required to complete setup." -ForegroundColor Yellow
                }
                default {
                    Write-Host "  'wsl --install' exited with code $($Proc.ExitCode)." -ForegroundColor Yellow
                    Write-Host "  Retry manually in an elevated shell: wsl --install --no-distribution"
                }
            }
        }
        catch {
            # Most common path: user clicked No on the UAC prompt.
            Write-Host "  WSL2 install was cancelled or failed: $_" -ForegroundColor Yellow
            Write-Host "  Retry later with 'wsl --install --no-distribution' in an elevated shell,"
            Write-Host "  or run 'zlayer node init --install-wsl yes' to be prompted again."
        }
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
