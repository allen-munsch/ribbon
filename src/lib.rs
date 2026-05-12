//! # Belt — Agent Communication Belt
//!
//! A file-first, schema-validated, human+machine readable communication
//! protocol for coordinating asynchronous agents.
//!
//! ## Core concepts
//!
//! - **ndjson event log** — append-only, git-versioned, streamable
//! - **6 event types** — submitted → working → committed → completed/failed, plus `note`
//! - **Reification** — compact ndjson (100 bytes) → rich markdown for humans
//! - **Verification** — git origin hash checking prevents false claims
//! - **A2A bridging** — optional sync to Agent-to-Agent protocol task stores
//!
//! ## Quick start
//!
//! ```bash
//! # Install
//! cargo install belt
//!
//! # Initialize
//! belt init
//!
//! # An agent starts a task
//! belt send working --agent frontend --task "add dark mode"
//!
//! # Complete it
//! belt send completed --agent frontend --task "add dark mode" \
//!   --commit abc123def --tests 339 --failures 0
//!
//! # Check status
//! belt status
//!
//! # Render for humans
//! belt render --since 2026-05-11
//!
//! # Verify git hashes
//! belt verify
//! ```
//!
//! ## Library usage
//!
//! ```rust,no_run
//! use belt::{BeltEvent, EventType, append_event, read_events, render, RenderOpts};
//! use std::path::Path;
//!
//! // Append
//! let event = BeltEvent::new("my-agent", EventType::Completed)
//!     .with_task("finished work")
//!     .with_commit("abc123");
//! append_event(Path::new("events.ndjson"), &event).unwrap();
//!
//! // Read and render
//! let events = read_events(Path::new("events.ndjson")).unwrap();
//! let markdown = render(&events, &RenderOpts::default());
//! println!("{markdown}");
//! ```

pub mod config;
pub mod event;
pub mod render;
pub mod store;
pub mod verify;

// Re-exports for convenience
pub use config::BeltConfig;
pub use event::{BeltEvent, EventType};
pub use render::{render, RenderFormat, RenderOpts};
pub use store::{agent_statuses, append_event, read_events, EventFilter, StoreError};
pub use verify::{verify_events, verify_report, GitRoots, VerifyResult};
