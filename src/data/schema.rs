//! SQLite schema - single source of truth for table, column, and index names.
//!
//! `db::migrate` creates the `audits` and `events` tables; the column names
//! are repeated in INSERT/UPDATE/SELECT statements and in the `sqlx::FromRow`
//! structs (whose field names must match by Rust identifier, so they are kept
//! in sync by proximity to the queries). The ordered `*_COLUMNS` arrays are
//! the canonical column lists used by the schema-drift tests, replacing the
//! hand-typed duplicates that previously drifted from the real DDL.

pub const AUDITS: &str = "audits";
pub const EVENTS: &str = "events";
pub const IDX_EVENTS_AUDIT: &str = "idx_events_audit";

// audits columns
pub const ID: &str = "id";
pub const NAME: &str = "name";
pub const ANNOUNCE_URL: &str = "announce_url";
pub const INFO_HASH: &str = "info_hash";
pub const TORRENT_SIZE: &str = "torrent_size";
pub const CONFIG_JSON: &str = "config_json";
pub const STATUS: &str = "status";
pub const WORKING_CLIENT: &str = "working_client";
pub const CREATED_AT: &str = "created_at";
pub const LAST_UPLOADED: &str = "last_uploaded";
pub const LAST_DOWNLOADED: &str = "last_downloaded";
pub const LAST_LEFT: &str = "last_left";
pub const LIFECYCLE_PHASE: &str = "lifecycle_phase";
pub const COMPLETED_SENT: &str = "completed_sent";
pub const ELAPSED_SECS: &str = "elapsed_secs";
pub const PEER_ID: &str = "peer_id";
pub const KEY: &str = "key";

// events columns
pub const AUDIT_ID: &str = "audit_id";
pub const SEQ: &str = "seq";
pub const TIMESTAMP: &str = "timestamp";
pub const PHASE: &str = "phase";
pub const CLIENT: &str = "client";
pub const EVENT: &str = "event";
pub const UPLOADED: &str = "uploaded";
pub const DOWNLOADED: &str = "downloaded";
pub const LEFT: &str = "left";
pub const SUCCESS: &str = "success";
pub const FAILURE_REASON: &str = "failure_reason";
pub const INTERVAL: &str = "interval";
pub const SEEDERS: &str = "seeders";
pub const LEECHERS: &str = "leechers";
pub const PEER_COUNT: &str = "peer_count";
pub const LATENCY_MS: &str = "latency_ms";
pub const FAIR_SHARE_BPS: &str = "fair_share_bps";
pub const DYNAMIC_TARGET_BPS: &str = "dynamic_target_bps";
pub const NEXT_ANNOUNCE_IN_SECS: &str = "next_announce_in_secs";

/// `audits` columns in the order `migrate` creates them (base DDL + ALTERs).
pub const AUDITS_COLUMNS: &[&str] = &[
 ID, NAME, ANNOUNCE_URL, INFO_HASH, TORRENT_SIZE, CONFIG_JSON, STATUS,
 WORKING_CLIENT, CREATED_AT, LAST_UPLOADED, LAST_DOWNLOADED, LAST_LEFT,
 LIFECYCLE_PHASE, COMPLETED_SENT, ELAPSED_SECS, PEER_ID, KEY,
];

/// `events` columns in definition order (including the internal rowid `id`;
/// consumers that don't persist `id` use `EVENTS_COLUMNS[1..]` to skip it).
pub const EVENTS_COLUMNS: &[&str] = &[
 ID, AUDIT_ID, SEQ, TIMESTAMP, PHASE, CLIENT, EVENT, UPLOADED, DOWNLOADED,
 LEFT, SUCCESS, FAILURE_REASON, INTERVAL, SEEDERS, LEECHERS, PEER_COUNT,
 LATENCY_MS, FAIR_SHARE_BPS, DYNAMIC_TARGET_BPS, NEXT_ANNOUNCE_IN_SECS,
 ELAPSED_SECS,
];

// Base vs migration partition
//
// Each table's columns are split into two groups:
// - BASE: columns in the original CREATE TABLE statement. These are always
// present on a fresh DB.
// - MIGRATION: columns added after the initial schema. `migrate()` issues an
// idempotent `ALTER TABLE ADD COLUMN` for each, so old DBs (created by
// earlier binaries) get the column.
//
// When you add a new column:
// 1. Add the column name const above.
// 2. Add it to `*_COLUMNS` (the full ordered list).
// 3. Add it to either `*_BASE_COLUMNS` or `*_MIGRATION_COLUMNS` (with DDL).
// 4. If it's a migration column, also add a `.bind()` in `insert_event` /
// `save_peer_state` and a field in `EventRow` / `PeerStateRowSql`.
//
// The partition test (`columns_partition_correct`) verifies that
// `*_BASE ∪ *_MIGRATION == *_COLUMNS` with no overlap, so a column can't
// silently land in neither group. The migration test (`migrate_adds_columns`)
// recreates a table with only base columns, runs `migrate()`, and verifies
// all `*_COLUMNS` appear - catching a missing ALTER TABLE automatically.

