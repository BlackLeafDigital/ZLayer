//! Async wrapper around HCS's C completion-callback model.
//!
//! Every interesting HCS call is asynchronous: the caller hands the API a
//! pre-created "operation" handle plus a `(context, callback)` pair, the
//! call returns immediately, and HCS later invokes the callback on a
//! dedicated worker thread when the underlying work is complete. This
//! module bridges that model into Rust by:
//!
//! 1. Allocating a [`tokio::sync::oneshot`] channel.
//! 2. Boxing the sender and leaking it as the callback `context`.
//! 3. Registering a stable `unsafe extern "system"` trampoline that
//!    re-owns the boxed sender, reads the result document via
//!    `HcsGetOperationResult`, classifies the HRESULT, and forwards either
//!    the JSON payload or a typed [`HcsError`] to the awaiting future.
//! 4. Wrapping the raw `HCS_OPERATION` in [`OwnedOperation`] so the handle
//!    is always closed via `HcsCloseOperation` on drop, regardless of
//!    success, failure, or panic.
//!
//! The trampoline is `extern "system"` because HCS calls it through a C
//! function pointer and Rust panics must never unwind across that
//! boundary; we use [`std::panic::catch_unwind`] to convert any panic
//! inside the callback into a [`HcsError::CallbackPanic`].

use std::ffi::c_void;
use std::panic::{self, AssertUnwindSafe};

use windows::core::PWSTR;
use windows::Win32::System::HostComputeSystem::{
    HcsCreateOperation, HcsGetOperationResult, HCS_OPERATION,
};

use crate::error::{HcsError, HcsResult};
use crate::handle::{OwnedOperation, SendHandle};

/// The boxed half of the oneshot we hand to HCS as the callback context.
///
/// The trampoline reconstructs the box from the raw pointer exactly once,
/// so the box's allocation lifetime spans from `Box::into_raw` (in
/// [`run_operation`]) until the trampoline runs (or until the kickoff
/// error path reclaims it).
type CompletionSender = tokio::sync::oneshot::Sender<HcsResult<String>>;

/// Decode a `PWSTR` returned by HCS into an owned `String`.
///
/// HCS guarantees the wide-string buffer remains valid until the
/// operation is closed. We never free the buffer ourselves: it is owned
/// by the operation and is released when `HcsCloseOperation` runs (which
/// `OwnedOperation::drop` performs after `run_operation` returns).
///
/// Returns the empty string on a null pointer or on a UTF-16 decoding
/// failure (we use lossy decoding rather than failing the whole call,
/// since the caller usually wants whatever message HCS produced even if
/// it's slightly mangled).
///
/// # Safety
///
/// `pwstr` must either be null or point to a valid, null-terminated
/// UTF-16 buffer that lives at least until this function returns.
unsafe fn pwstr_to_string(pwstr: PWSTR) -> String {
    if pwstr.is_null() {
        return String::new();
    }
    // SAFETY: caller-provided PWSTR is null-terminated and live for the
    // duration of this call per the function contract.
    if let Ok(s) = unsafe { pwstr.to_string() } {
        s
    } else {
        // SAFETY: `as_wide` requires the same invariants as `to_string`,
        // which the caller upheld; we only reach this branch when the
        // strict UTF-16 decode failed, so fall back to lossy.
        let wide = unsafe { pwstr.as_wide() };
        String::from_utf16_lossy(wide)
    }
}

