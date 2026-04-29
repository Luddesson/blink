//! blink-shadow-gate — offline divergence analysis tool.
//!
//! Reads captured shadow rows from JSON Lines files and computes a cross-tab
//! of legacy vs. v1 decisions, exiting with code 0 if divergence is within
//! acceptable thresholds, 1 otherwise.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process;

use blink_shadow::CapturedRow;

#[derive(Debug, Default)]
struct CrossTab {
    submitted_submitted: u64,
    submitted_aborted: u64,
    submitted_noop: u64,
    aborted_submitted: u64,
    aborted_aborted_same: u64,
    aborted_aborted_diff: u64,
    aborted_noop: u64,
    noop_submitted: u64,
    noop_aborted: u64,
    noop_noop: u64,
}

impl CrossTab {
    fn add(&mut self, legacy: &str, v1: &str) {
        let legacy_kind = decision_kind(legacy);
        let v1_kind = decision_kind(v1);

        match (legacy_kind, v1_kind) {
            ("Submitted", "Submitted") => self.submitted_submitted += 1,
            ("Submitted", "Aborted") => self.submitted_aborted += 1,
            ("Submitted", "NoOp") => self.submitted_noop += 1,
            ("Aborted", "Submitted") => self.aborted_submitted += 1,
            ("Aborted", "Aborted") => {
                if abort_reason(legacy) == abort_reason(v1) {
                    self.aborted_aborted_same += 1;
                } else {
                    self.aborted_aborted_diff += 1;
                }
            }
            ("Aborted", "NoOp") => self.aborted_noop += 1,
            ("NoOp", "Submitted") => self.noop_submitted += 1,
            ("NoOp", "Aborted") => self.noop_aborted += 1,
            ("NoOp", "NoOp") => self.noop_noop += 1,
            _ => {}
        }
    }

    fn total(&self) -> u64 {
        self.submitted_submitted
            + self.submitted_aborted
            + self.submitted_noop
            + self.aborted_submitted
            + self.aborted_aborted_same
            + self.aborted_aborted_diff
            + self.aborted_noop
            + self.noop_submitted
            + self.noop_aborted
            + self.noop_noop
    }

    fn submit_divergence(&self) -> u64 {
        self.submitted_aborted + self.submitted_noop + self.aborted_submitted + self.noop_submitted
    }

    fn abort_reassign_pct(&self) -> f64 {
        let total_aborts = self.aborted_aborted_same + self.aborted_aborted_diff;
        if total_aborts == 0 {
            0.0
        } else {
            (self.aborted_aborted_diff as f64 / total_aborts as f64) * 100.0
        }
    }

    fn print(&self) {
        println!("╭───────────────────────────────────────────────────────────╮");
        println!("│           Shadow Decision Cross-Tab                      │");
        println!("╞═══════════════════════════════════════════════════════════╡");
        println!("│                      V1 Decision                          │");
        println!("│ Legacy        │ Submitted │ Aborted   │ NoOp              │");
        println!("├───────────────┼───────────┼───────────┼───────────────────┤");
        println!(
            "│ Submitted     │ {:>9} │ {:>9} │ {:>9}         │",
            self.submitted_submitted, self.submitted_aborted, self.submitted_noop
        );
        println!(
            "│ Aborted (same)│ {:>9} │ {:>9} │ {:>9}         │",
            self.aborted_submitted, self.aborted_aborted_same, self.aborted_noop
        );
        println!(
            "│ Aborted (diff)│           │ {:>9} │                   │",
            self.aborted_aborted_diff
        );
        println!(
            "│ NoOp          │ {:>9} │ {:>9} │ {:>9}         │",
            self.noop_submitted, self.noop_aborted, self.noop_noop
        );
        println!("╰───────────────────────────────────────────────────────────╯");
        println!();
        println!("  Total rows: {}", self.total());
        println!("  Submit↔NoOp flips: {}", self.submit_divergence());
        println!(
            "  Abort reason reassignment: {:.2}%",
            self.abort_reassign_pct()
        );
        println!();
    }
}

fn decision_kind(s: &str) -> &str {
    if s.starts_with("Submitted") {
        "Submitted"
    } else if s.starts_with("Aborted") {
        "Aborted"
    } else if s.starts_with("NoOp") {
        "NoOp"
    } else {
        "Unknown"
    }
}

fn abort_reason(s: &str) -> &str {
    if let Some(idx) = s.find(':') {
        &s[idx + 1..]
    } else {
        ""
    }
}

fn parse_duration(s: &str) -> Option<u64> {
    if s.ends_with('h') {
        s.trim_end_matches('h').parse::<u64>().ok().map(|h| h * 3600)
    } else if s.ends_with('m') {
        s.trim_end_matches('m').parse::<u64>().ok().map(|m| m * 60)
    } else if s.ends_with('s') {
        s.trim_end_matches('s').parse::<u64>().ok()
    } else {
        s.parse::<u64>().ok()
    }
}

