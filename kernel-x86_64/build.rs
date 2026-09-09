use std::env;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn git_stdout(args: &[&str]) -> Option<String> {
    command_stdout("git", args)
}

fn git_build_tag() -> String {
    let hash = git_stdout(&["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    if dirty {
        format!("{hash}-dirty")
    } else {
        hash
    }
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    // Howard Hinnant's civil-from-days algorithm. Keeps build.rs dependency-free
    // while still producing an ISO-ish UTC timestamp for bare-metal boot photos.
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}

fn build_timestamp_utc() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = now / 86_400;
    let secs = now % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs / 3_600;
    let minute = (secs % 3_600) / 60;
    let second = secs % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn rust_toolchain_version() -> String {
    if let Ok(toolchain_toml) = std::fs::read_to_string("rust-toolchain.toml") {
        for line in toolchain_toml.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("channel") {
                if let Some((_, value)) = rest.split_once('=') {
                    return value.trim().trim_matches('"').to_string();
                }
            }
        }
    }

    if let Ok(toolchain) = env::var("RUSTUP_TOOLCHAIN") {
        if !toolchain.trim().is_empty() {
            return toolchain;
        }
    }

    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    command_stdout(&rustc, &["--version"]).unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    // Explicit override (M22a slot builds tag candidates REBUILD-A/B/SAB);
    // default stays the git build tag.
    println!("cargo:rerun-if-env-changed=SEMOS_BUILD_TAG");
    let tag = env::var("SEMOS_BUILD_TAG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(git_build_tag);
    let date = build_timestamp_utc();
    let rustc = rust_toolchain_version();

    println!("cargo:rustc-env=SEMOS_BUILD_TAG={tag}");
    println!("cargo:rustc-env=SEMOS_BUILD_DATE={date}");
    println!("cargo:rustc-env=SEMOS_RUSTC_VERSION={rustc}");

    // The agent endpoint credentials are read via `option_env!` in agent.rs.
    // Cargo does not fingerprint those on its own, so without these lines a
    // rebuild after `export KIMI_API_KEY=...` would silently keep the old
    // (empty) key baked in. Declaring them makes the key/base-url/model take
    // effect the moment they change.
    println!("cargo:rerun-if-env-changed=KIMI_API_KEY");
    println!("cargo:rerun-if-env-changed=KIMI_BASE_URL");
    println!("cargo:rerun-if-env-changed=KIMI_MODEL");
    println!("cargo:rerun-if-env-changed=ANTHROPIC_KEY");
    println!("cargo:rerun-if-env-changed=ANTHROPIC_BASE_URL");
    println!("cargo:rerun-if-env-changed=ANTHROPIC_MODEL");

    // Re-run when commit/dirty state can change. Git worktrees store .git as a
    // file, so also ask Git for the actual git-dir and watch its HEAD/index.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=rust-toolchain.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../kernel-core/Cargo.toml");
    println!("cargo:rerun-if-changed=../kernel-core/src");
    if let Some(git_dir) = git_stdout(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    } else {
        println!("cargo:rerun-if-changed=../.git/HEAD");
        println!("cargo:rerun-if-changed=../.git/index");
    }
}
