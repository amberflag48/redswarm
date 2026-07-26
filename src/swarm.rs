//! Swarm dynamics - calculates realistic upload speed based on live swarm data.
//!
//! The core insight: a seeder's real upload speed is bounded by leecher demand
//! divided among seeders. Faking more than ~2× your fair share is a statistical
//! outlier that advanced trackers can detect via per-torrent balance checks.
//!
//! Formula (from research):
//! fair_share = (leechers × avg_leecher_download × seed_share) / max(seeders, 1)
//!
//! Where:
//! - avg_leecher_download ≈ 3 MB/s (typical private tracker leecher)
//! - seed_share ≈ 0.8 (seeders meet ~80% of demand; P2P covers the rest)
//! - 0 leechers → 0 upload (uploading to nobody is impossible)

/// Swarm snapshot - seeder/leecher counts from the tracker announce response.
///
/// Every BitTorrent announce response (BEP-3) carries `complete` (seeders) and
/// `incomplete` (leechers) counts. This struct is the container those counts
/// flow into; it is the input to [`fair_share_bps`] and
/// [`dynamic_download_bps`].
#[derive(Debug, Clone, Default)]
pub struct SwarmData {
 pub seeders: i64,
 pub leechers: i64,
}

/// Configuration for the fair-share calculator.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SwarmConfig {
 /// Average leecher download speed in bytes/sec. Default 3 MB/s.
 pub avg_leecher_download_bps: u64,
 /// Fraction of leecher demand met by seeders (rest is P2P). Default 0.8.
 pub seed_share_factor: f64,
 /// Multiplier applied to fair share for our target speed. Default 1.0
 /// (match fair share exactly). 2.0 = upload 2× fair share (aggressive).
 pub fair_share_multiplier: f64,
 /// Hard cap on upload speed regardless of fair share (bytes/sec).
 /// 0 = unlimited. Default 0.
 pub max_upload_bps: u64,
 /// Hard cap on download speed regardless of supply (bytes/sec).
 /// 0 = unlimited. Default 0.
 pub max_download_bps: u64,
}

/// Construction is via [`SwarmConfig::from_defaults`], which reads the
/// `[swarm_defaults]` section of the loaded `config.toml`. There is
/// intentionally no `Default` impl - values always come from config (no
/// defaults in Rust code, per AGENTS.md).
impl SwarmConfig {
 /// Construct from the `[swarm_defaults]` section of `config.toml`.
 pub fn from_defaults(d: &crate::config::SwarmDefaultsConfig) -> Self {
 Self {
 avg_leecher_download_bps: d.avg_leecher_download_bps,
 seed_share_factor: d.seed_share_factor,
 fair_share_multiplier: d.fair_share_multiplier,
 max_upload_bps: d.max_upload_bps,
 max_download_bps: d.max_download_bps,
 }
 }
}

impl SwarmConfig {
 /// Validate range-constrained fields. Returns `Err(message)` naming the
 /// first invalid field, or `Ok(())` if every field is in range.
 pub fn validate(&self) -> Result<(), String> {
 if !self.seed_share_factor.is_finite() {
 return Err(format!(
 "seed_share_factor must be finite, got {}",
 self.seed_share_factor
 ));
 }
 if !(self.seed_share_factor > crate::data::units::SEED_SHARE_FACTOR_MIN
 && self.seed_share_factor <= crate::data::units::SEED_SHARE_FACTOR_MAX)
 {
 return Err(format!(
 "seed_share_factor must be in (0.0, 1.0], got {}",
 self.seed_share_factor
 ));
 }
 if !self.fair_share_multiplier.is_finite() {
 return Err(format!(
 "fair_share_multiplier must be finite, got {}",
 self.fair_share_multiplier
 ));
 }
 if self.fair_share_multiplier < 0.0 {
 return Err(format!(
 "fair_share_multiplier must be >= 0.0, got {}",
 self.fair_share_multiplier
 ));
 }
 if self.avg_leecher_download_bps == 0 {
 return Err("avg_leecher_download_bps must be greater than 0".into());
 }
 Ok(())
 }
}