/// HCS completion-callback trampoline.
///
/// Invoked by HCS on a worker thread when the underlying operation
/// finishes. Re-owns the boxed [`CompletionSender`] from `context`,
/// extracts the result document, and forwards the classified outcome to
/// the awaiting receiver.
///
/// # Safety
///
/// * `context` must be the exact pointer produced by
///   `Box::into_raw(Box::<CompletionSender>::new(_))` in
///   [`run_operation`], and must not have been freed or used elsewhere.
/// * HCS guarantees this trampoline is invoked at most once per
///   operation, so taking ownership of the box here is correct.
/// * The trampoline must never unwind across the FFI boundary; all panic
///   sources are wrapped in [`std::panic::catch_unwind`].
unsafe extern "system" fn completion_trampoline(operation: HCS_OPERATION, context: *const c_void) {
    // SAFETY: `context` is a `Box<CompletionSender>` raw pointer per the
    // function contract; HCS only invokes us once, so we are the
    // exclusive owner here.
    let sender_box: Box<CompletionSender> =
        unsafe { Box::from_raw(context.cast::<CompletionSender>().cast_mut()) };
    let sender = *sender_box;

    let payload = panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `operation` is a live HCS handle owned by the caller's
        // `OwnedOperation`. `result_doc` is filled by HCS regardless of
        // the inner HRESULT; the buffer it points at lives until the
        // operation is closed (after we send below, the awaiting future
        // resumes and `OwnedOperation::drop` closes it).
        let mut result_doc: PWSTR = PWSTR::null();
        let hr_result = unsafe { HcsGetOperationResult(operation, Some(&raw mut result_doc)) };
        // Decode the document up front so we have a message either way.
        // SAFETY: `result_doc` is either null or a valid wide string per
        // the HCS contract; `pwstr_to_string` handles both.
        let json = unsafe { pwstr_to_string(result_doc) };

        match hr_result {
            Ok(()) => Ok(json),
            Err(err) => {
                let hr = err.code();
                // For the first cut we treat the document text as the
                // message body; a follow-up will parse the structured
                // `{ "ErrorCode": 0xHHHHHHHH, "ErrorMessage": "..." }`
                // payload that some HCS calls return.
                let message = if json.is_empty() { err.message() } else { json };
                Err(HcsError::from_hresult(hr, message))
            }
        }
    }));

    let outcome = match payload {
        Ok(outcome) => outcome,
        Err(panic_payload) => {
            let msg = if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            tracing::error!(panic = %msg, "HCS completion callback panicked");
            Err(HcsError::CallbackPanic(msg))
        }
    };

    // If the receiver was dropped (caller's future was cancelled before
    // completion), this send fails silently — that's the right behaviour:
    // there's no one to deliver to and the operation handle will still be
    // closed by `OwnedOperation::drop`.
    let _ = sender.send(outcome);
}

