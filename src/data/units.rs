//! Binary byte units and human-readable byte formatting.
//!
//! KiB/MiB/GiB are physical constants (not config-tunable), so they live here
//! rather than in `config.toml`. [`fmt_bytes`] is the single formatter used by
//! the server-rendered templates; the frontend JS `formatBytes` mirrors it
//! (both must stay byte-identical so a table cell looks the same whether it
//! arrives via the initial snapshot or a live SSE update).

/// Percentage scale - 100 percent = 1 whole. Used as a validation bound
/// (`<= 100`) and a percent→fraction divisor (`/ 100`). A physical constant,
/// not config-tunable.
pub const PERCENT: u32 = 100;
/// String form of [`PERCENT`] for HTML `min`/`max` attributes. Single source
/// of truth so the UI bound and the validator bound can't drift.
pub const PERCENT_STR: &str = "100";

/// Bounds for `swarm_defaults.seed_share_factor` - the fraction of swarm
/// upload allocated to us. The lower bound is **exclusive** (0.0 is rejected:
/// a seeder contributing 0% of the upload is pointless and a detection
/// vector); the upper bound is inclusive (1.0 = 100%). Physical constants,
/// not config-tunable. Single source of truth shared by
/// `config::SwarmDefaultsConfig::validate`, `swarm::SwarmConfig::validate`,
/// and the settings UI so the two validators (which previously disagreed on
/// whether 0.0 is legal) and the UI can't drift again.
pub const SEED_SHARE_FACTOR_MIN: f64 = 0.0;
pub const SEED_SHARE_FACTOR_MAX: f64 = 1.0;
/// String form of [`SEED_SHARE_FACTOR_MAX`] for the settings UI `max`
/// attribute.
pub const SEED_SHARE_FACTOR_MAX_STR: &str = "1";

/// Upper bound for `watcher.debounce_ms` (the hot-reload quiet period).
/// Physical constant, not config-tunable. Single source of truth shared by
/// `config::WatcherConfig::validate` and the settings UI so the validator
/// bound and the UI `max` attribute can't drift.
pub const DEBOUNCE_MS_MAX: u64 = 10_000;
/// String form of [`DEBOUNCE_MS_MAX`] for the settings UI `max` attribute.
pub const DEBOUNCE_MS_MAX_STR: &str = "10000";

/// Seconds per day. A physical constant used both as the [`fmt_duration`]
/// days-tier threshold and as the upper bound for `defaults.ramp_up_secs`
/// (24h) and `defaults.goal_target_secs` (via [`GOAL_MAX_TIME_SECS`]). Single
/// source of truth so the formatter, the validators, and the settings UI
/// can't drift.
pub const SECS_PER_DAY: u64 = 86_400;
/// String form of [`SECS_PER_DAY`] for the settings UI `max` attribute on
/// `ramp_up_secs`.
pub const SECS_PER_DAY_STR: &str = "86400";

/// Upper bound for `defaults.goal_target_secs` (the goal deadline). One year
/// of seconds - a sane ceiling that rejects absurd values without blocking
/// any realistic long-running seed goal. Physical constant, not
/// config-tunable. Single source of truth shared by `config::validate` and
/// the settings UI `max` attribute. `0` means forward/ETA-only mode (no
/// deadline, no speed adjustment).
pub const GOAL_MAX_TIME_SECS: u64 = SECS_PER_DAY * 366;
/// String form of [`GOAL_MAX_TIME_SECS`] for the settings UI `max` attribute.
pub const GOAL_MAX_TIME_SECS_STR: &str = "31536600";

/// Upper bound for `defaults.goal_target_bytes` (the goal amount). 1 TiB -
/// a sane ceiling that rejects absurd values. Physical constant, not
/// config-tunable. Single source of truth shared by `config::validate`.
pub const GOAL_MAX_TARGET_BYTES: u64 = 1_099_511_627_776;

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;

