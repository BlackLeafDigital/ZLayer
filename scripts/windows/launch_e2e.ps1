#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Launch a cargo test in a detached process on the Windows test host.
  Survives SSH disconnect. Writes log + sentinel + rc to a run dir.

.PARAMETER Test
  --test argument to cargo, e.g. composite_dispatch_e2e

.PARAMETER Package
  -p argument to cargo, default zlayer-agent

.PARAMETER Features
  --features argument to cargo, default hcs-runtime,wsl

.PARAMETER RunDir
  Where to write log/sentinel/rc. Default %USERPROFILE%\.zlayer-tests\<test>

.PARAMETER RepoDir
  Working directory for cargo. Default C:\src\ZLayer

.PARAMETER Keep
  When set, exports ZLAYER_KEEP_UVM_ON_FAILURE=1 into the cargo test
  environment so a Hyper-V step-4 accept timeout leaves the UVM running
  and keeps the per-UVM sandbox VHDX + writable debug share on disk for
  offline inspection (instead of tearing down at the 120s mark).

.PARAMETER Image
  When set, exports ZLAYER_HCS_TEST_IMAGE=<value> into the cargo test
  environment so the Hyper-V e2e suite pulls and boots a non-default image
  (e.g. mcr.microsoft.com/windows/servercore:ltsc2022 for the A/B against
  the default nanoserver:ltsc2022).

.OUTPUTS
  Writes the launched PID to stdout as "PID=<n>" and the run dir as "RUNDIR=<path>".
  The detached process writes:
    <RunDir>\stdout.log     - combined stdout+stderr from cargo
    <RunDir>\rc             - exit code from cargo (written on completion)
    <RunDir>\done           - sentinel file (created on completion)
    <RunDir>\pid            - PID of the cargo wrapper

