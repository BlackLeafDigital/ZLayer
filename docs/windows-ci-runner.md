# Windows CI Runner + DNS Validation Runbook

Operational reference for the ZLayer self-hosted Windows forgejo runner and the
manual DNS-resolution validation that cannot be covered by unit tests (it
requires real HNS endpoints and nanoserver containers).

## Table of contents

1. [Current runner (MiniWindows)](#1-current-runner-miniwindows)
2. [Using the runner from CI](#2-using-the-runner-from-ci)
3. [Local remote-check script](#3-local-remote-check-script)
4. [Adding another Windows runner (replication runbook)](#4-adding-another-windows-runner-replication-runbook)
5. [Manual DNS resolution validation (Phase J-2)](#5-manual-dns-resolution-validation-phase-j-2)

---

## 1. Current runner (MiniWindows)

- **Host**: `MiniWindows@192.168.68.92`
- **Role**: self-hosted forgejo runner; serves as the `windows-latest` label target for this forgejo instance.
- **Toolchain** (installed; do NOT re-provision):
  - Rust stable + `clippy` component via chocolatey (`rust-ms` package — MSVC toolchain).
  - `protoc` at `C:\ProgramData\chocolatey\bin\protoc.exe`.
  - `uv` for Python provisioning — pyo3-build-config (pulled transitively via `zlayer-py` into `--workspace` builds) needs a Python interpreter at build time, and the SSH session PATH does not include a system Python.
  - WSL2 distro named `zlayer` already imported; used by the composite runtime tests (`crates/zlayer-agent/tests/composite_dispatch_e2e.rs`).
  - Git Bash on PATH so any `shell: bash` step in CI has a real shell and does not fall back to the LocalSystem→WSL path that the Forgejo runner-as-service would otherwise trip over.
- **Known toolchain gaps**:
  - No `x86_64-w64-mingw32-gcc` on the host. `x86_64-pc-windows-gnu` cross-builds break on `aws-lc-sys`. Stay on the host-native `x86_64-pc-windows-msvc` target, which is what `rust-ms` installs.
- **Runner registration**: already registered with forgejo. Labels include `windows-latest` (matches the existing `check-windows` job in `.forgejo/workflows/ci.yaml` and the `build-windows-amd64` job in `.forgejo/workflows/build.yml`). Do **not** re-register — re-registration rotates the token and will leave existing workflows stranded until every `runs-on:` consumer is updated.

## 2. Using the runner from CI

New Windows jobs should mirror the existing convention from
`.forgejo/workflows/ci.yaml` (`check-windows` job, line 120) and
`.forgejo/workflows/build.yml` (`build-windows-amd64`, line 265):

- `runs-on: windows-latest` — the label resolves to MiniWindows in this forgejo instance.
- Required env, set before any `cargo` invocation:
  - `PROTOC=C:/ProgramData/chocolatey/bin/protoc.exe` — otherwise `prost-build` fails to locate `protoc`.
- Prefix cargo commands with `uv run --python 3.12 --` so pyo3-build-config can find a Python interpreter at build time. The `--python 3.12` flag provisions 3.12 on first run and reuses the cached toolchain afterwards.

Example step (matches the pattern in `scripts/windows-remote-check.sh`):

```yaml
- name: Export PROTOC env
  shell: bash
  run: echo "PROTOC=C:/ProgramData/chocolatey/bin/protoc.exe" >> $GITHUB_ENV

- name: Run Windows composite tests
  run: uv run --python 3.12 -- cargo test -p zlayer-agent --features hcs-runtime,wsl --test composite_dispatch_e2e -- --ignored --nocapture
```

The `check-windows` job in `ci.yaml` exports `PROTOC` once via `$GITHUB_ENV` and
then runs four cargo steps (`cargo check` + `cargo clippy`, both for default
features and the `hcs-runtime,wsl` composite) without the `uv run` prefix — that
works today because those steps do not pull the pyo3-dependent crates. Any
workflow that widens to `--workspace --all-features` or that pulls in
`zlayer-py` must add the `uv run` prefix.

## 3. Local remote-check script

`scripts/windows-remote-check.sh` is the authoritative interactive equivalent
of the CI `check-windows` job. It rsyncs the local working tree to the Windows
host and runs the same four cargo check/clippy passes.

```bash
ZLAYER_WIN_HOST=MiniWindows@192.168.68.92 ./scripts/windows-remote-check.sh
```

- Remote path defaults to `/cygdrive/c/src/ZLayer` (maps to `C:\src\ZLayer` for PowerShell). Override with `ZLAYER_WIN_PATH`.
- The script already sets `$env:PROTOC` and wraps every command in `uv run --python 3.12 --`, matching the CI-side convention in section 2.

**Troubleshooting**:

- **rsync exit 12** — transient socket / SSH reset. Retry the command; it is not a content error.
- **SSH appears to hang on compile output** — OpenSSH on Windows does not always flush unbuffered stdout when cargo prints a compile graph. If you just want to spot-check, run a direct SSH command and tail the last few lines (e.g. `ssh $ZLAYER_WIN_HOST "... | Select-Object -Last 40"`).
- **PowerShell is the default remote shell** — the script assumes PowerShell (OpenSSH's Windows default). If the host's DefaultShell registry key was overridden, restore it per section 4 step 2 before running the script.

## 4. Adding another Windows runner (replication runbook)

Step-by-step for provisioning a new Windows host as a ZLayer CI runner. Run
each command in an elevated PowerShell session on the new host unless
otherwise noted.

1. **Install OpenSSH Server** (so the host is reachable for interactive
   debugging and the local remote-check script):

   ```powershell
   Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
   Start-Service sshd
   Set-Service -Name sshd -StartupType 'Automatic'
   ```

2. **Set PowerShell as the default OpenSSH shell** (OpenSSH defaults to
   `cmd.exe`; scripts in this repo assume PowerShell):

   ```powershell
   New-ItemProperty -Path "HKLM:\SOFTWARE\OpenSSH" -Name DefaultShell -Value "C:\Program Files\PowerShell\7\pwsh.exe" -PropertyType String -Force
   ```

3. **Bootstrap Chocolatey**:

   ```powershell
   Set-ExecutionPolicy Bypass -Scope Process -Force
   iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
   ```

4. **Install runtime dependencies** (`rust-ms` is Rust with the MSVC
   toolchain — do NOT install `rust` instead, the GNU toolchain cross-build
   breaks on `aws-lc-sys`):

   ```powershell
   choco install -y rust-ms protoc uv rsync git
   ```

   Confirm the binaries land on PATH: `cargo --version`, `rustc --version`,
   `protoc --version`, `uv --version`, `rsync --version`, `git --version`.

5. **Add Rust components**:

   ```powershell
   rustup component add clippy rustfmt
   ```

6. **Install WSL2** (required for composite-runtime tests — the
   `hcs-runtime,wsl` feature combination and the
   `composite_dispatch_e2e.rs` ignored tests):

   ```powershell
   wsl --install --no-distribution
   wsl --set-default-version 2
   # Import the zlayer distro rootfs (produced by the ZLayer release pipeline):
   wsl --import zlayer C:\wsl\zlayer C:\path\to\zlayer-rootfs.tar --version 2
   ```

7. **Register the forgejo runner** with label `windows-latest` (plus any
   additional labels your new workflows target). Follow the forgejo runner
   registration flow from the forgejo admin UI — copy the registration token
   from **Site Administration → Actions → Runners → Create new Runner** and
   run `forgejo-runner register` per the upstream docs. The label
   `windows-latest` is the existing convention the `check-windows` /
   `build-windows-amd64` jobs target; keep it stable across hosts.

8. **Verify** from a different machine:

   ```bash
   ssh <user>@<new-host> "cargo --version; clippy-driver --version; protoc --version; uv --version; wsl --list --verbose"
   ```

   All five commands must succeed. If any fail, fix PATH / installation
   before announcing the runner as available.

## 5. Manual DNS resolution validation (Phase J-2)

Validates that Windows containers attached to the overlay via an HNS endpoint
can resolve overlay service names through the overlay hickory DNS server. This
procedure exercises the code paths delivered by Phase J-1
(`OverlayManager::attach_container_hcn` wiring DNS into the HNS endpoint
schema, plus `DnsServer::bind_windows_fallback` on port 53 of the overlay
IP) and cannot be replaced by a unit test — it requires real HNS + real
nanoserver containers.

### Prerequisites

- MiniWindows joined to a two-peer cluster (one Linux peer plus MiniWindows).
- A deployment declaring two services:
  - `svc-a` — Linux service, placed on the Linux peer.
  - `svc-b` — Windows service (nanoserver or servercore), placed on MiniWindows.
- The Windows daemon has called `DnsServer::bind_windows_fallback` on port 53 of the overlay IP (the H-1/H-2 bootstrap path does this automatically when an overlay network is present; confirm by inspecting daemon logs for the `bind_windows_fallback` trace).

### Procedure

Run steps 1–4 on the MiniWindows host, step 5 on the Linux peer. Capture
the output of every step in `/tmp/j2-dns-validation-<date>.log` on the
collecting host and attach it to the release PR.

1. On MiniWindows: `zlayer ps` — confirm `svc-b` shows `Running`.
2. `zlayer exec svc-b -- ipconfig /all` — confirm the container has an overlay-range IP and the `DNS Servers` field lists the overlay IP (the same IP the Linux peer uses for its overlay resolver).
3. `zlayer exec svc-b -- nslookup svc-a.overlay.local` — expect the Linux peer's overlay IP for `svc-a` as the answer.
4. `zlayer exec svc-b -- ping -n 1 svc-a.overlay.local` — expect a reply from that IP.
5. On the Linux peer: `zlayer exec svc-a -- nslookup svc-b.overlay.local` and `zlayer exec svc-a -- ping -c 1 svc-b.overlay.local` — expect a reply from MiniWindows's `svc-b` overlay IP. This confirms the reverse path (Linux → Windows).

### Expected failure modes and diagnostics

- **`Server can't find svc-a.overlay.local: NXDOMAIN`** — the overlay DNS server is not running or not authoritative for the `overlay.local` zone. Check `zlayer daemon status` on the Windows host and confirm the port-53 listener is bound (`Get-NetTCPConnection -LocalPort 53` / `Get-NetUDPEndpoint -LocalPort 53`). If nothing is listening, the `bind_windows_fallback` call did not happen — re-verify the overlay bootstrap sequence in daemon logs.
- **`Request timed out`** — DNS resolution succeeded but overlay routing is broken. On the Windows host:
  - `Get-NetAdapter | Where-Object {$_.Name -like 'zl-*'}` — confirm the Wintun adapter is `Up`.
  - `Get-NetRoute -DestinationPrefix <cluster_cidr>` — confirm a route to the cluster CIDR exists via the Wintun adapter.
- **Windows DNS cache holds a stale NXDOMAIN** — Windows caches NXDOMAIN for up to 15 minutes by default. Run `Clear-DnsClientCache` inside the container between attempts (or on the host if resolution happens before container attach).

### Known content gaps

The following specifics in section 5 are filled in from general Windows /
DNS admin knowledge rather than sourced from in-repo code:

- The exact service names `svc-a` / `svc-b` and the `overlay.local` zone suffix are illustrative — the real service names and zone suffix depend on `OverlayManager::dns_domain` configured at daemon startup. Substitute whatever `OverlayManager::dns_domain()` returns for your deployment.
- The 15-minute NXDOMAIN cache TTL is the Windows default (`MaxNegativeCacheTtl` registry key); it is not overridden by ZLayer and is general Windows DNS client behaviour.
- The `Get-NetAdapter` / `Get-NetRoute` / `Get-NetTCPConnection` diagnostics are standard PowerShell cmdlets; the exact interface-name prefix `zl-*` follows the `make_interface_name()` convention in `crates/zlayer-agent/src/overlay_manager.rs`.
