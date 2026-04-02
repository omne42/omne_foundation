use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::{DiagnosticsOptions, Id};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum CloseReasonPriority {
    Fallback,
    Primary,
    Diagnostic,
}

#[derive(Debug, Default)]
pub(crate) struct CloseReasonState {
    inner: Mutex<Option<(CloseReasonPriority, String)>>,
}

impl CloseReasonState {
    pub(crate) fn publish(&self, priority: CloseReasonPriority, reason: String) -> bool {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first_close = guard.is_none();
        let should_replace = match guard.as_ref() {
            None => true,
            Some((current_priority, _)) => priority > *current_priority,
        };
        if should_replace {
            *guard = Some((priority, reason));
        }
        first_close
    }

    pub(crate) fn get(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|(_, reason)| reason.clone())
    }

    #[cfg(test)]
    pub(crate) fn set_for_test(&self, priority: CloseReasonPriority, reason: impl Into<String>) {
        let _ = self.publish(priority, reason.into());
    }
}

pub(crate) fn no_runtime_close_reason(action: &str) -> String {
    format!("cannot {action} without a Tokio runtime; transport closed fail-closed")
}

pub(crate) fn no_time_driver_close_reason(action: &str) -> String {
    format!("cannot {action} without a Tokio time driver; transport closed fail-closed")
}

pub(crate) fn dropped_request_response_timeout_reason(timeout: std::time::Duration) -> String {
    format!("timed out after {timeout:?} while writing dropped request response")
}

pub(crate) type CancelledRequestIds = Arc<Mutex<CancelledRequestIdsState>>;

#[derive(Debug, Default)]
pub(crate) struct CancelledRequestIdsState {
    pub(crate) order: VecDeque<(u64, Id)>,
    latest: HashMap<Id, u64>,
    next_generation: u64,
}

pub(crate) const CANCELLED_REQUEST_IDS_MAX: usize = 1024;

pub(crate) fn remember_cancelled_request_id(cancelled_request_ids: &CancelledRequestIds, id: &Id) {
    let mut guard = lock_cancelled_request_ids(cancelled_request_ids);
    while guard.order.len() >= CANCELLED_REQUEST_IDS_MAX {
        let Some((generation, evicted)) = guard.order.pop_front() else {
            break;
        };
        if guard.latest.get(&evicted).copied() == Some(generation) {
            guard.latest.remove(&evicted);
        }
    }
    let generation = guard.next_generation;
    guard.next_generation = guard.next_generation.wrapping_add(1);
    guard.order.push_back((generation, id.clone()));
    guard.latest.insert(id.clone(), generation);
}

pub(crate) fn take_cancelled_request_id(
    cancelled_request_ids: &CancelledRequestIds,
    id: &Id,
) -> bool {
    let mut guard = lock_cancelled_request_ids(cancelled_request_ids);
    guard.latest.remove(id).is_some()
}

pub(crate) fn take_cancelled_request_id_type_mismatch(
    cancelled_request_ids: &CancelledRequestIds,
    id: &Id,
) -> bool {
    let Some(candidate) = type_mismatch_candidate_id(id) else {
        return false;
    };

    let mut guard = lock_cancelled_request_ids(cancelled_request_ids);
    guard.latest.remove(&candidate).is_some()
}

pub(crate) fn lock_cancelled_request_ids(
    cancelled_request_ids: &CancelledRequestIds,
) -> std::sync::MutexGuard<'_, CancelledRequestIdsState> {
    cancelled_request_ids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn type_mismatch_candidate_id(id: &Id) -> Option<Id> {
    match id {
        Id::Integer(value) => Some(Id::String(value.to_string())),
        Id::Unsigned(value) => Some(Id::String(value.to_string())),
        Id::String(value) => parse_stringified_numeric_id(value),
    }
}

fn parse_stringified_numeric_id(value: &str) -> Option<Id> {
    match value.parse::<i64>() {
        Ok(parsed) if parsed.to_string() == value => return Some(Id::Integer(parsed)),
        _ => {}
    }

    match value.parse::<u64>() {
        Ok(parsed) if parsed.to_string() == value => Some(Id::Unsigned(parsed)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientStats {
    pub invalid_json_lines: u64,
    pub dropped_notifications_full: u64,
    pub dropped_notifications_closed: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ClientStatsInner {
    pub(crate) invalid_json_lines: AtomicU64,
    pub(crate) dropped_notifications_full: AtomicU64,
    pub(crate) dropped_notifications_closed: AtomicU64,
}

impl ClientStatsInner {
    pub(crate) fn snapshot(&self) -> ClientStats {
        ClientStats {
            invalid_json_lines: self.invalid_json_lines.load(Ordering::Relaxed),
            dropped_notifications_full: self.dropped_notifications_full.load(Ordering::Relaxed),
            dropped_notifications_closed: self.dropped_notifications_closed.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
pub(crate) struct DiagnosticsState {
    invalid_json_samples: Mutex<VecDeque<String>>,
    invalid_json_sample_lines: usize,
    invalid_json_sample_max_bytes: usize,
}

impl DiagnosticsState {
    pub(crate) fn new(opts: &DiagnosticsOptions) -> Option<Arc<Self>> {
        if opts.invalid_json_sample_lines == 0 {
            return None;
        }
        Some(Arc::new(Self {
            invalid_json_samples: Mutex::new(VecDeque::with_capacity(
                opts.invalid_json_sample_lines,
            )),
            invalid_json_sample_lines: opts.invalid_json_sample_lines,
            invalid_json_sample_max_bytes: opts.invalid_json_sample_max_bytes.max(1),
        }))
    }

    pub(crate) fn record_invalid_json_line(&self, line: &[u8]) {
        let mut guard = self
            .invalid_json_samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if guard.len() >= self.invalid_json_sample_lines {
            guard.pop_front();
        }

        let sample_len = line.len().min(self.invalid_json_sample_max_bytes);
        let mut s = String::from_utf8_lossy(&line[..sample_len]).into_owned();
        if sample_len < line.len() {
            s.push('…');
        }
        s = truncate_string(s, self.invalid_json_sample_max_bytes);
        guard.push_back(s);
    }

    pub(crate) fn invalid_json_samples(&self) -> Vec<String> {
        self.invalid_json_samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

fn truncate_string(mut s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    s.truncate(end);
    s
}