/// Calculate the fair-share upload speed for a seeder in the current swarm.
///
/// Returns the target upload speed in bytes/sec, or 0 if the swarm has no
/// leechers (uploading to nobody is the #1 detection vector).
pub fn fair_share_bps(swarm: &SwarmData, config: &SwarmConfig) -> u64 {
 let leechers = swarm.leechers;
 let seeders = swarm.seeders.max(1);

 // No leechers → no demand → no upload
 if leechers == 0 {
 return 0;
 }

 // Defense in depth: NaN/Infinity would corrupt the float→u64 cast
 // (`NaN as u64 = 0`, `Infinity as u64 = u64::MAX`). `validate()` is the
 // primary guard at the API boundary, but never trust a raw serde float
 // here - a caller can construct SwarmConfig directly.
 if !config.seed_share_factor.is_finite() || !config.fair_share_multiplier.is_finite() {
 return 0;
 }

 // fair_share = (L × avg_download × seed_share) / S
 let fair_share = (leechers as f64
 * config.avg_leecher_download_bps as f64
 * config.seed_share_factor)
 / seeders as f64;

 // Apply multiplier (1.0 = match fair share, 2.0 = 2× fair share)
 let target = fair_share * config.fair_share_multiplier;

 // Cap at persona's physical max (0 = unlimited)
 let target = if config.max_upload_bps > 0 {
 target.min(config.max_upload_bps as f64)
 } else {
 target
 };
 target as u64
}

/// Calculate a realistic download speed for a leecher in the current swarm.
///
/// A leecher's download speed is bounded by the total upload capacity of
/// the seeders divided among leechers. In dynamic mode this replaces the
/// manual download_bps setting.
pub fn dynamic_download_bps(swarm: &SwarmData, config: &SwarmConfig) -> u64 {
 let leechers = swarm.leechers.max(1);
 let seeders = swarm.seeders;

 // No seeders → nobody to download from
 if seeders == 0 {
 return 0;
 }

 // Defense in depth: guard against NaN/Infinity in seed_share_factor
 if !config.seed_share_factor.is_finite() {
 return 0;
 }

 // Total seeder upload capacity = seeders × avg_seeder_upload
 // avg_seeder_upload ≈ avg_leecher_download (bandwidth is symmetric in aggregate)
 // Each leecher's share = total_supply / leechers
 let total_supply = seeders as f64 * config.avg_leecher_download_bps as f64 * config.seed_share_factor;
 let per_leecher = total_supply / leechers as f64;

 // Cap at the typical leecher download speed
 let capped = per_leecher.min(config.avg_leecher_download_bps as f64);

 // Apply max download cap (0 = unlimited)
 if config.max_download_bps > 0 {
 (capped.min(config.max_download_bps as f64)) as u64
 } else {
 capped as u64
 }
}

/// Calculate the maximum safe cumulative upload for a torrent to stay under
/// the per-torrent balance threshold (4% of torrent size).
///
/// If we report download ≈ upload (download-offset strategy), the net
/// balance contribution is ~0, so the safe upload is ~torrent_size.
/// Without download offset, it's 4% of torrent_size.
#[cfg(test)]
pub fn max_safe_upload_bps(
 torrent_size: u64,
 reported_downloaded: u64,
 current_uploaded: u64,
) -> u64 {
 // Balance contribution = uploaded - downloaded
 // Safe limit: |balance| <= 4% of size (4% margin, not 5% - safety buffer)
 let safe_balance = torrent_size / 25; // 4%
 // current_balance can be negative (download > upload), which means we have MORE room
 let current_balance = current_uploaded as i64 - reported_downloaded as i64;
 let remaining_i = safe_balance as i64 - current_balance;
 if remaining_i <= 0 { 0 } else { remaining_i as u64 }
}

#[cfg(test)]
mod tests {
 use super::*;

 fn swarm(seeders: i64, leechers: i64) -> SwarmData {
 SwarmData {
 seeders,
 leechers,
 }
 }

