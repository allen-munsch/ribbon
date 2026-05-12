//! Belt CLI — command-line interface for the agent communication belt.

use anyhow::{Context, Result};
use belt::{
    agent_statuses, append_event, read_events, render, verify_events, verify_report, BeltConfig,
    BeltEvent, EventFilter, EventType, GitRoots, RenderFormat, RenderOpts,
};
use clap::{ArgAction, Parser, Subcommand};
use std::path::PathBuf;

/// Belt — agent communication via structured ndjson event log.
///
/// A file-first, git-versioned protocol for coordinating asynchronous agents.
/// Compact ndjson for machines, rich markdown for humans. Optional A2A bridging.
#[derive(Parser)]
#[command(name = "belt", version, about, long_about = None)]
struct Cli {
    /// Path to the ndjson event log (overrides config)
    #[arg(short = 'L', long, env = "BELT_LOG_PATH", global = true)]
    log: Option<PathBuf>,

    /// Path to belt config file
    #[arg(short = 'c', long, env = "BELT_CONFIG", global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize belt in the current directory
    Init,

    /// Send (append) an event to the log
    Send(SendArgs),

    /// Show current status of all agents
    Status(StatusArgs),

    /// Query events with filters
    Query(QueryArgs),

    /// Render events as human-readable markdown/text
    Render(RenderArgs),

    /// Watch for new events in real time
    Watch(WatchArgs),

    /// Verify commit hashes against git origins
    Verify(VerifyArgs),

    /// Pack the event log (Brotli compress)
    Pack(PackArgs),

    /// Unpack a compressed event log
    Unpack(UnpackArgs),
}

// ── send ──────────────────────────────────────────────────────────────

#[derive(Parser)]
struct SendArgs {
    /// Event type: submitted, working, committed, completed, failed, note
    #[arg(value_name = "EVENT")]
    event: String,

    /// Agent name/identifier
    #[arg(short, long)]
    agent: String,

    /// Task description or identifier
    #[arg(short, long)]
    task: Option<String>,

    /// Git commit hash (required for committed/completed)
    #[arg(short = 'H', long, value_name = "SHA")]
    commit: Option<String>,

    /// Human-readable message
    #[arg(short, long)]
    msg: Option<String>,

    /// Test count (for completed events)
    #[arg(long)]
    tests: Option<u32>,

    /// Test failure count (default 0)
    #[arg(long, default_value = "0")]
    failures: u32,

    /// Priority: LOW, MEDIUM, HIGH, CRITICAL
    #[arg(short, long)]
    priority: Option<String>,

    /// Whether this task blocks downstream work
    #[arg(short = 'B', long, action = ArgAction::SetTrue, default_value = "false")]
    blocking: bool,
}

fn cmd_send(args: SendArgs, log_path: &std::path::Path) -> Result<()> {
    let event_type: EventType = serde_json::from_str(&format!("\"{}\"", args.event)).context(
        "Invalid event type. Use: submitted, working, committed, completed, failed, note",
    )?;
    let emoji = event_type.emoji();
    let label = event_type.label();

    let mut event = BeltEvent::new(&args.agent, event_type);

    if let Some(task) = args.task {
        event = event.with_task(task);
    }
    if let Some(commit) = args.commit {
        event = event.with_commit(commit);
    }
    if let Some(msg) = args.msg {
        event = event.with_msg(msg);
    }
    if let Some(tests) = args.tests {
        event = event.with_tests(tests, args.failures);
    }
    if let Some(priority) = args.priority {
        event = event.with_priority(priority);
    }
    if args.blocking {
        event = event.with_blocking(true);
    }

    append_event(log_path, &event)?;
    eprintln!("{emoji} {label} {}", event.agent);
    Ok(())
}

// ── status ────────────────────────────────────────────────────────────

