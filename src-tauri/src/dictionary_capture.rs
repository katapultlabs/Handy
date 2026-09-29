//! In-place dictionary capture (macOS). Handy learns corrections from edits
//! the user makes to pasted text in the target application.
//!
//! This implements `docs/DICTIONARY_DESIGN.md` sections 7.3 and 16.3:
//!
//! - **Snapshot**, just before the paste. Read the focused AX element and the
//!   caret position. This runs on a dedicated AX thread with a 100 ms budget.
//!   The paste never waits longer. On timeout, capture skips this dictation.
//!   The snapshot never reads field content.
//! - **Check**, later. Read a bounded window around the anchor with
//!   `AXStringForRange`, or a truncated `AXValue` read when the application
//!   does not support it. Conservatively align the edited paste inside that
//!   window, then feed only the aligned text through the same conservative
//!   learning classifier as History edits.
//! - **Triggers**: `AXValueChanged` notifications on the anchored element,
//!   debounced on the AX thread. Applications that do not expose that
//!   notification get an anchor-scoped bounded fallback check. The next
//!   dictation also finalizes the current anchor. Anchors expire after 180 s.
//!
//! Every AX call happens on the dedicated thread with per-element messaging
//! timeouts. An unresponsive target application can never stall dictation.
//! All other platforms get a no-op manager.

// The manager's methods are called only from macOS-gated code paths, so the
// other platforms see them as dead code.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use tauri::AppHandle;

const SNAPSHOT_BUDGET_MS: u64 = 100;
const ANCHOR_TTL_SECS: u64 = 180;
/// Wait for a burst of AX value notifications to settle before reading. The
/// observer callback itself never reads field text.
const EDIT_DEBOUNCE_MS: u64 = 400;
/// A safely aligned edit gets one short confirmation period. This prevents a
/// pause in the middle of a multi-step edit from being learned immediately.
const CANDIDATE_CONFIRM_MS: u64 = 200;
/// Only applications without AX value notifications use this active-anchor
/// fallback. It stops with the anchor and never runs while capture is idle.
const FALLBACK_CHECK_MS: u64 = 750;
/// The channel cannot wake a CFRunLoop directly. Short run-loop slices keep
/// the pre-paste snapshot response well inside its 100 ms caller budget.
const RUN_LOOP_SLICE_MS: u64 = 50;
/// UTF-16 units read before the anchor and slack after the pasted length,
/// bounding how much target text is ever read on the AXStringForRange path.
const PRE_MARGIN: usize = 8;
const POST_MARGIN: usize = 96;
/// Fallback full-value reads are truncated to this many UTF-16 units around
/// the anchor before anything else looks at them (design doc section 16.4).
const FALLBACK_WINDOW: usize = 16 * 1024;
/// Bound the fallback edit alignment. Larger inputs need an unchanged suffix
/// anchor or capture skips them rather than doing quadratic work.
const ALIGNMENT_CELL_BUDGET: usize = 64 * 1024;
const ALIGNMENT_SUFFIX_CHARS: usize = 24;
const MAX_ALIGNMENT_EDIT_RATIO: f64 = 0.6;

#[derive(Debug, PartialEq, Eq)]
enum SettledObservation {
    Unchanged,
    Edited(String),
    Cleared,
    Unresolved,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
enum TemporalAction {
    Read,
    ConfirmRead,
    Learn(String),
    Drop,
}

/// Pure timing/state model for one capture anchor. `Instant`s are supplied by
/// the caller, so notification coalescing and finalization are unit-testable
/// without Accessibility or wall-clock sleeps.
struct CaptureTemporalState {
    observer_supported: bool,
    next_read: Option<std::time::Instant>,
    confirm_at: Option<std::time::Instant>,
    last_safe_edit: Option<String>,
}

impl CaptureTemporalState {
    fn new(now: std::time::Instant, observer_supported: bool) -> Self {
        Self {
            observer_supported,
            next_read: (!observer_supported)
                .then_some(now + std::time::Duration::from_millis(FALLBACK_CHECK_MS)),
            confirm_at: None,
            last_safe_edit: None,
        }
    }

    fn value_changed(&mut self, now: std::time::Instant) {
        // Registration success is not enough: some apps accept the observer
        // but never deliver value notifications. The first callback confirms
        // the event path and turns off repeated fallback reads.
        self.observer_supported = true;
        self.next_read = Some(now + std::time::Duration::from_millis(EDIT_DEBOUNCE_MS));
        // Preserve a previously observed safe edit until the new value can be
        // classified. A subsequent clear can then finalize that correction.
        self.confirm_at = None;
    }

    fn next_deadline(&self) -> Option<std::time::Instant> {
        [self.next_read, self.confirm_at]
            .into_iter()
            .flatten()
            .min()
    }

