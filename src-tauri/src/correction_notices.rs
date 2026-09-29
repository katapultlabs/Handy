//! One FIFO for correction decisions, independent of recording visibility.
//! All native presentation changes are serialized on the main thread. A notice
//! timer starts only after the frontend acknowledges rendering that token.
use crate::dictionary_store::{DictionaryManager, DictionaryRow};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const READING_MS: u64 = 8_000;
const CHANGED: &str = "correction-notice-changed";

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct CorrectionNotice {
    pub token: u64,
    pub entry: DictionaryRow,
    pub remaining_ms: u64,
    pub pending_count: u32,
    pub busy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct CorrectionNoticeSnapshot {
    pub revision: u64,
    pub recording: bool,
    pub notice: Option<CorrectionNotice>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionNoticeAction {
    Accept,
    Reject,
}

struct ActiveNotice {
    entry: DictionaryRow,
    token: u64,
    remaining: u64,
    started: Option<u64>,
    acknowledged: bool,
    paused: bool,
    busy: bool,
}

impl ActiveNotice {
    fn remaining_at(&self, now: u64) -> u64 {
        self.remaining
            .saturating_sub(self.started.map_or(0, |start| now.saturating_sub(start)))
    }

    fn stop_clock(&mut self, now: u64) {
        self.remaining = self.remaining_at(now);
        self.started = None;
    }
}

#[derive(Default)]
struct NoticeQueue {
    pending: VecDeque<DictionaryRow>,
    active: Option<ActiveNotice>,
    recording: bool,
    revision: u64,
    next_token: u64,
}

impl NoticeQueue {
    fn changed(&mut self) {
        self.revision += 1;
    }

    fn activate_next(&mut self) {
        if self.recording || self.active.is_some() {
            return;
        }
        if let Some(entry) = self.pending.pop_front() {
            self.next_token += 1;
            self.active = Some(ActiveNotice {
                entry,
                token: self.next_token,
                remaining: READING_MS,
                started: None,
                acknowledged: false,
                paused: false,
                busy: false,
            });
        }
    }

    fn enqueue(&mut self, entries: Vec<DictionaryRow>) {
        for entry in entries {
            if entry.state == "rejected"
                || self.active.as_ref().is_some_and(|a| a.entry.id == entry.id)
                || self.pending.iter().any(|p| p.id == entry.id)
            {
                continue;
            }
            self.pending.push_back(entry);
        }
        self.activate_next();
        self.changed();
    }

    fn set_recording(&mut self, recording: bool, now: u64) {
        if self.recording == recording {
            return;
        }
        self.recording = recording;
        if let Some(active) = &mut self.active {
            active.stop_clock(now);
            self.next_token += 1;
            active.token = self.next_token;
            active.acknowledged = false;
            active.paused = false;
        }
        // A recording event that arrived at the expiry boundary must not
        // leave a zero-duration card stuck waiting to be acknowledged.
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.remaining == 0 && !a.busy)
        {
            self.active = None;
        }
        self.activate_next();
        self.changed();
    }

    fn current_mut(&mut self, token: u64) -> Option<&mut ActiveNotice> {
        if self.recording {
            return None;
        }
        self.active.as_mut().filter(|a| a.token == token)
    }

    fn acknowledge(&mut self, token: u64, now: u64) {
        if let Some(active) = self.current_mut(token) {
            if active.acknowledged {
                return;
            }
            active.acknowledged = true;
            if !active.paused && !active.busy {
                active.started = Some(now);
            }
            self.changed();
        }
    }

    fn pause(&mut self, token: u64, paused: bool, now: u64) {
        if let Some(active) = self.current_mut(token) {
            if active.paused == paused {
                return;
            }
            active.stop_clock(now);
            active.paused = paused;
            if !paused && active.acknowledged && !active.busy {
                active.started = Some(now);
            }
            self.changed();
        }
    }

    fn dismiss(&mut self, token: u64) {
        if self.current_mut(token).is_some_and(|a| !a.busy) {
            self.active = None;
            self.activate_next();
            self.changed();
        }
    }

    fn remove_entry(&mut self, id: i64) {
        self.pending.retain(|entry| entry.id != id);
        if self.active.as_ref().is_some_and(|a| a.entry.id == id) {
            self.active = None;
        }
        self.activate_next();
        self.changed();
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.active = None;
        self.changed();
    }

    fn begin_action(&mut self, token: u64, now: u64) -> Result<i64, String> {
        let active = self
            .current_mut(token)
            .ok_or("This correction notice has changed")?;
        if active.busy {
            return Err("A correction action is already running".into());
        }
        active.stop_clock(now);
        active.busy = true;
        let id = active.entry.id;
        self.changed();
        Ok(id)
    }

    fn failed_action(&mut self, id: i64) {
        if let Some(active) = &mut self.active {
            if active.entry.id == id {
                active.busy = false;
                // Preserve the failed decision on screen for a retry. A user
                // action must not consume its remaining reading time.
                active.paused = true;
                self.changed();
            }
        }
    }

    fn timer(&self) -> Option<(u64, u64)> {
        let active = self.active.as_ref()?;
        if self.recording || active.paused || active.busy {
            return None;
        }
        active
            .started
            .map(|start| (active.token, start + active.remaining))
    }

    fn expire(&mut self, token: u64, deadline: u64, now: u64) {
        if self.timer() == Some((token, deadline)) && now >= deadline {
            self.active = None;
            self.activate_next();
            self.changed();
        }
    }

    fn snapshot(&self, now: u64) -> CorrectionNoticeSnapshot {
        CorrectionNoticeSnapshot {
            revision: self.revision,
            recording: self.recording,
            notice: if self.recording {
                None
            } else {
                self.active.as_ref().map(|a| CorrectionNotice {
                    token: a.token,
                    entry: a.entry.clone(),
                    remaining_ms: a.remaining_at(now),
                    pending_count: self.pending.len().min(u32::MAX as usize) as u32,
                    busy: a.busy,
                })
            },
        }
    }
}

static QUEUE: OnceLock<Mutex<NoticeQueue>> = OnceLock::new();
static CLOCK: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn queue() -> &'static Mutex<NoticeQueue> {
    QUEUE.get_or_init(|| Mutex::new(NoticeQueue::default()))
}