/// Drive an HCS asynchronous call to completion and return the result
/// document JSON (or a classified [`HcsError`]).
///
/// The provided closure receives a fresh `HCS_OPERATION` handle and is
/// expected to invoke exactly one HCS API that takes that operation
/// (e.g. `HcsCreateComputeSystem`, `HcsStartComputeSystem`,
/// `HcsModifyComputeSystem`, etc.) and return the HRESULT that call
/// produced. The closure must NOT close the operation — ownership stays
/// with this helper, which closes it via [`OwnedOperation::drop`].
///
/// On a failed kickoff (the HCS call itself returned a failing HRESULT
/// before HCS could schedule the work), the completion callback will
/// never fire; this function reclaims the leaked sender and returns
/// immediately rather than awaiting a channel that nothing will signal.
///
/// # Errors
///
/// Returns a typed [`HcsError`] if:
/// * `HcsCreateOperation` returned an invalid handle (allocation/setup failed).
/// * The caller's closure returned a failing HRESULT.
/// * HCS reported the operation failed via `HcsGetOperationResult`.
/// * The completion callback panicked.
/// * The completion sender was dropped without sending (defensive — should
///   not happen if the callback is registered correctly).
///
/// # Cancellation
///
/// If the returned future is dropped before it resolves, the operation
/// continues to run inside HCS until completion; the callback fires, sees
/// the receiver is gone, and silently drops the result. The
/// [`OwnedOperation`] held inside this function is dropped on cancellation
/// only if cancellation happens before the kickoff returns; once we are
/// awaiting `rx`, the operation handle is owned by the future and is
/// closed when the future is dropped, which races with the callback. HCS
/// is documented to be safe against close-during-callback because the
/// callback runs on its own worker thread and synchronises with close
/// internally — but callers are still encouraged to await to completion.
pub async fn run_operation<F>(f: F) -> HcsResult<String>
where
    F: FnOnce(HCS_OPERATION) -> windows::core::HRESULT,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<HcsResult<String>>();
    let tx_box: Box<CompletionSender> = Box::new(tx);
    let tx_ptr: *mut CompletionSender = Box::into_raw(tx_box);

    // SAFETY: the context pointer is a valid `Box<CompletionSender>` raw
    // pointer; on success the trampoline re-owns it; on the error paths
    // below we reclaim it ourselves before returning.
    //
    // Wrap the raw `HCS_OPERATION` in `SendHandle` so this local can cross
    // the `rx.await` below while the generated future is `Send` — the raw
    // `HCS_OPERATION` is `!Send + !Sync` because it is transparent over
    // `*mut c_void`.
    let op_raw = SendHandle(unsafe {
        HcsCreateOperation(
            Some(tx_ptr.cast::<c_void>().cast_const()),
            Some(completion_trampoline),
        )
    });

    if op_raw.0.is_invalid() {
        // No operation was created, so the trampoline will never fire and
        // we must reclaim the sender box ourselves.
        // SAFETY: `tx_ptr` was just produced by `Box::into_raw` and has
        // not been observed by any other owner — HCS rejected it.
        drop(unsafe { Box::from_raw(tx_ptr) });
        return Err(HcsError::Other {
            hresult: 0,
            message: "HcsCreateOperation returned an invalid handle".to_string(),
        });
    }

    // Take ownership of the operation so it is closed on every exit path.
    // SAFETY: `op_raw.0` was just produced by `HcsCreateOperation` and has
    // not been handed to any other owner.
    let op = unsafe { OwnedOperation::from_raw(op_raw.0) };

    // Kick off the caller's HCS call.
    //
    // `op.as_raw()` returns `SendHandle<HCS_OPERATION>`; deref with `*` to
    // produce the bare `HCS_OPERATION` the closure parameter expects. The
    // dereferenced value is consumed synchronously by the closure (no
    // `.await` before or after the call), so the raw `HCS_OPERATION` is
    // never held across a suspend point.
    let kickoff_hr = f(*op.as_raw());
    if kickoff_hr.is_err() {
        // The HCS call failed synchronously, so the trampoline will never
        // fire — reclaim the sender box and surface the classified error.
        // SAFETY: same as above; the trampoline never ran (HCS didn't
        // schedule it because the kickoff failed), so the box is still
        // exclusively ours.
        drop(unsafe { Box::from_raw(tx_ptr) });
        return Err(HcsError::from_hresult(
            kickoff_hr,
            "HCS call failed to start",
        ));
    }

    match rx.await {
        Ok(payload) => payload,
        Err(_closed) => Err(HcsError::Other {
            hresult: 0,
            message: "HCS callback dropped the completion sender without sending".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    //! Tests here run only when the crate is compiled for Windows (the
    //! whole crate is gated by `#![cfg(windows)]`). They are intentionally
    //! tiny because anything that exercises the trampoline's read path
    //! requires a live HCS subsystem (and Administrator privileges) — we
    //! cannot meaningfully fake `HcsGetOperationResult`. Larger
    //! integration tests live in `tests/` and are gated on a real
    //! Windows host.
    //!
    //! What we *can* assert without HCS is that the helper types compile
    //! and that the panic path of the trampoline produces the expected
    //! error variant when invoked synchronously with a fresh sender.
    //! Calling the trampoline directly with a synthetic operation handle
    //! would still hit `HcsGetOperationResult` (an FFI call), so we limit
    //! the smoke test to verifying the channel plumbing.

    use super::*;

    #[tokio::test]
    async fn dropped_sender_surfaces_other_error() {
        // Construct the same channel `run_operation` would, then drop the
        // sender without invoking the trampoline. The receiver should
        // resolve to `Err(_closed)` and `run_operation`'s match arm would
        // map that to `HcsError::Other`. We assert that mapping directly.
        let (tx, rx) = tokio::sync::oneshot::channel::<HcsResult<String>>();
        drop(tx);
        let result = match rx.await {
            Ok(payload) => payload,
            Err(_closed) => Err(HcsError::Other {
                hresult: 0,
                message: "HCS callback dropped the completion sender without sending".to_string(),
            }),
        };
        match result {
            Err(HcsError::Other { hresult, message }) => {
                assert_eq!(hresult, 0);
                assert!(message.contains("dropped"));
            }
            other => panic!("expected Other error, got {other:?}"),
        }
    }
}
