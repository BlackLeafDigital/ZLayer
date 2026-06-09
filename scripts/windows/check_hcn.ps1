#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Dump current HCN state: default namespace, orphan endpoint refs, endpoints,
  zlayer/overlay networks. Read-only.
#>
[CmdletBinding()]
param()

$ns = Get-HnsNamespace | Where-Object { $_.IsDefault -eq $true }
if ($ns) {
  Write-Host ("DefaultNS=" + $ns.ID)
  $orphans = @($ns.Resources | Where-Object { $_.Type -eq "Endpoint" })
  Write-Host ("OrphanEndpointRefs=" + $orphans.Count)
  $orphans | ForEach-Object { Write-Host ("  orphan=" + $_.ID) }
} else {
  Write-Host "DefaultNS=NONE"
}

$eps = @(Get-HnsEndpoint)
Write-Host ("HNSEndpoints=" + $eps.Count)
$eps | ForEach-Object {
  Write-Host ("  EP=" + $_.ID + " name=" + $_.Name + " ip=" + $_.IPAddress)
}

$nets = @(Get-HnsNetwork | Where-Object { $_.Name -like "*overlay*" -or $_.Name -like "zlayer-*" })
Write-Host ("ZlayerNets=" + $nets.Count)
$nets | ForEach-Object {
  Write-Host ("  NET=" + $_.ID + " name=" + $_.Name + " type=" + $_.Type)
}