// Public for use in render.rs (byte-unit <option> values). The test
// `no_raw_byte_constants_in_production` enforces that production code uses
// these consts instead of raw `1024`/`1048576`/`1073741824` literals.
pub const BYTE_UNIT_B: u64 = 1;
pub const BYTE_UNIT_KIB: u64 = KIB;
pub const BYTE_UNIT_MIB: u64 = MIB;
pub const BYTE_UNIT_GIB: u64 = GIB;

pub const UNIT_B: &str = "B";
pub const UNIT_KIB: &str = "KiB";
pub const UNIT_MIB: &str = "MiB";
pub const UNIT_GIB: &str = "GiB";

/// Compose a speed-unit label (e.g. `"KiB/s"`) from the byte-unit string and
/// `labels::PER_SEC`. Single source of truth for the speed-unit `<option>`
/// labels used by the settings UI.
pub fn speed_unit_label(unit: &str) -> String {
 format!("{}{}", unit, crate::data::labels::PER_SEC)
}

/// Human-readable byte count, binary units, 2-decimal precision above 1023.
pub fn fmt_bytes(n: u64) -> String {
 if n >= GIB {
 format!("{:.2} {}", n as f64 / GIB as f64, UNIT_GIB)
 } else if n >= MIB {
 format!("{:.2} {}", n as f64 / MIB as f64, UNIT_MIB)
 } else if n >= KIB {
 format!("{:.2} {}", n as f64 / KIB as f64, UNIT_KIB)
 } else {
 format!("{} {}", n, UNIT_B)
 }
}

/// `i64` wrapper for DB columns stored as INTEGER.
pub fn fmt_bytes_i64(n: i64) -> String {
 fmt_bytes(n as u64)
}

/// Human-readable duration in seconds, using the compact `Hh Mm` / `Mm Ss` /
/// `Ss` / `Nd Mh` format. Used for the "next announce in" countdown and for
/// goal ETAs (which can exceed a day).
///
/// - 0 → `"0s"`
/// - < 60 → `"45s"`
/// - < 3600 → `"4m 30s"`
/// - < 86400 → `"1h 5m"`
/// - >= 86400 → `"3d 4h"`
pub fn fmt_duration(secs: u64) -> String {
 if secs < 60 {
 format!("{secs}s")
 } else if secs < 3600 {
 format!("{}m {}s", secs / 60, secs % 60)
 } else if secs < SECS_PER_DAY {
 format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
 } else {
 format!("{}d {}h", secs / SECS_PER_DAY, (secs % SECS_PER_DAY) / 3600)
 }
}

/// Format a byte rate as a speed string (e.g. `"512.00 KiB/s"`). The single
/// source of truth for speed display - used by the stats panel (standalone)
/// and by [`fmt_speed_arrow`] / [`fmt_speed_cell`] (table). Zero → `"0 B/s"`.
pub fn fmt_speed_bps(bps: u64) -> String {
 format!("{}{}", fmt_bytes(bps), crate::data::labels::PER_SEC)
}

/// Format a speed with an arrow suffix. Always shows the arrow so the
/// direction (upload ↑ / download ↓) is visible even at zero speed.
pub fn fmt_speed_arrow(bps: u64, arrow: &str) -> String {
 format!("{} {arrow}", fmt_speed_bps(bps))
}

/// Compose the speed table cell from upload/download speeds.
/// Both arrows are always shown (even at zero speed).
/// The download part is only included when `show_download` is true.
pub fn fmt_speed_cell(up_bps: u64, down_bps: u64, show_download: bool) -> String {
 let up = fmt_speed_arrow(up_bps, "↑");
 if !show_download {
 return up;
 }
 format!("{up} {}", fmt_speed_arrow(down_bps, "↓"))
}

#[cfg(test)]
mod tests {
 use super::*;

 // fmt_duration

