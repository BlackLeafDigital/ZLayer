#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Aggressive cleanup of disk + HCS state on the Windows test host.

.DESCRIPTION
  Each failed e2e run leaves behind:
   - a scratch VHDX (hundreds of MB to multiple GB) under
     C:\ProgramData\ZLayer\tmp\zlayer-composite-e2e\<storage_suffix>\
   - stale HCS compute systems (the runtime never reached its terminate path)
   - leftover HCN endpoints/networks
   - day-old unpacked image dirs under C:\ProgramData\ZLayer\images\
  These accumulate and can fill a 512 GB disk in a few dozen iterations
  (each `win-dispatch` run alone is ~5 GB of scratch). Run this between
  debug rounds — idempotent, safe to repeat.

.NOTES
  Does NOT touch the cargo target/ directory (deleting it costs a 30-min
  recompile). For that, run `cargo clean` explicitly when you need it.
#>
$ErrorActionPreference = "Continue"

Write-Host "--- Stopping all containerd Windows containers ---"
$ctr = "C:\Program Files\containerd\ctr.exe"
if (Test-Path $ctr) {
  & $ctr -n windows task ls -q 2>$null | ForEach-Object {
    if ($_.Trim()) {
      Write-Host "killing task $_"
      & $ctr -n windows task kill -s SIGKILL $_ 2>$null
      & $ctr -n windows task delete --force $_ 2>$null
    }
  }
  & $ctr -n windows container ls -q 2>$null | ForEach-Object {
    if ($_.Trim()) {
      Write-Host "removing container $_"
      & $ctr -n windows container delete $_ 2>$null
    }
  }
}

Write-Host "--- Terminating stale HCS compute systems (non-WSL) ---"
$systems = hcsdiag list -raw 2>$null | ConvertFrom-Json
foreach ($s in $systems) {
  if ($s.SystemType -eq "Container") {
    Write-Host "terminating $($s.Id)"
    hcsdiag kill $s.Id 2>$null
  }
}

Write-Host "--- Purging test scratch dirs ---"
$tmpRoot = "C:\ProgramData\ZLayer\tmp\zlayer-composite-e2e"
if (Test-Path $tmpRoot) {
  $before = (Get-ChildItem $tmpRoot -Recurse -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
  Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
  $freedGB = if ($before) { [math]::Round($before/1GB, 2) } else { 0 }
  Write-Host "freed $freedGB GB from $tmpRoot"
}

Write-Host "--- Purging old image unpack dirs (older than 1 day) ---"
$imgRoot = "C:\ProgramData\ZLayer\images"
if (Test-Path $imgRoot) {
  $beforeImg = (Get-ChildItem $imgRoot -Recurse -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
  Get-ChildItem $imgRoot -Directory -Recurse -ErrorAction SilentlyContinue | Where-Object {
    $_.LastWriteTime -lt (Get-Date).AddDays(-1)
  } | ForEach-Object {
    Write-Host "removing old image dir: $($_.FullName)"
    Remove-Item -Recurse -Force $_.FullName -ErrorAction SilentlyContinue
  }
  $afterImg = (Get-ChildItem $imgRoot -Recurse -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
  $freedGB = if ($beforeImg -and $afterImg) { [math]::Round(($beforeImg - $afterImg)/1GB, 2) } else { 0 }
  Write-Host "freed $freedGB GB from $imgRoot"
}

Write-Host "--- Cleanup HCN leftovers ---"
Get-HnsEndpoint -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "zlayer-*" } | ForEach-Object {
  Write-Host "removing endpoint $($_.ID) $($_.Name)"
  $_ | Remove-HnsEndpoint -ErrorAction SilentlyContinue
}
Get-HnsNetwork -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*overlay*" -or $_.Name -like "zlayer-*" } | ForEach-Object {
  Write-Host "removing network $($_.ID) $($_.Name)"
  $_ | Remove-HnsNetwork -ErrorAction SilentlyContinue
}

Write-Host "--- Final disk state ---"
Get-PSDrive C | Select-Object @{N='UsedGB';E={[math]::Round($_.Used/1GB,2)}}, @{N='FreeGB';E={[math]::Round($_.Free/1GB,2)}} | Format-Table -AutoSize | Out-String
Write-Host "DONE"
