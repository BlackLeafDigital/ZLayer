//! Host-side receiver for the in-guest GCS log-forward stream (`windows-debug`).
//!
//! # What this mirrors in hcsshim
//!
//! hcsshim makes the in-guest GCS forward its OWN log to the host in three
//! coordinated steps (ground-truthed against `microsoft/hcsshim@main`):
//!
//! 1. **UVM doc** — `internal/uvm/create_wcow.go::prepareCommonConfigDoc`
//!    adds, when `opts.LogForwardingEnabled` (default `true` for WCOW), a
//!    `Devices.HvSocket.HvSocketConfig.ServiceTable` entry keyed by
//!    `prot.WindowsLoggingHvsockServiceID.String()`
//!    (`172dad59-976d-45f2-8b6c-6d1b13f2ac4d`) with:
//!    ```text
//!    AllowWildcardBinds:        true
//!    BindSecurityDescriptor:    "D:P(A;;FA;;;SY)(A;;FA;;;BA)"
//!    ConnectSecurityDescriptor: "D:P(A;;FA;;;SY)(A;;FA;;;BA)"
//!    ```
//!    ZLayer already emits this exact entry in
//!    `zlayer-agent/src/runtimes/hcs.rs::build_virtual_machine_doc`.
//!
//! 2. **Host listener** — `internal/uvm/create_wcow.go::makeUtilityVM` binds
//!    `uvm.outputListener = winio.ListenHvsock(&winio.HvsockAddr{VMID:
//!    uvm.RuntimeID(), ServiceID: prot.WindowsLoggingHvsockServiceID})`
//!    BEFORE start. After `hcsSystem.Start`, `internal/uvm/start.go` accepts
//!    on that listener and runs `uvm.outputHandler(conn)` =
//!    `vmutils.ParseGCSLogrus(opts.ID)`, which JSON-decodes a STREAM of
//!    newline-delimited logrus records (`{"time","level","msg",...}`) and
//!    re-emits them into the host logger. **This is the load-bearing piece
//!    for us: the ETW provider `microsoft.windows.logforwardservice.provider`
//!    is emitted by hcsshim's OWN host process via its logrus→ETW sink — it is
//!    NOT produced by the guest. A host that does not itself listen on the
//!    logging hvsock receives nothing.** Hence we re-implement the listener
//!    here rather than relying on the ETW provider firing for free.
//!
//! 3. **RPC trigger** — `internal/uvm/log_wcow.go::SetLogSources` +
//!    `StartLogForwarding`, gated on
//!    `gcs.GetWCOWCapabilities(...).IsLogForwardingSupported()`, issue a GCS
//!    `ModifyServiceSettings` RPC to service `prot.LogForwardService`
//!    (`"LogForwardService"`) with body
//!    `guestrequest.LogForwardServiceRPCRequest{RPCType, Settings}` where
//!    `RPCType` is `"StartLogForwarding"` / `"ModifyServiceSettings"` /
//!    `"StopLogForwarding"`. The agent issues this over the bridge after a
//!    successful negotiate.
//!
//! # What this module provides
//!
//! [`LogForwardListener`] — a host hvsock listener bound on
//! `(uvm_runtime_id, WINDOWS_LOGGING_HVSOCK_SERVICE_ID)`. It must be created
//! BEFORE `HcsStartComputeSystem` (the guest may dial as soon as it boots).
//! The caller [`LogForwardListener::spawn`]s the accept/read loop; each
//! received chunk is appended to `gcs-forward.log` in the UVM's debug dir (so
//! `read_uvm_debug_dump` collects it) and echoed to stderr via the same
//! `gcs_debug!`-style channel the bridge uses.

#![cfg(all(target_os = "windows", feature = "windows-debug"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Notify;

use crate::error::GcsResult;
use crate::transport::{HvSockListener, WINDOWS_LOGGING_HVSOCK_SERVICE_ID};

/// Max bytes pulled per `recv` off the log-forward connection. Logrus records
/// are small; 64 KiB amortizes syscalls without unbounded buffering.
const READ_CHUNK: usize = 64 * 1024;

/// Host-side receiver for the in-guest GCS log-forward hvsock stream.
///
/// Bind via [`LogForwardListener::bind`] BEFORE the UVM is started, then
/// [`LogForwardListener::spawn`] the background accept/read loop. Drop (or call
/// [`LogForwardListener::stop`]) on teardown to wake the loop and close the
/// listener.
pub struct LogForwardListener {
    listener: HvSockListener,
    /// Host-side sink path (`<debug_dir>\gcs-forward.log`). Forwarded records
    /// are appended here so `read_uvm_debug_dump` folds them into the
    /// CreateFailed error on an accept timeout.
    sink_path: PathBuf,
    /// Tripped on [`LogForwardListener::stop`] / drop to unblock the loop.
    stop: Arc<Notify>,
}