    fn due_action(&mut self, now: std::time::Instant) -> Option<TemporalAction> {
        if self.next_read.is_some_and(|deadline| deadline <= now) {
            self.next_read = None;
            return Some(TemporalAction::Read);
        }
        if self.confirm_at.is_some_and(|deadline| deadline <= now) {
            self.confirm_at = None;
            if self.observer_supported {
                return self.last_safe_edit.take().map(TemporalAction::Learn);
            }
            // A fallback read has no event stream proving that the value stayed
            // stable. Re-read before learning instead of trusting the cached
            // candidate across this confirmation window.
            return self
                .last_safe_edit
                .as_ref()
                .map(|_| TemporalAction::ConfirmRead);
        }
        None
    }

    fn observed(
        &mut self,
        now: std::time::Instant,
        observation: SettledObservation,
        confirming: bool,
    ) -> Option<TemporalAction> {
        if !self.observer_supported {
            self.next_read = Some(now + std::time::Duration::from_millis(FALLBACK_CHECK_MS));
        }
        match observation {
            SettledObservation::Unchanged => {
                // The user reverted the correction, or this is the initial
                // paste notification. Do not retain an older candidate.
                self.last_safe_edit = None;
                self.confirm_at = None;
                None
            }
            SettledObservation::Edited(corrected) => {
                if confirming
                    && self
                        .last_safe_edit
                        .as_deref()
                        .is_some_and(|candidate| candidate == corrected.as_str())
                {
                    self.last_safe_edit = None;
                    self.confirm_at = None;
                    return Some(TemporalAction::Learn(corrected));
                }
                // A different safely aligned value means editing continued.
                // Replace the candidate and restart its confirmation window.
                self.last_safe_edit = Some(corrected);
                self.confirm_at =
                    Some(now + std::time::Duration::from_millis(CANDIDATE_CONFIRM_MS));
                None
            }
            SettledObservation::Cleared => self
                .last_safe_edit
                .take()
                .map(TemporalAction::Learn)
                .or(Some(TemporalAction::Drop)),
            SettledObservation::Unresolved => {
                // Do not auto-learn after a newer value that cannot be aligned.
                // Keep the last safely observed span only so a later explicit
                // clear can finalize that already-observed correction.
                self.confirm_at = None;
                None
            }
            SettledObservation::Unavailable => Some(TemporalAction::Drop),
        }
    }

    fn learning_rejected(&mut self) {
        self.last_safe_edit = None;
        self.confirm_at = None;
    }

