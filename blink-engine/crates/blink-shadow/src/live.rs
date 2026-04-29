//! Live shadow capture runner.
//!
//! Provides `LiveShadowRunner` which collects `CapturedRow` observations from
//! the hot path (non-blocking push to an SPSC ring) and writes them to hourly-rotated
//! JSON Lines files in a dedicated writer thread.
//!
//! This module is distinct from the replay `ShadowRunner` — it owns a bounded ring
//! and writes data as it arrives, rather than reading from a journal.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Captured observation row for shadow comparison.
///
/// Field names match the spec: compact variant tags for decisions,
/// optional intent hashes, config hash, book reference hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedRow {
    /// Sequential event number assigned by the producer.
    pub event_seq: u64,
    /// Logical timestamp (wall-clock ns since epoch) when event was captured.
    pub logical_now_ns: u64,
    /// Legacy decision in compact form: "Submitted" | "Aborted:<reason>" | "NoOp:<code>"
    pub legacy_decision: String,
    /// V1 kernel decision in the same compact form.
    pub v1_decision: String,
    /// Legacy intent hash (semantic key), if applicable.
    #[serde(with = "hex_option")]
    pub legacy_intent_hash: Option<[u8; 32]>,
    /// V1 kernel intent hash (semantic key), if applicable.
    #[serde(with = "hex_option")]
    pub v1_intent_hash: Option<[u8; 32]>,
    /// Keccak256 hash of the v1 kernel config used for this decision.
    #[serde(with = "hex_array")]
    pub config_hash_v1: [u8; 32],
    /// Keccak256 hash of the book reference (top bid + top ask).
    #[serde(with = "hex_array")]
    pub book_ref_hash: [u8; 32],
}

mod hex_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S>(data: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match data {
            Some(bytes) => hex::encode(bytes).serialize(s),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D>(d: D) -> Result<Option<[u8; 32]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            Some(s) => {
                let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
                if bytes.len() != 32 {
                    return Err(serde::de::Error::custom("expected 32 bytes"));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                Ok(Some(arr))
            }
            None => Ok(None),
        }
    }
}

mod hex_array {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S>(data: &[u8; 32], s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        hex::encode(data).serialize(s)
    }
    pub fn deserialize<'de, D>(d: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("expected 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

/// Public snapshot of atomic counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShadowCounters {
    /// Total rows observed (pushed to ring).
    pub observed_total: u64,
    /// Total rows where legacy ≠ v1 decision.
    pub diverged_total: u64,
    /// Total rows dropped due to ring being full.
    pub dropped_total: u64,
    /// Total panics caught in the v1 kernel (tracked separately in ShadowCtx).
    pub panic_total: u64,
}

/// Live shadow capture runner.
///
/// Owns a bounded SPSC ring and spawns a dedicated writer thread that batches
/// rows and writes them to hourly-rotated JSON Lines files in `out_dir`.
///
/// The `observe()` method is non-blocking and drops rows if the ring is full
/// (incrementing `dropped_total`). This ensures the hot path never blocks.
pub struct LiveShadowRunner {
    ring_tx: Arc<std::sync::Mutex<blink_rings::Producer<CapturedRow>>>,
    observed_total: Arc<AtomicU64>,
    diverged_total: Arc<AtomicU64>,
    dropped_total: Arc<AtomicU64>,
    writer_handle: Option<JoinHandle<()>>,
    shutdown_flag: Arc<AtomicU64>,
}

impl LiveShadowRunner {
    /// Create a new runner with the specified output directory.
    ///
    /// The directory will be created if it doesn't exist. The writer thread
    /// is spawned immediately and begins waiting for rows.
    pub fn new(out_dir: impl AsRef<Path>) -> Self {
        let out_dir = out_dir.as_ref().to_path_buf();
        fs::create_dir_all(&out_dir).expect("failed to create shadow output directory");

        let (tx, rx) = blink_rings::bounded::<CapturedRow>(4096);

        let observed_total = Arc::new(AtomicU64::new(0));
        let diverged_total = Arc::new(AtomicU64::new(0));
        let dropped_total = Arc::new(AtomicU64::new(0));
        let shutdown_flag = Arc::new(AtomicU64::new(0));

        let writer_handle = {
            let diverged = Arc::clone(&diverged_total);
            let shutdown = Arc::clone(&shutdown_flag);
            thread::spawn(move || {
                writer_thread(rx, out_dir, diverged, shutdown);
            })
        };

        Self {
            ring_tx: Arc::new(std::sync::Mutex::new(tx)),
            observed_total,
            diverged_total,
            dropped_total,
            writer_handle: Some(writer_handle),
            shutdown_flag,
        }
    }

