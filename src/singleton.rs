//! Single-instance enforcement: on startup, terminate any other running
//! `redswarm` process so the new one can bind the HTTP + peer-wire ports
//! without "Address already in use" errors.
//!
//! On Linux, scans `/proc/*/comm` for processes whose executable name matches
//! the binary (from `CARGO_PKG_NAME`, e.g. `"redswarm"`) and terminates
//! them - SIGTERM first (graceful), then SIGKILL after a grace period -
//! excluding the current process. This handles restarts cleanly and needs no
//! PID file, so it works even when the prior instance predates this feature
//! or was started differently.
//!
//! Identity is verified via `/proc/<pid>/comm` (the executable name, truncated
//! to 15 chars by the kernel - `redswarm` fits) to avoid killing an
//! unrelated process that recycled a PID. Before escalating to SIGKILL, the
//! identity is re-checked so a PID that was recycled after the prior instance
//! exited is never force-killed.
//!
//! Zombies (state `Z`) are treated as gone - they have already released their
//! ports and file descriptors and only linger until reaped, so they don't
//! block the bind.

use std::time::{Duration, Instant};

/// The executable name this binary reports in `/proc/<pid>/comm`. Sourced from
/// `Cargo.toml` (`CARGO_PKG_NAME`) so it can't drift from the actual binary
/// name. The kernel truncates `comm` to 15 chars; enforced at compile time.
const PROC_NAME: &str = env!("CARGO_PKG_NAME");
const _: () = assert!(PROC_NAME.len() <= 15, "CARGO_PKG_NAME must fit in /proc/<pid>/comm (15 chars)");

/// Seconds to wait for the prior instance to exit gracefully after SIGTERM
/// before escalating to SIGKILL.
const GRACE_SECS: u64 = 5;
/// Seconds to wait for the process to disappear after SIGKILL before giving
/// up (the bind loop retries on failure anyway, so this is just a courtesy).
const KILL_WAIT_SECS: u64 = 2;
/// Polling interval when waiting for a process to exit.
const POLL_MILLIS: u64 = 100;

/// Terminate every other running `redswarm` process so this instance can
/// bind its ports. Safe to call at startup; a no-op when no other instance is
/// running. Logs each termination.
#[cfg(target_os = "linux")]
pub fn take_over() {
    for pid in find_other_instances() {
        terminate(pid);
    }
}

/// No-op on non-Linux platforms (the app is Linux-oriented: NAT-PMP, peer-wire
/// server, `/proc`-based identity checks). Ports will simply fail to bind if a
/// prior instance is running, and the bind loop retries per
/// `server.rebind_retry_secs`.
#[cfg(not(target_os = "linux"))]
pub fn take_over() {}

/// Scan `/proc` for live processes whose `comm` matches the binary name,
/// excluding the current process. Returns PIDs to terminate.
#[cfg(target_os = "linux")]
fn find_other_instances() -> Vec<i32> {
    let self_pid = std::process::id();
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten() {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        if is_our_process(pid as i32) {
            found.push(pid as i32);
        }
    }
    found
}

/// Graceful SIGTERM, then SIGKILL after `GRACE_SECS`. Re-verifies the PID's
/// identity before SIGKILL so a recycled PID is never force-killed.
#[cfg(target_os = "linux")]
fn terminate(pid: i32) {
    tracing::info!(pid, "terminating prior redswarm instance");
    // 1. Graceful SIGTERM.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        let e = std::io::Error::last_os_error();
        // ESRCH = no such process (already gone) - nothing to do.
        if e.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(pid, error = %e, "SIGTERM failed");
        }
        return;
    }
    wait_for_exit(pid, GRACE_SECS);
    if !is_running(pid) {
        return;
    }
    // 2. Still alive - re-verify identity (the PID may have been recycled
    //    after the prior instance exited) before escalating to SIGKILL.
    if !is_our_process(pid) {
        tracing::warn!(pid, "PID recycled after exit - skipping SIGKILL");
        return;
    }
    tracing::warn!(pid, "did not exit after SIGTERM - sending SIGKILL");
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(pid, error = %e, "SIGKILL failed");
        }
        return;
    }
    wait_for_exit(pid, KILL_WAIT_SECS);
}

/// Poll until the process is gone (not running, including zombie) or `secs`
/// elapse. Blocking sleep is fine here - this runs once at startup, before
/// the async runtime's main work begins.
#[cfg(target_os = "linux")]
fn wait_for_exit(pid: i32, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if !is_running(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(POLL_MILLIS));
    }
}

/// Is `pid` a live, non-zombie process? Reads `/proc/<pid>/stat` (the state
/// char), which is robust against PID recycling: a recycled PID has a
/// different `comm`, but we only use this for liveness after identity is
/// confirmed. Zombies (state `Z`) have released their ports/FDs and are
/// treated as gone.
#[cfg(target_os = "linux")]
fn is_running(pid: i32) -> bool {
    match proc_state(pid) {
        None => false,
        Some('Z') => false,
        Some(_) => true,
    }
}

/// The single-char process state from `/proc/<pid>/stat` (e.g. `R`, `S`,
/// `Z`). `None` if the process doesn't exist or the file can't be parsed.
/// `comm` can contain spaces/parens, so the state is the char after the last
/// `)` in the line.
#[cfg(target_os = "linux")]
fn proc_state(pid: i32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rparen = stat.rfind(')')?;
    // After the closing paren of comm: " <state> ...". Skip ')' and the space.
    stat[rparen..].chars().nth(2)
}

/// Does `pid` belong to a `redswarm` process? Compares `/proc/<pid>/comm`
/// (trimmed) to `PROC_NAME`. `comm` is the executable base name, truncated to
/// 15 chars by the kernel.
#[cfg(target_os = "linux")]
fn is_our_process(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim() == PROC_NAME)
        .unwrap_or(false)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn proc_name_fits_in_comm_limit() {
        // Enforced by the const assert above, but re-checked here for clarity.
        assert!(PROC_NAME.len() <= 15);
    }

    #[test]
    fn proc_state_returns_non_z_for_self() {
        // The test process is alive and not a zombie.
        let state = proc_state(std::process::id() as i32);
        assert!(state.is_some(), "self /proc/<pid>/stat must be readable");
        assert_ne!(state, Some('Z'), "self must not be a zombie");
    }

    #[test]
    fn is_our_process_false_for_init() {
        // PID 1 is init/systemd/launchd - never named "redswarm". This
        // guards the identity check against a live-but-unrelated process.
        // (If PID 1 doesn't exist or is unreadable, is_our_process returns
        // false, which is still the correct answer here.)
        assert!(!is_our_process(1), "PID 1 must not be identified as ours");
    }

    #[test]
    fn is_our_process_false_for_dead_pid() {
        // A very high PID is almost certainly not allocated.
        assert!(!is_our_process(2_000_000_000), "dead PID must not be ours");
    }

    #[test]
    fn find_other_instances_excludes_self() {
        // The scan must never return the current process - otherwise
        // take_over() would kill the very instance that called it.
        let self_pid = std::process::id() as i32;
        let others = find_other_instances();
        assert!(
            !others.contains(&self_pid),
            "find_other_instances must exclude self ({self_pid}); got {others:?}"
        );
    }

    #[test]
    fn is_running_false_for_dead_pid() {
        assert!(!is_running(2_000_000_000), "dead PID must not be running");
    }

    #[test]
    fn proc_state_none_for_dead_pid() {
        assert_eq!(proc_state(2_000_000_000), None, "dead PID has no /proc stat");
    }
}
