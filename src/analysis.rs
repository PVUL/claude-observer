//! Turn the snapshot time-series into the long-term patterns claude-observer is for:
//! weekly used/free, day-of-week rhythm, weekday vs weekend, monthly trend, burn series.

use anyhow::Result;
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use std::collections::BTreeMap;

use crate::store::Store;

pub struct WeeklyUsage {
    #[allow(dead_code)] // week-ending date, for the planned weekly-detail view
    pub label: String,
    pub used: f64,     // peak 7-day utilization reached that week (free = 100 - used)
}

pub struct Analysis {
    pub account: String,
    pub plan: String,
    /// Latest 7-day utilization + when it resets + projected end-of-window pace.
    pub current_used: Option<f64>,
    pub current_resets: Option<DateTime<Utc>>,
    pub current_pace: Option<f64>,
    pub weeks: Vec<WeeklyUsage>,
    /// Recent 7-day utilization series (for the burn sparkline/line).
    pub burn: Vec<f64>,
    /// Avg daily activity (peak 5-hour utilization) by weekday, Mon..Sun.
    pub dow: [f64; 7],
    pub weekday_avg: f64,
    pub weekend_avg: f64,
    /// (month label, avg weekly used) oldest→newest.
    pub months: Vec<(String, f64)>,
}

fn pace(used: f64, remaining_min: i64) -> Option<f64> {
    let rem_h = remaining_min.max(0) as f64 / 60.0;
    let elapsed = 168.0 - rem_h;
    if elapsed < 1.0 {
        return None;
    }
    Some(used * (168.0 / elapsed))
}

pub fn build(store: &Store, account: &str, plan: &str, weeks_n: usize) -> Result<Analysis> {
    let seven = store.samples(account, "seven_day")?;
    let five = store.samples(account, "five_hour")?;

    // Weekly buckets: group 7-day samples by their resetsAt; used = peak in that window.
    let mut wk: BTreeMap<i64, (f64, DateTime<Utc>)> = BTreeMap::new();
    for s in &seven {
        if let Some(r) = s.resets_at {
            let e = wk.entry(r.timestamp()).or_insert((0.0, r));
            if s.utilization > e.0 {
                e.0 = s.utilization;
            }
        }
    }
    let mut weeks: Vec<WeeklyUsage> = wk
        .values()
        .map(|(used, r)| WeeklyUsage {
            label: r.with_timezone(&Local).format("%b %d").to_string(),
            used: *used,
        })
        .collect();
    if weeks.len() > weeks_n {
        weeks = weeks.split_off(weeks.len() - weeks_n);
    }

    // Monthly trend: average weekly-used per calendar month.
    let mut mo: BTreeMap<(i32, u32), (f64, u32)> = BTreeMap::new();
    for (used, r) in wk.values() {
        let l = r.with_timezone(&Local);
        let e = mo.entry((l.year(), l.month())).or_insert((0.0, 0));
        e.0 += used;
        e.1 += 1;
    }
    let months: Vec<(String, f64)> = mo
        .iter()
        .map(|((y, m), (sum, cnt))| {
            let label = NaiveDate::from_ymd_opt(*y, *m, 1)
                .map(|d| d.format("%b").to_string())
                .unwrap_or_default();
            (label, sum / *cnt as f64)
        })
        .collect();

    // Current window + pace.
    let last = seven.last();
    let current_used = last.map(|s| s.utilization);
    let current_resets = last.and_then(|s| s.resets_at);
    let current_pace = match (current_used, current_resets) {
        (Some(u), Some(r)) => pace(u, (r - Utc::now()).num_minutes()),
        _ => None,
    };

    // Burn series: recent 7-day utilization samples.
    let burn: Vec<f64> = seven.iter().rev().take(60).rev().map(|s| s.utilization).collect();

    // Daily activity = peak 5-hour utilization per local calendar day.
    let mut daily: BTreeMap<NaiveDate, f64> = BTreeMap::new();
    for s in &five {
        let d = s.taken_at.with_timezone(&Local).date_naive();
        let e = daily.entry(d).or_insert(0.0);
        if s.utilization > *e {
            *e = s.utilization;
        }
    }
    let mut dow_sum = [0.0f64; 7];
    let mut dow_cnt = [0u32; 7];
    for (d, peak) in &daily {
        let i = d.weekday().num_days_from_monday() as usize;
        dow_sum[i] += *peak;
        dow_cnt[i] += 1;
    }
    let mut dow = [0.0f64; 7];
    for i in 0..7 {
        if dow_cnt[i] > 0 {
            dow[i] = dow_sum[i] / dow_cnt[i] as f64;
        }
    }
    let avg = |lo: usize, hi: usize| {
        let (mut s, mut c) = (0.0, 0u32);
        for i in lo..hi {
            if dow_cnt[i] > 0 {
                s += dow_sum[i];
                c += dow_cnt[i];
            }
        }
        if c > 0 {
            s / c as f64
        } else {
            0.0
        }
    };
    let weekday_avg = avg(0, 5);
    let weekend_avg = avg(5, 7);

    Ok(Analysis {
        account: account.to_string(),
        plan: plan.to_string(),
        current_used,
        current_resets,
        current_pace,
        weeks,
        burn,
        dow,
        weekday_avg,
        weekend_avg,
        months,
    })
}
