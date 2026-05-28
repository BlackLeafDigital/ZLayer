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
  [string]$Features = "hcs-runtime,wsl",
  [string]$RunDir   = $null,
  [string]$RepoDir  = "C:\src\ZLayer"
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
$cmdLines = @(
  "@echo off",
  "cd /d `"$RepoDir`"",
  "set RUST_LOG=error",
  "set ZLAYER_HCS_DOC_DUMP_DIR=$dumpDir",
  "set ZLAYER_HCN_DOC_DUMP_DIR=$dumpDir",
  "`"$cargoPath`" $cargoArgs > `"$logFile`" 2>&1",
  "echo %errorlevel% > `"$rcFile`"",
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
