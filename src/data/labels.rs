//! Human-readable UI strings - the labels shown in the audit log and config
//! display. Centralized so a label rename touches one place; the JSON
//! handlers (`audit_log_json`, `build_config_rows`) and tests import from
//! here instead of retyping literals.

// audit_log_json torrent_info rows
pub const L_ANNOUNCE_URL: &str = "Announce URL";
pub const L_INFO_HASH: &str = "Info hash";
pub const L_TORRENT_SIZE: &str = "Torrent size";

// build_config_rows
pub const L_MODE: &str = "Mode";
pub const L_STRATEGY: &str = "Strategy";
pub const L_UPLOAD_SPEED: &str = "Upload speed";
pub const L_DOWNLOAD_SPEED: &str = "Download speed";
pub const L_JITTER: &str = "Jitter";
pub const L_RAMP_UP: &str = "Ramp-up";
pub const L_START_PCT: &str = "Start pct";
pub const L_FREEZE_ZERO_LEECHERS: &str = "Freeze 0 leechers";
pub const L_FREEZE_ZERO_SEEDERS: &str = "Freeze 0 seeders";
pub const L_SWARM_MULTIPLIER: &str = "Swarm multiplier";
pub const L_MAX_UPLOAD: &str = "Max upload";
pub const L_MAX_DOWNLOAD: &str = "Max download";

// Goal config rows + stat tiles
pub const L_GOAL_DIRECTION: &str = "Goal direction";
pub const L_GOAL_TARGET: &str = "Goal upload target";
pub const L_GOAL_DOWNLOAD_TARGET: &str = "Goal download target";
pub const L_GOAL_TIME: &str = "Goal time";
pub const L_GOAL_REACHED_ACTION: &str = "On goal reached";
pub const L_GOAL_REACHED_SPEED: &str = "Reached speed";
/// GoalDirection display values - used in the audit-log config panel.
pub const GOAL_DIRECTION_UPLOAD: &str = "Upload";
pub const GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD: &str = "Download + Upload";
/// GoalReachedAction display values - used in the audit-log config panel.
pub const GOAL_REACHED_STOP: &str = "Stop";
pub const GOAL_REACHED_CONTINUE_INITIAL: &str = "Continue (initial speed)";
pub const GOAL_REACHED_CONTINUE_CUSTOM: &str = "Continue (custom speed)";
/// Stat-tile labels for the per-task log panel goal tiles.
pub const GOAL_ETA_LABEL: &str = "eta";
pub const GOAL_REQUIRED_LABEL: &str = "need";

// Mode / SpeedMode display values
/// Full spelling, used in the audit-log config panel.
pub const MODE_DU_FULL: &str = "Download + Upload";
pub const MODE_UO_FULL: &str = "Upload only";
/// Abbreviation, used in the task-list cells.
pub const MODE_DU_ABBR: &str = "D+U";
pub const MODE_UO_ABBR: &str = "Upload";
pub const STRATEGY_FIXED: &str = "Fixed";
pub const STRATEGY_DYNAMIC: &str = "Dynamic";

// Misc display tokens
pub const ON: &str = "on";
pub const OFF: &str = "off";
/// Empty-value placeholder (hyphen) for empty client / zero speed.
pub const EMPTY_DASH: &str = "-";
/// Empty task-list placeholder.
pub const EMPTY_TASKS: &str = "No tasks yet. Click \"New task\" to start.";
/// Empty goal-list placeholder.
pub const EMPTY_GOALS: &str = "No goals yet. Click \"New goal\" to create one.";
/// Empty log-panel placeholder.
pub const EMPTY_LOG: &str = "Select a task to view its log.";
/// Empty event-log placeholder.
pub const EMPTY_EVENTS: &str = "No events yet.";
/// Unlimited speed cap display.
pub const INFINITY: &str = "∞";
/// Speed suffix appended to formatted byte rates.
pub const PER_SEC: &str = "/s";
