//! Controlled vocabularies stored as TEXT in SQLite and emitted over SSE.
//!
//! These are NOT config-tunable values (those live in `config.toml`); they are
//! fixed protocol/UI vocabularies. Every site that writes, reads, or compares
//! one of these strings must go through these consts so a rename touches one
//! place instead of ~18 scattered literals.

// audits.status
pub const STATUS_IDLE: &str = "idle";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_STOPPED: &str = "stopped";

// events.phase
pub const PHASE_PROBE: &str = "probe";
pub const PHASE_ATTACK: &str = "attack";

// events.event
pub const EVENT_PROBE: &str = "probe";
pub const EVENT_STARTED: &str = "started";
pub const EVENT_STOPPED: &str = "stopped";
pub const EVENT_COMPLETED: &str = "completed";
pub const EVENT_REGULAR: &str = "regular";
pub const EVENT_TICK: &str = "tick";

// audits.lifecycle_phase
pub const LIFECYCLE_LEECH: &str = "leech";
pub const LIFECYCLE_SEED: &str = "seed";

// Mode / SpeedMode wire names (mirror engine.rs Display impls - single source
// of truth so render.rs, config, and the frontend all agree on the wire format).
pub const MODE_DU_WIRE: &str = "download_and_upload";
pub const MODE_UO_WIRE: &str = "upload_only";
pub const SPEED_FIXED_WIRE: &str = "fixed";
pub const SPEED_DYNAMIC_WIRE: &str = "dynamic";

// GoalDirection wire names (mirror engine.rs Display impl - single source of
// truth so render.rs, config, and the frontend agree on which counter a goal
// tracks: upload bytes or download bytes).
pub const GOAL_DIRECTION_UPLOAD_WIRE: &str = "upload";
pub const GOAL_DIRECTION_DOWNLOAD_AND_UPLOAD_WIRE: &str = "download_and_upload";

// GoalReachedAction wire names (mirror engine.rs Display impl - single source
// of truth so render.rs, config, and the frontend agree on what happens once
// the goal's cumulative counter reaches its target).
pub const GOAL_REACHED_STOP_WIRE: &str = "stop";
pub const GOAL_REACHED_CONTINUE_INITIAL_WIRE: &str = "continue_initial";
pub const GOAL_REACHED_CONTINUE_CUSTOM_WIRE: &str = "continue_custom";