 fn default_swarm_config() -> SwarmConfig {
 SwarmConfig {
 avg_leecher_download_bps: 3_000_000,
 seed_share_factor: 0.8,
 fair_share_multiplier: 1.0,
 max_upload_bps: 12_500_000,
 max_download_bps: 0,
 }
 }

 fn cfg_defaults() -> crate::config::SwarmDefaultsConfig {
 crate::config::test_helpers::swarm_defaults_cfg()
 }

 // fair_share_bps

 #[test]
 fn zero_leechers_means_zero_upload() {
 let swarm = swarm(130, 0);
 let config = default_swarm_config();
 assert_eq!(fair_share_bps(&swarm, &config), 0);
 }

 #[test]
 fn one_leecher_130_seeders_low_share() {
 let swarm = swarm(130, 1);
 let config = default_swarm_config();
 // fair_share = (1 × 3MB × 0.8) / 130 ≈ 18,461 B/s
 let speed = fair_share_bps(&swarm, &config);
 assert!(speed < 25_000, "130 seeders 1 leecher should be ~18 KB/s, got {speed}");
 assert!(speed > 10_000, "should not be zero with 1 leecher, got {speed}");
 }

 #[test]
 fn twenty_leechers_5_seeders_high_share() {
 let swarm = swarm(5, 20);
 let config = default_swarm_config();
 // fair_share = (20 × 3MB × 0.8) / 5 = 9.6 MB/s
 let speed = fair_share_bps(&swarm, &config);
 assert!(speed > 8_000_000, "5 seeders 20 leechers should be ~9.6 MB/s, got {speed}");
 assert!(speed < 12_000_000, "should be capped under max, got {speed}");
 }

 #[test]
 fn zero_seeders_treats_as_one() {
 let swarm = swarm(0, 10);
 let config = default_swarm_config();
 // fair_share = (10 × 3MB × 0.8) / 1 = 24 MB/s, capped at 12.5 MB/s
 let speed = fair_share_bps(&swarm, &config);
 assert_eq!(speed, 12_500_000, "0 seeders should cap at max_upload_bps");
 }

 #[test]
 fn multiplier_doubles_speed() {
 let swarm = swarm(5, 20);
 let mut config = default_swarm_config();
 config.fair_share_multiplier = 2.0;
 let speed = fair_share_bps(&swarm, &config);
 // 2× fair share = 19.2 MB/s, but capped at 12.5 MB/s
 assert_eq!(speed, 12_500_000, "2× multiplier should hit cap");
 }

 #[test]
 fn max_upload_cap_always_respected() {
 let swarm = swarm(1, 100); // huge fair share
 let mut config = default_swarm_config();
 config.max_upload_bps = 500_000; // 500 KB/s cap
 let speed = fair_share_bps(&swarm, &config);
 assert_eq!(speed, 500_000, "must respect max_upload_bps cap");
 }

 #[test]
 fn unlimited_upload_cap_no_limit() {
 let swarm = swarm(1, 100); // huge fair share
 let mut config = default_swarm_config();
 config.max_upload_bps = 0; // unlimited
 let speed = fair_share_bps(&swarm, &config);
 // Should NOT be capped - only bounded by the formula
 assert!(speed > 12_500_000, "unlimited cap should not restrict speed, got {speed}");
 }

 #[test]
 fn download_cap_respected() {
 let swarm = swarm(100, 1); // huge supply
 let mut config = default_swarm_config();
 config.max_download_bps = 500_000; // 500 KB/s cap
 let dl = dynamic_download_bps(&swarm, &config);
 assert_eq!(dl, 500_000, "must respect max_download_bps cap");
 }

 #[test]
 fn unlimited_download_cap_no_limit() {
 let swarm = swarm(100, 1); // huge supply
 let mut config = default_swarm_config();
 config.max_download_bps = 0; // unlimited
 let dl = dynamic_download_bps(&swarm, &config);
 // Capped only by avg_leecher_download (3 MB/s), not by max_download_bps
 assert_eq!(dl, 3_000_000, "unlimited download cap should only hit avg_leecher cap");
 }

