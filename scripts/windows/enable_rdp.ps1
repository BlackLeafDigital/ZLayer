#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Enable Windows RDP (Remote Desktop) on MiniWindows so the user (from a Mac)
  and Claude (from this Linux PC via xfreerdp) can attach a graphical session
  to drive Hyper-V Manager + vmconnect for live UVM diagnosis.

.DESCRIPTION
  Idempotent:
  - Sets fDenyTSConnections=0 (allows RDP).
  - Forces Network Level Authentication on (UserAuthentication=1).
  - Enables the Windows Defender Firewall "Remote Desktop" rule group.
  - Adds -RdpUser (default: current SSH user `MiniWindows`) to the
    Remote Desktop Users group.
  - Verifies TermService is running and that port 3389 is listening.

  Does NOT set or change any password — Windows requires the account to
  already have one for RDP. If the target user has no password the script
  emits PWD_MISSING=<user> and exits non-zero so the caller can prompt for
  one out-of-band rather than baking a password into a script.

.PARAMETER RdpUser
  Local account that should be permitted to RDP in. Defaults to MiniWindows.
#>
[CmdletBinding()]
param(
  [string]$RdpUser = 'MiniWindows'
)

Write-Host '--- Enabling RDP ---'
Set-ItemProperty -Path 'HKLM:\System\CurrentControlSet\Control\Terminal Server' `
    -Name 'fDenyTSConnections' -Value 0 -Type DWord
Write-Host '  fDenyTSConnections=0 set'

Set-ItemProperty -Path 'HKLM:\System\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp' `
    -Name 'UserAuthentication' -Value 1 -Type DWord
Write-Host '  UserAuthentication=1 (NLA) set'

Write-Host '--- Firewall ---'
Enable-NetFirewallRule -DisplayGroup 'Remote Desktop' -ErrorAction SilentlyContinue
Write-Host '  "Remote Desktop" firewall group enabled'

Write-Host ('--- Adding {0} to Remote Desktop Users ---' -f $RdpUser)
try {
  Add-LocalGroupMember -Group 'Remote Desktop Users' -Member $RdpUser -ErrorAction Stop
  Write-Host '  added'
} catch {
  $msg = $_.Exception.Message
  if ($msg -match 'is already a member') {
    Write-Host '  already a member'
  } else {
    Write-Host ('  add failed: ' + $msg)
  }
}

Write-Host '--- Service status ---'
$svc = Get-Service TermService
Write-Host ('  TermService.Status = ' + $svc.Status + '  StartType=' + $svc.StartType)
if ($svc.Status -ne 'Running') {
  Start-Service TermService
  Write-Host '  TermService started'
}

Write-Host '--- Listening port 3389 ---'
$listening = Get-NetTCPConnection -LocalPort 3389 -State Listen -ErrorAction SilentlyContinue
if ($listening) {
  $listening | ForEach-Object {
    Write-Host ('  LISTEN ' + $_.LocalAddress + ':' + $_.LocalPort)
  }
} else {
  Write-Host '  (nothing listening on 3389 yet — TermService may still be starting)'
}

Write-Host ('--- Password check for {0} ---' -f $RdpUser)
$account = Get-LocalUser -Name $RdpUser -ErrorAction SilentlyContinue
if (-not $account) {
  Write-Host ('  USER_MISSING=' + $RdpUser)
  exit 1
}
# PasswordRequired tells us whether the account *must* have a password — but
# does NOT tell us whether one is actually set. A more reliable signal is
# `LastPasswordSet`: if it is $null and the account is enabled, RDP will be
# rejected with 0x80004005-style errors at the credssp stage.
$lastSet = $account.PasswordLastSet
if (-not $lastSet) {
  Write-Host ('  PWD_MISSING=' + $RdpUser +
    '  — set a password (`net user ' + $RdpUser + ' <pw>`) or run this script with a different -RdpUser before RDP will accept the account.')
  Write-Host 'ENABLE_RDP_DONE_NEEDS_PASSWORD'
  exit 2
}
Write-Host ('  PasswordLastSet=' + $lastSet.ToString('o'))

Write-Host 'ENABLE_RDP_DONE'