#[derive(Parser)]
struct StatusArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn cmd_status(args: StatusArgs, log_path: &std::path::Path) -> Result<()> {
    let events = match read_events(log_path) {
        Ok(e) => e,
        Err(belt::StoreError::NotFound(_)) => {
            if args.json {
                println!("[]");
            } else {
                println!("No events yet. Use 'belt send' to add events.");
            }
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let statuses = agent_statuses(&events);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
    } else {
        if statuses.is_empty() {
            println!("No agents found.");
            return Ok(());
        }

        println!(
            "{:<16} {:<12} {:<10} {:<6} {:<6} ACTIVE TASK",
            "AGENT", "STATE", "COMMIT", "DONE", "FAIL"
        );
        println!("{}", "-".repeat(80));

        for s in &statuses {
            let commit = s
                .last_commit
                .as_ref()
                .map(|c| &c[..c.len().min(7)])
                .unwrap_or("-");
            let state_icon = match s.state.as_str() {
                "working" | "submitted" | "committed" => "🟡",
                "completed" => "✅",
                "failed" => "❌",
                _ => "⚪",
            };
            println!(
                "{:<16} {} {:<10} {:<7} {:<6} {:<6} {}",
                s.agent,
                state_icon,
                s.state,
                commit,
                s.completed_count,
                s.failed_count,
                s.active_task.as_deref().unwrap_or("-"),
            );
        }
    }
    Ok(())
}

// ── query ─────────────────────────────────────────────────────────────

#[derive(Parser)]
struct QueryArgs {
    /// Filter by agent
    #[arg(short, long)]
    agent: Option<String>,

    /// Filter by event type
    #[arg(short, long)]
    event: Option<String>,

    /// Filter since ISO timestamp (e.g. 2026-05-11T00:00:00Z)
    #[arg(long)]
    since: Option<String>,

    /// Filter by task substring
    #[arg(long)]
    task: Option<String>,

    /// Limit to last N events
    #[arg(short, long)]
    limit: Option<usize>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn cmd_query(args: QueryArgs, log_path: &std::path::Path) -> Result<()> {
    let events = read_events(log_path)?;
    let mut filter = EventFilter::all();

    if let Some(agent) = args.agent {
        filter = filter.agent(agent);
    }
    if let Some(event_str) = args.event {
        let et: EventType =
            serde_json::from_str(&format!("\"{event_str}\"")).context("Invalid event type")?;
        filter = filter.event_type(et);
    }
    if let Some(since_str) = args.since {
        let ts = chrono::DateTime::parse_from_rfc3339(&since_str)
            .context("Invalid timestamp. Use ISO 8601 format (e.g. 2026-05-11T00:00:00Z)")?;
        filter = filter.since(ts.with_timezone(&chrono::Utc));
    }
    if let Some(task) = args.task {
        filter = filter.task_contains(task);
    }
    if let Some(limit) = args.limit {
        filter = filter.limit(limit);
    }

    let results = filter.apply(events.iter());

    if args.json {
        let events: Vec<&belt::BeltEvent> = results.into_iter().collect();
        println!("{}", serde_json::to_string_pretty(&events)?);
    } else {
        for event in &results {
            println!("{}", event.to_ndjson_line()?);
        }
    }
    Ok(())
}

// ── render ────────────────────────────────────────────────────────────

#[derive(Parser)]
struct RenderArgs {
    /// Output format: markdown, plain, compact
    #[arg(short, long, default_value = "markdown")]
    format: String,

    /// Only render events since this ISO date (e.g. 2026-05-11)
    #[arg(long)]
    since: Option<String>,

    /// Only render events for this agent
    #[arg(short, long)]
    agent: Option<String>,

    /// Group events by agent
    #[arg(short = 'G', long)]
    group_by_agent: bool,

