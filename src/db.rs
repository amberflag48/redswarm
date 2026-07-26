//! SQLite persistence - audit runs and announce event logs.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

use crate::data::{schema, vocab};
use crate::engine::AuditEvent;

/// Build a `?, ?, ...` placeholder list with `n` entries.
fn placeholders(n: usize) -> String {
 (0..n).map(|_| "?").collect::<Vec<_>>().join(", ")
}

pub async fn connect(db_url: &str, max_connections: u32) -> anyhow::Result<SqlitePool> {
 let options = SqliteConnectOptions::from_str(db_url)?
 .create_if_missing(true)
 .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
 let pool = SqlitePoolOptions::new()
 .max_connections(max_connections)
 .connect_with(options)
 .await?;
 migrate(&pool).await?;
 Ok(pool)
}

/// Column names of `table` in definition order (via `pragma_table_info`).
async fn table_columns(pool: &SqlitePool, table: &str) -> anyhow::Result<Vec<String>> {
 let rows: Vec<(String,)> =
 sqlx::query_as(&format!("SELECT name FROM pragma_table_info('{table}')"))
 .fetch_all(pool)
 .await?;
 Ok(rows.into_iter().map(|(n,)| n).collect())
}

async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
 sqlx::query(&format!(
 r#"CREATE TABLE IF NOT EXISTS {} (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL,
 announce_url TEXT NOT NULL,
 info_hash TEXT NOT NULL,
 torrent_size INTEGER NOT NULL,
 config_json TEXT NOT NULL,
 status TEXT NOT NULL DEFAULT '{}',
 working_client TEXT,
 created_at TEXT NOT NULL DEFAULT (datetime('now'))
 )"#,
 schema::AUDITS,
 vocab::STATUS_IDLE,
 ))
 .execute(pool)
 .await?;

 // Add columns added after the initial schema via idempotent ALTER TABLE.
 // Each ALTER is a no-op if the column already exists (the error is
 // silently swallowed). Derived from `AUDITS_MIGRATION_COLUMNS` so adding a
 // new migration column is a one-line schema.rs edit - no change needed here.
 for (col, ddl) in schema::AUDITS_MIGRATION_COLUMNS {
 let _ = sqlx::query(&format!(
 "ALTER TABLE {} ADD COLUMN {} {}",
 schema::AUDITS, col, ddl
 ))
 .execute(pool)
 .await;
 }

 // Reconcile schema drift from older builds: a prior schema declared
 // `audits.source TEXT NOT NULL` (no default), which the current code
 // never inserts. `CREATE TABLE IF NOT EXISTS` cannot remove a stale
 // column, so without this every insert fails with
 // "NOT NULL constraint failed: audits.source".
 if table_columns(pool, schema::AUDITS).await?.iter().any(|c| c == "source") {
 sqlx::query(&format!("ALTER TABLE {} DROP COLUMN source", schema::AUDITS))
 .execute(pool)
 .await?;
 }

 sqlx::query(&format!(
 r#"CREATE TABLE IF NOT EXISTS {} (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 audit_id INTEGER NOT NULL REFERENCES {}({}),
 seq INTEGER NOT NULL,
 timestamp TEXT NOT NULL,
 phase TEXT NOT NULL,
 client TEXT NOT NULL,
 event TEXT NOT NULL,
 uploaded INTEGER NOT NULL,
 downloaded INTEGER NOT NULL,
 left INTEGER NOT NULL,
 success INTEGER NOT NULL,
 failure_reason TEXT,
 interval INTEGER NOT NULL,
 seeders INTEGER NOT NULL,
 leechers INTEGER NOT NULL,
 peer_count INTEGER NOT NULL,
 latency_ms INTEGER NOT NULL
 )"#,
 schema::EVENTS,
 schema::AUDITS,
 schema::ID,
 ))
 .execute(pool)
 .await?;

 sqlx::query(&format!(
 "CREATE INDEX IF NOT EXISTS {} ON {}({}, {})",
 schema::IDX_EVENTS_AUDIT,
 schema::EVENTS,
 schema::AUDIT_ID,
 schema::SEQ
 ))
 .execute(pool)
 .await?;

 // Add columns added after the initial schema via idempotent ALTER TABLE.
 // Derived from `EVENTS_MIGRATION_COLUMNS` - see the comment on
 // `AUDITS_MIGRATION_COLUMNS` above. The CREATE TABLE above lists only the
 // base columns (mirroring `EVENTS_BASE_COLUMNS` and the `audits` table);
 // the migration columns are added here so a single source of truth defines
 // both their names and DDL suffixes, and a fresh DB ends up identical to
 // an old DB that was migrated forward.
 for (col, ddl) in schema::EVENTS_MIGRATION_COLUMNS {
 let _ = sqlx::query(&format!(
 "ALTER TABLE {} ADD COLUMN {} {}",
 schema::EVENTS, col, ddl
 ))
 .execute(pool)
 .await;
 }

 // Running audits stay "running" across restarts so the boot loop in
 // main() can auto-restart them. The in-memory engine tasks are gone, but
 // the persisted peer state (saved every stat tick) is enough to resume.

 // global_goals + goal_tasks tables
 sqlx::query(&format!(
 r#"CREATE TABLE IF NOT EXISTS {} (
 {} INTEGER PRIMARY KEY AUTOINCREMENT,
 {} TEXT NOT NULL,
 {} INTEGER NOT NULL DEFAULT 1,
 {} TEXT NOT NULL DEFAULT '{}',
 {} INTEGER NOT NULL DEFAULT 0,
 {} INTEGER NOT NULL DEFAULT 0,
 {} INTEGER NOT NULL DEFAULT 0,
 {} TEXT NOT NULL DEFAULT '{}',
 {} INTEGER NOT NULL DEFAULT 0,
 {} TEXT NOT NULL DEFAULT (datetime('now'))
 )"#,
 schema::GLOBAL_GOALS,
 schema::ID, schema::NAME, schema::ENABLED, schema::DIRECTION,
 crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE,
 schema::UPLOAD_TARGET, schema::DOWNLOAD_TARGET, schema::TARGET_SECS,
 schema::REACHED_ACTION, crate::data::vocab::GOAL_REACHED_STOP_WIRE,
 schema::REACHED_BPS, schema::CREATED_AT,
 ))
 .execute(pool)
 .await?;

 sqlx::query(&format!(
 r#"CREATE TABLE IF NOT EXISTS {} (
 {} INTEGER NOT NULL,
 {} INTEGER NOT NULL,
 PRIMARY KEY ({}, {})
 )"#,
 schema::GOAL_TASKS,
 schema::GOAL_ID, schema::TASK_ID,
 schema::GOAL_ID, schema::TASK_ID,
 ))
 .execute(pool)
 .await?;

 // Migration columns for the new tables (currently empty - no ALTER TABLE
 // needed since every column is in the base CREATE TABLE). The loop runs
 // zero times but references the consts so the partition test is satisfied.
 for (col, ddl) in schema::GLOBAL_GOALS_MIGRATION_COLUMNS {
 let _ = sqlx::query(&format!(
 "ALTER TABLE {} ADD COLUMN {} {}", schema::GLOBAL_GOALS, col, ddl
 )).execute(pool).await;
 }
 for (col, ddl) in schema::GOAL_TASKS_MIGRATION_COLUMNS {
 let _ = sqlx::query(&format!(
 "ALTER TABLE {} ADD COLUMN {} {}", schema::GOAL_TASKS, col, ddl
 )).execute(pool).await;
 }

 Ok(())
}

pub struct AuditRow {
 pub id: i64,
 pub name: String,
 pub announce_url: String,
 pub info_hash: String,
 pub torrent_size: i64,
 pub config_json: String,
 pub status: String,
 pub working_client: Option<String>,
 pub created_at: String,
}

