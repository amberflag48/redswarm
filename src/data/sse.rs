//! Server-Sent-Event wire names.
//!
//! The SSE event names are emitted by `api::sse_global` and matched by
//! `addEventListener` in `index.html`. A typo in any of these silently breaks
//! one UI channel, so they are centralized here.

/// The global SSE stream route - a single long-lived connection that drives
/// all dynamic UI. Centralized so both the router registration and the
/// Cache-Control middleware (which exempts this route) reference one path.
pub const EVENTS_ROUTE: &str = "/api/events";

// SSE event names
pub const EV_AUDIT: &str = "audit";
pub const EV_TASK_CREATED: &str = "task_created";
pub const EV_TASK_DELETED: &str = "task_deleted";
pub const EV_TASK_STATUS: &str = "task_status";
pub const EV_TASK_CLIENT: &str = "task_client";
pub const EV_TASK_PROGRESS: &str = "task_progress";
pub const EV_TASK_UPDATED: &str = "task_updated";
/// `config_reloaded` - config.toml was edited and reloaded at runtime. Carries
/// the full new `AppConfig` as JSON so the UI can surgically update fields
/// without a re-fetch. The UI also shows a transient toast.
pub const EV_CONFIG_RELOADED: &str = "config_reloaded";
/// `capture_progress` - a fingerprint-capture session advanced. Carries the
/// session token, the new status, and the full fingerprint snapshot. Drives
/// the capture modal via the global SSE stream - no polling.
pub const EV_CAPTURE_PROGRESS: &str = "capture_progress";
/// `goal_progress` - a global goal's summed counters advanced. Carries the
/// goal id, summed uploaded/downloaded, total speeds, and ETA. Drives the
/// topbar goal tiles live - no polling.
pub const EV_GOAL_PROGRESS: &str = "goal_progress";
/// `goal_created` - a new global goal was created. Carries the goal id.
pub const EV_GOAL_CREATED: &str = "goal_created";
/// `goal_deleted` - a global goal was removed. Carries the goal id.
pub const EV_GOAL_DELETED: &str = "goal_deleted";
/// `goal_updated` - a global goal's config or task associations changed.
/// Carries the goal id; the UI re-fetches the full goal list.
pub const EV_GOAL_UPDATED: &str = "goal_updated";
