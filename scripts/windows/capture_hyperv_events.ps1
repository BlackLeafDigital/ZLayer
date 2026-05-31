#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Dump recent Hyper-V host event-log entries for diagnosing UVM boot / GCS
  failures. Read-only. Prints each channel delimited by a marker line so the
  Python driver (debug_e2e.py) can split + persist them.

.DESCRIPTION
  When a Hyper-V-isolated UVM powers on but the in-guest GCS never dials out,
  the cause (guest bugcheck, missing boot device, worker-process error) shows
  up in the Hyper-V Worker / Compute / VMMS channels, NOT in the test stdout.
  This script surfaces those entries for the run window.

.PARAMETER SinceMinutes
  How far back to look, in minutes. The driver passes the run duration + a
  small buffer.
#>
[CmdletBinding()]
param(
  [int]$SinceMinutes = 60
)

$start = (Get-Date).AddMinutes(-1 * [Math]::Abs($SinceMinutes))

$channels = @(
  'Microsoft-Windows-Hyper-V-Worker-Admin',
  'Microsoft-Windows-Hyper-V-Worker-Operational',
  'Microsoft-Windows-Hyper-V-Compute-Admin',
  'Microsoft-Windows-Hyper-V-Compute-Operational',
  'Microsoft-Windows-Hyper-V-VMMS-Admin',
  'Microsoft-Windows-Hyper-V-VMMS-Operational'
)

Write-Host ("SINCE=" + $start.ToString('o'))

foreach ($ch in $channels) {
  Write-Host ("=== CHANNEL: " + $ch + " ===")
  try {
    $events = Get-WinEvent -FilterHashtable @{ LogName = $ch; StartTime = $start } -ErrorAction Stop |
      Sort-Object TimeCreated
    if (-not $events -or $events.Count -eq 0) {
      Write-Host "(no events in window)"
    } else {
      foreach ($e in $events) {
        $msg = ($e.Message -replace "\r?\n", " ").Trim()
        Write-Host ("[" + $e.TimeCreated.ToString('o') + "] id=" + $e.Id +
          " level=" + $e.LevelDisplayName + " :: " + $msg)
      }
    }
  } catch {
    # No-such-channel / no-matching-events both throw; record and continue so
    # one missing channel never aborts the others.
    Write-Host ("(channel unavailable: " + $_.Exception.Message.Trim() + ")")
  }
}

Write-Host "EVENTS_DONE"