    /// Observe a captured row. Non-blocking; drops if ring is full.
    pub fn observe(&self, row: CapturedRow) {
        self.observed_total.fetch_add(1, Ordering::Relaxed);
        
        // Check for divergence
        if row.legacy_decision != row.v1_decision {
            self.diverged_total.fetch_add(1, Ordering::Relaxed);
        }

        // Try to push to ring; drop if full
        let mut tx = self.ring_tx.lock().unwrap();
        if tx.push(row).is_err() {
            self.dropped_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get a snapshot of current counters.
    pub fn counters(&self) -> ShadowCounters {
        ShadowCounters {
            observed_total: self.observed_total.load(Ordering::Relaxed),
            diverged_total: self.diverged_total.load(Ordering::Relaxed),
            dropped_total: self.dropped_total.load(Ordering::Relaxed),
            panic_total: 0, // tracked separately in ShadowCtx
        }
    }

    /// Shutdown the writer thread and flush pending rows.
    ///
    /// Blocks until the writer thread finishes.
    pub fn shutdown(mut self) {
        self.shutdown_flag.store(1, Ordering::Relaxed);
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LiveShadowRunner {
    fn drop(&mut self) {
        // Signal shutdown if not already done
        self.shutdown_flag.store(1, Ordering::Relaxed);
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }
    }
}

fn writer_thread(
    mut rx: blink_rings::Consumer<CapturedRow>,
    out_dir: PathBuf,
    diverged_total: Arc<AtomicU64>,
    shutdown: Arc<AtomicU64>,
) {
    let mut batch = Vec::with_capacity(4096);
    let mut current_file: Option<(BufWriter<File>, String)> = None;
    let tick_duration = Duration::from_secs(5);
    let mut last_flush = Instant::now();

    loop {
        // Drain available rows into batch
        while batch.len() < 4096 {
            match rx.pop() {
                Some(row) => batch.push(row),
                None => break,
            }
        }

        // Flush batch if:
        // - batch is full (4096)
        // - tick duration elapsed (5s)
        // - shutdown requested
        let should_flush = batch.len() >= 4096
            || last_flush.elapsed() >= tick_duration
            || shutdown.load(Ordering::Relaxed) != 0;

        if should_flush && !batch.is_empty() {
            flush_batch(&mut batch, &out_dir, &mut current_file, &diverged_total);
            batch.clear();
            last_flush = Instant::now();
        }

        // Exit if shutdown and queue drained
        if shutdown.load(Ordering::Relaxed) != 0 && rx.pop().is_none() {
            break;
        }

        // Sleep briefly if no work
        if batch.is_empty() {
            thread::sleep(Duration::from_millis(50));
        }
    }

    // Final flush
    if !batch.is_empty() {
        flush_batch(&mut batch, &out_dir, &mut current_file, &diverged_total);
    }
}

fn flush_batch(
    batch: &mut Vec<CapturedRow>,
    out_dir: &Path,
    current_file: &mut Option<(BufWriter<File>, String)>,
    _diverged_total: &Arc<AtomicU64>,
) {
    if batch.is_empty() {
        return;
    }

    // Determine target file (hourly rotation: shadow-YYYYMMDDTHH.jsonl)
    let now: DateTime<Utc> = Utc::now();
    let hour_key = now.format("%Y%m%dT%H").to_string();
    let filename = format!("shadow-{}.jsonl", hour_key);

    // Rotate file if hour changed
    let needs_rotation = current_file
        .as_ref()
        .map(|(_, key)| key != &hour_key)
        .unwrap_or(true);

    if needs_rotation {
        // Close old file
        if let Some((mut writer, _)) = current_file.take() {
            let _ = writer.flush();
        }

        // Open new file
        let path = out_dir.join(&filename);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("failed to open shadow output file");
        let writer = BufWriter::new(file);
        *current_file = Some((writer, hour_key));
    }

    // Write batch
    if let Some((ref mut writer, _)) = current_file {
        for row in batch.iter() {
            if let Ok(line) = serde_json::to_string(row) {
                let _ = writeln!(writer, "{}", line);
            }
        }
        let _ = writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_and_shutdown() {
        let temp_dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("shadow-test-capture");
        let _ = fs::remove_dir_all(&temp_dir);

        let runner = LiveShadowRunner::new(&temp_dir);
        
        for i in 0..10 {
            runner.observe(CapturedRow {
                event_seq: i,
                logical_now_ns: 1000 + i,
                legacy_decision: "Submitted".to_string(),
                v1_decision: if i % 3 == 0 { "Aborted:Drift(10bps)".to_string() } else { "Submitted".to_string() },
                legacy_intent_hash: Some([i as u8; 32]),
                v1_intent_hash: Some([i as u8; 32]),
                config_hash_v1: [0xAA; 32],
                book_ref_hash: [0xBB; 32],
            });
        }

        runner.shutdown();

        // Check that files were created
        let entries: Vec<_> = fs::read_dir(&temp_dir).unwrap().collect();
        assert!(!entries.is_empty(), "expected at least one shadow file");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