impl LogForwardListener {
    /// Bind the host listener on `(uvm_runtime_id,
    /// WINDOWS_LOGGING_HVSOCK_SERVICE_ID)`. Mirrors hcsshim's
    /// `create_wcow.go::makeUtilityVM` (`uvm.outputListener =
    /// winio.ListenHvsock{VMID: uvm.RuntimeID(), ServiceID:
    /// WindowsLoggingHvsockServiceID}`). Must be called BEFORE
    /// `HcsStartComputeSystem` so the host is listening when the guest
    /// log-forward service first dials.
    pub async fn bind(uvm_runtime_id: windows::core::GUID, debug_dir: &Path) -> GcsResult<Self> {
        let listener =
            HvSockListener::bind(uvm_runtime_id, WINDOWS_LOGGING_HVSOCK_SERVICE_ID).await?;
        Ok(Self {
            listener,
            sink_path: debug_dir.join("gcs-forward.log"),
            stop: Arc::new(Notify::new()),
        })
    }

    /// Spawn the background accept/read loop. The loop accepts the guest's
    /// log-forward connection, drains it chunk-by-chunk until EOF, then loops
    /// to accept a possible reconnection (hcsshim's Windows output handler
    /// likewise tolerates the logging service restarting). Each chunk is
    /// appended to `gcs-forward.log` and echoed to stderr. The loop exits when
    /// [`LogForwardListener::stop`] is signalled or the listener errors.
    pub fn spawn(&self) {
        let listener = self.listener.clone();
        let sink_path = self.sink_path.clone();
        let stop = Arc::clone(&self.stop);
        tokio::spawn(async move {
            eprintln!(
                "[t=+{}us] gcs-logfwd: listening on WindowsLoggingHvsockServiceID -> {}",
                crate::diagnostics::ts_us(),
                sink_path.display(),
            );
            loop {
                tokio::select! {
                    () = stop.notified() => {
                        eprintln!(
                            "[t=+{}us] gcs-logfwd: stop signalled; exiting accept loop",
                            crate::diagnostics::ts_us(),
                        );
                        break;
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok(stream) => {
                                eprintln!(
                                    "[t=+{}us] gcs-logfwd: guest log connection accepted",
                                    crate::diagnostics::ts_us(),
                                );
                                // Drain this connection until EOF/error. A
                                // clean close returns to accept() for a
                                // possible reconnect.
                                loop {
                                    tokio::select! {
                                        () = stop.notified() => return,
                                        chunk = stream.read_some(READ_CHUNK) => {
                                            match chunk {
                                                Ok(bytes) if bytes.is_empty() => break, // EOF
                                                Ok(bytes) => append_and_echo(&sink_path, &bytes),
                                                Err(e) => {
                                                    eprintln!(
                                                        "[t=+{}us] gcs-logfwd: read error: {e}",
                                                        crate::diagnostics::ts_us(),
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                // Listener closed (stop/drop) or hard error;
                                // either way the loop is done.
                                eprintln!(
                                    "[t=+{}us] gcs-logfwd: accept ended: {e}",
                                    crate::diagnostics::ts_us(),
                                );
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Signal the background loop to exit and stop accepting. Idempotent.
    pub fn stop(&self) {
        // Wake every waiter (the outer accept select and any in-flight drain
        // select) so the loop unwinds promptly.
        self.stop.notify_waiters();
    }
}

impl Drop for LogForwardListener {
    fn drop(&mut self) {
        // Best-effort wake so a dropped listener doesn't leave the spawned
        // loop parked on accept(). The underlying HvSockListener's own Drop
        // closes the socket, which also forces accept() to return an error.
        self.stop.notify_waiters();
    }
}

/// Append a forwarded log chunk to the host sink and echo it to stderr. The
/// guest writes newline-delimited JSON logrus records; we store the raw bytes
/// verbatim (no parsing) so the host-side reader / `read_uvm_debug_dump` sees
/// exactly what the guest GCS emitted. Best-effort: a sink write failure is
/// logged but never propagated (losing a log line must not affect teardown).
fn append_and_echo(sink_path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let text = String::from_utf8_lossy(bytes);
    eprintln!(
        "[t=+{}us] gcs-logfwd: guest <<< {}",
        crate::diagnostics::ts_us(),
        text.trim_end(),
    );
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sink_path)
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(bytes) {
                eprintln!(
                    "[t=+{}us] gcs-logfwd: sink write failed: {e}",
                    crate::diagnostics::ts_us(),
                );
            }
        }
        Err(e) => {
            eprintln!(
                "[t=+{}us] gcs-logfwd: sink open failed ({}): {e}",
                crate::diagnostics::ts_us(),
                sink_path.display(),
            );
        }
    }
}
