//! Current per-account usage, sourced from `claude-switcher usage --json`.
//!
//! This is the one network-touching path (claude-switcher queries the Anthropic usage
//! endpoint). Shelling out keeps claude-observer an "add-on" that reuses the switcher's
//! account auth; a future standalone mode can hit the endpoint directly (see README).

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::process::Command;

/// A single rolling usage window (5-hour, 7-day, or 7-day Opus).
#[derive(Debug, Clone, Deserialize)]
pub struct Window {
    #[serde(rename = "resetsAt")]
    pub resets_at: DateTime<Utc>,
    /// Percent of the window's allotment consumed so far (0-100).
    pub utilization: f64,
}

/// One Claude account as reported by claude-switcher.
#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub name: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub active: bool,
    pub available: bool,
    #[serde(rename = "fiveHour")]
    pub five_hour: Option<Window>,
    #[serde(rename = "sevenDay")]
    pub seven_day: Option<Window>,
    #[serde(rename = "sevenDayOpus")]
    pub seven_day_opus: Option<Window>,
}

impl Account {
    /// (key, label, window) for each present window, in display order.
    pub fn windows(&self) -> Vec<(&'static str, &'static str, &Window)> {
        let mut out = Vec::new();
        if let Some(w) = &self.five_hour {
            out.push(("five_hour", "5-hour", w));
        }
        if let Some(w) = &self.seven_day {
            out.push(("seven_day", "7-day ", w));
        }
        if let Some(w) = &self.seven_day_opus {
            out.push(("seven_day_opus", "7d-opus", w));
        }
        out
    }
}

/// Run `<switcher> usage --json` and parse it.
pub fn fetch(switcher_bin: &str) -> Result<Vec<Account>> {
    let out = Command::new(switcher_bin)
        .args(["usage", "--json"])
        .output()
        .with_context(|| format!("running `{switcher_bin} usage --json` (is claude-switcher installed?)"))?;
    if !out.status.success() {
        bail!(
            "`{switcher_bin} usage --json` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("parsing claude-switcher usage JSON")
}
