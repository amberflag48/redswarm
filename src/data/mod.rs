//! Centralized data - the single source of truth for every non-config value
//! in the project.
//!
//! `config.toml` holds the *tunable* values (speeds, intervals, ports, client
//! specs). This module holds everything else that would otherwise be a
//! retyped literal scattered across modules: SQL schema names, controlled
//! vocabularies, BitTorrent protocol keys, SSE wire names, byte units, and
//! UI labels. Changing any of those values is a one-line edit here that
//! ripples to every consumer automatically.
//!
//! Submodules:
//! - [`schema`] - SQLite table / column / index names
//! - [`vocab`] - status / phase / event / lifecycle string vocabularies
//! - [`protocol`]- bencode keys, magnet keys, query placeholders, field sizes
//! - [`sse`] - SSE event names and `data-*` DOM hooks
//! - [`units`] - binary byte units and `fmt_bytes`
//! - [`labels`] - human-readable UI strings

pub mod labels;
pub mod protocol;
pub mod schema;
pub mod sse;
pub mod units;
pub mod vocab;

#[cfg(test)]
pub mod fixtures;

#[cfg(test)]
mod tests {
 use super::*;

 /// Read a `.rs` file and return only production code - everything except
 /// `#[cfg(test)]`-attributed items. This handles three cases:
 /// 1. Trailing `#[cfg(test)] mod tests` / `pub mod test_helpers` blocks
 /// (stripped entirely from the `#[cfg(test)]` onward).
 /// 2. Inline `#[cfg(test)] pub async fn clear_events` helpers that sit
 /// mid-file (the item is located by brace-matching and removed).
 /// 3. Files with no `#[cfg(test)]` at all (returned verbatim).
 fn production_code(path: &str) -> String {
 let content = std::fs::read_to_string(path).unwrap_or_default();
 let bytes = content.as_bytes();

 // First pass: find the strip point for the trailing `mod`/`pub mod`
 // test module (case 1). Everything from here is test code.
 let mut mod_strip = content.len();
 let mut search_from = 0;
 while let Some(idx) = content[search_from..].find("#[cfg(test)]") {
 let abs = search_from + idx;
 let after = content[abs + "#[cfg(test)]".len()..].trim_start();
 if after.starts_with("mod ") || after.starts_with("pub mod ") {
 mod_strip = abs;
 break;
 }
 search_from = abs + "#[cfg(test)]".len();
 }
 let pre_mod = &content[..mod_strip];

 // Second pass: remove inline `#[cfg(test)]`-attributed items (case 2).
 // These are individual functions gated by `#[cfg(test)]` that sit in
 // the middle of production code (e.g. `db::clear_events`, the test-only
 // `swarm::max_safe_upload_bps`). Each is found by scanning for
 // `#[cfg(test)]`, then brace-matching from the item's opening `{` to
 // its closing `}` to determine the full span to remove.
 let mut result = String::with_capacity(pre_mod.len());
 let mut pos = 0;
 while pos < pre_mod.len() {
 let Some(rel) = pre_mod[pos..].find("#[cfg(test)]") else {
 result.push_str(&pre_mod[pos..]);
 break;
 };
 let abs = pos + rel;
 result.push_str(&pre_mod[pos..abs]);
 let after_attr = &pre_mod[abs + "#[cfg(test)]".len()..];
 let trimmed = after_attr.trim_start();
 if trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ") {
 // Shouldn't happen (case 1 already handled), but if it does,
 // stop here - the rest is test-module code.
 break;
 }
 // Inline `#[cfg(test)]` fn - find the item body `{` and brace-match
 // to the closing `}`, then skip the entire item.
 let attr_end = abs + "#[cfg(test)]".len();
 let trim_skip = after_attr.len() - trimmed.len();
 let item_start = attr_end + trim_skip;
 if let Some(rel_brace) = pre_mod[item_start..].find('{') {
 let open = item_start + rel_brace;
 let mut depth = 1i32;
 let mut i = open + 1;
 let mut in_string = false;
 while i < pre_mod.len() && depth > 0 {
 match bytes[i] as char {
 '"' if !in_string => in_string = true,
 '"' if in_string => in_string = false,
 '{' if !in_string => depth += 1,
 '}' if !in_string => {
 depth -= 1;
 if depth == 0 {
 break;
 }
 }
 _ => {}
 }
 i += 1;
 }
 if depth > 0 {
 // Unbalanced braces - append rest and stop (defensive).
 result.push_str(&pre_mod[abs..]);
 break;
 }
 // Skip the entire `#[cfg(test)]` item (abs through closing `}`).
 pos = i + 1;
 } else {
 // No `{` found - skip just the attribute.
 pos = abs + "#[cfg(test)]".len();
 }
 }
 result
 }

 /// Check if `haystack` contains `needle` as a standalone numeric token
 /// (not a substring of a larger number like `10240` matching `1024`).
 /// The character before and after the match must not be an ASCII digit.
 fn contains_number(haystack: &str, needle: &str) -> bool {
 let bytes = haystack.as_bytes();
 let mut from = 0;
 while let Some(idx) = haystack[from..].find(needle) {
 let abs = from + idx;
 let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_digit();
 let after = abs + needle.len();
 let after_ok = after >= bytes.len() || !bytes[after].is_ascii_digit();
 if before_ok && after_ok {
 return true;
 }
 from = abs + 1;
 }
 false
 }

 fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
 if let Ok(entries) = std::fs::read_dir(dir) {
 for entry in entries.flatten() {
 let path = entry.path();
 if path.is_dir() {
 collect_rs_files(&path, out);
 } else if path.extension().is_some_and(|e| e == "rs") {
 out.push(path);
 }
 }
 }
 }

 /// Production .rs files (excluding `data/` which holds the definitions).
 const PROD_FILES: &[&str] = &[
 "src/main.rs", "src/config.rs", "src/api.rs", "src/announce.rs",
 "src/bencode.rs", "src/capture.rs", "src/db.rs", "src/engine.rs", "src/magnet.rs",
 "src/peer_id.rs", "src/peer_server.rs", "src/render.rs", "src/swarm.rs", "src/templates.rs",
 "src/torrent.rs", "src/nat.rs", "src/reload.rs", "src/watcher.rs",
 "src/singleton.rs",
 ];

 // [u8; 20] must not appear in production code
 //
 // The protocol mandates 20-byte arrays for peer_id and info_hash, but
 // the length must be expressed via `protocol::PEER_ID_LEN` /
 // `protocol::INFO_HASH_LEN` - never as a raw `20`. A raw `[u8; 20]` in a
 // production signature bypasses the const: changing the protocol length
 // would silently leave the signature at 20 while the const changes.
 #[test]
 fn no_raw_u8_20_in_production() {
 for file in PROD_FILES {
 let code = production_code(file);
 assert!(
 !code.contains("[u8; 20]"),
 "{file}: production code contains `[u8; 20]` - use `[u8; protocol::INFO_HASH_LEN]` or `[u8; protocol::PEER_ID_LEN]` instead",
 );
 }
 }

 // DEFAULT_ANNOUNCE_INTERVAL_SECS must never reappear
 //
 // This const was deleted because it shadowed `config.toml [tracker]
 // default_interval_secs` and violated the "no defaults in Rust code" rule.
 // Fallback paths now read `cfg.tracker.default_interval_secs`. If this
 // identifier reappears anywhere, the refactor has regressed.
 #[test]
 fn deleted_interval_const_does_not_reappear() {
 for file in PROD_FILES {
 let code = production_code(file);
 assert!(
 !code.contains("DEFAULT_ANNOUNCE_INTERVAL_SECS"),
 "{file}: `DEFAULT_ANNOUNCE_INTERVAL_SECS` was deleted - fallback paths must read `cfg.tracker.default_interval_secs`",
 );
 }
 }

 // Raw byte-unit constants must not appear in production code
 //
 // `units::KIB` (1024), `units::MIB` (1048576), `units::GIB` (1073741824)
 // are the single source of truth. Hardcoding the raw integer in
 // production code bypasses the const and would drift if the const
 // changed. The check normalizes Rust's `1_048_576` underscore syntax to
 // `1048576` before matching, so both forms are caught.
 #[test]
 fn no_raw_byte_constants_in_production() {
 // The forbidden values (without underscores); the check normalizes
 // underscores away from each line before matching.
 let forbidden = ["1024", "1048576", "1073741824"];
 let forbidden_names = ["KIB", "MIB", "GIB"];
 for file in PROD_FILES {
 let code = production_code(file);
 for (lineno, line) in code.lines().enumerate() {
 let trimmed = line.trim_start();
 if trimmed.starts_with("//") {
 continue;
 }
 // Normalize: strip underscores so `1_048_576` → `1048576`.
 let normalized = line.replace('_', "");
 for (i, &num) in forbidden.iter().enumerate() {
 assert!(
 !contains_number(&normalized, num),
 "{file}:{} contains raw byte constant `{num}` (use `units::{}` instead)",
 lineno + 1,
 forbidden_names[i],
 );
 }
 }
 }
 }

 // CSS class selectors must match vocab consts
 //
 // `index.html` builds CSS classes dynamically from status/phase values:
 // `<span class="badge {{ a.status }}">` → `.badge.running`, `.badge.stopped`
 // `class="phase-{{ ev.phase }}"` → `.phase-probe`, `.phase-attack`
 // If a vocab const is renamed, the CSS selector must change too or the
 // styling silently breaks. This test asserts every CSS class suffix
 // equals a vocab const value.
 #[test]
 fn css_classes_match_vocab_consts() {
 // CSS now lives in frontend/styles/*.css - scan all of them.
 let css = std::fs::read_dir("frontend/styles")
 .expect("frontend/styles must be readable")
 .filter_map(|e| e.ok())
 .filter_map(|e| std::fs::read_to_string(e.path()).ok())
 .collect::<String>();

 // Status badge classes
 for &status in &[vocab::STATUS_IDLE, vocab::STATUS_RUNNING, vocab::STATUS_STOPPED] {
 let selector = format!(".badge.{status}");
 assert!(
 css.contains(&selector),
 "frontend CSS missing `{selector}` - must match vocab::STATUS_*"
 );
 }
 // Phase classes
 for &phase in &[vocab::PHASE_PROBE, vocab::PHASE_ATTACK] {
 let selector = format!(".phase-{phase}");
 assert!(
 css.contains(&selector),
 "frontend CSS missing `{selector}` - must match vocab::PHASE_*"
 );
 }
 }

 // JS status string literals must match vocab consts
 //
 // The inline JS in `index.html` compares `=== 'running'` / `=== 'stopped'`
 // to drive badge/button logic. These string literals must equal the vocab
 // consts or the UI silently breaks (no compile check). This test catches
 // a vocab rename that isn't reflected in the JS.
 #[test]
 fn js_status_literals_match_vocab() {
 // JS now lives in frontend/js/ - scan all .js files recursively.
 fn read_js_dir(dir: &std::path::Path) -> String {
 let mut out = String::new();
 if let Ok(entries) = std::fs::read_dir(dir) {
 for entry in entries.flatten() {
 let path = entry.path();
 if path.is_dir() {
 out.push_str(&read_js_dir(&path));
 } else if path.extension().is_some_and(|e| e == "js")
 && let Ok(content) = std::fs::read_to_string(&path) {
 out.push_str(&content);
 }
 }
 }
 out
 }
 let js = read_js_dir(std::path::Path::new("frontend/js"));
 assert!(js.contains("'running'"), "JS missing 'running' literal (must match vocab::STATUS_RUNNING)");
 assert!(js.contains("'stopped'"), "JS missing 'stopped' literal (must match vocab::STATUS_STOPPED)");
 assert_eq!(vocab::STATUS_RUNNING, "running", "vocab::STATUS_RUNNING must equal the JS literal 'running'");
 assert_eq!(vocab::STATUS_STOPPED, "stopped", "vocab::STATUS_STOPPED must equal the JS literal 'stopped'");
 }

 // SSE event names in JS must match sse consts
 //
 // The JS `addEventListener('audit', …)` etc. must match `sse::EV_*` or
 // the SSE channel silently breaks.
 #[test]
 fn js_sse_event_names_match_consts() {
 // SSE event listeners now live in frontend/js/services/sse.js
 let sse_js = std::fs::read_to_string("frontend/js/services/sse.js")
 .expect("frontend/js/services/sse.js must be readable");
 for &(name, const_val) in &[
 ("audit", sse::EV_AUDIT),
 ("task_created", sse::EV_TASK_CREATED),
 ("task_deleted", sse::EV_TASK_DELETED),
 ("task_status", sse::EV_TASK_STATUS),
 ("task_client", sse::EV_TASK_CLIENT),
 ("task_progress", sse::EV_TASK_PROGRESS),
 ("task_updated", sse::EV_TASK_UPDATED),
 ("config_reloaded", sse::EV_CONFIG_RELOADED),
 ("capture_progress", sse::EV_CAPTURE_PROGRESS),
 ("goal_progress", sse::EV_GOAL_PROGRESS),
 ("goal_created", sse::EV_GOAL_CREATED),
 ("goal_deleted", sse::EV_GOAL_DELETED),
 ("goal_updated", sse::EV_GOAL_UPDATED),
 ] {
 let pattern = format!("'{name}'");
 assert!(sse_js.contains(&pattern), "sse.js missing addEventListener('{name}')");
 assert_eq!(name, const_val, "sse const value mismatch for {name}");
 }
 }

 // JS labels.js must match Rust labels.rs
 //
 // The mirrored label values (EMPTY_DASH, MODE/STRATEGY option pairs) must
 // be byte-identical between frontend/js/data/labels.js and
 // src/data/labels.rs. A drift here silently changes the UI text on one
 // side. This test reads labels.js and compares each mirrored value to the
 // Rust const, so a Rust-side rename that isn't reflected in JS fails.
 #[test]
 fn labels_sync_with_js() {
 let js = std::fs::read_to_string("frontend/js/data/labels.js")
 .expect("frontend/js/data/labels.js must be readable");
 // Each tuple: (JS substring that must appear, Rust const it mirrors).
 // Asserts the JS file contains the literal AND the literal equals the
 // Rust const - catches both missing-on-JS and value-drift.
 for (js_lit, rust_val) in [
 ("'-'", labels::EMPTY_DASH),
 ("'Download + Upload'", labels::MODE_DU_FULL),
 ("'Upload only'", labels::MODE_UO_FULL),
 ("'Fixed'", labels::STRATEGY_FIXED),
 ("'Dynamic'", labels::STRATEGY_DYNAMIC),
 ("'Download + Upload'", labels::GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD),
 ("'Stop'", labels::GOAL_REACHED_STOP),
 ("'Continue (initial speed)'", labels::GOAL_REACHED_CONTINUE_INITIAL),
 ("'Continue (custom speed)'", labels::GOAL_REACHED_CONTINUE_CUSTOM),
 ] {
 assert!(js.contains(js_lit),
 "labels.js missing {} (mirrors labels.rs value {:?})", js_lit, rust_val);
 assert_eq!(js_lit.trim_matches('\''), rust_val,
 "labels.js value drift from labels.rs: JS has {}, Rust has {:?}", js_lit, rust_val);
 }
 }

 // Query-placeholder validation uses protocol consts
 //
 // `config.rs` validates that client query templates contain the required
 // placeholders. The validation must use `protocol::Q_INFO_HASH` /
 // `Q_PEER_ID` (not raw `"{info_hash}"` literals) or the validator and
 // `announce::build_url` could diverge if a placeholder is renamed.
 #[test]
 fn config_validation_uses_protocol_query_consts() {
 let code = production_code("src/config.rs");
 assert!(
 code.contains("protocol::Q_INFO_HASH"),
 "config.rs validate() must use protocol::Q_INFO_HASH, not a raw literal"
 );
 assert!(
 code.contains("protocol::Q_PEER_ID"),
 "config.rs validate() must use protocol::Q_PEER_ID, not a raw literal"
 );
 }

 // DDL status default uses vocab const
 //
 // The `audits.status` DDL default must use `vocab::STATUS_IDLE` (not a
 // raw `'idle'` literal) so the default tracks the vocab.
 #[test]
 fn ddl_status_default_uses_vocab_const() {
 let code = production_code("src/db.rs");
 assert!(
 code.contains("vocab::STATUS_IDLE"),
 "db.rs migrate() DDL must use vocab::STATUS_IDLE for the status default, not a raw 'idle' literal"
 );
 // The raw 'idle' literal must not appear in production db.rs (the
 // only place 'idle' was previously hardcoded was the DDL default).
 assert!(
 !code.contains("'idle'"),
 "db.rs production code contains raw 'idle' literal - use vocab::STATUS_IDLE instead"
 );
 }

 // Event::query_fragment uses vocab consts
 //
 // The BitTorrent wire `event=started/completed/stopped` strings must
 // route through `vocab::EVENT_*` so they stay in sync with the
 // `events.event` column values that the engine persists.
 #[test]
 fn event_query_fragment_uses_vocab() {
 let code = production_code("src/announce.rs");
 assert!(
 code.contains("vocab::EVENT_STARTED"),
 "announce.rs Event::query_fragment must use vocab::EVENT_STARTED"
 );
 assert!(
 code.contains("vocab::EVENT_COMPLETED"),
 "announce.rs Event::query_fragment must use vocab::EVENT_COMPLETED"
 );
 assert!(
 code.contains("vocab::EVENT_STOPPED"),
 "announce.rs Event::query_fragment must use vocab::EVENT_STOPPED"
 );
 }

 // Stopped announce records both Ok and Err
 //
 // The `event=stopped` announce is the audit's final call to the
 // tracker. It must be recorded in the event log on BOTH success and
 // failure - identical to the probe/started/regular/completed call
 // sites. The bug was an `if let Ok(resp) = … { … }` with no `else`
 // branch, which silently dropped network/parse errors: no event row,
 // no tracing log, nothing in the DB. The fix is a `match` with an
 // `Err` arm that emits an `AuditEvent`. This test forbids the
 // silent-drop form and requires the `match` form so the regression
 // cannot return.
 #[test]
 fn stopped_announce_handles_error_path() {
 let engine = production_code("src/engine.rs");
 assert!(
 !engine.contains("if let Ok(resp) = session.announce(state, Event::Stopped)"),
 "engine.rs stopped announce uses `if let Ok` - this silently drops the error path; \
 use a match with Ok and Err arms so failures are recorded"
 );
 assert!(
 engine.contains("match session.announce(state, Event::Stopped)"),
 "engine.rs stopped announce must use `match` so the Err arm is exercised"
 );
 }

 // Percent-encode is not duplicated
 //
 // The RFC 3986 unreserved-set predicate was previously duplicated in
 // announce.rs. The shared implementation lives in
 // `data::protocol::percent_encode_raw`. The call site must delegate to
 // it, not re-implement the loop.
 #[test]
 fn percent_encode_not_duplicated() {
 let announce = production_code("src/announce.rs");
 // announce.rs must not contain the raw unreserved-set predicate loop.
 assert!(
 !announce.contains("b.is_ascii_alphanumeric() || matches!(b, b'-'"),
 "announce.rs re-implements percent-encode - delegate to protocol::percent_encode_raw"
 );
 }

 // AnnounceSession must not generate its own peer_id/key
 //
 // The peer_id and key must be provided by the caller (engine::run)
 // via PeerIdentity, so they can be persisted and reused across
 // restarts. If AnnounceSession::new calls generate_peer_id/generate_key
 // internally, every restart produces a new random peer_id - the
 // tracker sees a brand-new peer whose baseline is the resumed total
 // (delta = 0), and all un-announced upload is lost.
 #[test]
 fn announce_session_does_not_generate_identity() {
 let announce = production_code("src/announce.rs");
 assert!(
 !announce.contains("generate_peer_id"),
 "announce.rs generates peer_id internally - the caller must provide PeerIdentity so the identity persists across restarts"
 );
 assert!(
 !announce.contains("generate_key"),
 "announce.rs generates key internally - the caller must provide PeerIdentity so the identity persists across restarts"
 );
 }

 // fmt_bytes lives in data::units, not templates.rs
 //
 // `fmt_bytes` was moved from `templates.rs` to `data::units` to
 // centralize byte formatting. The production definition must not
 // reappear in templates.rs.
 #[test]
 fn fmt_bytes_not_in_templates() {
 let code = production_code("src/templates.rs");
 assert!(
 !code.contains("fn fmt_bytes("),
 "templates.rs defines fmt_bytes - it was moved to data::units; import it instead"
 );
 }

 // KIB/MIB/GIB live in data::units, not config.rs
 //
 // The byte-unit constants were moved from `config.rs` to `data::units`.
 // They must not reappear as definitions in config.rs.
 #[test]
 fn byte_constants_not_in_config() {
 let code = production_code("src/config.rs");
 assert!(
 !code.contains("pub const KIB"),
 "config.rs defines KIB - it was moved to data::units"
 );
 assert!(
 !code.contains("pub const MIB"),
 "config.rs defines MIB - it was moved to data::units"
 );
 assert!(
 !code.contains("pub const GIB"),
 "config.rs defines GIB - it was moved to data::units"
 );
 }

 // Lifecycle strings must use vocab consts
 //
 // engine.rs production code must not contain raw `"leech"` or `"seed"`
 // string literals - they must route through `vocab::LIFECYCLE_LEECH`/
 // `LIFECYCLE_SEED`. A raw literal would silently desync from the DB
 // column values if the vocabulary changed.
 #[test]
 fn no_raw_lifecycle_strings_in_production() {
 let code = production_code("src/engine.rs");
 // Exclude comments - the word "leech" appears in config field names
 // like `leech_upload_factor` (those are identifier tokens, not string
 // literals). Only flag quoted string literals.
 for (lineno, line) in code.lines().enumerate() {
 let trimmed = line.trim_start();
 if trimmed.starts_with("//") {
 continue;
 }
 // Check for quoted string literals "leech" / "seed" (not
 // identifiers like leech_upload_factor or freeze_on_zero_leechers).
 assert!(
 !line.contains("\"leech\""),
 "src/engine.rs:{} contains raw `\"leech\"` - use `vocab::LIFECYCLE_LEECH`",
 lineno + 1,
 );
 assert!(
 !line.contains("\"seed\""),
 "src/engine.rs:{} contains raw `\"seed\"` - use `vocab::LIFECYCLE_SEED`",
 lineno + 1,
 );
 }
 }

 // Bencode keys must not appear raw outside data/protocol.rs
 //
 // All bencode dict keys (b"announce", b"info", b"complete", etc.) are
 // centralized in `data::protocol.rs` as `K_*` consts. Production files
 // that parse bencode (announce.rs, torrent.rs, bencode.rs) must use the
 // consts, not raw `b"..."` literals.
 #[test]
 fn no_raw_bencode_keys_outside_protocol() {
 // Source-text patterns for each bencode dict key (as `b"..."` literals).
 let bencode_key_patterns = [
 "b\"announce\"", "b\"info\"", "b\"name\"", "b\"length\"", "b\"files\"",
 "b\"failure reason\"", "b\"interval\"", "b\"complete\"", "b\"incomplete\"",
 "b\"peers\"", "b\"ip\"", "b\"port\"",
 ];
 for file in &["src/announce.rs", "src/torrent.rs", "src/bencode.rs", "src/peer_server.rs", "src/capture.rs"] {
 let code = production_code(file);
 for (lineno, line) in code.lines().enumerate() {
 let trimmed = line.trim_start();
 if trimmed.starts_with("//") {
 continue;
 }
 // Skip lines that reference protocol:: consts (wired correctly).
 if line.contains("protocol::") || line.contains("K_") {
 continue;
 }
 for &pat in &bencode_key_patterns {
 assert!(
 !line.contains(pat),
 "{file}:{} contains raw bencode key `{pat}` - use `protocol::K_*` const",
 lineno + 1,
 );
 }
 }
 }
 }

 // Magnet keys must use protocol consts
 //
 // magnet.rs production code must not contain raw `"xt"`, `"tr"`, `"dn"`,
 // `"xl"`, `"magnet:?"`, or `"urn:btih:"` literals - they must route
 // through `protocol::MAGNET_*`.
 #[test]
 fn no_raw_magnet_keys_in_production() {
 let code = production_code("src/magnet.rs");
 let forbidden = ["\"xt\"", "\"tr\"", "\"dn\"", "\"xl\""];
 for (lineno, line) in code.lines().enumerate() {
 let trimmed = line.trim_start();
 if trimmed.starts_with("//") {
 continue;
 }
 // Skip lines that reference protocol:: consts
 if line.contains("protocol::") {
 continue;
 }
 for &pat in &forbidden {
 assert!(
 !line.contains(pat),
 "src/magnet.rs:{} contains raw magnet key `{pat}` - use `protocol::MAGNET_*`",
 lineno + 1,
 );
 }
 }
 }

 // SSE event names on the emit side must use consts
 //
 // api.rs `sse_global` must use `sse::EV_*` consts (not raw `"audit"`,
 // `"task_created"`, etc.) so the wire protocol stays centralized.
 #[test]
 fn sse_emit_names_use_consts() {
 let code = production_code("src/api.rs");
 // The emit side must reference ALL sse::EV_* consts - a raw string
 // for any one would silently break that SSE channel.
 for (name, const_name) in [
 ("audit", "EV_AUDIT"),
 ("task_created", "EV_TASK_CREATED"),
 ("task_deleted", "EV_TASK_DELETED"),
 ("task_status", "EV_TASK_STATUS"),
 ("task_client", "EV_TASK_CLIENT"),
 ("task_progress", "EV_TASK_PROGRESS"),
 ("task_updated", "EV_TASK_UPDATED"),
 ("config_reloaded", "EV_CONFIG_RELOADED"),
 ("capture_progress", "EV_CAPTURE_PROGRESS"),
 ("goal_progress", "EV_GOAL_PROGRESS"),
 ("goal_created", "EV_GOAL_CREATED"),
 ("goal_deleted", "EV_GOAL_DELETED"),
 ("goal_updated", "EV_GOAL_UPDATED"),
 ] {
 assert!(
 code.contains(&format!("sse::{}", const_name)),
 "api.rs sse_global must use sse::{const_name}, not a raw \"{name}\" literal"
 );
 }
 }

 // insert_event bind count must match EVENTS_COLUMNS
 //
 // `insert_event` builds its placeholder list from `EVENTS_COLUMNS.len()-1`
 // but the `.bind()` calls are hardcoded. If a column is added to the
 // array without adding a corresponding `.bind()`, the placeholder count
 // auto-grows but the bind count doesn't → runtime sqlx error. This test
 // counts `.bind(` calls in the `insert_event` function and asserts they
 // equal the expected column count.
 #[test]
 fn insert_event_bind_count_matches_columns() {
 let code = production_code("src/db.rs");
 // Find the insert_event function body.
 let start = code.find("pub async fn insert_event")
 .expect("insert_event function not found in db.rs production code");
 // Find the closing brace by counting braces from the function body.
 let after_fn = &code[start..];
 let mut brace_depth = 0;
 let mut end = 0;
 let mut in_string = false;
 for (i, ch) in after_fn.char_indices() {
 match ch {
 '"' if !in_string => in_string = true,
 '"' if in_string => in_string = false,
 '{' if !in_string => brace_depth += 1,
 '}' if !in_string => {
 brace_depth -= 1;
 if brace_depth == 0 {
 end = i;
 break;
 }
 }
 _ => {}
 }
 }
 let body = &after_fn[..end];
 let bind_count = body.matches(".bind(").count();
 let expected = schema::EVENTS_COLUMNS.len() - 1; // exclude auto-increment `id`
 assert_eq!(
 bind_count, expected,
 "insert_event has {} .bind() calls but EVENTS_COLUMNS has {} non-id columns - \
 a column was added to the array without a corresponding .bind()",
 bind_count, expected,
 );
 }

 // No hardcoded Duration::from_secs/millis with raw literals
 //
 // Every `Duration::from_secs(N)` or `Duration::from_millis(N)` in production
 // code must reference a config field (cfg.*) or client spec (client.*) -
 // a raw literal is a DRY violation that bypasses config.toml. This is a
 // best-effort scan: it catches the common pattern of inline durations that
 // should come from config. Legitimate uses (e.g. in test helpers or when
 // constructing a Duration from an already-validated variable) are excluded
 // by checking for `cfg.` or `client.` on the same line.
 #[test]
 fn no_hardcoded_durations_in_production() {
 for file in PROD_FILES {
 let code = production_code(file);
 for (lineno, line) in code.lines().enumerate() {
 let trimmed = line.trim_start();
 if trimmed.starts_with("//") || trimmed.starts_with("*") {
 continue;
 }
 // Flag Duration::from_secs(N) / Duration::from_millis(N) with a
 // raw numeric literal. Allow if the line references cfg.* or client.*
 // (meaning the value comes from config.toml).
 let has_duration = line.contains("Duration::from_secs(")
 || line.contains("Duration::from_millis(");
 let from_config = line.contains("cfg.")
 || line.contains("client.")
 || line.contains("state.")
 || line.contains("opts.");
 if has_duration && !from_config {
 // Extract the argument to check if it's a raw number
 if let Some(start) = line.find("Duration::from_") {
 let rest = &line[start..];
 if let Some(args_start) = rest.find('(') {
 let args = &rest[args_start + 1..];
 if let Some(end) = args.find(')') {
 let arg = args[..end].trim();
 // Raw numeric literal (e.g. `5`, `100`, `15`).
 // Zero is allowed (used by disabled() sentinel).
 if arg != "0" && arg.chars().all(|c| c.is_ascii_digit()) && !arg.is_empty() {
 panic!(
 "{file}:{} contains hardcoded `{}` - all durations must come from config.toml (cfg.*) or state.*",
 lineno + 1,
 line.trim(),
 );
 }
 }
 }
 }
 }
 }
 }
 }

 // No raw protocol byte arrays outside data/protocol.rs
 //
 // Raw byte arrays like `[0xFF]`, `[0u8; 4]`, `[0u8; 256]` in production code
 // are protocol constants that must live in data/protocol.rs. This catches
 // the common pattern of inline byte arrays that should reference a const.
 // Excludes data/protocol.rs itself (where the consts are defined) and
 // lines that already reference protocol::*.
 #[test]
 fn no_raw_protocol_byte_arrays_in_production() {
 let forbidden_arrays = &[
 ("[0xFF]", "protocol::SEEDER_BITFIELD or similar const"),
 ("[0u8; 4]", "protocol::KEEPALIVE_MSG or similar const"),
 ("[0u8; 256]", "protocol::DISCARD_BUF_LEN or similar const"),
 ];
 for file in PROD_FILES {
 // Skip the data module itself - it defines the consts.
 if file.contains("data/") {
 continue;
 }
 let code = production_code(file);
 for (lineno, line) in code.lines().enumerate() {
 let trimmed = line.trim_start();
 if trimmed.starts_with("//") {
 continue;
 }
 for (pattern, suggestion) in forbidden_arrays {
 if line.contains(pattern) && !line.contains("protocol::") {
 panic!(
 "{file}:{} contains raw byte array `{}` - use `{suggestion}` instead",
 lineno + 1,
 pattern,
 );
 }
 }
 }
 }
 }

 // build.sh MODULES ↔ modules.js MODULE_PATHS order sync
 //
 // The JS module list lives in TWO places: build.sh `MODULES` (the bundle
 // concatenation order - deps before dependents, init.js last) and
 // frontend/tests/modules.js `MODULE_PATHS` (the test iteration order).
 // They must contain the same modules in the same order so a module added
 // to one list but not the other is caught. modules-sync.test.js (frontend)
 // only checks MODULE_PATHS internally; this Rust test cross-checks the
 // two lists since build.sh is not reachable from the browser.
 #[test]
 fn build_sh_modules_match_modules_js() {
 let build = std::fs::read_to_string("build.sh")
 .expect("build.sh must be readable");
 let modules_js = std::fs::read_to_string("frontend/tests/modules.js")
 .expect("frontend/tests/modules.js must be readable");

 // Extract basenames from build.sh MODULES="..." block.
 let mut build_modules: Vec<String> = Vec::new();
 for line in build.lines() {
 let trimmed = line.trim();
 if trimmed.starts_with("frontend/js/") && trimmed.ends_with(".js") {
 // "frontend/js/utils/format.js" → "utils/format.js"
 build_modules.push(trimmed.strip_prefix("frontend/js/").unwrap().to_string());
 }
 }

 // Extract basenames from modules.js MODULE_PATHS array.
 let mut js_modules: Vec<String> = Vec::new();
 for line in modules_js.lines() {
 let trimmed = line.trim();
 if let Some(rest) = trimmed.strip_prefix("'../js/")
 && let Some(path) = rest.strip_suffix("',") {
 js_modules.push(path.to_string());
 }
 }

 assert_eq!(
 build_modules.len(),
 js_modules.len(),
 "build.sh has {} modules but modules.js has {} - a module was added to one but not the other",
 build_modules.len(),
 js_modules.len(),
 );
 for (i, (b, j)) in build_modules.iter().zip(js_modules.iter()).enumerate() {
 assert_eq!(
 b, j,
 "module order mismatch at position {i}: build.sh has \"{b}\" but modules.js has \"{j}\""
 );
 }
 }

 #[test]
 fn peer_id_module_is_data_free() {
 let code = production_code("src/peer_id.rs");
 for pat in &["\"-qB", "\"-TR", "\"-DE", "\"-UT", "\"-BT"] {
 assert!(!code.contains(pat),
 "peer_id.rs contains client prefix literal {pat} - client spec data must come from config.toml");
 }
 }

 #[test]
 fn no_dead_pub_consts_in_data() {
 use std::collections::HashSet;
 let mut files: Vec<std::path::PathBuf> = Vec::new();
 collect_rs_files(std::path::Path::new("src/data"), &mut files);
 let mut consts: HashSet<String> = HashSet::new();
 for f in &files {
 let code = std::fs::read_to_string(f).unwrap_or_default();
 for line in code.lines() {
 if let Some(name) = line.trim().strip_prefix("pub const ")
 && let Some(name) = name.split(|c: char| !c.is_alphanumeric() && c != '_').next()
 && !name.is_empty() { consts.insert(name.to_string()); }
 }
 }
 let mut all_code = String::new();
 let mut src_files: Vec<std::path::PathBuf> = Vec::new();
 collect_rs_files(std::path::Path::new("src"), &mut src_files);
 for f in &src_files {
 if f.to_string_lossy().contains("src/data/") { continue; }
 all_code.push_str(&std::fs::read_to_string(f).unwrap_or_default());
 all_code.push('\n');
 }
 let mut data_code = String::new();
 for f in &files {
 data_code.push_str(&std::fs::read_to_string(f).unwrap_or_default());
 data_code.push('\n');
 }
 let dead: Vec<&String> = consts.iter()
 .filter(|name| {
 let in_src = all_code.contains(name.as_str());
 let in_data = data_code.matches(name.as_str()).count() > 1;
 !in_src && !in_data
 })
 .collect();
 assert!(dead.is_empty(),
 "dead pub consts in src/data/ (defined but never referenced outside their definition):\n {}",
 dead.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n "));
 }
}
