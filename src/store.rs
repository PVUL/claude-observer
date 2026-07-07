//! Local SQLite store of usage snapshots — the trend history claude-switcher itself
//! doesn't keep. One embedded file, no server; queried for pace/burn over time and,
//! later, per-ask token spend + the work queue.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

use crate::usage::Account;

/// Default DB path: $XDG_DATA_HOME/claude-observer/observer.db (else ~/.local/share/…).
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("claude-observer").join("observer.db")
}

pub struct Store {
    conn: Connection,
}

/// One historical usage sample.
#[derive(Debug)]
pub struct Sample {
    pub taken_at: DateTime<Utc>,
    pub utilization: f64,
    // Retained for trend analysis (detecting window-boundary resets); not yet displayed.
    #[allow(dead_code)]
    pub resets_at: Option<DateTime<Utc>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS snapshots (
                id          INTEGER PRIMARY KEY,
                taken_at    TEXT NOT NULL,
                account     TEXT NOT NULL,
                window      TEXT NOT NULL,   -- five_hour | seven_day | seven_day_opus
                utilization REAL NOT NULL,
                resets_at   TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_snap ON snapshots(account, window, taken_at);",
        )?;
        Ok(Self { conn })
    }

    /// Record one row per present window across all accounts. Returns rows written.
    pub fn record(&self, accounts: &[Account]) -> Result<usize> {
        let now = Utc::now().to_rfc3339();
        let mut n = 0;
        for a in accounts {
            for (key, _label, w) in a.windows() {
                self.conn.execute(
                    "INSERT INTO snapshots (taken_at, account, window, utilization, resets_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![now, a.name, key, w.utilization, w.resets_at.to_rfc3339()],
                )?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Most recent `limit` samples for an account+window, newest first.
    pub fn history(&self, account: &str, window: &str, limit: usize) -> Result<Vec<Sample>> {
        let mut stmt = self.conn.prepare(
            "SELECT taken_at, utilization, resets_at FROM snapshots
             WHERE account = ?1 AND window = ?2 ORDER BY taken_at DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![account, window, limit as i64], |r| {
            let taken: String = r.get(0)?;
            let util: f64 = r.get(1)?;
            let resets: Option<String> = r.get(2)?;
            Ok((taken, util, resets))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (taken, util, resets) = row?;
            out.push(Sample {
                taken_at: DateTime::parse_from_rfc3339(&taken)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                utilization: util,
                resets_at: resets
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc)),
            });
        }
        Ok(out)
    }
}
