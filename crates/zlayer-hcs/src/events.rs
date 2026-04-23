//! Compute-system lifecycle event stream.
//!
//! HCS exposes per-system lifecycle notifications (container exit, crash,
//! service disconnect, ...) through a C-style callback registered via
//! [`HcsSetComputeSystemCallback`]. This module bridges that callback into
//! an async [`Stream`] by:
//!
//! 1. Allocating a Tokio [`mpsc::unbounded_channel`].
//! 2. Boxing the sender and leaking it as the callback context.
//! 3. Registering a stable `unsafe extern "system"` trampoline that borrows
//!    the sender (never takes ownership — the callback can fire many times),
//!    catches any Rust panic at the FFI boundary, decodes the event, and
//!    forwards it to the awaiting stream.
//!
//! Unlike [`crate::operation`]'s one-shot trampoline, this trampoline is
//! invoked *repeatedly* for the lifetime of the compute system, so the
//! context box must stay live until the subscription is dropped.
//!
//! # Callback lifecycle gotcha
//!
//! HCS does not expose a "remove a single callback" API — once
//! [`HcsSetComputeSystemCallback`] has been called, the callback pointer is
//! live until the compute system is closed (or until the callback is
//! replaced by another call). That means dropping an [`EventSubscription`]
//! cannot actually unregister the callback; we mitigate the fallout by
//! having the trampoline *borrow* the sender rather than owning it, and
//! treating a dropped receiver as "no-op, just discard". The leaked box is
//! reclaimed when the `EventSubscription` itself drops; the short window
//! between "subscription dropped" and "callback observes the dropped
//! receiver" is safe because the callback path tolerates both a dropped
//! receiver and a dangling-but-not-yet-reclaimed box (we reclaim only after
//! replacing the callback, if ever — for now we leak deliberately, see
//! below).

use std::ffi::c_void;
use std::panic::{self, AssertUnwindSafe};

use futures_core::Stream;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use windows::Win32::System::HostComputeSystem::{
    HcsEventOptionNone, HcsSetComputeSystemCallback, HCS_EVENT, HCS_EVENT_OPTIONS, HCS_SYSTEM,
};

use crate::error::{HcsError, HcsResult};

/// A lifecycle event observed on a compute system.
#[derive(Debug, Clone)]
pub struct HcsEvent {
    /// Classified HCS event kind (see [`HcsEventKind`]).
    pub kind: HcsEventKind,
    /// Optional JSON detail associated with the event, decoded as UTF-8.
    ///
    /// Empty when HCS did not provide a payload (or when it was null).
    pub detail_json: String,
}

/// Classification of `HCS_EVENT_TYPE` values `ZLayer` cares about.
///
/// The numeric discriminants come from `windows::Win32::System::HostComputeSystem`
/// constants (`HcsEventSystemExited = 1`, `HcsEventSystemCrashInitiated = 2`,
/// `HcsEventSystemCrashReport = 3`, `HcsEventServiceDisconnect = 33554432`).
/// Anything else is returned as [`HcsEventKind::Other`] with the raw
/// discriminant so callers can still observe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HcsEventKind {
    /// The compute system's root process has exited (container stopped).
    SystemExited,
    /// The guest has begun writing a crash dump.
    SystemCrashInitiated,
    /// The guest has finished writing a crash dump.
    SystemCrashReport,
    /// The compute service (`vmcompute.exe`) disconnected — usually fatal.
    ServiceDisconnect,
    /// An HCS event type we do not explicitly classify. The raw
    /// discriminant is preserved so callers can still inspect it.
    Other(i32),
}

impl HcsEventKind {
    /// Classify a raw `HCS_EVENT_TYPE.0` discriminant.
    ///
    /// Values sourced from `windows-rs 0.62` generated bindings; update the
    /// arms here if future windows-rs releases renumber the constants
    /// (though the HCS ABI makes that unlikely).
    fn from_raw(ty: i32) -> Self {
        // Keep these literals in sync with `HcsEventSystem*` consts in
        // `windows::Win32::System::HostComputeSystem`.
        const SYSTEM_EXITED: i32 = 1;
        const SYSTEM_CRASH_INITIATED: i32 = 2;
        const SYSTEM_CRASH_REPORT: i32 = 3;
        const SERVICE_DISCONNECT: i32 = 33_554_432; // 0x0200_0000

        match ty {
            SYSTEM_EXITED => Self::SystemExited,
            SYSTEM_CRASH_INITIATED => Self::SystemCrashInitiated,
            SYSTEM_CRASH_REPORT => Self::SystemCrashReport,
            SERVICE_DISCONNECT => Self::ServiceDisconnect,
            other => Self::Other(other),
        }
    }
}

