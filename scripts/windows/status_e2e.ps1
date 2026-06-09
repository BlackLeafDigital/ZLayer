#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Check status of a detached cargo test launched by launch_e2e.ps1.

.PARAMETER Test
  Test name used at launch (matches the RunDir basename).

.PARAMETER RunDir
  Optional explicit run dir. Defaults to %USERPROFILE%\.zlayer-tests\<Test>.

.PARAMETER Tail
  How many trailing lines of stdout.log to print. Default 30.

.OUTPUTS
  Lines:
    STATE=DONE|RUNNING|MISSING
    RC=<n>                    (if done)
    PID=<n>                   (if pid file exists)
    ALIVE=true|false          (if pid file exists)
    LOGSIZE=<bytes>           (if log exists)
    LOGLASTWRITE=<iso ts>
    --tail--
    <last N lines of log>
#>
[CmdletBinding()]
param(
  [string]$Test   = "composite_dispatch_e2e",
  [string]$RunDir = $null,
  [int]$Tail      = 30
)

if (-not $RunDir) {
  $RunDir = Join-Path $env:USERPROFILE ".zlayer-tests\$Test"
}

if (-not (Test-Path $RunDir)) {
  Write-Host "STATE=MISSING"
  Write-Host "RUNDIR=$RunDir"
  exit 0
}

$logFile  = Join-Path $RunDir "stdout.log"
$rcFile   = Join-Path $RunDir "rc"
$doneFile = Join-Path $RunDir "done"
$pidFile  = Join-Path $RunDir "pid"

if (Test-Path $doneFile) {
  Write-Host "STATE=DONE"
  if (Test-Path $rcFile) {
    $rc = (Get-Content $rcFile -Raw).Trim()
    Write-Host "RC=$rc"
  }
} else {
  Write-Host "STATE=RUNNING"
}

if (Test-Path $pidFile) {
  $procPid = (Get-Content $pidFile -Raw).Trim()
  Write-Host "PID=$procPid"
  $p = Get-Process -Id $procPid -ErrorAction SilentlyContinue
  if ($p) { Write-Host "ALIVE=true" } else { Write-Host "ALIVE=false" }
}

if (Test-Path $logFile) {
  $log = Get-Item $logFile
  Write-Host "LOGSIZE=$($log.Length)"
  Write-Host "LOGLASTWRITE=$($log.LastWriteTime.ToString('o'))"
  Write-Host "--tail--"
  Get-Content $logFile -Tail $Tail
} else {
  Write-Host "LOGSIZE=0"
}