 #[test]
 fn from_defaults_provides_config_values() {
 let config = SwarmConfig::from_defaults(&cfg_defaults());
 assert_eq!(config.avg_leecher_download_bps, 3_000_000);
 assert_eq!(config.seed_share_factor, 0.8);
 assert_eq!(config.fair_share_multiplier, 1.0);
 assert_eq!(config.max_upload_bps, 0, "default max upload = unlimited");
 assert_eq!(config.max_download_bps, 0, "default max download = unlimited");
 }

 // dynamic_download_bps

 #[test]
 fn dynamic_download_zero_seeders() {
 let swarm = swarm(0, 10);
 let config = default_swarm_config();
 assert_eq!(dynamic_download_bps(&swarm, &config), 0);
 }

 #[test]
 fn dynamic_download_many_seeders_capped() {
 // 100 seeders, 1 leecher → supply huge, but capped at avg_leecher_download
 let swarm = swarm(100, 1);
 let config = default_swarm_config();
 let dl = dynamic_download_bps(&swarm, &config);
 assert_eq!(dl, 3_000_000, "download should cap at avg_leecher_download");
 }

 #[test]
 fn dynamic_download_few_seeders_many_leechers() {
 // 2 seeders, 20 leechers → supply = 2×3MB×0.8 = 4.8MB, per_leecher = 240KB
 let swarm = swarm(2, 20);
 let config = default_swarm_config();
 let dl = dynamic_download_bps(&swarm, &config);
 assert!(dl < 500_000, "2 seeders 20 leechers should be ~240 KB/s, got {dl}");
 assert!(dl > 100_000, "should not be zero with seeders present, got {dl}");
 }

 // max_safe_upload (balance check)

 #[test]
 fn safe_upload_with_no_download() {
 // 10 GB torrent, 0 downloaded, 0 uploaded → can upload 4% = 400 MB
 let safe = max_safe_upload_bps(10_000_000_000, 0, 0);
 assert_eq!(safe, 400_000_000);
 }

 #[test]
 fn safe_upload_with_download_offset() {
 // 10 GB torrent, 5 GB downloaded, 0 uploaded
 // balance = 0 - 5GB = -5GB, safe_balance = 400MB
 // remaining = 400MB - (-5GB) = 5.4GB
 let safe = max_safe_upload_bps(10_000_000_000, 5_000_000_000, 0);
 assert_eq!(safe, 5_400_000_000);
 }

 #[test]
 fn safe_upload_fully_downloaded() {
 // 10 GB torrent, fully downloaded → can upload ~10.4 GB
 let safe = max_safe_upload_bps(10_000_000_000, 10_000_000_000, 0);
 assert_eq!(safe, 10_400_000_000);
 }

 #[test]
 fn safe_upload_near_threshold() {
 // Already uploaded 390 MB over downloaded, threshold 400 MB → 10 MB left
 let safe = max_safe_upload_bps(10_000_000_000, 0, 390_000_000);
 assert_eq!(safe, 10_000_000);
 }

 #[test]
 fn safe_upload_over_threshold_returns_zero() {
 // Already 500 MB over downloaded, threshold 400 MB → 0
 let safe = max_safe_upload_bps(10_000_000_000, 0, 500_000_000);
 assert_eq!(safe, 0);
 }

 // SwarmConfig::validate

 #[test]
 fn validate_accepts_default_swarm_config() {
 assert!(SwarmConfig::from_defaults(&cfg_defaults()).validate().is_ok());
 }

 #[test]
 fn validate_rejects_zero_and_accepts_one_seed_share_factor() {
 // 0.0 is rejected: a seeder allocated 0% of the swarm upload is pointless
 // and a detection vector. This must match `config::SwarmDefaultsConfig::validate`,
 // which also rejects 0.0 - the two validators previously disagreed.
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.seed_share_factor = 0.0;
 assert!(config.validate().is_err(), "0.0 must be rejected (matches config.rs)");
 config.seed_share_factor = 1.0;
 assert!(config.validate().is_ok(), "1.0 should be valid");
 }

