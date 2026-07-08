//! claude-observer — the usage-metrics / long-term-patterns tool for Claude accounts.
//!
//! Collects window-utilization snapshots (via `claude-switcher usage --json`) into a
//! local SQLite time-series and turns them into patterns: weekly used/free, day-of-week
//! rhythm, weekday vs weekend, monthly trend, burn. Not a live gauge — that's
//! claude-switcher's TUI. See README.

mod analysis;
mod seed;
mod store;
mod tui;
mod usage;
mod view;

use anyhow::Result;
use chrono::Local;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "claude-observer", version, about = "Claude usage metrics & long-term patterns.")]
struct Cli {
    #[arg(long, default_value = "claude-switcher", env = "CLAUDE_OBSERVER_SWITCHER")]
    switcher: String,
    #[arg(long, env = "CLAUDE_OBSERVER_DB")]
    db: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record a usage snapshot into the trend store (run on a timer).
    Snapshot,
    /// Raw recent samples for an account + window.
    History {
        account: String,
        #[arg(long, default_value = "seven_day")]
        window: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Render the pattern screens as text (for review; the same rows the TUI shows).
    Preview {
        #[arg(long, default_value = "overview")]
        screen: String,
        #[arg(long, default_value = "paul-nhost")]
        account: String,
        #[arg(long, default_value = "Team")]
        plan: String,
    },
    /// Plain-text patterns summary (for the pi extension / scripts).
    Report {
        #[arg(long, default_value = "paul-nhost")]
        account: String,
        #[arg(long, default_value = "Team")]
        plan: String,
        #[arg(long, default_value_t = 8)]
        weeks: usize,
    },
    /// Seed dummy data so the screens can be previewed before real collection.
    Seed {
        #[arg(long, default_value = "paul-nhost")]
        account: String,
        #[arg(long, default_value_t = 35)]
        days: i64,
        #[arg(long)]
        reset: bool,
    },
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
        println!("no samples for {account}/{window} yet — run `snapshot` or `seed`");
        return Ok(());
    }
    println!("{account} · {window} (newest first):");
    for r in rows {
        println!("  {}   {:>5.1}%", r.taken_at.with_timezone(&Local).format("%m-%d %H:%M"), r.utilization);
    }
    Ok(())
}

fn preview(db: PathBuf, screen: &str, account: &str, plan: &str) -> Result<()> {
    let s = store::Store::open(&db)?;
    let a = analysis::build(&s, account, plan, 8)?;
    match screen {
        "overview" => println!("{}", view::render(&a, "overview")),
        "trends" => println!("{}", view::render(&a, "trends")),
        _ => {
            println!("{}", view::render(&a, "overview"));
            println!();
            println!("{}", view::render(&a, "trends"));
        }
    }
    Ok(())
}

fn report(db: PathBuf, account: &str, plan: &str, weeks: usize) -> Result<()> {
    let s = store::Store::open(&db)?;
    let a = analysis::build(&s, account, plan, weeks)?;
    let avg = if a.weeks.is_empty() {
        0.0
    } else {
        a.weeks.iter().map(|w| w.used).sum::<f64>() / a.weeks.len() as f64
    };
    println!("{} ({}) · last {} weeks", a.account, a.plan, a.weeks.len());
    println!("  weekly: avg {:.0}% used, ~{:.0}% free", avg, (100.0 - avg).max(0.0));
    if let Some(u) = a.current_used {
        let p = a.current_pace.map(|p| format!("~{:.0}%", p)).unwrap_or_else(|| "—".into());
        println!("  this week: {:.0}% used, pace {}", u, p);
    }
    println!("  rhythm: weekday {:.0}% · weekend {:.0}% (avg daily activity)", a.weekday_avg, a.weekend_avg);
    if !a.months.is_empty() {
        let m: Vec<String> = a.months.iter().map(|(l, v)| format!("{l} {v:.0}%")).collect();
        println!("  monthly: {}", m.join(" · "));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = cli.db.clone().unwrap_or_else(store::default_path);
    let Some(cmd) = cli.cmd else {
        // No subcommand: launch the interactive TUI.
        let store = store::Store::open(&db)?;
        return tui::run(store, cli.switcher, 8);
    };
    match cmd {
        Cmd::Snapshot => snapshot(&cli.switcher, db),
        Cmd::History { account, window, limit } => history(db, &account, &window, limit),
        Cmd::Preview { screen, account, plan } => preview(db, &screen, &account, &plan),
        Cmd::Report { account, plan, weeks } => report(db, &account, &plan, weeks),
        Cmd::Seed { account, days, reset } => {
            let mut s = store::Store::open(&db)?;
            let n = seed::run(&mut s, &account, days, reset)?;
            println!("seeded {n} dummy samples for {account} over {days}d");
            Ok(())
        }
    }
}
