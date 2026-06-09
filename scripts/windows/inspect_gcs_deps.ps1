#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Check whether each of `gcs`'s declared DependOnService entries (condrv,
  hvsocketcontrol, mpssvc, netsetupsvc) actually exists in the offline
  nanoserver:ltsc2022 UVM SYSTEM hive. Any missing dep stops SCM from ever
  launching `gcs`, which explains why the GCS never dials.
#>
[CmdletBinding()]
param([string]$UvmFilesRoot = '')

if (-not $UvmFilesRoot) {
  $cands = Get-ChildItem 'C:\Users\MiniWindows\AppData\Local\Temp' `
    -Filter 'zlayer-hcs-*' -Directory -ErrorAction SilentlyContinue
  foreach ($c in $cands) {
    $found = Get-ChildItem $c.FullName -Recurse -Directory -Filter 'Files' -EA SilentlyContinue |
      Where-Object { $_.FullName -like '*UtilityVM\Files' } |
      Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($found) { $UvmFilesRoot = $found.FullName; break }
  }
}
$hive = Join-Path $UvmFilesRoot 'Windows\System32\config\SYSTEM'
$mount = 'HKLM\ZLayerOfflineSystem'
& reg.exe load $mount $hive 2>$null

try {
  $deps = @('condrv','hvsocketcontrol','mpssvc','netsetupsvc')
  foreach ($d in $deps) {
    $p = "Registry::$mount\ControlSet001\Services\$d"
    if (Test-Path $p) {
      $props = Get-ItemProperty -Path $p
      $start = $props.Start
      $imgPath = $props.ImagePath
      $depOn = $props.DependOnService -join ','
      Write-Host ("PRESENT  " + $d + "  Start=" + $start + "  Img=" + $imgPath + "  DependOn=" + $depOn)
    } else {
      Write-Host ("ABSENT   " + $d)
    }
  }
} finally {
  & reg.exe unload $mount 2>$null | Out-Null
}
Write-Host 'INSPECT_DEPS_DONE'