 #[test]
 fn validate_rejects_negative_seed_share_factor() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.seed_share_factor = -0.5;
 let err = config.validate().unwrap_err();
 assert!(err.contains("seed_share_factor"), "got: {err}");
 }

 #[test]
 fn validate_rejects_nan_seed_share_factor() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.seed_share_factor = f64::NAN;
 let err = config.validate().unwrap_err();
 assert!(err.contains("seed_share_factor") && err.contains("finite"), "got: {err}");
 }

 #[test]
 fn validate_rejects_infinity_seed_share_factor() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.seed_share_factor = f64::INFINITY;
 let err = config.validate().unwrap_err();
 assert!(err.contains("seed_share_factor") && err.contains("finite"), "got: {err}");
 }

 #[test]
 fn validate_rejects_seed_share_factor_above_one() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.seed_share_factor = 1.5;
 let err = config.validate().unwrap_err();
 assert!(err.contains("seed_share_factor"), "got: {err}");
 }

 #[test]
 fn validate_rejects_negative_fair_share_multiplier() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.fair_share_multiplier = -1.0;
 let err = config.validate().unwrap_err();
 assert!(err.contains("fair_share_multiplier"), "got: {err}");
 }

 #[test]
 fn validate_rejects_nan_fair_share_multiplier() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.fair_share_multiplier = f64::NAN;
 let err = config.validate().unwrap_err();
 assert!(err.contains("fair_share_multiplier") && err.contains("finite"), "got: {err}");
 }

 #[test]
 fn validate_rejects_infinity_fair_share_multiplier() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.fair_share_multiplier = f64::INFINITY;
 let err = config.validate().unwrap_err();
 assert!(err.contains("fair_share_multiplier") && err.contains("finite"), "got: {err}");
 }

 #[test]
 fn validate_rejects_zero_avg_leecher_download_bps() {
 let mut config = SwarmConfig::from_defaults(&cfg_defaults());
 config.avg_leecher_download_bps = 0;
 let err = config.validate().unwrap_err();
 assert!(err.contains("avg_leecher_download_bps"), "got: {err}");
 }

 // fair_share_bps defensive guards

 #[test]
 fn fair_share_bps_zero_seed_share_factor_returns_zero() {
 let swarm = swarm(5, 20);
 let mut config = default_swarm_config();
 config.seed_share_factor = 0.0;
 assert_eq!(fair_share_bps(&swarm, &config), 0);
 }

 #[test]
 fn fair_share_bps_negative_seed_share_factor_returns_zero() {
 let swarm = swarm(5, 20);
 let mut config = default_swarm_config();
 config.seed_share_factor = -0.5;
 assert_eq!(fair_share_bps(&swarm, &config), 0);
 }

 #[test]
 fn fair_share_bps_nan_seed_share_factor_returns_zero() {
 let swarm = swarm(5, 20);
 let mut config = default_swarm_config();
 config.seed_share_factor = f64::NAN;
 assert_eq!(fair_share_bps(&swarm, &config), 0);
 }

 #[test]
 fn fair_share_bps_infinity_seed_share_factor_returns_zero() {
 let swarm = swarm(5, 20);
 let mut config = default_swarm_config();
 config.seed_share_factor = f64::INFINITY;
 assert_eq!(fair_share_bps(&swarm, &config), 0);
 }

 #[test]
 fn fair_share_bps_nan_fair_share_multiplier_returns_zero() {
 let swarm = swarm(5, 20);
 let mut config = default_swarm_config();
 config.fair_share_multiplier = f64::NAN;
 assert_eq!(fair_share_bps(&swarm, &config), 0);
 }

 #[test]
 fn fair_share_bps_infinity_fair_share_multiplier_returns_zero() {
 let swarm = swarm(5, 20);
 let mut config = default_swarm_config();
 config.fair_share_multiplier = f64::INFINITY;
 assert_eq!(fair_share_bps(&swarm, &config), 0);
 }
}
