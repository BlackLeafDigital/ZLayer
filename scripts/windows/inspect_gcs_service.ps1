#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Dump every registry value under the offline UtilityVM SYSTEM hive's
  `ControlSet001\Services\gcs` key. The earlier inspector printed only
  Start + ImagePath; the question now is whether `gcs` carries a
  DependOnService / Type / Group / Tag entry that explains why SCM never
  launches it on boot, while a forked invocation of the same binary dials
  the host hvsock just fine.
#>
[CmdletBinding()]
param(
  [string]$UvmFilesRoot = ''
)

if (-not $UvmFilesRoot) {
  $candidates = Get-ChildItem 'C:\Users\MiniWindows\AppData\Local\Temp' `
      -Filter 'zlayer-hcs-*' -Directory -ErrorAction SilentlyContinue
  foreach ($c in $candidates) {
    $found = Get-ChildItem $c.FullName -Recurse -Directory `
        -Filter 'Files' -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -like '*UtilityVM\Files' } |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1
    if ($found) { $UvmFilesRoot = $found.FullName; break }
  }
}

if (-not $UvmFilesRoot -or -not (Test-Path $UvmFilesRoot)) {
  Write-Host "ROOT_MISSING UvmFilesRoot='$UvmFilesRoot'"
  exit 1
}
Write-Host ("ROOT=" + $UvmFilesRoot)

$hive = Join-Path $UvmFilesRoot 'Windows\System32\config\SYSTEM'
if (-not (Test-Path $hive)) {
  Write-Host "HIVE_MISSING $hive"
  exit 1
}

$mount = 'HKLM\ZLayerOfflineSystem'
& reg.exe load $mount $hive 2>$null
if ($LASTEXITCODE -ne 0) {
  Write-Host "REG_LOAD_FAILED rc=$LASTEXITCODE"
  exit 1
}

try {
  $svcPath = "Registry::$mount\ControlSet001\Services\gcs"
  if (-not (Test-Path $svcPath)) {
    Write-Host "GCS_KEY_MISSING $svcPath"
    exit 1
  }
  Write-Host '--- gcs service: ALL values ---'
  $props = Get-ItemProperty -Path $svcPath -ErrorAction SilentlyContinue
  $props.PSObject.Properties |
    Where-Object { $_.Name -notmatch '^PS(Path|ParentPath|ChildName|Drive|Provider)$' } |
    ForEach-Object { Write-Host ("  " + $_.Name + " = " + $_.Value) }

  Write-Host '--- gcs service: subkeys ---'
  $sub = Get-ChildItem -Path $svcPath -ErrorAction SilentlyContinue
  if ($sub) {
    foreach ($k in $sub) {
      Write-Host ("  subkey: " + $k.PSChildName)
      $subProps = Get-ItemProperty -Path $k.PSPath -ErrorAction SilentlyContinue
      $subProps.PSObject.Properties |
        Where-Object { $_.Name -notmatch '^PS(Path|ParentPath|ChildName|Drive|Provider)$' } |
        ForEach-Object { Write-Host ("    " + $_.Name + " = " + $_.Value) }
    }
  } else {
    Write-Host "  (no subkeys)"
  }
} finally {
  & reg.exe unload $mount 2>$null | Out-Null
}

Write-Host 'INSPECT_GCS_DONE'
