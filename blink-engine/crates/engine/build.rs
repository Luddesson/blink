use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=BLINK_BUILD_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=BLINK_BUILD_TIMESTAMP_UTC");
    println!("cargo:rerun-if-env-changed=BLINK_BUILD_RELEASE_DIRTY");

    let git_commit = env_or_command("BLINK_BUILD_GIT_COMMIT", "git", &["rev-parse", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    let build_timestamp = env_or_command(
        "BLINK_BUILD_TIMESTAMP_UTC",
        "date",
        &["-u", "+%Y-%m-%dT%H:%M:%SZ"],
    )
    .unwrap_or_else(|| "unknown".to_string());
    let release_dirty = std::env::var("BLINK_BUILD_RELEASE_DIRTY")
        .ok()
        .unwrap_or_else(|| git_worktree_dirty().to_string());

    println!("cargo:rustc-env=BLINK_BUILD_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=BLINK_BUILD_TIMESTAMP_UTC={build_timestamp}");
    println!("cargo:rustc-env=BLINK_BUILD_RELEASE_DIRTY={release_dirty}");
}

fn env_or_command(env_name: &str, program: &str, args: &[&str]) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            Command::new(program)
                .args(args)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
}

fn git_worktree_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(true)
}