 #[test]
 fn fmt_duration_zero() {
 assert_eq!(fmt_duration(0), "0s");
 }

 #[test]
 fn fmt_duration_seconds_only() {
 assert_eq!(fmt_duration(1), "1s");
 assert_eq!(fmt_duration(45), "45s");
 assert_eq!(fmt_duration(59), "59s");
 }

 #[test]
 fn fmt_duration_minutes_and_seconds() {
 assert_eq!(fmt_duration(60), "1m 0s");
 assert_eq!(fmt_duration(90), "1m 30s");
 assert_eq!(fmt_duration(270), "4m 30s");
 assert_eq!(fmt_duration(599), "9m 59s");
 assert_eq!(fmt_duration(1800), "30m 0s");
 }

 #[test]
 fn fmt_duration_hours_and_minutes() {
 assert_eq!(fmt_duration(3600), "1h 0m");
 assert_eq!(fmt_duration(3900), "1h 5m");
 assert_eq!(fmt_duration(7384), "2h 3m");
 assert_eq!(fmt_duration(86399), "23h 59m");
 }

 #[test]
 fn fmt_duration_days_and_hours() {
 assert_eq!(fmt_duration(86400), "1d 0h");
 assert_eq!(fmt_duration(90000), "1d 1h");
 assert_eq!(fmt_duration(276_400), "3d 4h");
 assert_eq!(fmt_duration(1_209_600), "14d 0h");
 }

 // fmt_speed_bps

 #[test]
 fn fmt_speed_bps_zero_shows_zero() {
 assert_eq!(fmt_speed_bps(0), "0 B/s");
 }

 #[test]
 fn fmt_speed_bps_formats_with_per_sec() {
 assert_eq!(fmt_speed_bps(524_288), "512.00 KiB/s");
 assert_eq!(fmt_speed_bps(1_048_576), "1.00 MiB/s");
 assert_eq!(fmt_speed_bps(1), "1 B/s");
 }

 // fmt_speed_arrow

 #[test]
 fn fmt_speed_arrow_zero_shows_zero_with_arrow() {
 assert_eq!(fmt_speed_arrow(0, "↑"), "0 B/s ↑");
 assert_eq!(fmt_speed_arrow(0, "↓"), "0 B/s ↓");
 }

 #[test]
 fn fmt_speed_arrow_formats_with_per_sec_and_arrow() {
 assert_eq!(fmt_speed_arrow(524_288, "↑"), "512.00 KiB/s ↑");
 assert_eq!(fmt_speed_arrow(1_048_576, "↓"), "1.00 MiB/s ↓");
 }

 // fmt_speed_cell

 #[test]
 fn fmt_speed_cell_both_zero_shows_both_arrows() {
 assert_eq!(fmt_speed_cell(0, 0, true), "0 B/s ↑ 0 B/s ↓");
 assert_eq!(fmt_speed_cell(0, 0, false), "0 B/s ↑");
 }

 #[test]
 fn fmt_speed_cell_upload_only_zero_download() {
 assert_eq!(fmt_speed_cell(524_288, 0, true), "512.00 KiB/s ↑ 0 B/s ↓");
 assert_eq!(fmt_speed_cell(524_288, 0, false), "512.00 KiB/s ↑");
 }

 #[test]
 fn fmt_speed_cell_download_only_zero_upload() {
 assert_eq!(fmt_speed_cell(0, 1_048_576, true), "0 B/s ↑ 1.00 MiB/s ↓");
 }

 #[test]
 fn fmt_speed_cell_both_present_with_space() {
 assert_eq!(fmt_speed_cell(524_288, 1_048_576, true), "512.00 KiB/s ↑ 1.00 MiB/s ↓");
 }

 #[test]
 fn fmt_speed_cell_hides_download_when_flag_false() {
 assert_eq!(fmt_speed_cell(524_288, 1_048_576, false), "512.00 KiB/s ↑");
 }
}
