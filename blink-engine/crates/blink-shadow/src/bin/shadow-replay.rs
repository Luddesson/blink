//! Replay driver. Reads one `DecisionInput` per line from a JSONL file
//! and runs the shadow harness with two `StubKernel`s. Swapping in the
//! real legacy and v2 kernels is the `p0-shadow-hook` follow-up.
//!
//! Usage: `shadow-replay <path-to-inputs.jsonl>`

use std::io::{BufRead, BufReader};
use std::process::ExitCode;

use blink_shadow::{DecisionInput, MemoryJournal, ShadowRunner, StubKernel};
use blink_timestamps::{init_with_policy, InitPolicy};

fn main() -> ExitCode {
    let _ = init_with_policy(InitPolicy::AllowFallback);

    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1) {
        Some(p) => p.clone(),
        None => {
            eprintln!("usage: shadow-replay <path-to-inputs.jsonl>");
            return ExitCode::FAILURE;
        }
    };

    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("shadow-replay: open {}: {}", path, e);
            return ExitCode::FAILURE;
        }
    };

    let mut inputs: Vec<DecisionInput> = Vec::new();
    for (lineno, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("shadow-replay: read line {}: {}", lineno + 1, e);
                return ExitCode::FAILURE;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<DecisionInput>(&line) {
            Ok(i) => inputs.push(i),
            Err(e) => {
                eprintln!("shadow-replay: parse line {}: {} (src: {})", lineno + 1, e, line);
                return ExitCode::FAILURE;
            }
        }
    }

    let legacy = StubKernel::noop(1, "legacy-stub", "below edge threshold");
    let v2 = StubKernel::noop(2, "v2-stub", "below edge threshold");
    let mut runner = ShadowRunner::new(legacy, v2, MemoryJournal::new());
    runner.run(inputs);
    let report = runner.report();

    println!("{}", report.pretty_line());
    for d in &report.divergences {
        println!(
            "  diverge event_key={} field={:?} legacy={} v2={}",
            d.event_key, d.first_differing_field, d.legacy_outcome_summary, d.v2_outcome_summary
        );
    }
    ExitCode::SUCCESS
}
