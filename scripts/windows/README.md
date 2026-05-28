# Windows test-host helpers

PowerShell scripts that run on the MiniWindows test box (`192.168.68.92`,
`C:\src\ZLayer`). Rsync'd along with the code; invoke over SSH.

## Files

| Script             | Purpose                                                                 |
|--------------------|-------------------------------------------------------------------------|
| `launch_e2e.ps1`   | Start a `cargo test` in a detached `cmd.exe`. Survives SSH disconnect.  |
| `status_e2e.ps1`   | Read sentinel + rc + tail the log for a launched test.                  |
| `cleanup_hcn.ps1`  | Idempotent purge of leftover `zlayer-*` HCN endpoints and overlay nets. |
| `check_hcn.ps1`    | Read-only dump of HCN default namespace + endpoints + zlayer networks.  |

## Typical flow

```
# from Linux dev box
rsync -az --delete --exclude=target --exclude=.git . \
  MiniWindows@192.168.68.92:C:/src/ZLayer/

ssh MiniWindows@192.168.68.92 'powershell -NoProfile -ExecutionPolicy Bypass \
  -File C:/src/ZLayer/scripts/windows/cleanup_hcn.ps1'

ssh MiniWindows@192.168.68.92 'powershell -NoProfile -ExecutionPolicy Bypass \
  -File C:/src/ZLayer/scripts/windows/launch_e2e.ps1 -Test composite_dispatch_e2e'

# poll
ssh MiniWindows@192.168.68.92 'powershell -NoProfile -ExecutionPolicy Bypass \
  -File C:/src/ZLayer/scripts/windows/status_e2e.ps1 -Test composite_dispatch_e2e -Tail 60'
```

`launch_e2e.ps1` writes everything to `%USERPROFILE%\.zlayer-tests\<test>\`:

- `stdout.log`   — combined stdout+stderr from cargo
- `rc`           — exit code (written on completion)
- `done`         — sentinel file (created on completion)
- `pid`          — wrapper `cmd.exe` PID

The wrapper is `cmd.exe`, not PowerShell — PowerShell `Start-Process
-WindowStyle Hidden` does not reliably inherit rustup's PATH addition.
`cmd.exe` does, and the sentinel is always written even if `cargo` itself
crashes.