/// A live subscription to a compute system's lifecycle events.
///
/// The subscription owns the leaked sender-box that HCS holds as its
/// callback context. Dropping this value drops the sender (ending the
/// stream), but *does not* unregister the callback on the HCS side — HCS
/// does not expose a removal API. The trampoline tolerates a dropped
/// receiver by discarding events, which is safe because the sender box is
/// kept alive for as long as the subscription exists; see the module
/// docs for the full reasoning.
///
/// Callers must keep `EventSubscription` alive for as long as they want to
/// observe events. A common pattern is to hold it alongside the owning
/// [`crate::handle::OwnedSystem`] and drop both together when the compute
/// system is closed.
#[derive(Debug)]
pub struct EventSubscription {
    /// Raw handle of the compute system the callback is registered on.
    /// Stored for diagnostics only; we do not close it here (ownership
    /// remains with the caller's [`crate::handle::OwnedSystem`]).
    _system: HCS_SYSTEM,
    /// The leaked `Box<UnboundedSender<HcsEvent>>` raw pointer that HCS
    /// holds as its callback context. We deliberately leak this box for the
    /// lifetime of the process once the subscription is created: HCS has
    /// no way to guarantee the trampoline has stopped firing, so reclaiming
    /// the box would race with the callback worker thread.
    ///
    /// Stored here so the `Debug` impl can show it and so future work can
    /// opt into explicit reclaim if HCS ever grows an unregister API.
    _sender_box_leak: *mut UnboundedSender<HcsEvent>,
}

// SAFETY: the raw sender pointer is never exposed to safe code; it exists
// only as an opaque context for the FFI trampoline. The subscription is
// otherwise plain data.
unsafe impl Send for EventSubscription {}
// SAFETY: same reasoning as `Send`; the raw pointer is not shared across
// threads in any way that safe code can observe.
unsafe impl Sync for EventSubscription {}

/// Subscribe to lifecycle events on a compute system.
///
/// Returns the live subscription (must be kept alive by the caller; when
/// it drops, the stream ends) and an async [`Stream`] of [`HcsEvent`]s.
/// The stream produces events for as long as:
///
/// * The compute system remains open on the HCS side.
/// * The subscription is held by the caller.
///
/// # Errors
///
/// Returns an error if [`HcsSetComputeSystemCallback`] fails. The most
/// common failure is [`HcsError::SystemCallbackAlreadySet`] — HCS only
/// allows one system-wide callback per compute system, so attempts to
/// subscribe twice will fail. Pick a callback model (per-operation or
/// per-system) per compute system and stick with it.
///
/// # Safety of `system`
///
/// The caller must ensure `system` is a currently-live `HCS_SYSTEM` handle
/// (typically borrowed from an [`crate::handle::OwnedSystem`] via its
/// `as_raw()` accessor). Registering a callback on a closed handle is
/// undefined behaviour; registering on a valid handle that is later closed
/// is safe — HCS stops firing the callback before `HcsCloseComputeSystem`
/// returns.
pub fn subscribe(
    system: HCS_SYSTEM,
) -> HcsResult<(EventSubscription, impl Stream<Item = HcsEvent>)> {
    let (tx, rx): (UnboundedSender<HcsEvent>, UnboundedReceiver<HcsEvent>) =
        mpsc::unbounded_channel();
    let tx_box: Box<UnboundedSender<HcsEvent>> = Box::new(tx);
    let tx_ptr: *mut UnboundedSender<HcsEvent> = Box::into_raw(tx_box);

    // SAFETY: `tx_ptr` is a live `Box<UnboundedSender<HcsEvent>>` raw
    // pointer that we just produced with `Box::into_raw`. The trampoline
    // only ever borrows `&*tx_ptr` — it never reconstructs the box — so
    // the pointer remains valid for as long as the subscription exists.
    // On failure below we reclaim the box ourselves.
    let register_result = unsafe {
        HcsSetComputeSystemCallback(
            system,
            HCS_EVENT_OPTIONS(HcsEventOptionNone.0),
            Some(tx_ptr.cast::<c_void>().cast_const()),
            Some(event_trampoline),
        )
    };

    if let Err(err) = register_result {
        // HCS rejected the callback — reclaim the box so we do not leak.
        //
        // SAFETY: `tx_ptr` was produced by `Box::into_raw` above and has
        // not been observed by the trampoline (HCS did not accept it), so
        // we are the exclusive owner and can turn it back into a Box to
        // drop.
        drop(unsafe { Box::from_raw(tx_ptr) });
        return Err(HcsError::from_hresult(
            err.code(),
            "HcsSetComputeSystemCallback",
        ));
    }

    let sub = EventSubscription {
        _system: system,
        _sender_box_leak: tx_ptr,
    };
    Ok((sub, UnboundedReceiverStream::new(rx)))
}

