//! Generate a realistic dummy time-series so the pattern screens can be previewed
//! before real collection has accumulated. Deterministic (no rng): a smooth
//! time-of-day/weekday intensity model + a sine "noise" term. Dev/demo only.

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};

use crate::store::Store;

// A Sunday 06:00 UTC, used as the reset-boundary anchor for both windows.
fn anchor() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 4, 6, 0, 0).unwrap()
}

// Ceil-boundary strictly after `t` for a window of `secs` seconds, from the anchor.
fn next_boundary(t: DateTime<Utc>, secs: i64) -> DateTime<Utc> {
    let a = anchor();
    let elapsed = (t - a).num_seconds();
    let n = elapsed.div_euclid(secs) + 1;
    a + Duration::seconds(n * secs)
}

fn intensity(t: DateTime<Utc>) -> f64 {
    // Local-ish rhythm: busy weekday afternoons, quiet mornings + weekends.
    let l = t.with_timezone(&chrono::Local);
    let weekend = l.weekday().num_days_from_monday() >= 5;
    let base = if weekend { 0.18 } else { 0.85 };
    let hour = l.hour() as f64 + l.minute() as f64 / 60.0;
    let tod = (-((hour - 14.0) / 5.0).powi(2)).exp(); // gaussian peak ~2pm
    base * (0.20 + 0.80 * tod)
}

pub fn run(store: &mut Store, account: &str, days: i64, reset: bool) -> Result<()> {
    if reset {
        store.clear(account)?;
    }
    let week = 7 * 24 * 3600;
    let five = 5 * 3600;
    let weekly_peaks = [72.0, 88.0, 100.0, 79.0, 91.0, 84.0];

    let now = Utc::now();
    let start = now - Duration::days(days);
    let mut rows: Vec<(String, String, String, f64, String)> = Vec::new();
    let mut t = start;
    while t <= now {
        let noise = ((t.timestamp() as f64) * 0.7).sin() * 1.5;

        // 7-day window: linear ramp toward that week's peak.
        let r7 = next_boundary(t, week);
        let wk_idx = ((r7 - anchor()).num_seconds() / week) as usize;
        let peak7 = weekly_peaks[wk_idx % weekly_peaks.len()];
        let frac7 = 1.0 - (r7 - t).num_seconds() as f64 / week as f64;
        let u7 = (peak7 * frac7 + noise).clamp(0.0, 100.0);
        rows.push((t.to_rfc3339(), account.into(), "seven_day".into(), u7, r7.to_rfc3339()));

        // 5-hour window: ramp toward an intensity-scaled peak.
        let r5 = next_boundary(t, five);
        let frac5 = 1.0 - (r5 - t).num_seconds() as f64 / five as f64;
        let peak5 = (intensity(t) * 100.0).clamp(0.0, 100.0);
        let u5 = (peak5 * frac5 + noise).clamp(0.0, 100.0);
        rows.push((t.to_rfc3339(), account.into(), "five_hour".into(), u5, r5.to_rfc3339()));

        t = t + Duration::minutes(15);
    }
    let n = rows.len();
    store.insert_many(&rows)?;
    println!("seeded {n} dummy samples for {account} over {days}d");
    Ok(())
}