    fn cancel(&mut self) {
        self.next_read = None;
        self.confirm_at = None;
        self.last_safe_edit = None;
    }
}

/// Convert a UTF-16 offset, as reported by AX, into a Rust character index.
/// An offset in the middle of a surrogate pair is invalid.
fn utf16_offset_to_char_index(text: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (index, ch) in text.chars().enumerate() {
        if units == offset {
            return Some(index);
        }
        units += ch.len_utf16();
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.chars().count())
}

/// Isolate the edited paste from the surrounding AX window.
///
/// The caret-derived offset fixes the start. For long text, a unique unchanged
/// suffix fixes the end in linear work. Shorter text uses prefix Levenshtein
/// alignment under a fixed cell budget. Weak or equally-good alignments are
/// rejected so surrounding application text never becomes a dictionary pair.
fn align_captured_edit(pasted: &str, window: &str, expected_start_u16: usize) -> Option<String> {
    let pasted_chars: Vec<char> = pasted.chars().collect();
    let window_chars: Vec<char> = window.chars().collect();
    if pasted_chars.is_empty() {
        return None;
    }
    let start = utf16_offset_to_char_index(window, expected_start_u16)?;
    let tail = window_chars.get(start..)?;
    if tail.is_empty() {
        return None;
    }

    // Long dictations usually change only one word. Their unchanged ending is
    // a cheap, strong boundary that avoids a full edit-distance matrix.
    if pasted_chars.len() >= ALIGNMENT_SUFFIX_CHARS {
        let suffix = &pasted_chars[pasted_chars.len() - ALIGNMENT_SUFFIX_CHARS..];
        let mut end = None;
        for index in 0..=tail.len().saturating_sub(suffix.len()) {
            if tail[index..].starts_with(suffix) {
                let candidate_end = index + suffix.len();
                if end.replace(candidate_end).is_some() {
                    return None; // the boundary anchor is ambiguous
                }
            }
        }
        if let Some(end) = end {
            let min_len = pasted_chars.len().saturating_mul(2) / 5;
            let max_len = pasted_chars.len().saturating_mul(8) / 5;
            if (min_len..=max_len).contains(&end) {
                return Some(tail[..end].iter().collect());
            }
            return None;
        }
    }

    let cells = pasted_chars.len().checked_mul(tail.len())?;
    if cells > ALIGNMENT_CELL_BUDGET {
        return None;
    }

    // Distance from `pasted` to every prefix of `tail`. The best unique prefix
    // is the current extent of the pasted text; later window content is not
    // allowed into learn_pairs.
    let mut previous: Vec<usize> = (0..=tail.len()).collect();
    let mut current = vec![0; tail.len() + 1];
    for (old_index, old) in pasted_chars.iter().enumerate() {
        current[0] = old_index + 1;
        for (new_index, new) in tail.iter().enumerate() {
            let substitution = previous[new_index] + if old == new { 0 } else { 1 };
            let deletion = previous[new_index + 1] + 1;
            let insertion = current[new_index] + 1;
            current[new_index + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    let best_cost = previous.iter().skip(1).copied().min()?;
    let mut best_ends = previous
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(end, cost)| (*cost == best_cost).then_some(end));
    let end = best_ends.next()?;
    if best_ends.next().is_some() {
        return None;
    }
    let longer = pasted_chars.len().max(end);
    if (best_cost as f64) / (longer as f64) > MAX_ALIGNMENT_EDIT_RATIO {
        return None;
    }
    Some(tail[..end].iter().collect())
}

pub struct CaptureManager {
    #[cfg(target_os = "macos")]
    tx: std::sync::mpsc::Sender<macos_impl::Msg>,
}

impl CaptureManager {
    pub fn new(app: AppHandle) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                tx: macos_impl::spawn(app),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = app;
            Self {}
        }
    }

    /// Called just before a paste. Blocks for at most [`SNAPSHOT_BUDGET_MS`];
    /// on timeout the paste proceeds and this dictation is not captured.
    pub fn snapshot_before_paste(&self, pasted_text: String) {
        #[cfg(target_os = "macos")]
        {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(SNAPSHOT_BUDGET_MS);
            let (ack_tx, ack_rx) = std::sync::mpsc::channel();
            if self
                .tx
                .send(macos_impl::Msg::Snapshot {
                    pasted: pasted_text,
                    deadline,
                    ack: ack_tx,
                })
                .is_ok()
            {
                let _ = ack_rx.recv_timeout(std::time::Duration::from_millis(SNAPSHOT_BUDGET_MS));
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = pasted_text;
        }
    }

    /// Ask the AX thread to check the anchored text now (non-blocking).
    pub fn check_now(&self) {
        #[cfg(target_os = "macos")]
        {
            let _ = self.tx.send(macos_impl::Msg::Check);
        }
    }

    /// Drop any pending anchor. The paste it belonged to did not happen.
    pub fn cancel(&self) {
        #[cfg(target_os = "macos")]
        {
            let _ = self.tx.send(macos_impl::Msg::Cancel);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::*;
    use accessibility_sys::{
        kAXErrorSuccess, kAXValueChangedNotification, AXError, AXObserverAddNotification,
        AXObserverCreate, AXObserverGetRunLoopSource, AXObserverRef, AXObserverRemoveNotification,
        AXUIElementCopyAttributeValue, AXUIElementCopyParameterizedAttributeValue,
        AXUIElementCreateSystemWide, AXUIElementGetPid, AXUIElementRef,
        AXUIElementSetMessagingTimeout, AXValueCreate, AXValueGetValue, AXValueRef,
    };
    use core_foundation::base::{CFRange, CFRelease, CFTypeRef, TCFType};
    use core_foundation::number::CFNumber;
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopSource};
    use core_foundation::string::{CFString, CFStringRef};
    use log::{debug, info, warn};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tauri::Manager;

    /// kAXValueCFRangeType (AXValue.h). accessibility-sys spells it as an enum
    /// constant; the numeric value is stable ABI.
    const AX_VALUE_CFRANGE_TYPE: u32 = 4;

    pub enum Msg {
        Snapshot {
            pasted: String,
            /// Discard the request past this point: the caller stopped
            /// waiting, the paste went ahead, and a late snapshot would
            /// anchor the wrong caret or application.
            deadline: Instant,
            ack: Sender<()>,
        },
        Check,
        Cancel,
    }

    /// RAII wrapper so every AX/CF object gets released exactly once.
    struct Retained(CFTypeRef);
    impl Drop for Retained {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    unsafe extern "C" fn value_changed_callback(
        _observer: AXObserverRef,
        _element: AXUIElementRef,
        _notification: CFStringRef,
        refcon: *mut c_void,
    ) {
        if let Some(generation) = unsafe { (refcon as *const AtomicU64).as_ref() } {
            generation.fetch_add(1, Ordering::Release);
        }
    }

    /// Registration for one anchored element. The run-loop source and refcon
    /// live exactly as long as the anchor and are removed on the AX thread.
    struct ValueObserver {
        observer: Retained,
        source: CFRunLoopSource,
        element: AXUIElementRef,
        generation: Arc<AtomicU64>,
        seen_generation: u64,
    }

    impl ValueObserver {
        fn register(element: AXUIElementRef, pid: i32) -> Option<Self> {
            let generation = Arc::new(AtomicU64::new(0));
            let mut observer: AXObserverRef = std::ptr::null_mut();
            let err = unsafe { AXObserverCreate(pid, value_changed_callback, &mut observer) };
            if err != kAXErrorSuccess || observer.is_null() {
                debug!("capture: AX value observer creation failed ({err}); using fallback");
                return None;
            }
            let observer = Retained(observer as CFTypeRef);
            let notification = CFString::new(kAXValueChangedNotification);
            let err = unsafe {
                AXObserverAddNotification(
                    observer.0 as AXObserverRef,
                    element,
                    notification.as_concrete_TypeRef(),
                    Arc::as_ptr(&generation) as *mut c_void,
                )
            };
            if err != kAXErrorSuccess {
                debug!("capture: AX value notification unsupported ({err}); using fallback");
                return None;
            }
            let source_ref = unsafe { AXObserverGetRunLoopSource(observer.0 as AXObserverRef) };
            if source_ref.is_null() {
                unsafe {
                    AXObserverRemoveNotification(
                        observer.0 as AXObserverRef,
                        element,
                        notification.as_concrete_TypeRef(),
                    );
                }
                return None;
            }
            let source = unsafe { CFRunLoopSource::wrap_under_get_rule(source_ref) };
            CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopDefaultMode });
            Some(Self {
                observer,
                source,
                element,
                generation,
                seen_generation: 0,
            })
        }

        fn take_change(&mut self) -> bool {
            let current = self.generation.load(Ordering::Acquire);
            if current == self.seen_generation {
                false
            } else {
                self.seen_generation = current;
                true
            }
        }
    }

    impl Drop for ValueObserver {
        fn drop(&mut self) {
            let notification = CFString::new(kAXValueChangedNotification);
            unsafe {
                AXObserverRemoveNotification(
                    self.observer.0 as AXObserverRef,
                    self.element,
                    notification.as_concrete_TypeRef(),
                );
            }
            CFRunLoop::get_current().remove_source(&self.source, unsafe { kCFRunLoopDefaultMode });
        }
    }

    struct Anchor {
        // Drop the observer before the element whose raw reference it uses.
        observer: Option<ValueObserver>,
        element: Retained,
        /// Caret position (UTF-16 units) just before the paste keystroke.
        caret: usize,
        pasted: String,
        pasted_u16: usize,
        created: Instant,
        temporal: CaptureTemporalState,
    }

    pub fn spawn(app: AppHandle) -> Sender<Msg> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("dictionary-capture-ax".into())
            .spawn(move || run(app, rx))
            .expect("spawn dictionary capture thread");
        tx
    }

    fn run(app: AppHandle, rx: Receiver<Msg>) {
        let mut anchor: Option<Anchor> = None;
        loop {
            // With no anchor there is no timer and no run-loop polling. The
            // thread sleeps until the next paste asks for a snapshot.
            if anchor.is_none() {
                match rx.recv() {
                    Ok(msg) => handle_msg(&app, msg, &mut anchor),
                    Err(_) => return,
                }
                continue;
            }

            loop {
                match rx.try_recv() {
                    Ok(msg) => handle_msg(&app, msg, &mut anchor),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                }
            }
            let Some(a) = anchor.as_mut() else {
                continue;
            };

            let now = Instant::now();
            if a.created.elapsed() > Duration::from_secs(ANCHOR_TTL_SECS) {
                debug!("capture: anchor expired");
                anchor = None;
                continue;
            }

            if a.observer.as_mut().is_some_and(ValueObserver::take_change) {
                a.temporal.value_changed(now);
            }

            if let Some(action) = a.temporal.due_action(now) {
                apply_temporal_action(&app, action, &mut anchor);
                continue;
            }

            let Some(a) = anchor.as_ref() else {
                continue;
            };
            let ttl_deadline = a.created + Duration::from_secs(ANCHOR_TTL_SECS);
            let next_deadline = a.temporal.next_deadline().unwrap_or(ttl_deadline);
            let wait = next_deadline.saturating_duration_since(Instant::now());
            if a.observer.is_some() {
                // Pumping delivers AX callbacks on this dedicated thread.
                // Slice the wait because mpsc sends cannot wake a CFRunLoop.
                CFRunLoop::run_in_mode(
                    unsafe { kCFRunLoopDefaultMode },
                    wait.min(Duration::from_millis(RUN_LOOP_SLICE_MS)),
                    true,
                );
            } else {
                // recv_timeout is interruptible by snapshot/check/cancel, so
                // the unsupported-app fallback does no extra channel polling.
                match rx.recv_timeout(wait) {
                    Ok(msg) => handle_msg(&app, msg, &mut anchor),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }

    fn handle_msg(app: &AppHandle, msg: Msg, anchor: &mut Option<Anchor>) {
        match msg {
            Msg::Snapshot {
                pasted,
                deadline,
                ack,
            } => {
                // Removing the previous AX registration can itself take time.
                // Do it before the snapshot and include it in the same caller
                // deadline so a late request never leaves a live new anchor.
                *anchor = None;
                if Instant::now() > deadline {
                    debug!("capture: discarding stale snapshot request");
                    return;
                }
                let taken = take_snapshot(pasted);
                // AX calls carry their own messaging timeouts and can outlive
                // the caller's budget. A late caret can be post-paste, so the
                // temporary anchor (including observer) must be discarded.
                if Instant::now() > deadline {
                    debug!("capture: snapshot finished late; discarding");
                } else {
                    *anchor = taken;
                    let _ = ack.send(());
                }
            }
            Msg::Check => {
                if anchor.is_some() {
                    apply_temporal_action(app, TemporalAction::Read, anchor);
                    // A next-dictation boundary may arrive while a safe edit
                    // is awaiting confirmation. Finalize it synchronously on
                    // the AX thread after the current value has been read.
                    if let Some(a) = anchor.as_mut() {
                        if let Some(corrected) = a.temporal.last_safe_edit.take() {
                            a.temporal.confirm_at = None;
                            apply_temporal_action(app, TemporalAction::Learn(corrected), anchor);
                        }
                    }
                }
            }
            Msg::Cancel => {
                if let Some(a) = anchor.as_mut() {
                    debug!("capture: anchor cancelled (paste failed)");
                    a.temporal.cancel();
                }
                *anchor = None;
            }
        }
    }

    // ------------------------------------------------------------------
    // AX helpers (all called on this thread only)
    // ------------------------------------------------------------------

    fn ax_attr(element: AXUIElementRef, name: &str) -> Option<Retained> {
        let attr = CFString::new(name);
        let mut out: CFTypeRef = std::ptr::null();
        let err: AXError =
            unsafe { AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut out) };
        if err == kAXErrorSuccess && !out.is_null() {
            Some(Retained(out))
        } else {
            None
        }
    }

    fn cf_to_string(v: &Retained) -> Option<String> {
        if v.0.is_null() {
            return None;
        }
        let s = unsafe { CFString::wrap_under_get_rule(v.0 as CFStringRef) };
        Some(s.to_string())
    }

    fn focused_element() -> Option<(Retained, i32)> {
        unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return None;
            }
            AXUIElementSetMessagingTimeout(system, 0.1);
            let system = Retained(system as CFTypeRef);
            let elem = ax_attr(system.0 as AXUIElementRef, "AXFocusedUIElement")?;
            AXUIElementSetMessagingTimeout(elem.0 as AXUIElementRef, 0.1);
            let mut pid: i32 = 0;
            if AXUIElementGetPid(elem.0 as AXUIElementRef, &mut pid) != kAXErrorSuccess {
                return None;
            }
            Some((elem, pid))
        }
    }

    /// True when the element is a password field (role or subrole
    /// `AXSecureTextField`). Both attributes are CFStrings.
    fn is_secure_text_field(element: AXUIElementRef) -> bool {
        const SECURE: &str = "AXSecureTextField";
        ["AXRole", "AXSubrole"].iter().any(|attr| {
            ax_attr(element, attr)
                .and_then(|v| cf_to_string(&v))
                .is_some_and(|s| s == SECURE)
        })
    }

    fn selected_range(element: AXUIElementRef) -> Option<CFRange> {
        let v = ax_attr(element, "AXSelectedTextRange")?;
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        let ok = unsafe {
            AXValueGetValue(
                v.0 as AXValueRef,
                AX_VALUE_CFRANGE_TYPE,
                &mut range as *mut CFRange as *mut _,
            )
        };
        if ok {
            Some(range)
        } else {
            None
        }
    }

    fn char_count(element: AXUIElementRef) -> Option<i64> {
        let v = ax_attr(element, "AXNumberOfCharacters")?;
        let n = unsafe { CFNumber::wrap_under_get_rule(v.0 as _) };
        n.to_i64()
    }

    /// Bounded window read: AXStringForRange first, truncated AXValue second.
    fn read_window(element: AXUIElementRef, start: usize, len: usize) -> Option<String> {
        // Clamp to the field's length when the app reports one.
        let (start, len) = match char_count(element) {
            Some(total) if total >= 0 => {
                let total = total as usize;
                let start = start.min(total);
                (start, len.min(total.saturating_sub(start)))
            }
            _ => (start, len),
        };
        if len == 0 {
            return Some(String::new());
        }

        let range = CFRange {
            location: start as isize,
            length: len as isize,
        };
        let param =
            unsafe { AXValueCreate(AX_VALUE_CFRANGE_TYPE, &range as *const CFRange as *const _) };
        if !param.is_null() {
            let param = Retained(param as CFTypeRef);
            let attr = CFString::new("AXStringForRange");
            let mut out: CFTypeRef = std::ptr::null();
            let err = unsafe {
                AXUIElementCopyParameterizedAttributeValue(
                    element,
                    attr.as_concrete_TypeRef(),
                    param.0,
                    &mut out,
                )
            };
            if err == kAXErrorSuccess && !out.is_null() {
                return cf_to_string(&Retained(out));
            }
        }

        // Fallback: full value read, truncated to a window at once. The rest
        // of the value is dropped here and never leaves this function.
        let v = ax_attr(element, "AXValue")?;
        let full = cf_to_string(&v)?;
        let units: Vec<u16> = full.encode_utf16().collect();
        drop(full);
        let end = (start + len).min(units.len()).min(start + FALLBACK_WINDOW);
        let start = start.min(units.len());
        Some(String::from_utf16_lossy(&units[start..end]))
    }

    // ------------------------------------------------------------------
    // Snapshot and check
    // ------------------------------------------------------------------

    fn take_snapshot(pasted: String) -> Option<Anchor> {
        let Some((element, pid)) = focused_element() else {
            info!("capture: no focused AX element; skipping this dictation");
            return None;
        };
        // Never read a password field. This is checked on the element itself.
        // A global "secure input is on" check is too wide: loginwindow or a
        // chat app can hold secure input for hours and block every capture.
        if is_secure_text_field(element.0 as AXUIElementRef) {
            info!("capture: focused element is a secure text field; skipping");
            return None;
        }
        let Some(range) = selected_range(element.0 as AXUIElementRef) else {
            info!("capture: focused element reports no selected-text range (app may not expose AX text); skipping");
            return None;
        };
        if range.location < 0 {
            return None;
        }
        let pasted_u16 = pasted.encode_utf16().count();
        let observer = ValueObserver::register(element.0 as AXUIElementRef, pid);
        let now = Instant::now();
        debug!("capture: anchored at {} in pid {}", range.location, pid);
        Some(Anchor {
            observer,
            element,
            caret: range.location as usize,
            pasted,
            pasted_u16,
            created: now,
            // Keep the fallback active until the observer proves that this
            // application actually delivers AXValueChanged notifications.
            temporal: CaptureTemporalState::new(now, false),
        })
    }

    fn observe_anchor(a: &Anchor) -> SettledObservation {
        let start = a.caret.saturating_sub(PRE_MARGIN);
        let len = a.pasted_u16 + PRE_MARGIN + POST_MARGIN;
        let window = match read_window(a.element.0 as AXUIElementRef, start, len) {
            Some(w) => w,
            None => {
                info!("capture: window read failed (app closed or AX text unsupported); dropping anchor");
                return SettledObservation::Unavailable;
            }
        };

        if window.trim().is_empty() {
            debug!("capture: anchored field is empty");
            return SettledObservation::Cleared;
        }

        if window.contains(a.pasted.trim_end()) {
            debug!("capture: pasted text unchanged; watching");
            return SettledObservation::Unchanged;
        }

        let pasted = a.pasted.trim_end();
        let expected_start_u16 = a.caret.saturating_sub(start);
        let Some(corrected) = align_captured_edit(pasted, &window, expected_start_u16) else {
            info!(
                "capture: edit detected but the pasted span could not be aligned safely; keeping anchor"
            );
            return SettledObservation::Unresolved;
        };
        let corrected = corrected.trim();
        if corrected == pasted {
            SettledObservation::Unchanged
        } else {
            SettledObservation::Edited(corrected.to_string())
        }
    }

    fn apply_temporal_action(app: &AppHandle, action: TemporalAction, anchor: &mut Option<Anchor>) {
        match action {
            read_action @ (TemporalAction::Read | TemporalAction::ConfirmRead) => {
                let confirming = matches!(read_action, TemporalAction::ConfirmRead);
                let Some(a) = anchor.as_mut() else {
                    return;
                };
                let observation = observe_anchor(a);
                let action = a.temporal.observed(Instant::now(), observation, confirming);
                if let Some(action) = action {
                    apply_temporal_action(app, action, anchor);
                }
            }
            TemporalAction::Learn(corrected) => {
                let Some(a) = anchor.as_mut() else {
                    return;
                };
                match store_learned(app, a.pasted.trim_end(), corrected.trim()) {
                    StoreOutcome::NoPairs => {
                        // A rewrite or incomplete correction can pass AX
                        // alignment but fail the conservative learning gates.
                        // Keep watching without retaining its content.
                        info!("capture: settled edit produced no learnable pairs; keeping anchor");
                        a.temporal.learning_rejected();
                    }
                    StoreOutcome::Done => *anchor = None,
                }
            }
            TemporalAction::Drop => *anchor = None,
        }
    }

    enum StoreOutcome {
        NoPairs,
        Done,
    }

    fn store_learned(app: &AppHandle, original: &str, corrected: &str) -> StoreOutcome {
        let settings = crate::settings::get_settings(app);
        // Authoritative runtime gate: the user may have turned the feature (or
        // Experimental as a whole) off while this anchor was live.
        if !settings.experimental_enabled
            || !settings.dictionary_enabled
            || !settings.dictionary_capture_enabled
        {
            debug!("capture: learning disabled since anchor was taken; discarding");
            return StoreOutcome::Done;
        }
        let Some(manager) = app.try_state::<Arc<crate::dictionary_store::DictionaryManager>>()
        else {
            warn!("capture: dictionary store not ready; discarding");
            return StoreOutcome::Done;
        };
        let report = match manager.learn_from_edit(original, corrected, "capture") {
            Ok(report) => report,
            Err(err) => {
                warn!("capture: could not store learned pairs: {err}");
                return StoreOutcome::Done;
            }
        };
        if report.added.is_empty() && report.known.is_empty() {
            return StoreOutcome::NoPairs;
        }
        if report.added.is_empty() {
            debug!(
                "capture: {} pair(s) already known; nothing new",
                report.known.len()
            );
            return StoreOutcome::Done;
        }
        // Log counts only. Learned pairs can contain confidential names, and
        // the log file must never hold field content (design doc section 12).
        info!(
            "capture: learned {} new dictionary entries",
            report.added.len()
        );
        StoreOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_excludes_surrounding_window_text() {
        let pasted = "We use Catapult for releases.";
        let prefix = "before: ";
        let window = format!("{prefix}We use Katapult for releases. after");
        let aligned = align_captured_edit(pasted, &window, prefix.encode_utf16().count());
        assert_eq!(aligned.as_deref(), Some("We use Katapult for releases."));
    }

    #[test]
    fn alignment_handles_utf16_caret_offset() {
        let pasted = "Catapult";
        let prefix = "🙂 ";
        let window = format!("{prefix}Katapult trailing context");
        let aligned = align_captured_edit(pasted, &window, prefix.encode_utf16().count());
        assert_eq!(aligned.as_deref(), Some("Katapult"));
    }

    #[test]
    fn alignment_keeps_simple_full_field_correction() {
        let aligned = align_captured_edit("Catapult", "Katapult", 0);
        assert_eq!(aligned.as_deref(), Some("Katapult"));
    }

    #[test]
    fn alignment_fails_closed_when_best_boundary_is_ambiguous() {
        // "ab" (delete c) and "abx" (replace c) are equally close. Without
        // surrounding context there is no safe way to choose the edit span.
        assert_eq!(align_captured_edit("abc", "abx suffix", 0), None);
    }

    #[test]
    fn alignment_budget_rejects_large_text_without_suffix_anchor() {
        let pasted = "a".repeat(400);
        let window = "b".repeat(500);
        assert_eq!(align_captured_edit(&pasted, &window, 0), None);
    }

    #[test]
    fn rapid_value_changes_coalesce_into_one_settled_read() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, true);
        state.value_changed(start);
        state.value_changed(start + std::time::Duration::from_millis(120));
        state.value_changed(start + std::time::Duration::from_millis(260));

        assert_eq!(
            state.due_action(start + std::time::Duration::from_millis(659)),
            None
        );
        assert_eq!(
            state.due_action(start + std::time::Duration::from_millis(660)),
            Some(TemporalAction::Read)
        );
        assert_eq!(
            state.due_action(start + std::time::Duration::from_secs(2)),
            None
        );
    }