/// Must run on the main thread: queue changes and native presentation form one
/// ordered operation, including recording starts, timer expiry, and UI actions.
fn mutate<R>(
    app: &AppHandle,
    force_show: bool,
    change: impl FnOnce(&mut NoticeQueue, u64) -> Result<R, String>,
) -> Result<(R, CorrectionNoticeSnapshot), String> {
    let now = now_ms();
    let (result, before, after, old_timer, timer) = {
        let mut state = queue()
            .lock()
            .map_err(|_| "Correction notice queue unavailable")?;
        let before = state.snapshot(now);
        let old_timer = state.timer();
        let result = change(&mut state, now)?;
        let after = state.snapshot(now);
        (result, before, after, old_timer, state.timer())
    };
    if let Some(notice) = &after.notice {
        if force_show || before.notice.as_ref().map(|n| n.token) != Some(notice.token) {
            log::debug!(
                "correction notice: showing token={} pending={} remaining_ms={}",
                notice.token,
                notice.pending_count,
                notice.remaining_ms
            );
            crate::overlay::show_correction_notice_on_main(app);
        }
    } else if !after.recording && before.notice.is_some() {
        crate::overlay::hide_overlay_window_on_main(app);
    }
    if force_show || before.revision != after.revision {
        let _ = app.emit_to("recording_overlay", CHANGED, &after);
    }
    if timer != old_timer {
        if let Some((token, deadline)) = timer {
            schedule_expiry(app.clone(), token, deadline);
        }
    }
    Ok((result, after))
}

// Keep timer recursion outside generic `mutate` so each callback has a finite
// concrete type and scheduling does not recursively monomorphize the presenter.
fn schedule_expiry(handle: AppHandle, token: u64, deadline: u64) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(deadline.saturating_sub(now_ms()))).await;
        let cloned = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let _ = mutate(&cloned, false, |state, now| {
                state.expire(token, deadline, now);
                Ok(())
            });
        });
    });
}

async fn on_main<R: Send + 'static>(
    app: AppHandle,
    force_show: bool,
    change: impl FnOnce(&mut NoticeQueue, u64) -> Result<R, String> + Send + 'static,
) -> Result<(R, CorrectionNoticeSnapshot), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cloned = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(mutate(&cloned, force_show, change));
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "Correction notice presenter unavailable".to_string())?
}

/// Both History and capture publish only newly committed pairs here. Previously
/// seen pairs remain silent, and all pairs remain available in Dictionary.
pub fn enqueue(app: &AppHandle, entries: Vec<DictionaryRow>) {
    if entries.is_empty() {
        return;
    }
    let cloned = app.clone();
    let _ = app.run_on_main_thread(move || {
        let settings = crate::settings::get_settings(&cloned);
        if !settings.experimental_enabled || !settings.dictionary_enabled {
            return;
        }
        let _ = mutate(&cloned, false, |state, _| {
            state.enqueue(entries);
            Ok(())
        });
    });
}