    /// Disable emoji
    #[arg(long)]
    no_emoji: bool,
}

fn cmd_render(args: RenderArgs, log_path: &std::path::Path) -> Result<()> {
    let events = read_events(log_path)?;
    let format = match args.format.as_str() {
        "markdown" | "md" => RenderFormat::Markdown,
        "plain" | "text" | "txt" => RenderFormat::Plain,
        "compact" | "short" => RenderFormat::Compact,
        other => anyhow::bail!("Unknown format: {other}. Use: markdown, plain, compact"),
    };

    let opts = RenderOpts {
        format,
        since: args.since,
        agent: args.agent,
        group_by_agent: args.group_by_agent,
        emoji: !args.no_emoji,
    };

    let output = render(&events, &opts);
    print!("{output}");
    Ok(())
}

// ── watch ─────────────────────────────────────────────────────────────

#[derive(Parser)]
struct WatchArgs {
    /// Output format: json, compact
    #[arg(short, long, default_value = "compact")]
    format: String,
}

fn cmd_watch(args: WatchArgs, log_path: &std::path::Path) -> Result<()> {
    // Simple polling-based watch that works without the notify dependency
    let log_path = log_path.to_path_buf();

    // Get initial file size
    let mut last_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

    eprintln!("Watching {} (Ctrl+C to stop)", log_path.display());

    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));

        let current_size = match std::fs::metadata(&log_path) {
            Ok(m) => m.len(),
            Err(_) => {
                // File might not exist yet
                continue;
            }
        };

        if current_size > last_size {
            // Read new content
            if let Ok(events) = read_events(&log_path) {
                let new_events: Vec<_> = if !events.is_empty() {
                    // Estimate which events are new based on size increase
                    // For simplicity, just show the last few
                    let show = events.len().saturating_sub(3);
                    events[show..].to_vec()
                } else {
                    vec![]
                };

                for event in &new_events {
                    match args.format.as_str() {
                        "json" => println!("{}", event.to_ndjson_line()?),
                        _ => {
                            let opts = RenderOpts {
                                format: RenderFormat::Compact,
                                emoji: true,
                                ..Default::default()
                            };
                            let line = render(std::slice::from_ref(event), &opts);
                            print!("{line}");
                        }
                    }
                }
            }
            last_size = current_size;
        }
    }
}

// ── verify ────────────────────────────────────────────────────────────