/// `audits` columns in the original CREATE TABLE.
#[cfg(test)]
pub const AUDITS_BASE_COLUMNS: &[&str] = &[
 ID, NAME, ANNOUNCE_URL, INFO_HASH, TORRENT_SIZE, CONFIG_JSON, STATUS,
 WORKING_CLIENT, CREATED_AT,
];

/// `audits` columns added via ALTER TABLE (name + DDL suffix).
pub const AUDITS_MIGRATION_COLUMNS: &[(&str, &str)] = &[
 (LAST_UPLOADED, "INTEGER NOT NULL DEFAULT 0"),
 (LAST_DOWNLOADED, "INTEGER NOT NULL DEFAULT 0"),
 (LAST_LEFT, "INTEGER NOT NULL DEFAULT 0"),
 (LIFECYCLE_PHASE, "TEXT"),
 (COMPLETED_SENT, "INTEGER NOT NULL DEFAULT 0"),
 (ELAPSED_SECS, "INTEGER NOT NULL DEFAULT 0"),
 (PEER_ID, "TEXT"),
 (KEY, "TEXT"),
];

/// `events` columns in the original CREATE TABLE.
#[cfg(test)]
pub const EVENTS_BASE_COLUMNS: &[&str] = &[
 ID, AUDIT_ID, SEQ, TIMESTAMP, PHASE, CLIENT, EVENT, UPLOADED, DOWNLOADED,
 LEFT, SUCCESS, FAILURE_REASON, INTERVAL, SEEDERS, LEECHERS, PEER_COUNT,
 LATENCY_MS,
];

/// `events` columns added via ALTER TABLE (name + DDL suffix).
pub const EVENTS_MIGRATION_COLUMNS: &[(&str, &str)] = &[
 (FAIR_SHARE_BPS, "INTEGER NOT NULL DEFAULT 0"),
 (DYNAMIC_TARGET_BPS, "INTEGER NOT NULL DEFAULT 0"),
 (NEXT_ANNOUNCE_IN_SECS, "INTEGER NOT NULL DEFAULT 0"),
 (ELAPSED_SECS, "INTEGER NOT NULL DEFAULT 0"),
];

// global_goals table
pub const GLOBAL_GOALS: &str = "global_goals";
pub const GOAL_TASKS: &str = "goal_tasks";

// global_goals columns (reuse ID, NAME, ENABLED is new, CREATED_AT reused)
pub const ENABLED: &str = "enabled";
pub const DIRECTION: &str = "direction";
pub const UPLOAD_TARGET: &str = "upload_target";
pub const DOWNLOAD_TARGET: &str = "download_target";
pub const TARGET_SECS: &str = "target_secs";
pub const REACHED_ACTION: &str = "reached_action";
pub const REACHED_BPS: &str = "reached_bps";

// goal_tasks junction columns
pub const GOAL_ID: &str = "goal_id";
pub const TASK_ID: &str = "task_id";

/// `global_goals` columns in the order `migrate` creates them.
pub const GLOBAL_GOALS_COLUMNS: &[&str] = &[
 ID, NAME, ENABLED, DIRECTION, UPLOAD_TARGET, DOWNLOAD_TARGET,
 TARGET_SECS, REACHED_ACTION, REACHED_BPS, CREATED_AT,
];

/// `goal_tasks` junction columns.
pub const GOAL_TASKS_COLUMNS: &[&str] = &[GOAL_ID, TASK_ID];

/// `global_goals` base columns (all in the original CREATE TABLE - no migrations).
#[cfg(test)]
pub const GLOBAL_GOALS_BASE_COLUMNS: &[&str] = GLOBAL_GOALS_COLUMNS;
pub const GLOBAL_GOALS_MIGRATION_COLUMNS: &[(&str, &str)] = &[];

#[cfg(test)]
pub const GOAL_TASKS_BASE_COLUMNS: &[&str] = GOAL_TASKS_COLUMNS;
pub const GOAL_TASKS_MIGRATION_COLUMNS: &[(&str, &str)] = &[];
