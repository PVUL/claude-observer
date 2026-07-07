//! claude-observer — monitor Claude account usage windows, pace, and trends.
//!
//! v0.1: reads `claude-switcher usage --json`, renders a status dashboard with a
//! *pace* projection (are you under- or over-utilizing each window before it resets?),
//! records snapshots to a local SQLite trend store, and prints history. The interactive
//! TUI and the pi extension build on this core (see README roadmap).

mod store;
mod usage;

use anyhow::Result;
use chrono::{Duration, Local, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "claude-observer", version, about = "Monitor Claude account usage: windows, pace, trends.")]
struct Cli {
    /// claude-switcher binary (the usage data source).
    #[arg(long, default_value = "claude-switcher", env = "CLAUDE_OBSERVER_SWITCHER")]
    switcher: String,
    /// SQLite trend store (default: $XDG_DATA_HOME/claude-observer/observer.db).
    #[arg(long, env = "CLAUDE_OBSERVER_DB")]
    db: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show current usage for all accounts (default).
    Status {
        /// Machine-readable JSON (for the pi extension / scripting).
        #[arg(long)]
        json: bool,
    },
    /// Record a usage snapshot into the trend store (run me on a timer).
    Snapshot,
    /// Print recent snapshot history for an account + window.
    History {
        account: String,
        #[arg(long, default_value = "five_hour")]
        window: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

fn window_hours(key: &str) -> f64 {
    if key == "five_hour" {
        5.0
    } else {
        168.0 // 7 days
    }
}

fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round().clamp(0.0, width as f64) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn humanize(d: Duration) -> String {
    let mins = d.num_minutes().max(0);
    let (days, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    if days > 0 {
        format!("{days}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Project end-of-window utilization if the current rate holds, and a verdict.
fn pace(util: f64, key: &str, remaining: Duration) -> (Option<f64>, &'static str) {
    let len = window_hours(key);
    let rem_h = remaining.num_minutes().max(0) as f64 / 60.0;
    let elapsed = len - rem_h;
    if elapsed < 0.15 {
        return (None, "just reset");
    }
    let projected = util * (len / elapsed);
    let verdict = if projected < 85.0 {
        "under-utilizing — room to spare"
    } else if projected <= 108.0 {
        "on pace to use it fully"
    } else {
        "over pace — you'll hit the limit early"
    };
    (Some(projected), verdict)
}

#[derive(Serialize)]
struct WinOut {
    window: String,
    utilization: f64,
    remaining_pct: f64,
    resets_at: String,
    resets_in_minutes: i64,
    projected_pct: Option<f64>,
    verdict: String,
}

#[derive(Serialize)]
struct AcctOut {
    account: String,
    email: Option<String>,
    plan: Option<String>,
    active: bool,
    available: bool,
    windows: Vec<WinOut>,
}

fn status(switcher: &str, json: bool) -> Result<()> {
    let accounts = usage::fetch(switcher)?;
    let now = Utc::now();

    if json {
        let out: Vec<AcctOut> = accounts
            .iter()
            .map(|a| AcctOut {
                account: a.name.clone(),
                email: a.email.clone(),
                plan: a.plan.clone(),
                active: a.active,
                available: a.available,
                windows: a
                    .windows()
                    .into_iter()
                    .map(|(key, _label, w)| {
                        let remaining = w.resets_at - now;
                        let (projected, verdict) = pace(w.utilization, key, remaining);
                        WinOut {
                            window: key.to_string(),
                            utilization: w.utilization,
                            remaining_pct: (100.0 - w.utilization).max(0.0),
                            resets_at: w.resets_at.to_rfc3339(),
                            resets_in_minutes: remaining.num_minutes().max(0),
                            projected_pct: projected,
                            verdict: verdict.to_string(),
                        }
                    })
                    .collect(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    for a in &accounts {
        let marker = if a.active { "*" } else { " " };
        let email = a.email.clone().unwrap_or_default();
        let plan = a.plan.clone().unwrap_or_default();
        println!("{marker} {} ({email} · {plan})", a.name);
        let windows = a.windows();
        if windows.is_empty() {
            println!("      usage: unavailable (not signed in, token expired, or offline)");
            continue;
        }
        for (key, label, w) in windows {
            let remaining = w.resets_at - now;
            let local = w.resets_at.with_timezone(&Local).format("%a %-I:%M%p");
            println!(
                "      {label}  [{}]  {:>3.0}% used · {:>3.0}% left · resets in {} ({})",
                bar(w.utilization, 20),
                w.utilization,
                (100.0 - w.utilization).max(0.0),
                humanize(remaining),
                local
            );
            let (projected, verdict) = pace(w.utilization, key, remaining);
            match projected {
                Some(p) => println!("              pace: ~{p:.0}% projected by reset — {verdict}"),
                None => println!("              pace: {verdict}"),
            }
        }
    }
    Ok(())
}

fn snapshot(switcher: &str, db: PathBuf) -> Result<()> {
    let accounts = usage::fetch(switcher)?;
    let s = store::Store::open(&db)?;
    let n = s.record(&accounts)?;
    println!("recorded {n} window sample(s) to {}", db.display());
    Ok(())
}

fn history(db: PathBuf, account: &str, window: &str, limit: usize) -> Result<()> {
    let s = store::Store::open(&db)?;
    let rows = s.history(account, window, limit)?;
    if rows.is_empty() {
        println!("no snapshots yet for {account}/{window} — run `claude-observer snapshot` first");
        return Ok(());
    }
    println!("{account} · {window} (newest first):");
    for r in rows {
        let when = r.taken_at.with_timezone(&Local).format("%m-%d %H:%M");
        println!("  {when}   {:>5.1}%", r.utilization);
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = cli.db.clone().unwrap_or_else(store::default_path);
    match cli.cmd.unwrap_or(Cmd::Status { json: false }) {
        Cmd::Status { json } => status(&cli.switcher, json),
        Cmd::Snapshot => snapshot(&cli.switcher, db),
        Cmd::History { account, window, limit } => history(db, &account, &window, limit),
    }
}