    #[test]
    fn clearing_after_observed_edit_finalizes_last_safe_span() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, true);
        state.value_changed(start);
        assert_eq!(
            state.due_action(start + std::time::Duration::from_millis(EDIT_DEBOUNCE_MS)),
            Some(TemporalAction::Read)
        );
        assert_eq!(
            state.observed(
                start + std::time::Duration::from_millis(EDIT_DEBOUNCE_MS),
                SettledObservation::Edited("Katapult".into()),
                false,
            ),
            None
        );

        state.value_changed(start + std::time::Duration::from_millis(450));
        assert_eq!(
            state.due_action(start + std::time::Duration::from_millis(850)),
            Some(TemporalAction::Read)
        );
        assert_eq!(
            state.observed(
                start + std::time::Duration::from_millis(850),
                SettledObservation::Cleared,
                false,
            ),
            Some(TemporalAction::Learn("Katapult".into()))
        );
    }

    #[test]
    fn unresolved_intermediate_value_cannot_auto_learn_but_clear_keeps_safe_edit() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, true);
        state.last_safe_edit = Some("Katapult".into());
        state.confirm_at = Some(start + std::time::Duration::from_millis(10));

        assert_eq!(
            state.observed(start, SettledObservation::Unresolved, false),
            None
        );
        assert_eq!(
            state.due_action(start + std::time::Duration::from_secs(1)),
            None
        );
        assert_eq!(
            state.observed(start, SettledObservation::Cleared, false),
            Some(TemporalAction::Learn("Katapult".into()))
        );
    }

    #[test]
    fn cancellation_drops_pending_text_and_work() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, true);
        state.value_changed(start);
        state.last_safe_edit = Some("private correction".into());
        state.confirm_at = Some(start + std::time::Duration::from_millis(10));

        state.cancel();

        assert_eq!(state.next_deadline(), None);
        assert_eq!(
            state.due_action(start + std::time::Duration::from_secs(1)),
            None
        );
        assert_eq!(state.last_safe_edit, None);
    }

    #[test]
    fn unchanged_paste_never_creates_a_learning_action() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, true);
        state.value_changed(start);
        assert_eq!(
            state.due_action(start + std::time::Duration::from_millis(EDIT_DEBOUNCE_MS)),
            Some(TemporalAction::Read)
        );
        assert_eq!(
            state.observed(
                start + std::time::Duration::from_millis(EDIT_DEBOUNCE_MS),
                SettledObservation::Unchanged,
                false,
            ),
            None
        );
        assert_eq!(state.next_deadline(), None);
        assert_eq!(state.last_safe_edit, None);
    }

    #[test]
    fn unproven_fallback_rereads_and_restarts_when_edit_changed() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, false);
        let first_read = start + std::time::Duration::from_millis(FALLBACK_CHECK_MS);
        assert_eq!(state.due_action(first_read), Some(TemporalAction::Read));
        assert_eq!(
            state.observed(
                first_read,
                SettledObservation::Edited("Katapul".into()),
                false,
            ),
            None
        );

        let first_confirmation =
            first_read + std::time::Duration::from_millis(CANDIDATE_CONFIRM_MS);
        assert_eq!(
            state.due_action(first_confirmation),
            Some(TemporalAction::ConfirmRead)
        );
        assert_eq!(
            state.observed(
                first_confirmation,
                SettledObservation::Edited("Katapult".into()),
                true,
            ),
            None,
            "a changed edit must restart confirmation instead of learning the stale span"
        );

        let second_confirmation =
            first_confirmation + std::time::Duration::from_millis(CANDIDATE_CONFIRM_MS);
        assert_eq!(
            state.due_action(second_confirmation),
            Some(TemporalAction::ConfirmRead)
        );
        assert_eq!(
            state.observed(
                second_confirmation,
                SettledObservation::Edited("Katapult".into()),
                true,
            ),
            Some(TemporalAction::Learn("Katapult".into()))
        );
    }

    #[test]
    fn unproven_fallback_confirmation_cancels_when_edit_reverted() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, false);
        let first_read = start + std::time::Duration::from_millis(FALLBACK_CHECK_MS);
        assert_eq!(state.due_action(first_read), Some(TemporalAction::Read));
        assert_eq!(
            state.observed(
                first_read,
                SettledObservation::Edited("Katapul".into()),
                false,
            ),
            None
        );
        let confirmation = first_read + std::time::Duration::from_millis(CANDIDATE_CONFIRM_MS);
        assert_eq!(
            state.due_action(confirmation),
            Some(TemporalAction::ConfirmRead)
        );
        assert_eq!(
            state.observed(confirmation, SettledObservation::Unchanged, true),
            None
        );
        assert_eq!(state.last_safe_edit, None);
        assert_eq!(state.confirm_at, None);
    }

    #[test]
    fn unsupported_app_fallback_schedules_bounded_single_reads() {
        let start = std::time::Instant::now();
        let mut state = CaptureTemporalState::new(start, false);
        for step in 1..=4 {
            let due = start + std::time::Duration::from_millis(FALLBACK_CHECK_MS * step);
            assert_eq!(state.due_action(due), Some(TemporalAction::Read));
            assert_eq!(
                state.observed(due, SettledObservation::Unchanged, false),
                None
            );
            assert_eq!(
                state.next_deadline(),
                Some(due + std::time::Duration::from_millis(FALLBACK_CHECK_MS))
            );
        }
    }
}
