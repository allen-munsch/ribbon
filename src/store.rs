//! Belt event store — append to ndjson log, read, and query.
//!
//! The ndjson file is the source of truth. All operations are atomic at the
//! line level (each line is a complete JSON object). Appends are O(1) via
//! filesystem append. Reads stream line-by-line for constant memory.

use crate::event::{BeltEvent, EventType};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Errors that can occur during store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error at line {line}: {source}")]
    Json {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("File not found: {0}")]
    NotFound(PathBuf),
}

/// Append a single event to the ndjson log file.
///
/// Creates the file and parent directories if they don't exist.
/// Each event is written as one line terminated by `\n`.
pub fn append_event(path: &Path, event: &BeltEvent) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    let line = event
        .to_ndjson_line()
        .map_err(|e| StoreError::Json { line: 0, source: e })?;
    writeln!(file, "{line}")?;
    file.flush()?;
    Ok(())
}

/// Read all events from the ndjson log file.
///
/// Invalid lines are skipped with a warning. Returns events in file order
/// (which is chronological for an append-only log).
pub fn read_events(path: &Path) -> Result<Vec<BeltEvent>, StoreError> {
    let file = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            StoreError::NotFound(path.to_path_buf())
        } else {
            StoreError::Io(e)
        }
    })?;

    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match BeltEvent::from_ndjson_line(trimmed) {
            Ok(event) => events.push(event),
            Err(e) => {
                eprintln!(
                    "belt: warning: skipping malformed line {}: {}",
                    line_num + 1,
                    e
                );
            }
        }
    }

    Ok(events)
}

/// Query filter for selecting events.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Only events from this agent.
    pub agent: Option<String>,
    /// Only events of this type.
    pub event_type: Option<EventType>,
    /// Only events since this timestamp (inclusive).
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    /// Only events matching this task substring (case-insensitive).
    pub task_contains: Option<String>,
    /// Limit to the last N events.
    pub limit: Option<usize>,
}

impl EventFilter {
    /// Create a filter that matches all events.
    pub fn all() -> Self {
        EventFilter::default()
    }

    /// Filter by agent.
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// Filter by event type.
    pub fn event_type(mut self, et: EventType) -> Self {
        self.event_type = Some(et);
        self
    }

    /// Filter since timestamp.
    pub fn since(mut self, ts: chrono::DateTime<chrono::Utc>) -> Self {
        self.since = Some(ts);
        self
    }

    /// Filter by task substring.
    pub fn task_contains(mut self, task: impl Into<String>) -> Self {
        self.task_contains = Some(task.into());
        self
    }

    /// Limit results.
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    /// Apply this filter to an iterator of events.
    pub fn apply<'a>(&self, events: impl Iterator<Item = &'a BeltEvent>) -> Vec<&'a BeltEvent> {
        let mut results: Vec<&BeltEvent> = events
            .filter(|e| {
                if let Some(ref agent) = self.agent {
                    if &e.agent != agent {
                        return false;
                    }
                }
                if let Some(ref et) = self.event_type {
                    if &e.event_type != et {
                        return false;
                    }
                }
                if let Some(ref since) = self.since {
                    if e.ts < *since {
                        return false;
                    }
                }
                if let Some(ref task_substr) = self.task_contains {
                    if let Some(ref task) = e.task {
                        if !task.to_lowercase().contains(&task_substr.to_lowercase()) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            })
            .collect();

        if let Some(limit) = self.limit {
            let start = results.len().saturating_sub(limit);
            results = results[start..].to_vec();
        }

        results
    }
}

/// Summary of an agent's current state based on its events.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentStatus {
    pub agent: String,
    /// Current state: "idle", "submitted", "working", "committed", "completed",
    /// "failed", "blocked", "confused", "sudo_pending", "hitl_pending".
    pub state: String,
    /// Most recent commit hash if any.
    pub last_commit: Option<String>,
    /// Number of completed tasks total.
    pub completed_count: usize,
    /// Number of failed tasks total.
    pub failed_count: usize,
    /// Whether agent has any active (non-terminal) tasks.
    pub has_active: bool,
    /// The active task description if working.
    pub active_task: Option<String>,
    /// If blocked, what's the blocker (from msg).
    pub blocked_by: Option<String>,
    /// If confused, what's the question (from msg).
    pub confused_about: Option<String>,
    /// If sudo pending, what's requested (from msg).
    pub sudo_request: Option<String>,
    /// If HITL pending, what's needed (from msg).
    pub hitl_request: Option<String>,
}