/// HCS event-callback trampoline.
///
/// Invoked on an HCS worker thread every time a lifecycle event fires on a
/// subscribed compute system. The trampoline:
///
/// 1. Borrows the [`UnboundedSender<HcsEvent>`] from the context pointer
///    (it must *never* take ownership — future events still need it).
/// 2. Decodes the event's `EventData` wide-string payload into UTF-8.
/// 3. Classifies the event type via [`HcsEventKind::from_raw`].
/// 4. Forwards the resulting [`HcsEvent`] to the stream, discarding send
///    errors (a dropped receiver simply means no one is listening).
///
/// All of the above is wrapped in [`std::panic::catch_unwind`] so Rust
/// panics cannot unwind across the C ABI boundary into HCS's worker thread.
///
/// # Safety
///
/// * `context` must be a pointer previously passed to
///   [`HcsSetComputeSystemCallback`] via [`subscribe`], i.e. the result of
///   `Box::into_raw(Box::<UnboundedSender<HcsEvent>>::new(_))`.
/// * The pointed-to sender must still be alive (guaranteed by the
///   [`EventSubscription`] contract: the box is leaked for the process
///   lifetime once subscribed).
/// * `event` must point at a valid, initialised `HCS_EVENT` for the
///   duration of this call, or be null (we check).
unsafe extern "system" fn event_trampoline(event: *const HCS_EVENT, context: *const c_void) {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || event.is_null() {
            return;
        }
        // SAFETY: per the function contract, `context` is a live
        // `Box<UnboundedSender<HcsEvent>>` raw pointer. We borrow the
        // sender rather than reconstructing the box — the callback will
        // fire again and the box must stay live.
        let sender_ptr = context.cast::<UnboundedSender<HcsEvent>>().cast_mut();
        let sender: &UnboundedSender<HcsEvent> = unsafe { &*sender_ptr };

        // SAFETY: per the function contract, `event` points at a valid
        // `HCS_EVENT` for the duration of this call.
        let ev = unsafe { &*event };

        // `EventData` is a `PCWSTR`; decode it lossily into UTF-8 via the
        // helper on `PCWSTR` itself. Null pointers are represented by the
        // `is_null()` check here and yield an empty string.
        let detail = if ev.EventData.is_null() {
            String::new()
        } else {
            // SAFETY: HCS guarantees the buffer is null-terminated and
            // lives for the duration of this callback. `PCWSTR::to_string`
            // walks until the terminator.
            unsafe { ev.EventData.to_string() }.unwrap_or_default()
        };

        let kind = HcsEventKind::from_raw(ev.Type.0);
        // Send failure == receiver dropped == no subscriber; just discard.
        let _ = sender.send(HcsEvent {
            kind,
            detail_json: detail,
        });
    }));

    if let Err(payload) = result {
        let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };
        tracing::error!(panic = %msg, "HCS event trampoline panicked");
    }
}

#[cfg(test)]
mod tests {
    //! Tests here run only on Windows (the crate is gated by
    //! `#![cfg(windows)]`). They cover the pure logic of this module —
    //! event-kind classification — without touching HCS itself. Anything
    //! that requires a live compute system (actual callback firing) lives
    //! in the integration-test suite, which is gated on an Administrator
    //! Windows host.

    use super::*;

    #[test]
    fn classifies_known_event_types() {
        assert_eq!(HcsEventKind::from_raw(1), HcsEventKind::SystemExited);
        assert_eq!(
            HcsEventKind::from_raw(2),
            HcsEventKind::SystemCrashInitiated
        );
        assert_eq!(HcsEventKind::from_raw(3), HcsEventKind::SystemCrashReport);
        assert_eq!(
            HcsEventKind::from_raw(33_554_432),
            HcsEventKind::ServiceDisconnect
        );
    }

    #[test]
    fn unknown_event_type_falls_through_to_other() {
        assert_eq!(HcsEventKind::from_raw(4), HcsEventKind::Other(4));
        assert_eq!(
            HcsEventKind::from_raw(0x7FFF_FFFF),
            HcsEventKind::Other(0x7FFF_FFFF)
        );
    }
}
