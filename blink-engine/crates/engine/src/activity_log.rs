//! Thread-safe activity log — a ring buffer of recent engine events.
//!
//! The [`PaperEngine`] pushes entries here; the TUI reads them for display.
//! Uses `std::sync::Mutex` (not tokio) so it can be written from any thread.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::Local;

// ─── Types ────────────────────────────────────────────────────────────────────

pub const MAX_ENTRIES: usize = 200;

/// Severity / colour hint for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Engine, // grey  — startup, connection events
    Signal, // cyan  — RN1 order detected
    Fill,   // green — paper order filled
    Abort,  // red   — drift failsafe triggered
    Skip,   // yellow — signal skipped (size too small)
    Warn,   // yellow — generic warning
}

#[derive(Debug, Clone)]
pub struct ActivityEntry {
    /// Wall-clock time, formatted as `HH:MM:SS`.
    pub timestamp: String,
    pub kind: EntryKind,
    pub message: String,
}

/// Shared activity log handle.
pub type ActivityLog = Arc<Mutex<VecDeque<ActivityEntry>>>;

// ─── Constructor ─────────────────────────────────────────────────────────────

/// Creates a new, empty activity log.
pub fn new_activity_log() -> ActivityLog {
    Arc::new(Mutex::new(VecDeque::with_capacity(MAX_ENTRIES)))
}

// ─── Push helper ─────────────────────────────────────────────────────────────

/// Appends an entry, evicting the oldest if the buffer is full.
pub fn push(log: &ActivityLog, kind: EntryKind, message: impl Into<String>) {
    let timestamp = Local::now().format("%H:%M:%S").to_string();
    let entry = ActivityEntry {
        timestamp,
        kind,
        message: message.into(),
    };
    let mut deque = log.lock().unwrap();
    if deque.len() >= MAX_ENTRIES {
        deque.pop_front();
    }
    deque.push_back(entry);
}
