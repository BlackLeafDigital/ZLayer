#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Inspect a UtilityVM Files directory for the in-guest Windows GCS binary and
  related host-side guest services. Used to diagnose why a Hyper-V utility VM
  boots successfully but the in-guest GCS never dials the host.

.PARAMETER UvmFilesRoot
  Absolute path to a `<image>\UtilityVM\Files` directory. If omitted, the
  script searches under `C:\Users\MiniWindows\AppData\Local\Temp\zlayer-hcs-*`
  for the most recent one.
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

Write-Host '--- gcs.exe locations (recursive) ---'
$gcs = Get-ChildItem -Path $UvmFilesRoot -Recurse -Filter 'gcs.exe' `
    -ErrorAction SilentlyContinue
if (-not $gcs) {
  Write-Host '(no gcs.exe found)'
} else {
  $gcs | ForEach-Object { Write-Host ("  " + $_.FullName + "  size=" + $_.Length) }
}

Write-Host '--- *gcs* in Windows\System32 ---'
$sys32 = Join-Path $UvmFilesRoot 'Windows\System32'
if (Test-Path $sys32) {
  Get-ChildItem -Path $sys32 -Filter '*gcs*' -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Host ("  " + $_.Name + "  size=" + $_.Length) }
} else {
  Write-Host "(no Windows\System32 under $UvmFilesRoot)"
}

Write-Host '--- vmcompute / containerplat / hostsvc / hcsutils ---'
foreach ($pat in @('vmcompute*','containerplat*','hostsvc*','hcsutils*','vmwp*','vmms*')) {
  Get-ChildItem -Path $sys32 -Filter $pat -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Host ("  " + $_.Name + "  size=" + $_.Length) }
}

Write-Host '--- SYSTEM hive Services keys with "gcs" in name ---'
$hive = Join-Path $UvmFilesRoot 'Windows\System32\config\SYSTEM'
if (Test-Path $hive) {
  $mount = 'HKLM\ZLayerOfflineSystem'
  $loadOk = $false
  & reg.exe load $mount $hive 2>$null
  if ($LASTEXITCODE -eq 0) {
    $loadOk = $true
    try {
      $services = Get-ChildItem -Path ("Registry::" + $mount + "\ControlSet001\Services") `
          -ErrorAction SilentlyContinue
      $svcMatches = $services | Where-Object {
        $_.PSChildName -match 'gcs|vmcompute|containerplat|hostsvc|hcsguest'
      }
      if (-not $svcMatches) {
        Write-Host '  (no service key names matching gcs/vmcompute/containerplat/hostsvc/hcsguest)'
      } else {
        foreach ($s in $svcMatches) {
          $props = Get-ItemProperty -Path $s.PSPath -ErrorAction SilentlyContinue
          $start = if ($null -ne $props.Start) { $props.Start } else { '?' }
          $imgPath = $props.ImagePath
          Write-Host ("  Service=" + $s.PSChildName + "  Start=" + $start +
            "  ImagePath=" + $imgPath)
        }
      }
    } finally {
      & reg.exe unload $mount 2>$null | Out-Null
    }
  } else {
    Write-Host "  (reg load failed, hive may be in use or locked)"
  }
} else {
  Write-Host "  (no SYSTEM hive at $hive)"
}

Write-Host 'INSPECT_DONE'
