//! Text rendering of the pattern-explorer screens — trend sparklines + a 24-hour
//! heatmap (more informative than flat %-bars). Kept as plain framed text so the exact
//! look can be previewed/approved without a TTY; the interactive nav shell reuses these.

use crate::analysis::Analysis;
use chrono::Local;

const W: usize = 74; // inner content width (between "│ " and " │")

fn spark(vals: &[f64]) -> String {
    const B: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    vals.iter()
        .map(|v| B[((v / 100.0 * 7.0).round().clamp(0.0, 7.0)) as usize])
        .collect()
}

fn shade(v: f64, max: f64) -> char {
    const B: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if max <= 0.0 {
        '▁'
    } else {
        B[((v / max) * 7.0).round().clamp(0.0, 7.0) as usize]
    }
}

fn verdict(pace: Option<f64>) -> &'static str {
    match pace {
        Some(p) if p < 85.0 => "under",
        Some(p) if p <= 108.0 => "on pace",
        Some(_) => "over",
        None => "—",
    }
}

fn row(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let content: String = if chars.len() > W {
        chars[..W].iter().collect()
    } else {
        format!("{}{}", s, " ".repeat(W - chars.len()))
    };
    format!("│ {} │", content)
}

fn frame(right: &str, tabs: &str, body: Vec<String>, footer: &str) -> Vec<String> {
    let mut out = vec![format!("┌─{}─┐", "─".repeat(W))];
    let brand = "claude-observer · usage patterns";
    let gap = W.saturating_sub(brand.chars().count() + right.chars().count());
    out.push(row(&format!("{brand}{}{right}", " ".repeat(gap))));
    out.push(row(tabs));
    out.push(format!("├─{}─┤", "─".repeat(W)));
    for b in body {
        out.push(row(&b));
    }
    let f = format!("─ {footer} ");
    out.push(format!("└{}{}┘", f, "─".repeat((W + 2).saturating_sub(f.chars().count()))));
    out
}

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn weekly(a: &Analysis) -> (String, String, f64) {
    let nums: Vec<String> = a
        .weeks
        .iter()
        .map(|w| if w.used >= 99.5 { format!("{:.0}%⚑", w.used) } else { format!("{:.0}%", w.used) })
        .collect();
    let sp = spark(&a.weeks.iter().map(|w| w.used).collect::<Vec<_>>());
    let avg = if a.weeks.is_empty() {
        0.0
    } else {
        a.weeks.iter().map(|w| w.used).sum::<f64>() / a.weeks.len() as f64
    };
    (nums.join(" "), sp, avg)
}

fn heatmap_lines(a: &Analysis) -> Vec<String> {
    let mut b = vec![
        "USAGE BY HOUR  (when tokens are actually consumed · local time)".to_string(),
        format!(
            "        {}",
            ["12a", "3a", "6a", "9a", "12p", "3p", "6p", "9p"]
                .iter()
                .map(|h| format!("{:<4}", h))
                .collect::<String>()
        ),
    ];
    for (i, d) in DAYS.iter().enumerate() {
        let cells: String = (0..8).map(|bk| format!("{:<4}", shade(a.heat[i][bk], a.heat_max))).collect();
        b.push(format!("  {}   {}", d, cells));
    }
    b.push("  low ▁▂▃▄▅▆▇█ high".to_string());
    b
}

pub fn overview(a: &Analysis) -> Vec<String> {
    let mut b = Vec::new();
    let used = a.current_used.unwrap_or(0.0);
    let resets = a
        .current_resets
        .map(|r| r.with_timezone(&Local).format("%a %-l%p").to_string())
        .unwrap_or_else(|| "—".into());
    let (nums, sp, avg) = weekly(a);

    b.push("7-DAY ALLOTMENT".into());
    match a.current_pace {
        Some(p) => b.push(format!(
            "  this week  {:.0}% used · {:.0}% free · resets {} · pace ~{:.0}% ({})",
            used, (100.0 - used).max(0.0), resets, p, verdict(a.current_pace)
        )),
        None => b.push(format!("  this week  {:.0}% used · {:.0}% free · resets {}", used, (100.0 - used).max(0.0), resets)),
    }
    b.push(format!("  weekly     {}   {}   avg {:.0}% used", nums, sp, avg));
    let burn = &a.burn[a.burn.len().saturating_sub(48)..];
    b.push(format!("  burn (7d)  {}  now {:.0}%", spark(burn), used));
    b.push(String::new());
    b.extend(heatmap_lines(a));
    b.push(String::new());
    b.push(format!("▏ ~{:.0}% of your weekly allotment is typically free.", (100.0 - avg).max(0.0)));
    b
}

pub fn trends(a: &Analysis) -> Vec<String> {
    let mut b = Vec::new();
    let (wnums, wsp, wavg) = weekly(a);

    b.push("WEEKLY  used % (free = 100 − used)".into());
    b.push(format!("  {}   {}   avg {:.0}%", wnums, wsp, wavg));
    b.push(String::new());

    b.push("DAY-OF-WEEK  (avg daily activity)".into());
    let dnums: String = DAYS.iter().enumerate().map(|(i, d)| format!("{}{:.0}% ", &d[..1], a.dow[i])).collect();
    b.push(format!("  {}   {}", dnums.trim_end(), spark(&a.dow)));
    b.push(format!("  weekday {:.0}%  ·  weekend {:.0}%", a.weekday_avg, a.weekend_avg));
    b.push(String::new());

    b.push("MONTHLY  (avg weekly used)".into());
    let mnums: String = a.months.iter().map(|(l, v)| format!("{l} {v:.0}%   ")).collect();
    let msp = spark(&a.months.iter().map(|(_, v)| *v).collect::<Vec<_>>());
    b.push(format!("  {}  {}", mnums.trim_end(), msp));
    b.push(String::new());

    b.push("7-DAY BURN  (recent samples)".into());
    b.push(format!("  {}", spark(&a.burn[a.burn.len().saturating_sub(60)..])));
    if let Some(u) = a.current_used {
        let p = a.current_pace.map(|p| format!("~{:.0}% ({})", p, verdict(a.current_pace))).unwrap_or_else(|| "—".into());
        b.push(format!("  now {:.0}% · pace {}", u, p));
    }
    b
}

pub fn render(a: &Analysis, tab: &str) -> String {
    let right = format!("{} · {}", a.account, a.plan);
    let footer = "q quit · tab panels · ←/→ weeks · s snapshot now";
    let (tabs, body) = match tab {
        "trends" => ("Overview  [Trends]", trends(a)),
        _ => ("[Overview]  Trends", overview(a)),
    };
    frame(&right, tabs, body, footer).join("\n")
}