/// Compute current status for all agents found in the event log.
pub fn agent_statuses(events: &[BeltEvent]) -> Vec<AgentStatus> {
    let mut by_agent: HashMap<String, Vec<&BeltEvent>> = HashMap::new();
    for e in events {
        by_agent.entry(e.agent.clone()).or_default().push(e);
    }

    let mut statuses: Vec<AgentStatus> = by_agent
        .into_iter()
        .map(|(agent, agent_events)| {
            let completed_count = agent_events
                .iter()
                .filter(|e| e.event_type == EventType::Completed)
                .count();
            let failed_count = agent_events
                .iter()
                .filter(|e| e.event_type == EventType::Failed)
                .count();

            let mut state = "idle";
            let mut last_commit = None;
            let mut has_active = false;
            let mut active_task = None;
            let mut blocked_by = None;
            let mut confused_about = None;
            let mut sudo_request = None;
            let mut hitl_request = None;

            // Walk backwards to find current state
            for e in agent_events.iter().rev() {
                match &e.event_type {
                    EventType::Submitted | EventType::Working | EventType::Committed => {
                        if state == "idle" {
                            state = e.event_type.state_name();
                            has_active = true;
                            active_task = e.task.clone();
                        }
                    }
                    EventType::Completed => {
                        if state == "idle" {
                            state = "completed";
                        }
                    }
                    EventType::Failed => {
                        if state == "idle" {
                            state = "failed";
                        }
                    }
                    EventType::Blocked => {
                        if state == "idle" {
                            state = "blocked";
                            has_active = true;
                            active_task = e.task.clone();
                            blocked_by = e.msg.clone();
                        }
                    }
                    EventType::Confused => {
                        if state == "idle" {
                            state = "confused";
                            has_active = true;
                            active_task = e.task.clone();
                            confused_about = e.msg.clone();
                        }
                    }
                    EventType::Sudo => {
                        if state == "idle" {
                            state = "sudo_pending";
                            has_active = true;
                            active_task = e.task.clone();
                            sudo_request = e.msg.clone();
                        }
                    }
                    EventType::Hitl => {
                        if state == "idle" {
                            state = "hitl_pending";
                            has_active = true;
                            active_task = e.task.clone();
                            hitl_request = e.msg.clone();
                        }
                    }
                    // Response events revert to previous state
                    EventType::SudoGranted | EventType::SudoDenied | EventType::HitlResolved => {
                        if state == "idle" {
                            state = "idle";
                        }
                    }
                    EventType::Note => {}
                }
                if last_commit.is_none() {
                    last_commit = e.commit.clone();
                }
                if state != "idle" && last_commit.is_some() {
                    break;
                }
            }

            AgentStatus {
                agent,
                state: state.to_string(),
                last_commit,
                completed_count,
                failed_count,
                has_active,
                active_task,
                blocked_by,
                confused_about,
                sudo_request,
                hitl_request,
            }
        })
        .collect();

    statuses.sort_by(|a, b| a.agent.cmp(&b.agent));
    statuses
}

/// Find the previous non-note event for a specific agent+task combination.
/// Returns the EventType of the last state-changing event, or None if this is a new task.
pub fn find_previous_state(events: &[BeltEvent], agent: &str, task: &str) -> Option<EventType> {
    // Walk backwards through events for this agent+task
    for e in events.iter().rev() {
        if e.agent == agent && e.task.as_deref() == Some(task) {
            // Skip notes and responses (they don't change state)
            if e.event_type != EventType::Note && !e.event_type.is_response() {
                return Some(e.event_type.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_append_and_read() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path();

        let e1 = BeltEvent::new("mosaic", EventType::Working).with_task("grpc migration");
        let e2 = BeltEvent::new("mosaic", EventType::Completed)
            .with_task("grpc migration")
            .with_commit("abc123");

        append_event(path, &e1).unwrap();
        append_event(path, &e2).unwrap();

        let events = read_events(path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, EventType::Working);
        assert_eq!(events[1].event_type, EventType::Completed);
    }

    #[test]
    fn test_filter_by_agent() {
        let events = [
            BeltEvent::new("alice", EventType::Working),
            BeltEvent::new("bob", EventType::Completed),
            BeltEvent::new("alice", EventType::Completed),
        ];

        let filter = EventFilter::all().agent("alice");
        let results = filter.apply(events.iter());
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_filter_limit() {
        let events: Vec<BeltEvent> = (0..10)
            .map(|i| BeltEvent::new("agent", EventType::Note).with_msg(format!("msg {i}")))
            .collect();

        let filter = EventFilter::all().limit(3);
        let results = filter.apply(events.iter());
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].msg.as_ref().unwrap(), "msg 7");
    }

    #[test]
    fn test_agent_statuses() {
        let events = vec![
            BeltEvent::new("mosaic", EventType::Submitted).with_task("task1"),
            BeltEvent::new("mosaic", EventType::Working).with_task("task1"),
            BeltEvent::new("mosaic", EventType::Committed)
                .with_task("task1")
                .with_commit("abc"),
            BeltEvent::new("mosaic", EventType::Completed)
                .with_task("task1")
                .with_commit("abc"),
            BeltEvent::new("zypi", EventType::Working).with_task("task2"),
        ];

        let statuses = agent_statuses(&events);
        let mosaic = statuses.iter().find(|s| s.agent == "mosaic").unwrap();
        assert_eq!(mosaic.state, "completed");
        assert_eq!(mosaic.completed_count, 1);

        let zypi = statuses.iter().find(|s| s.agent == "zypi").unwrap();
        assert_eq!(zypi.state, "working");
        assert!(zypi.has_active);
    }
}
