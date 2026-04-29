//! Minimal divergence journal, self-contained for the MVP.
//!
//! We intentionally do NOT depend on the `blink-journal` crate today —
//! its `Journal` trait uses `async fn shutdown` which would pull tokio
//! into the shadow runner, and the rubber-duck review explicitly vetoed
//! async/ClickHouse for this MVP. The follow-up todo `p0-shadow-hook`
//! can reconcile this once `blink-journal` exposes a sync-friendly
//! entry point.

use std::sync::Mutex;

use crate::divergence::DivergenceRecord;

/// Sink that accepts divergence records.
pub trait ShadowJournal: Send {
    /// Record one divergence.
    fn record(&self, record: DivergenceRecord);
    /// Drain all accumulated records. After draining the journal may
    /// still accept new records.
    fn drain(&self) -> Vec<DivergenceRecord>;
}

/// In-memory divergence journal.
#[derive(Debug, Default)]
pub struct MemoryJournal {
    rows: Mutex<Vec<DivergenceRecord>>,
}

impl MemoryJournal {
    /// Construct an empty journal.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many records are currently stored.
    pub fn len(&self) -> usize {
        self.rows.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether no records are stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ShadowJournal for MemoryJournal {
    fn record(&self, record: DivergenceRecord) {
        if let Ok(mut g) = self.rows.lock() {
            g.push(record);
        }
    }

    fn drain(&self) -> Vec<DivergenceRecord> {
        self.rows
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }
}