struct Args {
    in_dir: String,
    window_secs: Option<u64>,
    submit_divergence_max: u64,
    abort_reassign_max_pct: f64,
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().collect();
    let mut in_dir = None;
    let mut window_secs = None;
    let mut submit_divergence_max = 0u64;
    let mut abort_reassign_max_pct = 0.1f64;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--in" => {
                i += 1;
                if i >= args.len() {
                    return Err("--in requires an argument".to_string());
                }
                in_dir = Some(args[i].clone());
            }
            "--window" => {
                i += 1;
                if i >= args.len() {
                    return Err("--window requires an argument".to_string());
                }
                window_secs = Some(
                    parse_duration(&args[i])
                        .ok_or_else(|| format!("invalid window duration: {}", args[i]))?,
                );
            }
            "--submit-divergence-max" => {
                i += 1;
                if i >= args.len() {
                    return Err("--submit-divergence-max requires an argument".to_string());
                }
                submit_divergence_max = args[i]
                    .parse()
                    .map_err(|_| format!("invalid number: {}", args[i]))?;
            }
            "--abort-reassign-max-pct" => {
                i += 1;
                if i >= args.len() {
                    return Err("--abort-reassign-max-pct requires an argument".to_string());
                }
                abort_reassign_max_pct = args[i]
                    .parse()
                    .map_err(|_| format!("invalid number: {}", args[i]))?;
            }
            "--help" | "-h" => {
                print_help();
                process::exit(0);
            }
            other => {
                return Err(format!("unknown argument: {}", other));
            }
        }
        i += 1;
    }

    let in_dir = in_dir.ok_or_else(|| "--in <DIR> is required".to_string())?;

    Ok(Args {
        in_dir,
        window_secs,
        submit_divergence_max,
        abort_reassign_max_pct,
    })
}

fn print_help() {
    println!("blink-shadow-gate — offline divergence analysis");
    println!();
    println!("USAGE:");
    println!("  blink-shadow-gate --in <DIR> [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  --in <DIR>                       Directory containing *.jsonl shadow files (required)");
    println!("  --window <DURATION>              Only analyze rows newer than DURATION ago (e.g., 24h, 1h, 30m)");
    println!("  --submit-divergence-max <N>      Max allowed submit↔noop flips (default: 0)");
    println!("  --abort-reassign-max-pct <F>     Max allowed abort reason reassignment % (default: 0.1)");
    println!("  -h, --help                       Print this help");
    println!();
    println!("EXIT CODE:");
    println!("  0 if divergence within thresholds, 1 otherwise");
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!("Run with --help for usage");
            process::exit(2);
        }
    };

    let dir = Path::new(&args.in_dir);
    if !dir.is_dir() {
        eprintln!("Error: {} is not a directory", args.in_dir);
        process::exit(2);
    }

    let cutoff_ns = args.window_secs.map(|secs| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        now.saturating_sub(secs * 1_000_000_000)
    });

    let mut crosstab = CrossTab::default();
    let mut files_read = 0;
    let mut rows_read = 0;

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error reading directory: {}", e);
            process::exit(2);
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let filename = path.file_name().unwrap().to_string_lossy();
        if !filename.ends_with(".jsonl") {
            continue;
        }

        files_read += 1;
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }

            let row: CapturedRow = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Apply window filter
            if let Some(cutoff) = cutoff_ns {
                if row.logical_now_ns < cutoff {
                    continue;
                }
            }

            rows_read += 1;
            crosstab.add(&row.legacy_decision, &row.v1_decision);
        }
    }

    if files_read == 0 {
        eprintln!("Warning: no .jsonl files found in {}", args.in_dir);
    }
    if rows_read == 0 {
        eprintln!("Warning: no rows read");
    }

    println!("Files read: {}", files_read);
    println!("Rows analyzed: {}", rows_read);
    println!();

    crosstab.print();

    let submit_div = crosstab.submit_divergence();
    let abort_reassign = crosstab.abort_reassign_pct();

    let submit_ok = submit_div <= args.submit_divergence_max;
    let abort_ok = abort_reassign <= args.abort_reassign_max_pct;

    if submit_ok && abort_ok {
        println!("✓ PASS: divergence within thresholds");
        process::exit(0);
    } else {
        println!("✗ FAIL: divergence exceeds thresholds");
        if !submit_ok {
            println!(
                "  Submit divergence: {} > {} (max)",
                submit_div, args.submit_divergence_max
            );
        }
        if !abort_ok {
            println!(
                "  Abort reassignment: {:.2}% > {:.2}% (max)",
                abort_reassign, args.abort_reassign_max_pct
            );
        }
        process::exit(1);
    }
}
