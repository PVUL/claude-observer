//! Text rendering of the pattern-explorer screens. Kept as plain framed text so the
//! exact look can be previewed/approved without a TTY; the interactive navigation shell
//! (added once the layout is signed off) reuses these same rows.

use crate::analysis::Analysis;
use chrono::Local;

const W: usize = 74; // inner content width (between the "│ " and " │" borders)

fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round().clamp(0.0, width as f64) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn spark(vals: &[f64]) -> String {
    const B: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    vals.iter()
        .map(|v| B[((v / 100.0 * 7.0).round().clamp(0.0, 7.0)) as usize])
        .collect()
}

fn verdict(pace: Option<f64>) -> (char, &'static str) {
    match pace {
        Some(p) if p < 85.0 => ('▽', "under — room to spare"),
        Some(p) if p <= 108.0 => ('—', "on pace to use it fully"),
        Some(_) => ('▲', "over — will hit the limit early"),
        None => (' ', "—"),
    }
}

/// Force a line to exactly the inner width (truncate if long, pad if short) so the box
/// borders always align. char count == display cols for our glyphs (no wide/emoji).
fn row(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let content: String = if chars.len() > W {
        chars[..W].iter().collect()
    } else {
        format!("{}{}", s, " ".repeat(W - chars.len()))
    };
    format!("│ {} │", content)
}

fn sep() -> String {
    format!("├─{}─┤", "─".repeat(W))
}

fn frame(right: &str, tabs: &str, body: Vec<String>, footer: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("┌─{}─┐", "─".repeat(W)));
    // title row: left brand, right-aligned context
    let brand = "claude-observer · usage patterns";
    let gap = W.saturating_sub(brand.chars().count() + right.chars().count());
    out.push(row(&format!("{brand}{}{right}", " ".repeat(gap))));
    out.push(row(tabs));
    out.push(sep());
    for b in body {
        out.push(row(&b));
    }
    let f = format!("─ {footer} ");
    let fill = (W + 2).saturating_sub(f.chars().count());
    out.push(format!("└{}{}┘", f, "─".repeat(fill)));
    out
}

fn weeks_inline(a: &Analysis) -> (String, f64) {
    let parts: Vec<String> = a
        .weeks
        .iter()
        .map(|w| if w.used >= 99.5 { format!("{:.0}⚑", w.used) } else { format!("{:.0}", w.used) })
        .collect();
    let avg = if a.weeks.is_empty() {
        0.0
    } else {
        a.weeks.iter().map(|w| w.used).sum::<f64>() / a.weeks.len() as f64
    };
    (parts.join(" "), avg)
}

pub fn overview(a: &Analysis) -> Vec<String> {
    let mut b = Vec::new();
    let used = a.current_used.unwrap_or(0.0);
    let resets = a
        .current_resets
        .map(|r| r.with_timezone(&Local).format("%a %-l%p").to_string())
        .unwrap_or_else(|| "—".into());
    let (arrow, vtext) = verdict(a.current_pace);
    let (winline, avg) = weeks_inline(a);

    b.push("7-DAY ALLOTMENT".into());
    b.push(format!(
        "  this week  [{}]  {:.0}% used · {:.0}% free · resets {}",
        bar(used, 20), used, (100.0 - used).max(0.0), resets
    ));
    match a.current_pace {
        Some(p) => b.push(format!("  pace       ~{:.0}% projected  {} {}", p, arrow, vtext)),
        None => b.push("  pace       —".into()),
    }
    b.push(String::new());
    b.push(format!("  last {} wks: {}   avg {:.0}% used · {:.0}% free", a.weeks.len(), winline, avg, (100.0 - avg).max(0.0)));
    let burn = &a.burn[a.burn.len().saturating_sub(56)..];
    b.push(format!("  burn       {}", spark(burn)));
    b.push(String::new());
    let letters = ["M", "T", "W", "T", "F", "S", "S"];
    let dow: String = letters
        .iter()
        .enumerate()
        .map(|(i, d)| format!("{}{}", d, spark(&[a.dow[i]])))
        .collect::<Vec<_>>()
        .join(" ");
    b.push(format!("DAY-OF-WEEK   {}   weekday {:.0}% · weekend {:.0}%", dow, a.weekday_avg, a.weekend_avg));
    b.push(String::new());
    b.push(format!("▏ ~{:.0}% of your weekly allotment is typically free.", (100.0 - avg).max(0.0)));
    b
}

pub fn trends(a: &Analysis) -> Vec<String> {
    let mut b = Vec::new();
    let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

    b.push("DAY-OF-WEEK  (avg daily activity, 5h peak)".into());
    for (i, d) in days.iter().enumerate() {
        b.push(format!("  {}  [{}] {:>3.0}%", d, bar(a.dow[i], 24), a.dow[i]));
    }
    b.push(format!(
        "  ── weekday [{}] {:.0}%  ·  weekend [{}] {:.0}%",
        bar(a.weekday_avg, 10), a.weekday_avg, bar(a.weekend_avg, 10), a.weekend_avg
    ));
    b.push(String::new());
    b.push("MONTHLY TREND  (avg weekly used)".into());
    for (label, v) in &a.months {
        b.push(format!("  {}  [{}] {:>3.0}%", label, bar(*v, 24), v));
    }
    b.push(String::new());
    b.push("7-DAY BURN  (recent samples)".into());
    b.push(format!("  {}", spark(&a.burn)));
    if let Some(u) = a.current_used {
        let p = a.current_pace.map(|p| format!("~{:.0}%", p)).unwrap_or_else(|| "—".into());
        b.push(format!("  currently {:.0}% · pace {}", u, p));
    }
    b
}

pub fn render(a: &Analysis, tab: &str) -> String {
    let right = format!("{} · {}", a.account, a.plan);
    let (tabs, body, footer) = match tab {
        "trends" => ("Overview  [Trends]", trends(a), "q quit · tab panels · ←/→ weeks · s snapshot now"),
        _ => ("[Overview]  Trends", overview(a), "q quit · tab panels · ←/→ weeks · s snapshot now"),
    };
    frame(&right, tabs, body, footer).join("\n")
}
