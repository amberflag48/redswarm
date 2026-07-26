//! File watcher that hot-reloads `config.toml` on change.
//!
//! [`spawn`] starts a `notify` watcher on the config file's parent directory.
//! Watching the directory (not the bare file) survives the atomic
//! write-temp-then-rename that many editors use, which a file-only watch
//! would miss on some platforms. Events for the config file are debounced:
//! a burst of edits triggers exactly one [`reload::reload_config`] call once
//! writes settle for [`DEBOUNCE`].

use std::path::Path;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::api::SharedState;
use crate::reload;

/// Whether a filesystem event should trigger a config reload: it's a
/// modify/create event touching the watched config file (by file name, so
/// events on sibling files in the same watched directory are ignored).
/// Extracted as a pure predicate so the debounce-relevant filter logic is
/// unit-testable without a real filesystem watcher.
fn is_config_file_event(
 kind: &notify::EventKind,
 paths: &[std::path::PathBuf],
 filename: &std::ffi::OsStr,
) -> bool {
 matches!(kind, notify::EventKind::Modify(_) | notify::EventKind::Create(_))
 && paths.iter().any(|p| p.file_name() == Some(filename))
}

/// Spawn the config hot-reload watcher. Returns immediately; the watcher
/// runs in a background task for the process lifetime. Errors are logged and
/// the watcher gives up gracefully (hot reload is best-effort - the app
/// keeps the last good config and never crashes on a watcher failure).
///
/// The debounce interval is read from `watcher.debounce_ms` in config.toml on
/// each cycle, so it is itself hot-reloadable.
pub fn spawn(state: SharedState) {
 let path = crate::config::path();
 let file = Path::new(&path);
 let watch_dir = file
 .parent()
 .map(|p| p.to_path_buf())
 .unwrap_or_else(|| std::path::Path::new(".").to_path_buf());
 let filename = file.file_name().map(|n| n.to_os_string()).unwrap_or_default();

 let (tx, mut rx) = mpsc::channel::<()>(64);
 let mut watcher = match RecommendedWatcher::new(
 move |res: notify::Result<Event>| {
 if let Ok(event) = res
 && is_config_file_event(&event.kind, &event.paths, filename.as_os_str())
 {
 let _ = tx.try_send(());
 }
 },
 notify::Config::default(),
 ) {
 Ok(w) => w,
 Err(e) => {
 tracing::warn!(error = ?e, "config watcher failed to start - hot reload disabled");
 return;
 }
 };

 if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
 tracing::warn!(error = ?e, dir = ?watch_dir, "config watcher failed to watch - hot reload disabled");
 return;
 }

 let reload_state = state;
 tokio::spawn(async move {
 // Hold `watcher` for the process lifetime; dropping it stops events.
 let _watcher = watcher;
 loop {
 // Wait for the first event of a burst.
 if rx.recv().await.is_none() {
 break;
 }
 // Drain the burst, wait out the debounce, drain again so a
 // multi-step editor write coalesces into one reload. The debounce
 // interval is read from config so it is itself hot-reloadable.
 while rx.try_recv().is_ok() {}
 let debounce_ms = reload_state.config.load().watcher.debounce_ms;
 tokio::time::sleep(std::time::Duration::from_millis(debounce_ms)).await;
 while rx.try_recv().is_ok() {}
 if let Err(e) = reload::reload_config(&reload_state).await {
 tracing::warn!(error = %e, "config reload failed");
 }
 }
 });

 tracing::info!(path, "config hot-reload watcher active");
}

#[cfg(test)]
mod tests {
 use super::is_config_file_event;
 use std::ffi::OsStr;
 use std::path::PathBuf;

 // `OsStr::new` is not `const` yet, so this is a fn instead of a const.
 fn cfg() -> &'static OsStr {
 OsStr::new("config.toml")
 }

 // matching events

 #[test]
 fn modify_event_on_config_file_matches() {
 let kind = notify::EventKind::Modify(notify::event::ModifyKind::Any);
 let paths = vec![PathBuf::from("/app/config.toml")];
 assert!(is_config_file_event(&kind, &paths, cfg()));
 }

 #[test]
 fn create_event_on_config_file_matches() {
 // Editors that write-temp-then-rename surface as a Create on the
 // destination - the watcher must catch this (atomic save).
 let kind = notify::EventKind::Create(notify::event::CreateKind::Any);
 let paths = vec![PathBuf::from("/app/config.toml")];
 assert!(is_config_file_event(&kind, &paths, cfg()));
 }

 #[test]
 fn config_file_anywhere_in_paths_matches() {
 // A directory watch emits one event for the changed file; the config
 // file may be one of several paths in the event.
 let kind = notify::EventKind::Modify(notify::event::ModifyKind::Any);
 let paths = vec![PathBuf::from("/app/.tmp"), PathBuf::from("/app/config.toml")];
 assert!(is_config_file_event(&kind, &paths, cfg()));
 }

 // non-matching events

 #[test]
 fn modify_event_on_different_file_does_not_match() {
 let kind = notify::EventKind::Modify(notify::event::ModifyKind::Any);
 let paths = vec![PathBuf::from("/app/other.toml")];
 assert!(!is_config_file_event(&kind, &paths, cfg()));
 }

 #[test]
 fn remove_event_does_not_match() {
 // A remove means the file is gone - reloading would read a missing
 // config. The watcher ignores removes (reload triggers on the next
 // create/modify, e.g. the editor's rename-into-place).
 let kind = notify::EventKind::Remove(notify::event::RemoveKind::File);
 let paths = vec![PathBuf::from("/app/config.toml")];
 assert!(!is_config_file_event(&kind, &paths, cfg()));
 }

 #[test]
 fn access_event_does_not_match() {
 // Reads/access must not trigger a reload (only writes).
 let kind = notify::EventKind::Access(notify::event::AccessKind::Any);
 let paths = vec![PathBuf::from("/app/config.toml")];
 assert!(!is_config_file_event(&kind, &paths, cfg()));
 }

 #[test]
 fn any_kind_does_not_match() {
 // EventKind::Any is the catch-all native mapping; the watcher only
 // reloads on concrete Modify/Create, not the ambiguous Any.
 let kind = notify::EventKind::Any;
 let paths = vec![PathBuf::from("/app/config.toml")];
 assert!(!is_config_file_event(&kind, &paths, cfg()));
 }

 #[test]
 fn modify_with_no_paths_does_not_match() {
 let kind = notify::EventKind::Modify(notify::event::ModifyKind::Any);
 assert!(!is_config_file_event(&kind, &[], cfg()));
 }

 #[test]
 fn filename_mismatch_on_create_does_not_match() {
 let kind = notify::EventKind::Create(notify::event::CreateKind::Any);
 let paths = vec![PathBuf::from("/app/redswarm.db")];
 assert!(!is_config_file_event(&kind, &paths, cfg()));
 }
}
