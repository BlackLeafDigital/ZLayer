#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Search VmComputeAgent.exe (the in-guest GCS in nanoserver UtilityVM) for the
  hvsock service GUIDs hcsshim uses, to determine whether this build dials the
  modern `acef5661-...` service ID or some other one.
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

$bin = Join-Path $UvmFilesRoot 'Windows\System32\VmComputeAgent.exe'
if (-not (Test-Path $bin)) {
  Write-Host "BIN_MISSING $bin"
  exit 1
}
Write-Host ("BIN=" + $bin)

# Read the binary as bytes. The GUIDs may be embedded as 16-byte little-endian
# binary (.data section) — the form ML/Windows code-gens for static GUIDs.
# We search for the LE byte pattern of each candidate GUID.
$bytes = [System.IO.File]::ReadAllBytes($bin)
$len = $bytes.Length
Write-Host ("SIZE=" + $len)

function Find-GuidLE([byte[]]$haystack, [string]$guidString, [string]$label) {
  $g = [System.Guid]::Parse($guidString)
  $needle = $g.ToByteArray()
  $needleLen = $needle.Length
  $hits = @()
  $end = $haystack.Length - $needleLen
  for ($i = 0; $i -le $end; $i++) {
    $match = $true
    for ($j = 0; $j -lt $needleLen; $j++) {
      if ($haystack[$i + $j] -ne $needle[$j]) { $match = $false; break }
    }
    if ($match) { $hits += $i }
  }
  if ($hits.Count -eq 0) {
    Write-Host ("MISS " + $label + " " + $guidString)
  } else {
    Write-Host ("HIT  " + $label + " " + $guidString + " count=" + $hits.Count +
      " offsets=" + ($hits | ForEach-Object { "0x{0:X}" -f $_ }) -join ',')
  }
}

# hcsshim's well-known GUIDs for Windows GCS:
Find-GuidLE $bytes 'acef5661-84a1-4e44-856b-6245e69f4620' 'WindowsGcsHvsockServiceID'
Find-GuidLE $bytes '894cc2d6-9d79-424f-93fe-42969ae6d8d1' 'WindowsGcsHvHostID'
Find-GuidLE $bytes 'ae8da506-a019-4553-a52b-902bc0fa0411' 'WindowsSidecarGcsHvsockServiceID'
Find-GuidLE $bytes '172dad59-976d-45f2-8b6c-6d1b13f2ac4d' 'WindowsLoggingHvsockServiceID'

# Also check for the well-known Hyper-V partition GUIDs:
Find-GuidLE $bytes 'a42e7cda-d03f-480c-9cc2-a4de20abb878' 'HV_GUID_PARENT'
Find-GuidLE $bytes 'e0e16197-dd56-4a10-9195-5ee7a155a838' 'HV_GUID_LOOPBACK'
Find-GuidLE $bytes '00000000-0000-0000-0000-000000000000' 'HV_GUID_ZERO'

Write-Host 'GUID_SCAN_DONE'