/// Persisted peer state - the last reported counters and lifecycle flags.
/// Survives stop/start so progression isn't lost.
#[derive(Debug, Clone, Default)]
pub struct PeerStateRow {
 pub uploaded: u64,
 pub downloaded: u64,
 pub left: u64,
 pub lifecycle_phase: Option<String>,
 pub completed_sent: bool,
 pub elapsed_secs: u64,
 pub peer_id: Option<String>,
 pub key: Option<String>,
}

pub async fn insert_audit(
 pool: &SqlitePool,
 name: &str,
 announce_url: &str,
 info_hash: &str,
 torrent_size: u64,
 config_json: &str,
) -> anyhow::Result<i64> {
 let insert_cols = &schema::AUDITS_COLUMNS[1..6];
 let result = sqlx::query(&format!(
 "INSERT INTO {} ({}) VALUES ({})",
 schema::AUDITS,
 insert_cols.join(", "),
 placeholders(insert_cols.len())
 ))
 .bind(name)
 .bind(announce_url)
 .bind(info_hash)
 .bind(torrent_size as i64)
 .bind(config_json)
 .execute(pool)
 .await?;
 Ok(result.last_insert_rowid())
}

pub async fn update_status(pool: &SqlitePool, id: i64, status: &str) -> anyhow::Result<()> {
 sqlx::query(&format!(
 "UPDATE {} SET {} = ? WHERE {} = ?",
 schema::AUDITS,
 schema::STATUS,
 schema::ID
 ))
 .bind(status)
 .bind(id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Replace only the `config_json` column for an audit - used by the edit-task
/// flow to update config without touching the immutable torrent identity
/// (name, announce_url, info_hash, torrent_size).
pub async fn update_audit_config(
 pool: &SqlitePool,
 id: i64,
 config_json: &str,
) -> anyhow::Result<()> {
 sqlx::query(&format!(
 "UPDATE {} SET {} = ? WHERE {} = ?",
 schema::AUDITS,
 schema::CONFIG_JSON,
 schema::ID
 ))
 .bind(config_json)
 .bind(id)
 .execute(pool)
 .await?;
 Ok(())
}

pub async fn delete_audit(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
 sqlx::query(&format!(
 "DELETE FROM {} WHERE {} = ?",
 schema::EVENTS,
 schema::AUDIT_ID
 ))
 .bind(id)
 .execute(pool)
 .await?;
 sqlx::query(&format!(
 "DELETE FROM {} WHERE {} = ?",
 schema::AUDITS,
 schema::ID
 ))
 .bind(id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Clear all events for an audit - test-only helper.
#[cfg(test)]
pub async fn clear_events(pool: &SqlitePool, audit_id: i64) -> anyhow::Result<()> {
 sqlx::query(&format!(
 "DELETE FROM {} WHERE {} = ?",
 schema::EVENTS,
 schema::AUDIT_ID
 ))
 .bind(audit_id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Reset an audit for a config change: wipe the event log, zero the persisted
/// peer state (counters, lifecycle, peer_id, key), and clear the working
/// client. After this, the next `start_engine` generates a fresh peer identity
/// and probes from scratch - equivalent to delete + recreate, but keeping the
/// row id and torrent identity. Used by the edit handler when config changes.
pub async fn reset_audit(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
 sqlx::query(&format!(
 "DELETE FROM {} WHERE {} = ?",
 schema::EVENTS,
 schema::AUDIT_ID
 ))
 .bind(id)
 .execute(pool)
 .await?;
 sqlx::query(&format!(
 "UPDATE {} SET {} = 0, {} = 0, {} = 0, {} = NULL, {} = 0, {} = 0, {} = NULL, {} = NULL, {} = NULL WHERE {} = ?",
 schema::AUDITS,
 schema::LAST_UPLOADED,
 schema::LAST_DOWNLOADED,
 schema::LAST_LEFT,
 schema::LIFECYCLE_PHASE,
 schema::COMPLETED_SENT,
 schema::ELAPSED_SECS,
 schema::PEER_ID,
 schema::KEY,
 schema::WORKING_CLIENT,
 schema::ID
 ))
 .bind(id)
 .execute(pool)
 .await?;
 Ok(())
}

pub async fn set_working_client(pool: &SqlitePool, id: i64, client: &str) -> anyhow::Result<()> {
 sqlx::query(&format!(
 "UPDATE {} SET {} = ? WHERE {} = ?",
 schema::AUDITS,
 schema::WORKING_CLIENT,
 schema::ID
 ))
 .bind(client)
 .bind(id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Persisted peer state - passed to `save_peer_state`.
pub struct SavePeerState<'a> {
 pub uploaded: u64,
 pub downloaded: u64,
 pub left: u64,
 pub lifecycle_phase: &'a str,
 pub completed_sent: bool,
 pub elapsed_secs: u64,
 pub peer_id: &'a str,
 pub key: &'a str,
}

/// Persist the last reported peer state so it survives stop/start.
pub async fn save_peer_state(
 pool: &SqlitePool,
 id: i64,
 s: SavePeerState<'_>,
) -> anyhow::Result<()> {
 let set_clause = schema::AUDITS_COLUMNS[9..]
 .iter()
 .map(|c| format!("{c} = ?"))
 .collect::<Vec<_>>()
 .join(", ");
 sqlx::query(&format!(
 "UPDATE {} SET {} WHERE {} = ?",
 schema::AUDITS,
 set_clause,
 schema::ID
 ))
 .bind(s.uploaded as i64)
 .bind(s.downloaded as i64)
 .bind(s.left as i64)
 .bind(s.lifecycle_phase)
 .bind(s.completed_sent)
 .bind(s.elapsed_secs as i64)
 .bind(s.peer_id)
 .bind(s.key)
 .bind(id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Get the highest seq number for an audit (to continue numbering on restart).
pub async fn get_max_seq(pool: &SqlitePool, audit_id: i64) -> anyhow::Result<u64> {
 let row: Option<(i64,)> = sqlx::query_as(&format!(
 "SELECT MAX({}) FROM {} WHERE {} = ?",
 schema::SEQ,
 schema::EVENTS,
 schema::AUDIT_ID
 ))
 .bind(audit_id)
 .fetch_optional(pool)
 .await?;
 Ok(row.and_then(|(max,)| max.try_into().ok()).unwrap_or(0))
}

/// Read the last persisted peer state (for resuming after stop/start).
pub async fn get_peer_state(pool: &SqlitePool, id: i64) -> anyhow::Result<PeerStateRow> {
 let cols = schema::AUDITS_COLUMNS[9..].join(", ");
 let row = sqlx::query_as::<_, PeerStateRowSql>(&format!(
 "SELECT {} FROM {} WHERE {} = ?",
 cols,
 schema::AUDITS,
 schema::ID
 ))
 .bind(id)
 .fetch_one(pool)
 .await?;
 Ok(row.into())
}

pub async fn list_audits(pool: &SqlitePool) -> anyhow::Result<Vec<AuditRow>> {
 let cols = schema::AUDITS_COLUMNS[..9].join(", ");
 let rows = sqlx::query_as::<_, AuditRowSql>(&format!(
 "SELECT {} FROM {} ORDER BY {} DESC",
 cols,
 schema::AUDITS,
 schema::ID
 ))
 .fetch_all(pool)
 .await?;
 Ok(rows.into_iter().map(|r| r.into()).collect())
}

pub async fn get_audit(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<AuditRow>> {
 let cols = schema::AUDITS_COLUMNS[..9].join(", ");
 let row = sqlx::query_as::<_, AuditRowSql>(&format!(
 "SELECT {} FROM {} WHERE {} = ?",
 cols,
 schema::AUDITS,
 schema::ID
 ))
 .bind(id)
 .fetch_optional(pool)
 .await?;
 Ok(row.map(|r| r.into()))
}

pub async fn list_events(pool: &SqlitePool, audit_id: i64, limit: u32) -> anyhow::Result<Vec<AuditEvent>> {
 let cols = schema::EVENTS_COLUMNS[1..].join(", ");
 let rows = sqlx::query_as::<_, EventRow>(&format!(
 "SELECT {} FROM {} WHERE {} = ? ORDER BY {} DESC LIMIT ?",
 cols,
 schema::EVENTS,
 schema::AUDIT_ID,
 schema::SEQ
 ))
 .bind(audit_id)
 .bind(limit)
 .fetch_all(pool)
 .await?;
 Ok(rows.into_iter().map(|r| r.into_event()).collect())
}

pub async fn insert_event(pool: &SqlitePool, ev: &AuditEvent) -> anyhow::Result<()> {
 let cols = schema::EVENTS_COLUMNS[1..].join(", ");
 sqlx::query(&format!(
 "INSERT INTO {} ({}) VALUES ({})",
 schema::EVENTS,
 cols,
 placeholders(schema::EVENTS_COLUMNS.len() - 1)
 ))
 .bind(ev.audit_id)
 .bind(ev.seq as i64)
 .bind(ev.timestamp.to_rfc3339())
 .bind(ev.phase)
 .bind(&ev.client)
 .bind(ev.event)
 .bind(ev.uploaded as i64)
 .bind(ev.downloaded as i64)
 .bind(ev.left as i64)
 .bind(ev.success as i32)
 .bind(&ev.failure_reason)
 .bind(ev.interval as i64)
 .bind(ev.seeders)
 .bind(ev.leechers)
 .bind(ev.peer_count as i64)
 .bind(ev.latency_ms as i64)
 .bind(ev.fair_share_bps as i64)
 .bind(ev.dynamic_target_bps as i64)
 .bind(ev.next_announce_in_secs as i64)
 .bind(ev.elapsed_secs as i64)
 .execute(pool)
 .await?;
 Ok(())
}

// SQL row types

#[derive(Debug, sqlx::FromRow)]
struct AuditRowSql {
 id: i64,
 name: String,
 announce_url: String,
 info_hash: String,
 torrent_size: i64,
 config_json: String,
 status: String,
 working_client: Option<String>,
 created_at: String,
}

impl From<AuditRowSql> for AuditRow {
 fn from(r: AuditRowSql) -> Self {
 AuditRow {
 id: r.id,
 name: r.name,
 announce_url: r.announce_url,
 info_hash: r.info_hash,
 torrent_size: r.torrent_size,
 config_json: r.config_json,
 status: r.status,
 working_client: r.working_client,
 created_at: r.created_at,
 }
 }
}

#[derive(Debug, sqlx::FromRow)]
struct PeerStateRowSql {
 last_uploaded: i64,
 last_downloaded: i64,
 last_left: i64,
 lifecycle_phase: Option<String>,
 completed_sent: i32,
 elapsed_secs: i64,
 peer_id: Option<String>,
 key: Option<String>,
}

impl From<PeerStateRowSql> for PeerStateRow {
 fn from(r: PeerStateRowSql) -> Self {
 PeerStateRow {
 uploaded: r.last_uploaded as u64,
 downloaded: r.last_downloaded as u64,
 left: r.last_left as u64,
 lifecycle_phase: r.lifecycle_phase,
 completed_sent: r.completed_sent != 0,
 elapsed_secs: r.elapsed_secs as u64,
 peer_id: r.peer_id,
 key: r.key,
 }
 }
}

#[derive(Debug, sqlx::FromRow)]
struct EventRow {
 audit_id: i64,
 seq: i64,
 timestamp: String,
 phase: String,
 client: String,
 event: String,
 uploaded: i64,
 downloaded: i64,
 left: i64,
 success: i32,
 failure_reason: Option<String>,
 interval: i64,
 seeders: i64,
 leechers: i64,
 peer_count: i64,
 latency_ms: i64,
 fair_share_bps: i64,
 dynamic_target_bps: i64,
 next_announce_in_secs: i64,
 elapsed_secs: i64,
}

impl EventRow {
 fn into_event(self) -> AuditEvent {
 let phase: &'static str = if self.phase == vocab::PHASE_PROBE {
 vocab::PHASE_PROBE
 } else {
 vocab::PHASE_ATTACK
 };
 let event: &'static str = match self.event.as_str() {
 vocab::EVENT_PROBE => vocab::EVENT_PROBE,
 vocab::EVENT_STARTED => vocab::EVENT_STARTED,
 vocab::EVENT_STOPPED => vocab::EVENT_STOPPED,
 vocab::EVENT_COMPLETED => vocab::EVENT_COMPLETED,
 vocab::EVENT_TICK => vocab::EVENT_TICK,
 vocab::EVENT_REGULAR => vocab::EVENT_REGULAR,
 other => {
 // Unknown value in persisted rows - flag loudly instead of
 // silently coercing to "regular" (the previous default masked
 // the missing "completed" arm). Keep the row usable by falling
 // back, but log so drift is visible.
 tracing::warn!(event = other, "unknown events.event value on readback");
 vocab::EVENT_REGULAR
 }
 };
 AuditEvent {
 audit_id: self.audit_id,
 seq: self.seq as u64,
 timestamp: chrono::DateTime::parse_from_rfc3339(&self.timestamp)
 .unwrap_or_else(|_| chrono::Utc::now().into())
 .with_timezone(&chrono::Utc),
 phase,
 client: self.client,
 event,
 uploaded: self.uploaded as u64,
 downloaded: self.downloaded as u64,
 left: self.left as u64,
 success: self.success != 0,
 failure_reason: self.failure_reason,
 interval: self.interval as u32,
 seeders: self.seeders,
 leechers: self.leechers,
 peer_count: self.peer_count as usize,
 latency_ms: self.latency_ms as u64,
 working_client: None,
 fair_share_bps: self.fair_share_bps as u64,
 dynamic_target_bps: self.dynamic_target_bps as u64,
 next_announce_in_secs: self.next_announce_in_secs as u64,
 elapsed_secs: self.elapsed_secs as u64,
 }
 }
}

// global_goals + goal_tasks CRUD

/// A global goal row - the wire shape for the API and SSE. Serializes directly
/// to JSON so the frontend receives flat fields it can destructure.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GoalRow {
 pub id: i64,
 pub name: String,
 pub enabled: bool,
 pub direction: String,
 pub upload_target: u64,
 pub download_target: u64,
 pub target_secs: u64,
 pub reached_action: String,
 pub reached_bps: u64,
 pub created_at: String,
}

/// Input shape for insert/update - the mutable fields without id/created_at.
/// Kept separate from `GoalRow` (which includes id + created_at) so the API
/// handlers don't pass auto-generated fields.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GoalRowInput {
 pub name: String,
 pub enabled: bool,
 pub direction: String,
 pub upload_target: u64,
 pub download_target: u64,
 pub target_secs: u64,
 pub reached_action: String,
 pub reached_bps: u64,
}

#[derive(Debug, sqlx::FromRow)]
struct GoalRowSql {
 id: i64, name: String, enabled: i32, direction: String,
 upload_target: i64, download_target: i64, target_secs: i64,
 reached_action: String, reached_bps: i64, created_at: String,
}

impl From<GoalRowSql> for GoalRow {
 fn from(r: GoalRowSql) -> Self {
 GoalRow {
 id: r.id, name: r.name, enabled: r.enabled != 0, direction: r.direction,
 upload_target: r.upload_target as u64, download_target: r.download_target as u64,
 target_secs: r.target_secs as u64, reached_action: r.reached_action,
 reached_bps: r.reached_bps as u64, created_at: r.created_at,
 }
 }
}

pub async fn insert_goal(pool: &SqlitePool, row: &GoalRowInput) -> anyhow::Result<i64> {
 let cols = &schema::GLOBAL_GOALS_COLUMNS[1..9];
 let result = sqlx::query(&format!(
 "INSERT INTO {} ({}) VALUES ({})", schema::GLOBAL_GOALS,
 cols.join(", "), placeholders(cols.len())
 ))
 .bind(&row.name).bind(row.enabled as i32).bind(&row.direction)
 .bind(row.upload_target as i64).bind(row.download_target as i64).bind(row.target_secs as i64)
 .bind(&row.reached_action).bind(row.reached_bps as i64)
 .execute(pool).await?;
 Ok(result.last_insert_rowid())
}

pub async fn list_goals(pool: &SqlitePool) -> anyhow::Result<Vec<GoalRow>> {
 let rows = sqlx::query_as::<_, GoalRowSql>(&format!(
 "SELECT {} FROM {} ORDER BY {} DESC",
 schema::GLOBAL_GOALS_COLUMNS.join(", "), schema::GLOBAL_GOALS, schema::ID
 )).fetch_all(pool).await?;
 Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_goal(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<GoalRow>> {
 let row = sqlx::query_as::<_, GoalRowSql>(&format!(
 "SELECT {} FROM {} WHERE {} = ?",
 schema::GLOBAL_GOALS_COLUMNS.join(", "), schema::GLOBAL_GOALS, schema::ID
 )).bind(id).fetch_optional(pool).await?;
 Ok(row.map(Into::into))
}

pub async fn update_goal(pool: &SqlitePool, id: i64, row: &GoalRowInput) -> anyhow::Result<()> {
 sqlx::query(&format!(
 "UPDATE {} SET {} = ?, {} = ?, {} = ?, {} = ?, {} = ?, {} = ?, {} = ?, {} = ? WHERE {} = ?",
 schema::GLOBAL_GOALS, schema::NAME, schema::ENABLED, schema::DIRECTION,
 schema::UPLOAD_TARGET, schema::DOWNLOAD_TARGET, schema::TARGET_SECS,
 schema::REACHED_ACTION, schema::REACHED_BPS, schema::ID,
 ))
 .bind(&row.name).bind(row.enabled as i32).bind(&row.direction)
 .bind(row.upload_target as i64).bind(row.download_target as i64).bind(row.target_secs as i64)
 .bind(&row.reached_action).bind(row.reached_bps as i64).bind(id)
 .execute(pool).await?;
 Ok(())
}

/// Delete a goal + its junction rows (manual cascade - matches `delete_audit`).
pub async fn delete_goal(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
 sqlx::query(&format!("DELETE FROM {} WHERE {} = ?", schema::GOAL_TASKS, schema::GOAL_ID))
 .bind(id).execute(pool).await?;
 sqlx::query(&format!("DELETE FROM {} WHERE {} = ?", schema::GLOBAL_GOALS, schema::ID))
 .bind(id).execute(pool).await?;
 Ok(())
}

/// Replace the set of tasks associated with a goal.
pub async fn set_goal_tasks(pool: &SqlitePool, goal_id: i64, task_ids: &[i64]) -> anyhow::Result<()> {
 sqlx::query(&format!("DELETE FROM {} WHERE {} = ?", schema::GOAL_TASKS, schema::GOAL_ID))
 .bind(goal_id).execute(pool).await?;
 for &tid in task_ids {
 sqlx::query(&format!("INSERT INTO {} ({}, {}) VALUES (?, ?)",
 schema::GOAL_TASKS, schema::GOAL_ID, schema::TASK_ID))
 .bind(goal_id).bind(tid).execute(pool).await?;
 }
 Ok(())
}

pub async fn get_goal_task_ids(pool: &SqlitePool, goal_id: i64) -> anyhow::Result<Vec<i64>> {
 let rows: Vec<(i64,)> = sqlx::query_as(&format!(
 "SELECT {} FROM {} WHERE {} = ? ORDER BY {}", schema::GOAL_TASKS_COLUMNS[1],
 schema::GOAL_TASKS, schema::GOAL_ID, schema::TASK_ID
 )).bind(goal_id).fetch_all(pool).await?;
 Ok(rows.into_iter().map(|(t,)| t).collect())
}

/// Every goal_id that includes `task_id` - used by the DB writer to find which
/// goals to recompute when a task's counters change.
pub async fn goal_ids_for_task(pool: &SqlitePool, task_id: i64) -> anyhow::Result<Vec<i64>> {
 let rows: Vec<(i64,)> = sqlx::query_as(&format!(
 "SELECT {} FROM {} WHERE {} = ?", schema::GOAL_ID, schema::GOAL_TASKS, schema::TASK_ID
 )).bind(task_id).fetch_all(pool).await?;
 Ok(rows.into_iter().map(|(g,)| g).collect())
}

/// All task IDs that are "occupied" by goals other than `exclude_goal_id`.
/// A task is occupied if it appears in another goal's task_ids.
pub async fn occupied_tasks(pool: &SqlitePool, exclude_goal_id: i64) -> anyhow::Result<Vec<i64>> {
 let rows: Vec<(i64,)> = sqlx::query_as(&format!(
 "SELECT DISTINCT {} FROM {} WHERE {} != ?",
 schema::GOAL_TASKS_COLUMNS[1], schema::GOAL_TASKS, schema::GOAL_ID
 ))
 .bind(exclude_goal_id)
 .fetch_all(pool).await?;
 Ok(rows.into_iter().map(|(t,)| t).collect())
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::engine::AuditEvent;

 async fn fresh_pool() -> SqlitePool {
 connect("sqlite::memory:", 4)
 .await
 .expect("in-memory pool should connect")
 }

 async fn insert_sample(pool: &SqlitePool, name: &str) -> i64 {
 insert_audit(
 pool,
 name,
 "http://tracker.example.com/announce",
 crate::data::fixtures::SAMPLE_INFO_HASH,
 1_073_741_824,
 r#"{"mode":"fixed"}"#,
 )
 .await
 .expect("insert_audit should succeed")
 }

 fn sample_event(audit_id: i64, seq: u64) -> AuditEvent {
 AuditEvent {
 audit_id,
 seq,
 timestamp: chrono::Utc::now(),
 phase: "attack",
 client: "qBittorrent 4.6.0".to_string(),
 event: "regular",
 uploaded: 1000 * seq,
 downloaded: 500,
 left: 200,
 success: true,
 failure_reason: None,
 interval: 1800,
 seeders: 3,
 leechers: 5,
 peer_count: 8,
 latency_ms: 42,
 working_client: None,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 }
 }

 fn sample_event_row() -> EventRow {
 EventRow {
 audit_id: 1,
 seq: 1,
 timestamp: chrono::Utc::now().to_rfc3339(),
 phase: "attack".to_string(),
 client: "qBittorrent".to_string(),
 event: "regular".to_string(),
 uploaded: 1000,
 downloaded: 500,
 left: 200,
 success: 1,
 failure_reason: None,
 interval: 1800,
 seeders: 3,
 leechers: 5,
 peer_count: 8,
 latency_ms: 42,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 }
 }

 #[tokio::test]
 async fn connect_creates_tables() {
 let pool = fresh_pool().await;
 let tables: Vec<(String,)> = sqlx::query_as(
 "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('audits','events') ORDER BY name",
 )
 .fetch_all(&pool)
 .await
 .expect("query sqlite_master");
 assert_eq!(tables.len(), 2);
 assert_eq!(tables[0].0, "audits");
 assert_eq!(tables[1].0, "events");
 }

 #[tokio::test]
 async fn migrate_preserves_running_status_for_auto_restart() {
 // Running audits must stay "running" across restarts so the boot loop
 // in main() can auto-restart them. Previously migrate() reset them to
 // "stopped", killing any chance of resumption.
 let url = "file:memdb_running_preserve?mode=memory&cache=shared";
 let pool1 = connect(url, 4).await.expect("first connect");
 let running_id = insert_sample(&pool1, "was-running").await;
 update_status(&pool1, running_id, vocab::STATUS_RUNNING).await.unwrap();
 let stopped_id = insert_sample(&pool1, "was-stopped").await;
 update_status(&pool1, stopped_id, vocab::STATUS_STOPPED).await.unwrap();

 let pool2 = connect(url, 4).await.expect("second connect migrates");
 // Running must stay running - auto-restart depends on it.
 assert_eq!(
 get_audit(&pool2, running_id).await.unwrap().unwrap().status,
 vocab::STATUS_RUNNING,
 "running status must survive restart for auto-restart",
 );
 // Stopped stays stopped.
 assert_eq!(
 get_audit(&pool2, stopped_id).await.unwrap().unwrap().status,
 vocab::STATUS_STOPPED,
 );
 }

 #[tokio::test]
 async fn insert_audit_returns_id() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "first").await;
 assert!(id > 0);
 }

 #[tokio::test]
 async fn insert_audit_multiple_increments_id() {
 let pool = fresh_pool().await;
 let a = insert_sample(&pool, "a").await;
 let b = insert_sample(&pool, "b").await;
 let c = insert_sample(&pool, "c").await;
 assert_eq!(a, 1);
 assert_eq!(b, 2);
 assert_eq!(c, 3);
 }

 #[tokio::test]
 async fn get_audit_returns_row() {
 let pool = fresh_pool().await;
 let id = insert_audit(
 &pool, "My Audit", "http://t.com/a", crate::data::fixtures::SAMPLE_INFO_HASH,
 1_073_741_824, r#"{"key":"value"}"#,
 ).await.unwrap();
 let row = get_audit(&pool, id).await.unwrap().expect("must exist");
 assert_eq!(row.id, id);
 assert_eq!(row.name, "My Audit");
 assert_eq!(row.status, "idle");
 }

 #[tokio::test]
 async fn get_audit_nonexistent_returns_none() {
 let pool = fresh_pool().await;
 assert!(get_audit(&pool, 999).await.unwrap().is_none());
 assert!(get_audit(&pool, -1).await.unwrap().is_none());
 }

 #[tokio::test]
 async fn list_audits_returns_all() {
 let pool = fresh_pool().await;
 insert_sample(&pool, "a").await;
 insert_sample(&pool, "b").await;
 insert_sample(&pool, "c").await;
 assert_eq!(list_audits(&pool).await.expect("list").len(), 3);
 }

 #[tokio::test]
 async fn list_audits_empty_returns_empty_vec() {
 let pool = fresh_pool().await;
 assert!(list_audits(&pool).await.expect("list").is_empty());
 }

 #[tokio::test]
 async fn update_status_changes_status() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "s").await;
 assert_eq!(get_audit(&pool, id).await.unwrap().unwrap().status, "idle");
 update_status(&pool, id, "running").await.unwrap();
 assert_eq!(get_audit(&pool, id).await.unwrap().unwrap().status, "running");
 }

 #[tokio::test]
 async fn delete_status_nonexistent_id_is_noop() {
 let pool = fresh_pool().await;
 update_status(&pool, 9999, "running").await.expect("noop");
 }

 #[tokio::test]
 async fn update_audit_config_replaces_config() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "cfg").await;
 // Verify the initial config from insert_sample
 let before = get_audit(&pool, id).await.unwrap().unwrap();
 assert_eq!(before.config_json, r#"{"mode":"fixed"}"#);
 // Replace with a different config
 update_audit_config(&pool, id, r#"{"mode":"upload_only"}"#)
 .await
 .expect("update");
 let after = get_audit(&pool, id).await.unwrap().unwrap();
 assert_eq!(after.config_json, r#"{"mode":"upload_only"}"#);
 // The torrent identity must be untouched
 assert_eq!(after.name, "cfg");
 assert_eq!(after.announce_url, "http://tracker.example.com/announce");
 assert_eq!(after.info_hash, crate::data::fixtures::SAMPLE_INFO_HASH);
 }

 #[tokio::test]
 async fn update_audit_config_nonexistent_is_noop() {
 let pool = fresh_pool().await;
 update_audit_config(&pool, 9999, r#"{"mode":"fixed"}"#)
 .await
 .expect("no-op for nonexistent id");
 }

 #[tokio::test]
 async fn delete_audit_removes_row() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "del").await;
 delete_audit(&pool, id).await.expect("delete");
 assert!(get_audit(&pool, id).await.unwrap().is_none());
 }

 #[tokio::test]
 async fn delete_audit_cascades_events() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "cascade").await;
 insert_event(&pool, &sample_event(id, 1)).await.unwrap();
 insert_event(&pool, &sample_event(id, 2)).await.unwrap();
 assert_eq!(list_events(&pool, id, 10).await.unwrap().len(), 2);
 delete_audit(&pool, id).await.unwrap();
 assert!(get_audit(&pool, id).await.unwrap().is_none());
 assert!(list_events(&pool, id, 10).await.unwrap().is_empty());
 }

 #[tokio::test]
 async fn insert_and_list_events() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "ev").await;
 for s in 1..=3u64 {
 insert_event(&pool, &sample_event(id, s)).await.unwrap();
 }
 let rows = list_events(&pool, id, 10).await.expect("list");
 assert_eq!(rows.len(), 3);
 assert_eq!(rows[0].seq, 3, "DESC by seq");
 assert_eq!(rows[2].seq, 1);
 }

 #[tokio::test]
 async fn list_events_respects_limit() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "lim").await;
 for s in 1..=5u64 {
 insert_event(&pool, &sample_event(id, s)).await.unwrap();
 }
 assert_eq!(list_events(&pool, id, 3).await.unwrap().len(), 3);
 assert!(list_events(&pool, id, 0).await.unwrap().is_empty());
 }

 #[tokio::test]
 async fn list_events_empty_returns_empty() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "none").await;
 assert!(list_events(&pool, id, 10).await.unwrap().is_empty());
 }

 #[tokio::test]
 async fn set_working_client_sets_value() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "wc").await;
 assert!(get_audit(&pool, id).await.unwrap().unwrap().working_client.is_none());
 set_working_client(&pool, id, "Transmission 3.0").await.unwrap();
 assert_eq!(
 get_audit(&pool, id).await.unwrap().unwrap().working_client.as_deref(),
 Some("Transmission 3.0"),
 );
 }

 #[tokio::test]
 async fn clear_events_removes_all() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "clr").await;
 for s in 1..=3u64 {
 insert_event(&pool, &sample_event(id, s)).await.unwrap();
 }
 clear_events(&pool, id).await.expect("clear");
 assert_eq!(list_events(&pool, id, 10).await.unwrap().len(), 0);
 assert!(get_audit(&pool, id).await.unwrap().is_some());
 }

 #[test]
 fn into_event_unknown_phase_defaults_to_attack() {
 let mut r = sample_event_row();
 r.phase = "weird".to_string();
 assert_eq!(r.into_event().phase, "attack");
 }

 #[test]
 fn into_event_unknown_event_defaults_to_regular() {
 let mut r = sample_event_row();
 r.event = "weird".to_string();
 assert_eq!(r.into_event().event, "regular");
 }

 #[test]
 fn into_event_bad_timestamp_falls_back_to_now() {
 let before = chrono::Utc::now();
 let mut r = sample_event_row();
 r.timestamp = "not-a-date".to_string();
 let ev = r.into_event();
 let after = chrono::Utc::now();
 assert!(ev.timestamp >= before && ev.timestamp <= after);
 }

 #[test]
 fn into_event_negative_uploaded_wraps_to_u64_max() {
 let mut r = sample_event_row();
 r.uploaded = -1;
 assert_eq!(r.into_event().uploaded, u64::MAX);
 }

 // Peer state persistence (regression: progression lost on restart)

 #[tokio::test]
 async fn save_and_get_peer_state_roundtrips() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "state").await;
 // Default state should be all zeros
 let initial = get_peer_state(&pool, id).await.unwrap();
 assert_eq!(initial.uploaded, 0);
 assert_eq!(initial.downloaded, 0);
 assert_eq!(initial.left, 0);
 // Save state
 save_peer_state(&pool, id, SavePeerState { uploaded: 1_000_000, downloaded: 500_000, left: 250_000, lifecycle_phase: "leech", completed_sent: false, elapsed_secs: 0, peer_id: "", key: "" })
 .await
 .unwrap();
 // Read back
 let restored = get_peer_state(&pool, id).await.unwrap();
 assert_eq!(restored.uploaded, 1_000_000);
 assert_eq!(restored.downloaded, 500_000);
 assert_eq!(restored.left, 250_000);
 assert_eq!(restored.lifecycle_phase.as_deref(), Some("leech"));
 assert!(!restored.completed_sent);
 // Update to seed state
 save_peer_state(&pool, id, SavePeerState { uploaded: 2_000_000, downloaded: 500_000, left: 0, lifecycle_phase: "seed", completed_sent: true, elapsed_secs: 100, peer_id: "", key: "" })
 .await
 .unwrap();
 let restored2 = get_peer_state(&pool, id).await.unwrap();
 assert_eq!(restored2.uploaded, 2_000_000);
 assert_eq!(restored2.left, 0);
 assert_eq!(restored2.lifecycle_phase.as_deref(), Some("seed"));
 assert!(restored2.completed_sent);
 }

 // Regression: peer_id/key lost on restart → tracker sees a new peer
 //
 // The peer_id and key must persist across stop/start so the tracker
 // credits resumed cumulative counters to the same peer. Without
 // persistence, every restart generates a new random peer_id - the
 // tracker sees a brand-new peer whose baseline is the resumed total
 // (delta = 0), and all un-announced upload is lost from the tracker's
 // perspective.

 #[tokio::test]
 async fn save_and_get_peer_identity_roundtrips() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "identity").await;
 // A fresh audit has no peer_id/key (old rows or first save with empty)
 let initial = get_peer_state(&pool, id).await.unwrap();
 assert!(initial.peer_id.is_none(), "peer_id should be None before first save");
 assert!(initial.key.is_none(), "key should be None before first save");
 // Save with a specific peer_id/key (hex-encoded, as the engine does)
 let peer_id_hex = "2d7142353232302dabcdef0123456789abcdef01"; // "-qB5220-" + random
 let key = "DEADBEEF";
 save_peer_state(&pool, id, SavePeerState {
 uploaded: 1_000_000, downloaded: 500_000, left: 0,
 lifecycle_phase: "seed", completed_sent: true, elapsed_secs: 100,
 peer_id: peer_id_hex, key,
 })
 .await
 .unwrap();
 // Read back - peer_id and key must match exactly
 let restored = get_peer_state(&pool, id).await.unwrap();
 assert_eq!(restored.peer_id.as_deref(), Some(peer_id_hex), "peer_id must round-trip");
 assert_eq!(restored.key.as_deref(), Some(key), "key must round-trip");
 // Counters must also survive (not clobbered by identity columns)
 assert_eq!(restored.uploaded, 1_000_000);
 assert_eq!(restored.left, 0);
 }

 #[tokio::test]
 async fn peer_identity_survives_repeated_saves() {
 // save_peer_state is called every stat tick (5s) - the peer_id/key
 // must not be clobbered or corrupted by repeated writes.
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "repeated").await;
 let peer_id_hex = "2d7142353232302dabcdef0123456789abcdef01";
 let key = "CAFEBABE";
 for uploaded in 0..10 {
 save_peer_state(&pool, id, SavePeerState {
 uploaded, downloaded: 0, left: 0,
 lifecycle_phase: "seed", completed_sent: true, elapsed_secs: uploaded,
 peer_id: peer_id_hex, key,
 })
 .await
 .unwrap();
 }
 let restored = get_peer_state(&pool, id).await.unwrap();
 assert_eq!(restored.peer_id.as_deref(), Some(peer_id_hex), "peer_id must survive 10 repeated saves");
 assert_eq!(restored.key.as_deref(), Some(key), "key must survive 10 repeated saves");
 assert_eq!(restored.uploaded, 9, "last uploaded value must be 9");
 }

 #[tokio::test]
 async fn get_peer_state_nonexistent_id_errors() {
 let pool = fresh_pool().await;
 assert!(get_peer_state(&pool, 9999).await.is_err());
 }

 #[tokio::test]
 async fn peer_state_survives_clear_events() {
 // clear_events must NOT wipe peer state (stored on audits row, not events)
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "survive").await;
 save_peer_state(&pool, id, SavePeerState { uploaded: 999, downloaded: 888, left: 777, lifecycle_phase: "seed", completed_sent: true, elapsed_secs: 0, peer_id: "", key: "" })
 .await
 .unwrap();
 insert_event(&pool, &sample_event(id, 1)).await.unwrap();
 clear_events(&pool, id).await.unwrap();
 let state = get_peer_state(&pool, id).await.unwrap();
 assert_eq!(state.uploaded, 999, "peer state must survive clear_events");
 assert_eq!(state.downloaded, 888);
 assert_eq!(state.left, 777);
 }

 // Regression: progression lost on restart

 #[tokio::test]
 async fn get_max_seq_returns_highest_seq() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "seq").await;
 for s in 1..=5u64 {
 insert_event(&pool, &sample_event(id, s)).await.unwrap();
 }
 assert_eq!(get_max_seq(&pool, id).await.unwrap(), 5);
 }

 #[tokio::test]
 async fn get_max_seq_zero_when_no_events() {
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "empty").await;
 assert_eq!(get_max_seq(&pool, id).await.unwrap(), 0);
 }

 #[tokio::test]
 async fn save_peer_state_on_stop_preserves_uploaded() {
 // Regression: uploaded was 0 on resume because save_peer_state
 // was only called on announces (every 1800s), not on stat events
 // or graceful stop. Now we save on every stat tick + on stop.
 let pool = fresh_pool().await;
 let id = insert_sample(&pool, "upload").await;
 // Simulate what the engine does: accumulate uploaded via ticks,
 // then save on stop
 save_peer_state(&pool, id, SavePeerState { uploaded: 5_000_000, downloaded: 1_000_000, left: 0, lifecycle_phase: "seed", completed_sent: true, elapsed_secs: 0, peer_id: "", key: "" })
 .await
 .unwrap();
 let state = get_peer_state(&pool, id).await.unwrap();
 assert_eq!(state.uploaded, 5_000_000, "uploaded must be saved on stop");
 }

 // Schema drift (regression: NOT NULL constraint failed: audits.source)

 #[tokio::test]
 async fn migrate_drops_orphaned_source_column() {
 // Regression: an older build created `audits` with `source TEXT NOT
 // NULL` (no default). The current code never inserts `source`, so
 // every insert failed with "NOT NULL constraint failed:
 // audits.source". `CREATE TABLE IF NOT EXISTS` can't remove the
 // column, so migrate() must drop it explicitly.
 let url = "file:memdb_drift_source?mode=memory&cache=shared";
 let seed = connect(url, 1).await.expect("seed connect");
 sqlx::query("DROP TABLE audits")
 .execute(&seed)
 .await
 .expect("drop current schema");
 sqlx::query(
 r#"CREATE TABLE audits (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL,
 announce_url TEXT NOT NULL,
 info_hash TEXT NOT NULL,
 torrent_size INTEGER NOT NULL,
 source TEXT NOT NULL,
 config_json TEXT NOT NULL,
 status TEXT NOT NULL DEFAULT 'idle',
 working_client TEXT,
 created_at TEXT NOT NULL DEFAULT (datetime('now'))
 )"#,
 )
 .execute(&seed)
 .await
 .expect("create old-schema audits");
 // `seed` holds the shared in-memory DB alive while we reconnect and
 // re-run migrate(), which must reconcile the drift.
 let pool = connect(url, 2).await.expect("migrate connect");

 let cols = table_columns(&pool, "audits").await.expect("columns");
 assert!(
 !cols.iter().any(|c| c == "source"),
 "orphaned `source` column must be dropped; got {cols:?}",
 );
 let id = insert_audit(
 &pool,
 "drift-test",
 "http://t.com/a",
 crate::data::fixtures::SAMPLE_INFO_HASH,
 1_073_741_824,
 r#"{"mode":"fixed"}"#,
 )
 .await
 .expect("insert_audit must succeed after drift reconciliation");
 assert!(id > 0);
 }

 #[tokio::test]
 async fn migrated_audits_schema_matches_expectations() {
 // Guard against future drift: the expected column set is the single
 // source of truth `schema::AUDITS_COLUMNS`, so a DDL change that isn't
 // reflected there (or a migrate() that leaves an orphaned column) fails
 // here instead of silently desyncing tests from the real schema.
 let pool = fresh_pool().await;
 let mut cols = table_columns(&pool, schema::AUDITS).await.expect("columns");
 cols.sort();
 let mut expected: Vec<String> = schema::AUDITS_COLUMNS
 .iter()
 .map(|&s| s.to_string())
 .collect();
 expected.sort();
 assert_eq!(
 cols, expected,
 "audits schema drifted from codebase expectations",
 );
 }

 #[tokio::test]
 async fn migrated_events_schema_matches_expectations() {
 // Parallel guard for the `events` table: the expected column set is
 // `schema::EVENTS_COLUMNS`. Without this test, an events DDL change
 // that isn't reflected in the const array (or vice versa) would be
 // undetected - the audits-only drift test can't catch it.
 let pool = fresh_pool().await;
 let mut cols = table_columns(&pool, schema::EVENTS).await.expect("columns");
 cols.sort();
 let mut expected: Vec<String> = schema::EVENTS_COLUMNS
 .iter()
 .map(|&s| s.to_string())
 .collect();
 expected.sort();
 assert_eq!(
 cols, expected,
 "events schema drifted from codebase expectations",
 );
 }

 #[test]
 fn completed_event_roundtrips_correctly() {
 // Regression test: the old `into_event` match had `_ => "regular"` which
 // silently coerced "completed" to "regular" on readback (latent bug).
 // The fix added an explicit `vocab::EVENT_COMPLETED` arm. This test
 // fails against the old code and passes against the fix, closing the
 // regression-discipline loop.
 let row = EventRow {
 audit_id: 1,
 seq: 1,
 timestamp: chrono::Utc::now().to_rfc3339(),
 phase: vocab::PHASE_ATTACK.to_string(),
 client: "test".to_string(),
 event: vocab::EVENT_COMPLETED.to_string(),
 uploaded: 0,
 downloaded: 0,
 left: 0,
 success: 1,
 failure_reason: None,
 interval: 0,
 seeders: 0,
 leechers: 0,
 peer_count: 0,
 latency_ms: 0,
 fair_share_bps: 0,
 dynamic_target_bps: 0,
 next_announce_in_secs: 0, elapsed_secs: 0,
 };
 let ev = row.into_event();
 assert_eq!(ev.event, vocab::EVENT_COMPLETED, "completed event must round-trip as completed, not regular");
 }

 // Migration: old DB missing columns added after the initial schema
 //
 // CREATE TABLE IF NOT EXISTS does not add columns to an existing table.
 // When a column is added to the DDL + EVENTS_COLUMNS, migrate() must also
 // issue a corresponding ALTER TABLE so existing DBs (created by older
 // binaries) get the column. Without this, inserts fail at runtime with
 // "table events has no column named X". This test reproduces that scenario
 // by creating a table with the old schema, running migrate(), and
 // verifying the new columns exist with correct defaults.

 // Dynamic schema tests
 //
 // These tests verify that `migrate()` is complete and correct for BOTH
 // tables, now and for future column additions. They are data-driven from
 // the `*_BASE_COLUMNS` / `*_MIGRATION_COLUMNS` / `*_COLUMNS` arrays in
 // schema.rs - adding a new column to those arrays automatically covers it
 // here without touching the tests.

 #[test]
 fn columns_partition_correct() {
 // Verify that *_BASE ∪ *_MIGRATION == *_COLUMNS with no overlap for
 // both tables. This catches a column added to *_COLUMNS but forgotten
 // in either base or migration - which would mean it's never created
 // on fresh DBs (not in CREATE TABLE) nor on old DBs (no ALTER TABLE).
 for (all, base, migration, table) in [
 (schema::AUDITS_COLUMNS, schema::AUDITS_BASE_COLUMNS, schema::AUDITS_MIGRATION_COLUMNS, "audits"),
 (schema::EVENTS_COLUMNS, schema::EVENTS_BASE_COLUMNS, schema::EVENTS_MIGRATION_COLUMNS, "events"),
 (schema::GLOBAL_GOALS_COLUMNS, schema::GLOBAL_GOALS_BASE_COLUMNS, schema::GLOBAL_GOALS_MIGRATION_COLUMNS, "global_goals"),
 (schema::GOAL_TASKS_COLUMNS, schema::GOAL_TASKS_BASE_COLUMNS, schema::GOAL_TASKS_MIGRATION_COLUMNS, "goal_tasks"),
 ] {
 let mut combined: Vec<&str> = base.to_vec();
 combined.extend(migration.iter().map(|(n, _)| *n));
 let mut combined_sorted = combined.clone();
 combined_sorted.sort();
 let mut all_sorted = all.to_vec();
 all_sorted.sort();
 assert_eq!(
 combined_sorted, all_sorted,
 "{table}: base + migration columns must equal *_COLUMNS (a column is in neither group)"
 );
 // No overlap between base and migration
 for b in base {
 assert!(
 !migration.iter().any(|(m, _)| m == b),
 "{table}: column `{b}` is in both base and migration - must be one or the other"
 );
 }
 }
 }

 #[tokio::test]
 async fn migrate_adds_all_migration_columns() {
 // Recreate both tables with ONLY the base columns (simulating an old
 // DB created by the first binary), run migrate(), and verify every
 // column from *_COLUMNS appears. This catches a missing ALTER TABLE
 // for any migration column - now and for future additions.
 let pool = fresh_pool().await;

 sqlx::query(&format!("DROP TABLE {}", schema::EVENTS))
 .execute(&pool).await.unwrap();
 sqlx::query(&format!("DROP TABLE {}", schema::AUDITS))
 .execute(&pool).await.unwrap();

 // Audits: base columns only.
 sqlx::query(&format!(
 r#"CREATE TABLE {} (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL,
 announce_url TEXT NOT NULL,
 info_hash TEXT NOT NULL,
 torrent_size INTEGER NOT NULL,
 config_json TEXT NOT NULL,
 status TEXT NOT NULL DEFAULT '{}',
 working_client TEXT,
 created_at TEXT NOT NULL DEFAULT (datetime('now'))
 )"#,
 schema::AUDITS, vocab::STATUS_IDLE,
 ))
 .execute(&pool).await.unwrap();

 // Events: base columns only.
 sqlx::query(&format!(
 r#"CREATE TABLE {} (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 audit_id INTEGER NOT NULL REFERENCES {}({}),
 seq INTEGER NOT NULL,
 timestamp TEXT NOT NULL,
 phase TEXT NOT NULL,
 client TEXT NOT NULL,
 event TEXT NOT NULL,
 uploaded INTEGER NOT NULL,
 downloaded INTEGER NOT NULL,
 left INTEGER NOT NULL,
 success INTEGER NOT NULL,
 failure_reason TEXT,
 interval INTEGER NOT NULL,
 seeders INTEGER NOT NULL,
 leechers INTEGER NOT NULL,
 peer_count INTEGER NOT NULL,
 latency_ms INTEGER NOT NULL
 )"#,
 schema::EVENTS, schema::AUDITS, schema::ID,
 ))
 .execute(&pool).await.unwrap();

 migrate(&pool).await.expect("migrate on old schema");

 for (table, expected) in [
 (schema::AUDITS, schema::AUDITS_COLUMNS),
 (schema::EVENTS, schema::EVENTS_COLUMNS),
 ] {
 let mut cols = table_columns(&pool, table).await.expect("columns");
 cols.sort();
 let mut exp: Vec<String> = expected.iter().map(|&s| s.to_string()).collect();
 exp.sort();
 assert_eq!(cols, exp, "migrate() did not add all migration columns to {table}");
 }

 // Full app-level round-trip on the migrated schema.
 let audit_id = insert_sample(&pool, "migration-test").await;
 save_peer_state(&pool, audit_id, SavePeerState {
 uploaded: 999, downloaded: 888, left: 777,
 lifecycle_phase: "seed", completed_sent: true, elapsed_secs: 42,
 peer_id: "", key: "",
 }).await.expect("save_peer_state after migration");
 let state = get_peer_state(&pool, audit_id).await.expect("get_peer_state");
 assert_eq!(state.elapsed_secs, 42, "audits migration column must round-trip");

 let mut ev = sample_event(audit_id, 1);
 ev.next_announce_in_secs = 42;
 insert_event(&pool, &ev).await.expect("insert_event after migration");
 let rows = list_events(&pool, audit_id, 1).await.expect("list_events");
 assert_eq!(rows[0].next_announce_in_secs, 42, "events migration column must round-trip");
 }

 #[tokio::test]
 async fn event_roundtrips_every_field_dynamically() {
 // Insert an AuditEvent with every field set to a distinctive value,
 // read it back, and compare via serde_json - so future fields added
 // to AuditEvent are automatically checked without updating this test.
 let pool = fresh_pool().await;
 let audit_id = insert_sample(&pool, "roundtrip").await;
 let ev = AuditEvent {
 audit_id,
 seq: 42,
 timestamp: chrono::Utc::now(),
 phase: vocab::PHASE_ATTACK,
 client: "Roundtrip Client".into(),
 event: vocab::EVENT_REGULAR,
 uploaded: 1_000_000,
 downloaded: 2_000_000,
 left: 3_000_000,
 success: true,
 failure_reason: None,
 interval: 1800,
 seeders: 50,
 leechers: 7,
 peer_count: 57,
 latency_ms: 123,
 working_client: None,
 fair_share_bps: 524_288,
 dynamic_target_bps: 1_048_576,
 next_announce_in_secs: 900, elapsed_secs: 0,
 };
 insert_event(&pool, &ev).await.expect("insert");
 let rows = list_events(&pool, audit_id, 1).await.expect("list");
 assert_eq!(rows.len(), 1);
 // Dynamic comparison: serialize both to JSON Values and compare.
 // Any field added to AuditEvent in the future is automatically checked
 // here - a missing .bind() in insert_event or a missing field in
 // EventRow/into_event would produce a JSON mismatch.
 let original = serde_json::to_value(&ev).unwrap();
 let readback = serde_json::to_value(&rows[0]).unwrap();
 assert_eq!(original, readback, "AuditEvent fields must round-trip through the DB");
 }

 fn sample_goal_input(name: &str) -> GoalRowInput {
 GoalRowInput {
 name: name.to_string(), enabled: true,
 direction: crate::data::vocab::GOAL_DIRECTION_UPLOAD_WIRE.to_string(),
 upload_target: 1_073_741_824, download_target: 0, target_secs: 0,
 reached_action: crate::data::vocab::GOAL_REACHED_STOP_WIRE.to_string(),
 reached_bps: 0,
 }
 }

 #[tokio::test]
 async fn test_goal_crud_roundtrip() {
 let pool = fresh_pool().await;
 let id = insert_goal(&pool, &sample_goal_input("G1")).await.expect("insert");
 let goal = get_goal(&pool, id).await.expect("get").expect("exists");
 assert_eq!(goal.name, "G1");
 assert!(goal.enabled);
 update_goal(&pool, id, &sample_goal_input("G1-renamed")).await.expect("update");
 let goal = get_goal(&pool, id).await.expect("get").expect("exists");
 assert_eq!(goal.name, "G1-renamed");
 delete_goal(&pool, id).await.expect("delete");
 assert!(get_goal(&pool, id).await.expect("get").is_none());
 }

 #[tokio::test]
 async fn test_goal_task_association() {
 let pool = fresh_pool().await;
 let t1 = insert_sample(&pool, "T1").await;
 let t2 = insert_sample(&pool, "T2").await;
 let g = insert_goal(&pool, &sample_goal_input("G")).await.expect("insert");
 set_goal_tasks(&pool, g, &[t1, t2]).await.expect("set");
 let ids = get_goal_task_ids(&pool, g).await.expect("get");
 assert_eq!(ids, vec![t1, t2]);
 // Replace association.
 set_goal_tasks(&pool, g, &[t2]).await.expect("set");
 let ids = get_goal_task_ids(&pool, g).await.expect("get");
 assert_eq!(ids, vec![t2]);
 // Clear.
 set_goal_tasks(&pool, g, &[]).await.expect("set");
 let ids = get_goal_task_ids(&pool, g).await.expect("get");
 assert!(ids.is_empty());
 }

 #[tokio::test]
 async fn test_occupied_tasks_no_goals() {
 let pool = fresh_pool().await;
 let _t1 = insert_sample(&pool, "T1").await;
 let occ = occupied_tasks(&pool, 0).await.expect("ok");
 assert!(occ.is_empty());
 }

 #[tokio::test]
 async fn test_occupied_tasks_specific_goal() {
 let pool = fresh_pool().await;
 let t1 = insert_sample(&pool, "T1").await;
 let t2 = insert_sample(&pool, "T2").await;
 let g = insert_goal(&pool, &sample_goal_input("G")).await.expect("insert");
 set_goal_tasks(&pool, g, &[t1]).await.expect("set");
 // Excluding goal g → nothing occupied.
 let occ = occupied_tasks(&pool, g).await.expect("ok");
 assert!(occ.is_empty());
 // Excluding 0 (create mode) → t1 is occupied.
 let occ = occupied_tasks(&pool, 0).await.expect("ok");
 assert_eq!(occ, vec![t1]);
 // t2 is not occupied.
 assert!(!occ.contains(&t2));
 }

 #[tokio::test]
 async fn test_goal_ids_for_task() {
 let pool = fresh_pool().await;
 let t1 = insert_sample(&pool, "T1").await;
 let t2 = insert_sample(&pool, "T2").await;
 let g_specific = insert_goal(&pool, &sample_goal_input("G-specific")).await.expect("insert");
 set_goal_tasks(&pool, g_specific, &[t1]).await.expect("set");
 let g_other = insert_goal(&pool, &sample_goal_input("G-other")).await.expect("insert");
 set_goal_tasks(&pool, g_other, &[t2]).await.expect("set");
 // t1 is only associated with g_specific.
 let ids = goal_ids_for_task(&pool, t1).await.expect("ok");
 assert!(ids.contains(&g_specific));
 assert!(!ids.contains(&g_other));
 }
}
