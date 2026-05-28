#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Purge leftover zlayer-* HCN endpoints and overlay networks. Idempotent.

.NOTES
  Run BEFORE a test that creates HCN state, OR AFTER one whose own cleanup
  was interrupted (e.g. SSH disconnect mid-run). Does NOT touch:
   - the host Ethernet endpoint
   - the singleton HCN default namespace (910F7D92-...)
   - non-zlayer networks
#>
[CmdletBinding()]
param(
  [string]$EndpointPattern = "zlayer-*",
  [string]$NetworkPattern  = "*overlay*"
)

$removed = 0
Get-HnsEndpoint | Where-Object { $_.Name -like $EndpointPattern } | ForEach-Object {
  Write-Host ("Removing endpoint " + $_.ID + " " + $_.Name)
  $_ | Remove-HnsEndpoint
  $removed++
}
Get-HnsNetwork | Where-Object { $_.Name -like $NetworkPattern -or $_.Name -like "zlayer-*" } | ForEach-Object {
  Write-Host ("Removing network " + $_.ID + " " + $_.Name)
  $_ | Remove-HnsNetwork
  $removed++
}
Write-Host "REMOVED=$removed"
