//! Belt event types — the canonical communication schema for agent coordination.
//!
//! # Event lifecycle (maps to A2A task states)
//!
//! ```text
//! submitted → working → committed → completed
//!                    ↘ failed
//!
//! note: informational, no state transition
//! ```
//!
//! Every event is a single line of ndjson (newline-delimited JSON).
//! The file is append-only, git-versioned, and streamable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The canonical event type for agent communication.
///
/// Designed to be compact yet human-readable in raw form.
/// When compressed (belt pack), the ndjson typically shrinks 80%+ with Brotli.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeltEvent {
    /// ISO 8601 timestamp with timezone. Always set on creation.
    pub ts: DateTime<Utc>,

    /// Agent identifier — e.g. "frontend", "backend", "human:alice".
    /// Free-form but should be consistent within a project.
    pub agent: String,

    /// Event type — determines the state transition (or lack thereof).
    #[serde(rename = "event")]
    pub event_type: EventType,

    // ── Optional standard fields ──
    /// Task identifier or human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,

    /// Git commit hash (full SHA, not short). Required for `committed` and `completed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,

    /// Human-readable message. Free-form but concise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,

    /// Test count (for completed events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<u32>,

    /// Test failure count (0 = all passing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failures: Option<u32>,

    /// Priority: "LOW", "MEDIUM", "HIGH", "CRITICAL".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,

    /// Whether this blocks downstream work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,

    /// Extension fields — arbitrary key-value pairs for project-specific data.
    /// Example: `{"branch": "feat/grpc", "pr": "https://..."}`
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ext: HashMap<String, serde_json::Value>,
}

/// Event type — the state transition or informational marker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    /// Task has been requested but not yet claimed.
    Submitted,
    /// Agent has claimed the task and started working.
    Working,
    /// Code has been committed locally but not yet verified/pushed.
    /// Belt convention: committed → belt verify → completed.
    Committed,
    /// Task is complete — committed, verified, pushed, tested.
    Completed,
    /// Task has failed. Must include an error message.
    Failed,
    /// Informational note. No state transition. Use for observations, decisions, etc.
    Note,
}

impl EventType {
    /// Returns true if this event type represents a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, EventType::Completed | EventType::Failed)
    }

    /// Returns true if this event type represents an active/in-flight state.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            EventType::Submitted | EventType::Working | EventType::Committed
        )
    }

    /// Human-readable label for rendering.
    pub fn label(&self) -> &'static str {
        match self {
            EventType::Submitted => "SUBMITTED",
            EventType::Working => "WORKING",
            EventType::Committed => "COMMITTED",
            EventType::Completed => "COMPLETED",
            EventType::Failed => "FAILED",
            EventType::Note => "NOTE",
        }
    }

    /// Simple emoji for compact rendering.
    pub fn emoji(&self) -> &'static str {
        match self {
            EventType::Submitted => "📥",
            EventType::Working => "🟡",
            EventType::Committed => "📦",
            EventType::Completed => "✅",
            EventType::Failed => "❌",
            EventType::Note => "📝",
        }
    }
}

impl BeltEvent {
    /// Create a new event with current timestamp.
    pub fn new(agent: impl Into<String>, event_type: EventType) -> Self {
        BeltEvent {
            ts: Utc::now(),
            agent: agent.into(),
            event_type,
            task: None,
            commit: None,
            msg: None,
            tests: None,
            failures: None,
            priority: None,
            blocking: None,
            ext: HashMap::new(),
        }
    }

    /// Builder: set task description.
    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    /// Builder: set commit hash.
    pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
        self.commit = Some(commit.into());
        self
    }

    /// Builder: set message.
    pub fn with_msg(mut self, msg: impl Into<String>) -> Self {
        self.msg = Some(msg.into());
        self
    }

    /// Builder: set test results.
    pub fn with_tests(mut self, tests: u32, failures: u32) -> Self {
        self.tests = Some(tests);
        self.failures = Some(failures);
        self
    }

    /// Builder: set priority.
    pub fn with_priority(mut self, priority: impl Into<String>) -> Self {
        self.priority = Some(priority.into());
        self
    }

    /// Builder: set blocking flag.
    pub fn with_blocking(mut self, blocking: bool) -> Self {
        self.blocking = Some(blocking);
        self
    }

    /// Builder: add an extension field.
    pub fn with_ext(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.ext.insert(key.into(), value.into());
        self
    }

    /// Serialize to a single ndjson line (no trailing newline — caller adds if needed).
    pub fn to_ndjson_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from a single ndjson line.
    pub fn from_ndjson_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_roundtrip() {
        let event = BeltEvent::new("mosaic", EventType::Completed)
            .with_task("gRPC migration")
            .with_commit("7cba506161d9388cb65394aaad7822eaad2523a3")
            .with_msg("All tests passing, gRPC live on :4041")
            .with_tests(339, 0)
            .with_priority("HIGH")
            .with_blocking(false)
            .with_ext("branch", "feat/grpc");

        let line = event.to_ndjson_line().unwrap();
        let parsed = BeltEvent::from_ndjson_line(&line).unwrap();

        assert_eq!(parsed.agent, "mosaic");
        assert_eq!(parsed.event_type, EventType::Completed);
        assert_eq!(
            parsed.commit.unwrap(),
            "7cba506161d9388cb65394aaad7822eaad2523a3"
        );
        assert_eq!(parsed.tests, Some(339));
        assert_eq!(parsed.failures, Some(0));
        assert_eq!(parsed.ext.get("branch").unwrap(), "feat/grpc");
    }

    #[test]
    fn test_terminal_states() {
        assert!(EventType::Completed.is_terminal());
        assert!(EventType::Failed.is_terminal());
        assert!(!EventType::Working.is_terminal());
        assert!(!EventType::Note.is_terminal());
    }

    #[test]
    fn test_minimal_event() {
        let event =
            BeltEvent::new("human:alice", EventType::Note).with_msg("Thinking about architecture");
        let line = event.to_ndjson_line().unwrap();
        assert!(line.contains("human:alice"));
        assert!(line.contains("note"));
    }
}
