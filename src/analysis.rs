//! Turn the snapshot time-series into the long-term patterns claude-observer is for:
//! weekly used/free, day-of-week rhythm, weekday vs weekend, monthly trend, burn series.

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Timelike, Utc};
use std::collections::BTreeMap;

use crate::store::Store;

pub struct WeeklyUsage {
    #[allow(dead_code)] // week-ending date, for the planned weekly-detail view
    pub label: String,
    pub used: f64,     // peak 7-day utilization reached that week (free = 100 - used)
}

/// One 5-hour session window (grouped by its reset): span, peak utilization (100% =
/// fully exhausted), and the intra-window utilization trace `(hours-since-start, %)`.
pub struct FiveWindow {
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub peak: f64,
    pub points: Vec<(f64, f64)>,
}

/// One calendar day, assembled for display.
pub struct DayView {
    pub date: NaiveDate,
    /// Weekly-allotment % consumed per hour of the day (0..24) — backs the burn-up line.
    pub alloc_hours: [f64; 24],
    /// Weekly-allotment % consumed per 6-hour block (derived from `alloc_hours`).
    pub blocks: [f64; 4],
    /// Total weekly-allotment % consumed that day (sum of `alloc_hours`).
    pub total: f64,
    /// Activity per hour of the day (0..24), from the 5-hour window — "when burned".
    pub hours: [f64; 24],
    /// Peak 5-hour window utilization reached that day.
    pub peak_5h: f64,
    pub is_today: bool,
    pub is_future: bool,
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
    /// Today's consumption by 3-hour bucket (local time), summed per bucket — the
    /// "when did I burn tokens today" row. Shade relative to `today_max`.
    pub today: [f64; 8],
    pub today_max: f64,
    /// "Now" (local date) — the boundary the week view will not scroll past.
    pub today_date: NaiveDate,
    /// Weekly-allotment consumption per hour, keyed by local date.
    day_alloc_hours: BTreeMap<NaiveDate, [f64; 24]>,
    /// Hourly activity (5-hour window), keyed by local date.
    day_hours: BTreeMap<NaiveDate, [f64; 24]>,
    /// Peak 5-hour utilization, keyed by local date.
    peak_5h: BTreeMap<NaiveDate, f64>,
    /// Every 5-hour session window, chronological — the overlay bars + breakdowns.
    pub five_windows: Vec<FiveWindow>,
}

impl Analysis {
    /// Sunday of the current calendar week.
    pub fn current_week_start(&self) -> NaiveDate {
        self.today_date - Duration::days(self.today_date.weekday().num_days_from_sunday() as i64)
    }

    /// Assemble one day for display (hourly consumption clamped ≥ 0; 6-hour blocks and
    /// the day total derived from it).
    pub fn day_view(&self, date: NaiveDate) -> DayView {
        let alloc_hours = self.day_alloc_hours.get(&date).copied().unwrap_or([0.0; 24]).map(|v| v.max(0.0));
        let mut blocks = [0.0f64; 4];
        for (h, v) in alloc_hours.iter().enumerate() {
            blocks[h / 6] += v;
        }
        DayView {
            date,
            alloc_hours,
            blocks,
            total: alloc_hours.iter().sum(),
            hours: self.day_hours.get(&date).copied().unwrap_or([0.0; 24]),
            peak_5h: self.peak_5h.get(&date).copied().unwrap_or(0.0),
            is_today: date == self.today_date,
            is_future: date > self.today_date,
        }
    }

    /// The seven days (Sun..Sat) starting at `start`.
    pub fn week_view(&self, start: NaiveDate) -> Vec<DayView> {
        (0..7).map(|i| self.day_view(start + Duration::days(i))).collect()
    }
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

    // Activity per hour per local day: positive deltas in 5-hour utilization between
    // consecutive same-window samples (reset drops skipped) — "when tokens were burned".
    // Feeds the hourly day-detail chart; today's row folds up for the text preview.
    let today_date = Local::now().date_naive();
    let mut day_hours: BTreeMap<NaiveDate, [f64; 24]> = BTreeMap::new();
    for w in five.windows(2) {
        let (p, c) = (&w[0], &w[1]);
        if p.resets_at != c.resets_at || c.resets_at.is_none() {
            continue;
        }
        let l = c.taken_at.with_timezone(&Local);
        day_hours.entry(l.date_naive()).or_insert([0.0; 24])[l.hour() as usize] +=
            (c.utilization - p.utilization).max(0.0);
    }
    // 5-hour session windows: group five-hour samples by their reset, keep the peak and
    // the intra-window utilization trace (for the overlay bars + per-session breakdown).
    // (resetsAt, peak, [(hours-since-start, utilization)]) keyed by reset timestamp.
    type FiveAccum = (DateTime<Utc>, f64, Vec<(f64, f64)>);
    let mut fw: BTreeMap<i64, FiveAccum> = BTreeMap::new();
    for s in &five {
        if let Some(r) = s.resets_at {
            let start_utc = r - Duration::hours(5);
            let e = fw.entry(r.timestamp()).or_insert((r, 0.0, Vec::new()));
            e.1 = e.1.max(s.utilization);
            e.2.push(((s.taken_at - start_utc).num_seconds() as f64 / 3600.0, s.utilization));
        }
    }
    let five_windows: Vec<FiveWindow> = fw
        .into_values()
        .map(|(r, peak, mut points)| {
            points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let end = r.with_timezone(&Local);
            FiveWindow { start: end - Duration::hours(5), end, peak, points }
        })
        .collect();

    let mut today = [0.0f64; 8];
    if let Some(hrs) = day_hours.get(&today_date) {
        for (h, v) in hrs.iter().enumerate() {
            today[h / 3] += v;
        }
    }
    let today_max = today.iter().cloned().fold(0.0f64, f64::max);

    // Weekly-allotment consumption per hour per local day: the *net* change in the 7-day
    // window's utilization across consecutive same-window samples (reset drops skipped).
    // Net (not per-sample positives) so sampling jitter cancels within the hour.
    let mut day_alloc_hours: BTreeMap<NaiveDate, [f64; 24]> = BTreeMap::new();
    for w in seven.windows(2) {
        let (p, c) = (&w[0], &w[1]);
        if p.resets_at != c.resets_at || c.resets_at.is_none() {
            continue;
        }
        let l = c.taken_at.with_timezone(&Local);
        day_alloc_hours.entry(l.date_naive()).or_insert([0.0; 24])[l.hour() as usize] +=
            c.utilization - p.utilization;
    }

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
        today,
        today_max,
        today_date,
        day_alloc_hours,
        day_hours,
        peak_5h: daily,
        five_windows,
    })
}
