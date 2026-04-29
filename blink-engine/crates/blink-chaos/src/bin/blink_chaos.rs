//! Optional CLI smoke-runner. Thin wrapper around the integration tests
//! — primarily exists so operators can run a single chaos scenario on
//! a dev box without going through `cargo test`.
//!
//! The runner does **not** embed the full scenario harness code (that
//! lives in the `tests/` directory so it runs under `cargo test`).
//! Instead, it shells out to `cargo test -p blink-chaos --test
//! scenario_<name>` so we don't duplicate the assertion wiring.

use std::process::{exit, Command};

fn usage() -> ! {
    eprintln!(
        "blink-chaos runner\n\n\
         usage: blink-chaos --scenario <name> [--verbose]\n\n\
         scenarios:\n\
         \tws_drop_reconnect\n\
         \trpc_stall_95p\n\
         \tconnection_reset_mid_post\n\
         \tclob_500_streak              (currently #[ignore], needs blink-breakers)\n\
         \trate_limit_429_streak        (currently #[ignore], needs blink-breakers)\n\
         \tclock_skew_jump              (currently #[ignore], needs test-clock shim)\n"
    );
    exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut scenario: Option<String> = None;
    let mut verbose = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--scenario" => scenario = args.next(),
            "--verbose" | "-v" => verbose = true,
            "--help" | "-h" => usage(),
            _ => {
                eprintln!("unknown arg: {a}");
                usage();
            }
        }
    }
    let name = scenario.unwrap_or_else(|| usage());

    let mut cmd = Command::new(env!("CARGO"));
    cmd.args([
        "test",
        "-p",
        "blink-chaos",
        "--test",
        &format!("scenario_{name}"),
        "--",
        "--include-ignored",
        "--nocapture",
    ]);
    if verbose {
        cmd.env("RUST_LOG", "debug");
    }
    eprintln!("running: {:?}", cmd);
    let status = cmd.status().expect("spawn cargo");
    exit(status.code().unwrap_or(1));
}