.NOTES
  Uses cmd.exe as the wrapper so cargo inherits the standard user PATH
  (PowerShell Start-Process -WindowStyle Hidden does not always inherit
  rustup's PATH addition reliably across sessions).
#>
[CmdletBinding()]
param(
  [string]$Test     = "composite_dispatch_e2e",
  [string]$Package  = "zlayer-agent",
  [string]$Features = "hcs-runtime,wsl,windows-debug",
  [string]$RunDir   = $null,
  [string]$RepoDir  = "C:\src\ZLayer",
  [switch]$Keep,
  [string]$Image    = ""
)

$ErrorActionPreference = "Stop"

if (-not $RunDir) {
  $RunDir = Join-Path $env:USERPROFILE ".zlayer-tests\$Test"
}

# Fresh run dir — wipe previous artifacts so old sentinels don't lie to us
if (Test-Path $RunDir) { Remove-Item -Recurse -Force $RunDir }
New-Item -ItemType Directory -Path $RunDir -Force | Out-Null

$logFile  = Join-Path $RunDir "stdout.log"
$rcFile   = Join-Path $RunDir "rc"
$doneFile = Join-Path $RunDir "done"
$pidFile  = Join-Path $RunDir "pid"
$runCmd   = Join-Path $RunDir "run.cmd"
# UVM snapshotter plumbing: polls %TEMP%\zlayer-hcs-hyperv-e2e-*\...\UtilityVM
# while a test is running and xcopy-clones the first complete one it sees
# to C:\zlayer-uvm-reference\ before the test's teardown nukes %TEMP%.
# The stop sentinel is created post-cargo so the snapshotter exits cleanly.
$snapshotStopFile = Join-Path $RunDir "snapshot-stop.sentinel"
$snapshotLog      = Join-Path $RunDir "snapshot.log"
$snapshotPidFile  = Join-Path $RunDir "snapshotter.pid"
# COM-pipe reader plumbing (early-boot kernel capture from UEFI / bootmgr /
# winload / SMSS / wininit before in-guest GCS comes up). The UVM doc
# emits `Devices.ComPorts["0"].NamedPipe = \\.\pipe\zlayer-uvm-<id>-com1`
# and `Chipset.Uefi.Console = "ComPort1"`, so HCS streams firmware POST +
# kernel boot output into that pipe. ZLayer is NOT the pipe server — HCS
# itself owns the server side of the named pipe — so this reader opens
# the host CLIENT side and tees bytes into a per-pipe log file.
$comReaderStopFile = Join-Path $RunDir "com-reader-stop.sentinel"
$comReaderLog      = Join-Path $RunDir "com-reader.log"
$comReaderPidFile  = Join-Path $RunDir "com-reader.pid"

# Resolve cargo absolute path so the detached cmd.exe child never depends on
# the (possibly missing) PATH of its parent.
$cargoCmd = (Get-Command cargo -ErrorAction SilentlyContinue)
if (-not $cargoCmd) {
  $candidate = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
  if (Test-Path $candidate) {
    $cargoPath = $candidate
  } else {
    throw "cargo not found on PATH and $candidate does not exist"
  }
} else {
  $cargoPath = $cargoCmd.Source
}

# Write a .cmd script to disk and invoke that. Avoids the cmd.exe /c quoting
# nightmare (multiple nested quoted paths chained with &/&& cause cmd.exe to
# strip the wrong quotes and exit immediately writing nothing).
$cargoArgs = "test -p $Package --features $Features --test $Test -- --ignored --nocapture --test-threads=1"
$dumpDir = Join-Path $RunDir "hcs-docs"
# HCS host-side ETW capture artifacts. Live alongside stdout.log so
# debug_e2e.py's scp -r fetch grabs them automatically.
$etlFile        = Join-Path $RunDir "hcs.etl"
$etlXml         = Join-Path $RunDir "hcs.xml"
$etwSetupLog    = Join-Path $RunDir "etw-setup.log"
$etwTeardownLog = Join-Path $RunDir "etw-teardown.log"
$workerOpXml    = Join-Path $RunDir "hyperv-worker-op.xml"
$computeOpXml   = Join-Path $RunDir "hyperv-compute-op.xml"
# ETW provider GUIDs — verbatim from hcsshim
# `internal/vm/vmutils/etw/etw_map.go` (plus RunHCS from
# `cmd/runhcs/main.go`'s hard-coded const). Verbose level (`0xFF`) + all
# keywords (`0xFFFFFFFFFFFFFFFF`); TraceLogging providers don't publish
# keyword enums so we keep the mask wide-open. Decode via `tracerpt -of XML`
# (built-in; no PerfView dependency).
$etwProviders = @(
  '{80ce50de-d264-4581-950d-abadeee0d340}', # microsoft.windows.hyperv.compute (TraceLogging)
  '{17103e3f-3c6e-4677-bb17-3b267eb5be57}', # microsoft-windows-hyper-v-compute (manifested)
  '{0b52781f-b24d-5685-ddf6-69830ed40ec3}', # microsoft.virtualization.runhcs (hcsshim's own)
  '{396a26ff-fb73-5465-0d17-dd4930896239}', # microsoft.windows.logforwardservice.provider (GCS bridge)
  '{c7c9e4f7-c41d-5c68-f104-d72a920016c7}', # microsoft-windows-hyper-v-crashdump (fires on 0xEF)
  '{ecdaacfa-6fe9-477c-b5f0-85b76f8f50aa}', # microsoft-windows-crashdump
  '{51ddfa29-d5c8-4803-be4b-2ecb715570fe}', # microsoft-windows-hyper-v-worker
  '{6066f867-7ca1-4418-85fd-36e3f9c0600c}', # microsoft-windows-hyper-v-vmms
  '{52fc89f8-995e-434c-a91e-199986449890}', # microsoft-windows-hyper-v-hypervisor
  '{7b0ea079-e3bc-424a-b2f0-e3d8478d204b}', # microsoft-windows-hyper-v-vsmb
  '{eded5085-79d0-4e31-9b4e-4299b78cbeeb}', # microsoft-windows-hyper-v-debug
  '{af7fd3a7-b248-460c-a9f5-fec39ef8468c}', # microsoft-windows-hyper-v-computelib
  '{02f3a5e3-e742-4720-85a5-f64c4184e511}'  # microsoft-windows-hyper-v-config
)
$cmdLines = @(
  "@echo off",
  "cd /d `"$RepoDir`"",
  # Bump tracing verbosity for the in-process Hyper-V step logs (mirrored to
  # stderr via eprintln! anyway, but we keep RUST_LOG aligned for any future
  # subscriber). `info` matches `Hyper-V step` lines without flooding with
  # tokio/hyper noise.
  "set RUST_LOG=zlayer_agent=info,zlayer_gcs=info,error",
  "set ZLAYER_HCS_DOC_DUMP_DIR=$dumpDir",
  "set ZLAYER_HCN_DOC_DUMP_DIR=$dumpDir",
  # Force in-memory blob cache for tests so multi-GB layers (e.g. servercore)
  # bypass the SQLite SQLITE_MAX_LENGTH (1 GiB) limit hit on the default
  # `Persistent` SQLite cache. The persistent cache stores blobs as a single
  # BLOB row, which is unsuitable for image layer blobs — fixing that is a
  # separate effort (TODO: move blob bytes to disk, keep only metadata in DB).
  "set ZLAYER_CACHE_TYPE=memory"
)
if ($Keep) {
  $cmdLines += "set ZLAYER_KEEP_UVM_ON_FAILURE=1"
}
if ($Image) {
  $cmdLines += "set ZLAYER_HCS_TEST_IMAGE=$Image"
}
# Pre-test: spawn UVM snapshotter (background polling loop). Watches
# %TEMP%\zlayer-hcs-hyperv-e2e-*\...\UtilityVM dirs for a complete boot
# tree (bootmgfw.efi present) and xcopy-clones it to
# C:\zlayer-uvm-reference\ — outside %TEMP% so Windows won't auto-clean.
# Wipes any prior snapshot first so each run gets fresh bytes.
# We inline the polling loop as a -Command string (vs. a separate .ps1)
# to keep the snapshotter self-contained in this script.
$snapshotInline = @"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue 'C:\zlayer-uvm-reference\';
Add-Content -Path '$snapshotLog' -Value (""[{0}] snapshotter started, pid=`$PID"" -f (Get-Date -Format o));
Set-Content -Path '$snapshotPidFile' -Value `$PID;
while (-not (Test-Path '$snapshotStopFile')) {
  `$candidates = Get-ChildItem -Path `$env:TEMP -Recurse -Filter 'UtilityVM' -ErrorAction SilentlyContinue |
                 Where-Object { `$_.FullName -match 'zlayer-hcs-hyperv-e2e-' -and (Test-Path (Join-Path `$_.FullName 'Files\EFI\Microsoft\Boot\bootmgfw.efi')) };
  foreach (`$c in `$candidates) {
    `$src = `$c.FullName;
    `$dst = 'C:\zlayer-uvm-reference\UtilityVM';
    if (-not (Test-Path `$dst)) {
      New-Item -ItemType Directory -Path 'C:\zlayer-uvm-reference\' -Force | Out-Null;
      xcopy /E /I /Y /Q `$src `$dst 2>&1 | Out-Null;
      if (Test-Path (Join-Path `$dst 'Files\EFI\Microsoft\Boot\bootmgfw.efi')) {
        Add-Content -Path '$snapshotLog' -Value (""[{0}] snapshotted {1} -> {2}"" -f (Get-Date -Format o), `$src, `$dst);
        exit 0;
      }
    }
  }
  Start-Sleep -Seconds 2;
}
Add-Content -Path '$snapshotLog' -Value (""[{0}] stop sentinel hit before any UVM appeared"" -f (Get-Date -Format o));
"@
# Collapse to single line for cmd.exe `start /B powershell -Command "..."`
# (newlines inside -Command break cmd.exe quoting). Statements are already
# terminated with `;` so this is safe.
$snapshotOneLine = ($snapshotInline -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 }) -join ' '
# Escape embedded double-quotes for the cmd.exe-level quoting layer
# (start /B powershell -Command "..."). Inside the cmd.exe quoted arg,
# `""` is the cmd.exe escape for a literal double quote.
$snapshotEscaped = $snapshotOneLine -replace '"', '""'
$cmdLines += @(
  "REM ----- UVM snapshotter: spawn -----",
  "del /Q `"$snapshotStopFile`" 2>nul",
  "start /B powershell -NoProfile -ExecutionPolicy Bypass -Command `"$snapshotEscaped`""
)
# Pre-test: spawn COM-pipe reader. Polls `\\.\pipe\` for pipes matching
# `zlayer-uvm-*-com1` (one per UVM, keyed off RuntimeId GUID) and opens
# the host CLIENT side of each, streaming bytes into
# `$RunDir\com-<pipe>.log`. Captures UEFI / bootmgr / winload / SMSS /
# wininit early-boot output that has no other channel.
#
# Pipe direction note: HCS / Hyper-V owns the SERVER side of the COM
# named pipe (that's how COM-port-over-named-pipe works in Hyper-V), so
# the host reader must connect as the CLIENT. Verified against hcsshim
# `internal/uvm/create_wcow.go:313-319` where `opts.ConsolePipe` is just
# a NamedPipe path with no server/client annotation — meaning HCS picks
# the server side and external readers must dial in as clients.
$comReaderInline = @"
Add-Content -Path '$comReaderLog' -Value (""[{0}] com-reader started, pid=`$PID, pattern=zlayer-uvm-*-com1 -> $RunDir"" -f (Get-Date -Format o));
Set-Content -Path '$comReaderPidFile' -Value `$PID;
`$seen = @{};
while (-not (Test-Path '$comReaderStopFile')) {
  try {
    Get-ChildItem '\\.\pipe\' -ErrorAction SilentlyContinue | Where-Object { `$_.Name -like 'zlayer-uvm-*-com1' } | ForEach-Object {
      `$pipeName = `$_.Name;
      if (-not `$seen.ContainsKey(`$pipeName)) {
        `$seen[`$pipeName] = `$true;
        `$sanitized = `$pipeName -replace '[^A-Za-z0-9._-]', '_';
        `$perPipeLog = Join-Path '$RunDir' (""com-{0}.log"" -f `$sanitized);
        Add-Content -Path '$comReaderLog' -Value (""[{0}] saw pipe {1}, spawning reader -> {2}"" -f (Get-Date -Format o), `$pipeName, `$perPipeLog);
        Start-Job -ScriptBlock {
          param(`$name, `$log, `$rdrLog)
          try {
            `$client = New-Object System.IO.Pipes.NamedPipeClientStream('.', `$name, [System.IO.Pipes.PipeDirection]::In);
            `$client.Connect(15000);
            Add-Content -Path `$rdrLog -Value (""[{0}] connected to {1}"" -f (Get-Date -Format o), `$name);
            `$buf = New-Object byte[] 4096;
            `$fs = [System.IO.File]::Open(`$log, [System.IO.FileMode]::Append, [System.IO.FileAccess]::Write, [System.IO.FileShare]::Read);
            try { while (`$true) { `$n = `$client.Read(`$buf, 0, `$buf.Length); if (`$n -le 0) { break }; `$fs.Write(`$buf, 0, `$n); `$fs.Flush(); } } finally { `$fs.Close(); `$client.Dispose(); }
            Add-Content -Path `$rdrLog -Value (""[{0}] pipe {1} closed"" -f (Get-Date -Format o), `$name);
          } catch { Add-Content -Path `$rdrLog -Value (""[{0}] reader-error on {1}: {2}"" -f (Get-Date -Format o), `$name, `$_); }
        } -ArgumentList `$pipeName, `$perPipeLog, '$comReaderLog' | Out-Null;
      }
    }
  } catch { Add-Content -Path '$comReaderLog' -Value (""[{0}] poll-error: {1}"" -f (Get-Date -Format o), `$_); }
  Start-Sleep -Seconds 1;
}
Add-Content -Path '$comReaderLog' -Value (""[{0}] stop sentinel hit, exiting"" -f (Get-Date -Format o));
Get-Job | Where-Object { `$_.State -eq 'Running' } | Stop-Job -ErrorAction SilentlyContinue;
Get-Job | Remove-Job -Force -ErrorAction SilentlyContinue;
"@
$comReaderOneLine = ($comReaderInline -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 }) -join ' '
$comReaderEscaped = $comReaderOneLine -replace '"', '""'
$cmdLines += @(
  "REM ----- COM-pipe reader: spawn -----",
  "del /Q `"$comReaderStopFile`" 2>nul",
  "start /B powershell -NoProfile -ExecutionPolicy Bypass -Command `"$comReaderEscaped`""
)
# Pre-test: start HCS ETW capture (best-effort; failures logged but don't
# block the cargo run). Wevtutil channel enables are idempotent.
$cmdLines += @(
  "REM ----- HCS ETW capture: setup -----",
  "logman stop HcsTrace 2>nul",
  "logman delete HcsTrace 2>nul",
  "logman create trace HcsTrace -ow -o `"$etlFile`" -nb 64 256 -bs 1024 -mode 0x00080002 -max 4096 -p `"$($etwProviders[0])`" 0xFFFFFFFFFFFFFFFF 0xFF 2>>`"$etwSetupLog`""
)
# Add the rest of the providers via `logman update`; first provider is
# attached at create time above.
for ($i = 1; $i -lt $etwProviders.Length; $i++) {
  $cmdLines += "logman update trace HcsTrace -p `"$($etwProviders[$i])`" 0xFFFFFFFFFFFFFFFF 0xFF 2>>`"$etwSetupLog`""
}
$cmdLines += @(
  "logman start HcsTrace 2>>`"$etwSetupLog`"",
  "wevtutil set-log Microsoft-Windows-Hyper-V-Worker-Analytic    /enabled:true /quiet 2>>`"$etwSetupLog`"",
  "wevtutil set-log Microsoft-Windows-Hyper-V-Worker-Operational /enabled:true /quiet 2>>`"$etwSetupLog`"",
  "wevtutil set-log Microsoft-Windows-Hyper-V-Compute-Operational /enabled:true /quiet 2>>`"$etwSetupLog`""
)
# Run cargo and capture its exit code into a temp var so post-test ETW
# teardown still runs even if cargo failed.
$cmdLines += @(
  "REM ----- cargo test -----",
  "`"$cargoPath`" $cargoArgs > `"$logFile`" 2>&1",
  "set CARGO_RC=%errorlevel%"
)
# Post-test: stop ETW + decode to XML so debug_e2e.py picks it up via scp.
# `tracerpt -of XML` is built-in (no PerfView dependency required).
$cmdLines += @(
  "REM ----- HCS ETW capture: teardown -----",
  "logman stop HcsTrace 2>>`"$etwTeardownLog`"",
  "logman delete HcsTrace 2>>`"$etwTeardownLog`"",
  "tracerpt `"$etlFile`" -o `"$etlXml`" -of XML -lr -y 2>>`"$etwTeardownLog`"",
  "wevtutil qe Microsoft-Windows-Hyper-V-Worker-Operational /c:500 /rd:true /f:xml > `"$workerOpXml`" 2>>`"$etwTeardownLog`"",
  "wevtutil qe Microsoft-Windows-Hyper-V-Compute-Operational /c:500 /rd:true /f:xml > `"$computeOpXml`" 2>>`"$etwTeardownLog`""
)
# Auto-collect per-UVM guest dump artifacts that survived (or were
# generated by) the test. Two source dirs per UVM:
#   - debug/ — writable VSMB share the in-guest zlayer-dump service writes
#              `app-tail.xml`, `sys-tail.xml`, `tasklist.txt`, `sc-gcs.txt`,
#              `sc-wersvc.txt`, `start.txt`, plus any
#              `VmComputeAgent.exe.<pid>.dmp` WER LocalDumps drops.
#   - crash/ — host-only, holds `bugcheck-savedstate.vmrs` /
#              `bugcheck-nocrashdump.vmrs` / `guest-crash.dmp` that HCS
#              writes from the `DebugOptions` + `GuestCrashReporting`
#              blocks.
# Wildcards match every UVM created by every concurrent test case.
# `xcopy /Q` keeps the batch quiet; failures hit nul (no test temp = no
# guest = nothing to collect, which is fine).
$collectLog = Join-Path $RunDir "collect.log"
$cmdLines += @(
  "REM ----- UVM snapshotter: stop -----",
  "type nul > `"$snapshotStopFile`"",
  "ping -n 3 127.0.0.1 >NUL",
  "REM ----- COM-pipe reader: stop -----",
  "type nul > `"$comReaderStopFile`"",
  "ping -n 3 127.0.0.1 >NUL"
)
$cmdLines += @(
  "REM ----- Guest dump auto-collect -----",
  "for /D %%i in (`"%TEMP%\zlayer-hcs-hyperv-e2e-*`") do (",
  "  for /D %%j in (`"%%i\uvms\*`") do (",
  "    if exist `"%%j\debug`" xcopy /E /I /Y /Q `"%%j\debug`" `"$RunDir\guest-%%~nj-debug\`" >>`"$collectLog`" 2>&1",
  "    if exist `"%%j\crash`" xcopy /E /I /Y /Q `"%%j\crash`" `"$RunDir\guest-%%~nj-crash\`" >>`"$collectLog`" 2>&1",
  "  )",
  ")"
)
# Write rc + done sentinel using the captured cargo exit code (NOT
# %errorlevel%, which by now reflects whichever teardown command ran last).
$cmdLines += @(
  "echo %CARGO_RC% > `"$rcFile`"",
  "type nul > `"$doneFile`""
)
Set-Content -Path $runCmd -Value $cmdLines -Encoding ascii

# Use WMI Win32_Process.Create for true session-detached spawn.
# Start-Process from an SSH-launched PowerShell ties the child to the SSH job
# object; when SSH disconnects, the child dies. WMI spawns the process under
# the system's process tree, fully detached from the calling session.
$cmdLine = "cmd.exe /c `"$runCmd`""
$result = Invoke-CimMethod -ClassName Win32_Process -MethodName Create `
  -Arguments @{ CommandLine = $cmdLine }

if ($result.ReturnValue -ne 0) {
  throw "Win32_Process.Create failed: ReturnValue=$($result.ReturnValue)"
}
$childPid = $result.ProcessId
Set-Content -Path $pidFile -Value $childPid

Write-Host "PID=$childPid"
Write-Host "RUNDIR=$RunDir"
Write-Host "LOG=$logFile"
