//! In-place dictionary capture (macOS): learn corrections from edits the user
//! makes to pasted text in the target application.
//!
//! Implements the design in `docs/DICTIONARY_DESIGN.md` sections 7.3 and 16.3:
//!
//! - **Snapshot** just before the paste: focused AX element + caret position.
//!   Runs on a dedicated AX thread with a 100 ms budget; the paste never waits
//!   longer, and on timeout capture is skipped for this dictation. The
//!   snapshot never reads field content.
//! - **Check** later: read a bounded window around the anchor
//!   (`AXStringForRange`, falling back to a truncated `AXValue` read), compare
//!   with what was pasted, and feed differences through the same
//!   [`crate::dictionary::learn_pairs`] gates as History edits.
//! - **Triggers**: the next dictation start, a focused-app change (polled from
//!   the AX thread — no main-thread observer needed), and a 20 s timer.
//!   Anchors expire after 180 s.
//!
//! Every AX call happens on the dedicated thread with per-element messaging
//! timeouts, so an unresponsive target application can never stall dictation.
//! All other platforms get a no-op manager.

#![allow(dead_code)]

use tauri::AppHandle;

const SNAPSHOT_BUDGET_MS: u64 = 100;
const CHECK_INTERVAL_SECS: u64 = 20;
const POLL_TICK_SECS: u64 = 5;
const ANCHOR_TTL_SECS: u64 = 180;
/// UTF-16 units read before the anchor and slack after the pasted length,
/// bounding how much target text is ever read on the AXStringForRange path.
const PRE_MARGIN: usize = 8;
const POST_MARGIN: usize = 96;
/// Fallback full-value reads are truncated to this many UTF-16 units around
/// the anchor before anything else looks at them (design doc section 16.4).
const FALLBACK_WINDOW: usize = 16 * 1024;
/// At most this many entries are auto-added per check.
const MAX_LEARNED_PER_CHECK: usize = 3;

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug, specta::Type, tauri_specta::Event)]
pub struct DictionaryLearnedEvent {
    pub entries: Vec<crate::dictionary::DictionaryEntry>,
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
            let (ack_tx, ack_rx) = std::sync::mpsc::channel();
            if self
                .tx
                .send(macos_impl::Msg::Snapshot {
                    pasted: pasted_text,
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
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::*;
    use crate::dictionary::{learn_pairs, DictionaryEntry};
    use accessibility_sys::{
        kAXErrorSuccess, AXError, AXUIElementCopyAttributeValue,
        AXUIElementCopyParameterizedAttributeValue, AXUIElementCreateSystemWide, AXUIElementGetPid,
        AXUIElementRef, AXUIElementSetMessagingTimeout, AXValueCreate, AXValueGetValue, AXValueRef,
    };
    use core_foundation::base::{CFRange, CFRelease, CFTypeRef, TCFType};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use log::{debug, info, warn};
    use std::sync::mpsc::{Receiver, Sender};
    use std::time::{Duration, Instant};
    use tauri_specta::Event as _;

    /// kAXValueCFRangeType (AXValue.h). accessibility-sys spells it as an enum
    /// constant; the numeric value is stable ABI.
    const AX_VALUE_CFRANGE_TYPE: u32 = 4;

    pub enum Msg {
        Snapshot { pasted: String, ack: Sender<()> },
        Check,
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

    struct Anchor {
        element: Retained,
        pid: i32,
        /// Caret position (UTF-16 units) just before the paste keystroke.
        caret: usize,
        pasted: String,
        pasted_u16: usize,
        created: Instant,
        last_check: Instant,
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
            match rx.recv_timeout(Duration::from_secs(POLL_TICK_SECS)) {
                Ok(Msg::Snapshot { pasted, ack }) => {
                    anchor = take_snapshot(pasted);
                    let _ = ack.send(());
                }
                Ok(Msg::Check) => {
                    anchor = run_check(&app, anchor);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    anchor = on_tick(&app, anchor);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn on_tick(app: &AppHandle, anchor: Option<Anchor>) -> Option<Anchor> {
        let a = anchor?;
        if a.created.elapsed() > Duration::from_secs(ANCHOR_TTL_SECS) {
            debug!("capture: anchor expired");
            return None;
        }
        // Focus moved to another app, or the periodic interval elapsed: check.
        let focus_changed = focused_pid().is_some_and(|pid| pid != a.pid);
        if focus_changed || a.last_check.elapsed() > Duration::from_secs(CHECK_INTERVAL_SECS) {
            return run_check(app, Some(a));
        }
        Some(a)
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

    fn focused_pid() -> Option<i32> {
        focused_element().map(|(_, pid)| pid)
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
        let Some(range) = selected_range(element.0 as AXUIElementRef) else {
            info!("capture: focused element reports no selected-text range (app may not expose AX text); skipping");
            return None;
        };
        if range.location < 0 {
            return None;
        }
        let pasted_u16 = pasted.encode_utf16().count();
        debug!("capture: anchored at {} in pid {}", range.location, pid);
        Some(Anchor {
            element,
            pid,
            caret: range.location as usize,
            pasted,
            pasted_u16,
            created: Instant::now(),
            last_check: Instant::now(),
        })
    }

    fn run_check(app: &AppHandle, anchor: Option<Anchor>) -> Option<Anchor> {
        let mut a = anchor?;
        a.last_check = Instant::now();

        let start = a.caret.saturating_sub(PRE_MARGIN);
        let len = a.pasted_u16 + PRE_MARGIN + POST_MARGIN;
        let window = match read_window(a.element.0 as AXUIElementRef, start, len) {
            Some(w) => w,
            None => {
                info!("capture: window read failed (app closed or AX text unsupported); dropping anchor");
                return None;
            }
        };

        if window.contains(a.pasted.trim_end()) {
            // Unchanged so far; keep watching until the anchor expires.
            debug!("capture: pasted text unchanged; watching");
            return Some(a);
        }

        let mut learned = learn_pairs(a.pasted.trim_end(), window.trim());
        if learned.is_empty() {
            // Changed but nothing passed the gates — could be mid-edit or a
            // rewrite. Keep the anchor; a later check may see a settled edit.
            // Counts only — never log field content.
            info!(
                "capture: edit detected but no pair passed the learn gates (window {} chars vs pasted {} chars); keeping anchor",
                window.chars().count(),
                a.pasted.chars().count()
            );
            return Some(a);
        }
        learned.truncate(MAX_LEARNED_PER_CHECK);
        for e in &mut learned {
            e.source = "capture".to_string();
        }

        store_learned(app, learned);
        None // learned once; this dictation's anchor is done
    }

    fn store_learned(app: &AppHandle, learned: Vec<DictionaryEntry>) {
        let mut settings = crate::settings::get_settings(app);
        let mut added: Vec<DictionaryEntry> = Vec::new();
        for entry in learned {
            let dup = settings.dictionary_entries.iter().any(|e| {
                e.wrong.to_lowercase() == entry.wrong.to_lowercase() && e.right == entry.right
            });
            if !dup {
                settings.dictionary_entries.push(entry.clone());
                added.push(entry);
            }
        }
        if added.is_empty() {
            return;
        }
        for e in &added {
            info!("capture: learned '{}' -> '{}'", e.wrong, e.right);
        }
        crate::settings::write_settings(app, settings);
        if let Err(err) = (DictionaryLearnedEvent { entries: added }).emit(app) {
            warn!("capture: failed to emit learned event: {err}");
        }
    }
}