pub fn remove_entry(app: &AppHandle, id: i64) {
    let cloned = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = mutate(&cloned, false, |state, _| {
            state.remove_entry(id);
            Ok(())
        });
    });
}

/// Turning Dictionary off clears transient notices; stored choices remain.
pub fn clear(app: &AppHandle) {
    let cloned = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = mutate(&cloned, false, |state, _| {
            state.clear();
            Ok(())
        });
    });
}

pub fn recording_started_on_main(app: &AppHandle) {
    let _ = mutate(app, false, |state, now| {
        state.set_recording(true, now);
        Ok(())
    });
}

/// Returns whether a correction is now using the shared window.
pub fn recording_finished_on_main(app: &AppHandle) -> bool {
    mutate(app, false, |state, now| {
        state.set_recording(false, now);
        Ok(())
    })
    .ok()
    .is_some_and(|(_, snapshot)| snapshot.notice.is_some())
}

#[tauri::command]
#[specta::specta]
pub async fn get_correction_notice(app: AppHandle) -> Result<CorrectionNoticeSnapshot, String> {
    on_main(app, true, |_, _| Ok(()))
        .await
        .map(|(_, snapshot)| snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn acknowledge_correction_notice(
    app: AppHandle,
    token: u64,
) -> Result<CorrectionNoticeSnapshot, String> {
    on_main(app, false, move |state, now| {
        let first_ack = state
            .current_mut(token)
            .is_some_and(|active| !active.acknowledged);
        state.acknowledge(token, now);
        if first_ack {
            log::debug!("correction notice: rendered token={token}");
        }
        Ok(())
    })
    .await
    .map(|(_, snapshot)| snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn pause_correction_notice(
    app: AppHandle,
    token: u64,
    paused: bool,
) -> Result<CorrectionNoticeSnapshot, String> {
    on_main(app, false, move |state, now| {
        state.pause(token, paused, now);
        Ok(())
    })
    .await
    .map(|(_, snapshot)| snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn dismiss_correction_notice(
    app: AppHandle,
    token: u64,
) -> Result<CorrectionNoticeSnapshot, String> {
    on_main(app, false, move |state, _| {
        state.dismiss(token);
        Ok(())
    })
    .await
    .map(|(_, snapshot)| snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn act_on_correction_notice(
    app: AppHandle,
    token: u64,
    action: CorrectionNoticeAction,
) -> Result<CorrectionNoticeSnapshot, String> {
    let (id, _) = on_main(app.clone(), false, move |state, now| {
        state.begin_action(token, now)
    })
    .await?;
    let manager = app
        .try_state::<Arc<DictionaryManager>>()
        .map(|m| Arc::clone(&m));
    let result = if let Some(manager) = manager {
        tauri::async_runtime::spawn_blocking(move || match action {
            CorrectionNoticeAction::Accept => manager.confirm_dictionary_entry(id),
            CorrectionNoticeAction::Reject => manager.reject_dictionary_entry(id),
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|result| result.map_err(|e| e.to_string()))
    } else {
        Err("Dictionary is not ready".into())
    };
    let succeeded = result.is_ok();
    let (_, snapshot) = on_main(app, false, move |state, _| {
        if succeeded {
            state.remove_entry(id);
        } else {
            state.failed_action(id);
        }
        Ok(())
    })
    .await?;
    result.map(|_| snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::CaseMode;

    fn row(id: i64) -> DictionaryRow {
        DictionaryRow {
            id,
            wrong: format!("wrong{id}"),
            right: format!("right{id}"),
            case_mode: CaseMode::Exact,
            source: "capture".into(),
            state: "proposed".into(),
            enabled: true,
            auto_learned: false,
            context_words: vec![],
            seen_count: 1,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn token(q: &NoticeQueue) -> u64 {
        q.active.as_ref().unwrap().token
    }

    #[test]
    fn disabling_discards_notices_and_invalidates_in_flight_timers() {
        let mut q = NoticeQueue::default();
        q.enqueue(vec![row(1), row(2)]);
        let old = token(&q);
        q.acknowledge(old, 0);
        q.clear();
        assert!(q.snapshot(100).notice.is_none());
        q.set_recording(true, 100);
        q.set_recording(false, 200);
        assert!(q.snapshot(200).notice.is_none());
        q.enqueue(vec![row(3)]);
        q.expire(old, 8000, 8000);
        assert_eq!(q.snapshot(8000).notice.unwrap().entry.id, 3);
        assert!(q.begin_action(old, 8000).is_err());
    }

    #[test]
    fn burst_batches_are_fifo_and_deduplicated() {
        let mut q = NoticeQueue::default();
        q.set_recording(true, 0);
        q.enqueue(vec![row(1), row(2)]);
        q.enqueue(vec![row(2), row(3)]);
        assert!(q.snapshot(0).notice.is_none());
        q.set_recording(false, 100);
        for id in [1, 2, 3] {
            assert_eq!(q.snapshot(100).notice.unwrap().entry.id, id);
            q.dismiss(token(&q));
        }
        assert!(q.snapshot(100).notice.is_none());
    }

    #[test]
    fn reading_time_starts_only_after_actual_render_acknowledgement() {
        let mut q = NoticeQueue::default();
        q.enqueue(vec![row(1)]);
        let id = token(&q);
        assert!(q.timer().is_none());
        q.acknowledge(id, 100_000);
        assert_eq!(q.timer(), Some((id, 108_000)));
        q.acknowledge(id, 105_000);
        assert_eq!(q.timer(), Some((id, 108_000)));
    }

    #[test]
    fn recording_restores_remaining_reading_time_with_a_fresh_token() {
        let mut q = NoticeQueue::default();
        q.enqueue(vec![row(1), row(2)]);
        let old = token(&q);
        q.acknowledge(old, 0);
        q.set_recording(true, 2000);
        assert!(q.snapshot(3000).notice.is_none());
        q.expire(old, 8000, 9000);
        q.set_recording(false, 10_000);
        let new = token(&q);
        assert_ne!(new, old);
        assert_eq!(q.snapshot(10_000).notice.unwrap().remaining_ms, 6000);
        q.acknowledge(old, 11_000);
        assert!(q.timer().is_none());
        q.acknowledge(new, 11_000);
        assert_eq!(q.timer(), Some((new, 17_000)));
        q.expire(old, 8000, 12_000);
        assert_eq!(q.active.as_ref().unwrap().entry.id, 1);
    }

    #[test]
    fn hover_and_slow_actions_do_not_consume_reading_time() {
        let mut q = NoticeQueue::default();
        q.enqueue(vec![row(1)]);
        let id = token(&q);
        q.acknowledge(id, 0);
        q.pause(id, true, 1000);
        assert!(q.timer().is_none());
        q.pause(id, false, 10_000);
        assert_eq!(q.timer(), Some((id, 17_000)));
        q.begin_action(id, 11_000).unwrap();
        assert!(q.timer().is_none());
        q.failed_action(1);
        assert!(q.timer().is_none());
        q.pause(id, false, 20_000);
        assert_eq!(q.timer(), Some((id, 26_000)));
    }

    #[test]
    fn old_timer_cannot_hide_next_notice_and_each_gets_full_time() {
        let mut q = NoticeQueue::default();
        q.enqueue(vec![row(1)]);
        let old = token(&q);
        q.acknowledge(old, 0);
        q.enqueue(vec![row(2)]);
        q.dismiss(old);
        let new = token(&q);
        q.acknowledge(new, 3000);
        q.expire(old, 8000, 8000);
        assert_eq!(q.active.as_ref().unwrap().entry.id, 2);
        assert_eq!(q.timer(), Some((new, 11_000)));
        q.expire(new, 11_000, 11_000);
        assert!(q.active.is_none());
    }

    #[test]
    fn stale_clicks_and_external_decisions_do_not_affect_next_entry() {
        let mut q = NoticeQueue::default();
        q.enqueue(vec![row(1), row(2), row(3)]);
        let old = token(&q);
        q.remove_entry(1);
        q.remove_entry(3);
        assert!(q.begin_action(old, 0).is_err());
        q.dismiss(old);
        assert_eq!(q.active.as_ref().unwrap().entry.id, 2);
        assert_eq!(q.snapshot(0).notice.unwrap().pending_count, 0);
    }

    #[test]
    fn action_can_complete_after_recording_interrupts_its_notice() {
        let mut q = NoticeQueue::default();
        q.enqueue(vec![row(1), row(2)]);
        let id = q.begin_action(token(&q), 0).unwrap();
        q.set_recording(true, 1);
        q.remove_entry(id);
        q.set_recording(false, 2);
        assert_eq!(q.active.as_ref().unwrap().entry.id, 2);
        assert!(!q.active.as_ref().unwrap().busy);
    }
}