#[derive(Parser)]
struct VerifyArgs {
    /// Only verify this agent
    #[arg(short, long)]
    agent: Option<String>,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

fn cmd_verify(args: VerifyArgs, log_path: &std::path::Path, config: &BeltConfig) -> Result<()> {
    let events = read_events(log_path)?;

    // Build git roots from config
    let mut git_roots = GitRoots::default()
        .with_remote(&config.git_remote)
        .with_branch(&config.git_branch);

    for (agent, path) in &config.git_roots {
        git_roots = git_roots.with_agent(agent, path);
    }

    let mut results = verify_events(&events, &git_roots);

    // Filter by agent if requested
    if let Some(ref agent_filter) = args.agent {
        results.retain(|r| &r.agent == agent_filter);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        println!("{}", verify_report(&results));
    }

    // Exit with non-zero if any unverified commits
    let all_ok = results.iter().all(|r| r.on_origin);
    if !all_ok {
        std::process::exit(1);
    }

    Ok(())
}

// ── pack / unpack ─────────────────────────────────────────────────────

#[derive(Parser)]
struct PackArgs {
    /// Output file (default: events.ndjson.br)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[cfg(feature = "compress")]
fn cmd_pack(args: PackArgs, log_path: &std::path::Path) -> Result<()> {
    let events = read_events(log_path)?;
    let output_path = args
        .output
        .unwrap_or_else(|| log_path.with_extension("ndjson.br"));

    // Serialize all events to a single JSON array
    let json_bytes = serde_json::to_vec(&events)?;

    // Compress with Brotli
    let mut compressed = Vec::new();
    {
        let mut compressor = brotli::CompressorReader::new(
            &json_bytes[..],
            4096, // buffer size
            6,    // compression level (1-11)
            22,   // lgwin
        );
        std::io::copy(&mut compressor, &mut compressed)?;
    }

    std::fs::write(&output_path, &compressed)?;
    let ratio = (compressed.len() as f64 / json_bytes.len() as f64) * 100.0;
    eprintln!(
        "Packed: {} → {} ({:.0}% of original)",
        format_bytes(json_bytes.len()),
        format_bytes(compressed.len()),
        ratio
    );
    Ok(())
}

#[derive(Parser)]
struct UnpackArgs {
    /// Input file (default: events.ndjson.br)
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Output file (default: stdout)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[cfg(feature = "compress")]
fn cmd_unpack(args: UnpackArgs, log_path: &std::path::Path) -> Result<()> {
    let input_path = args
        .input
        .unwrap_or_else(|| log_path.with_extension("ndjson.br"));
    let compressed = std::fs::read(&input_path)?;

    // Decompress with Brotli
    let mut decompressed = Vec::new();
    {
        let mut decompressor = brotli::Decompressor::new(&compressed[..], 4096);
        std::io::copy(&mut decompressor, &mut decompressed)?;
    }

    // Deserialize JSON array
    let events: Vec<BeltEvent> = serde_json::from_slice(&decompressed)?;

    // Write ndjson
    let ndjson: String = events
        .iter()
        .map(|e| e.to_ndjson_line())
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");

    if let Some(output_path) = args.output {
        std::fs::write(&output_path, ndjson)?;
        eprintln!("Unpacked to {}", output_path.display());
    } else {
        println!("{ndjson}");
    }

    Ok(())
}

// ── pack/unpack dispatchers (feature-gated) ─────────────────────────────

#[cfg(feature = "compress")]
fn pack_handler(args: PackArgs, log_path: &std::path::Path) -> Result<()> {
    cmd_pack(args, log_path)
}

#[cfg(not(feature = "compress"))]
fn pack_handler(_args: PackArgs, _log_path: &std::path::Path) -> Result<()> {
    anyhow::bail!("Pack is not available. Rebuild with: cargo install belt --features compress");
}

#[cfg(feature = "compress")]
fn unpack_handler(args: UnpackArgs, log_path: &std::path::Path) -> Result<()> {
    cmd_unpack(args, log_path)
}

#[cfg(not(feature = "compress"))]
fn unpack_handler(_args: UnpackArgs, _log_path: &std::path::Path) -> Result<()> {
    anyhow::bail!("Unpack is not available. Rebuild with: cargo install belt --features compress");
}

#[cfg_attr(not(feature = "compress"), allow(dead_code))]
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ── init ──────────────────────────────────────────────────────────────

fn cmd_init() -> Result<()> {
    let belt_dir = std::path::Path::new(".belt");
    if belt_dir.exists() {
        anyhow::bail!(".belt/ already exists. Remove it first to reinitialize.");
    }

    std::fs::create_dir_all(belt_dir)?;

    let default_config = BeltConfig::default();
    let toml_str = toml::to_string_pretty(&default_config)?;

    let config_path = belt_dir.join("config.toml");
    std::fs::write(&config_path, toml_str)?;

    eprintln!("Initialized belt in .belt/");
    eprintln!("  Config: {}", config_path.display());
    eprintln!(
        "  Log:    {} (will be created on first 'belt send')",
        default_config.log_path.display()
    );
    eprintln!();
    eprintln!("Next steps:");
    eprintln!("  1. Edit .belt/config.toml to add your agents and git roots");
    eprintln!("  2. belt send working --agent my-agent --task 'first task'");
    eprintln!("  3. belt status");

    Ok(())
}

// ── main ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load config
    let config = match cli.config {
        Some(ref path) => BeltConfig::from_file(path)?,
        None => BeltConfig::discover()?,
    };

    // Determine log path
    let log_path = match cli.log {
        Some(ref path) => path.clone(),
        None => {
            let config_dir = cli.config.as_ref().and_then(|p| p.parent());
            config.resolve_log_path(config_dir)
        }
    };

    match cli.command {
        Commands::Init => cmd_init()?,
        Commands::Send(args) => cmd_send(args, &log_path)?,
        Commands::Status(args) => cmd_status(args, &log_path)?,
        Commands::Query(args) => cmd_query(args, &log_path)?,
        Commands::Render(args) => cmd_render(args, &log_path)?,
        Commands::Watch(args) => cmd_watch(args, &log_path)?,
        Commands::Verify(args) => cmd_verify(args, &log_path, &config)?,
        Commands::Pack(args) => pack_handler(args, &log_path)?,
        Commands::Unpack(args) => unpack_handler(args, &log_path)?,
    }

    Ok(())
}
