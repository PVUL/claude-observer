//! Interactive terminal UI — the default screen when claude-observer is run with no
//! subcommand. The Week panel (usage gauge + a bar per day), the account cycled with
//! ↑/↓, and a status footer with the hotkeys. Mirrors claude-switcher's feel. Renders
//! native widgets — gauge + bar charts — off the `Analysis` the text `preview` formats.
//! A Trends panel exists (see `draw_trends` / `TABS`) but is parked for now.

use std::io::{self, Stdout};

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Timelike};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::symbols::Marker;
use ratatui::widgets::{
    Axis, Bar, BarChart, BarGroup, Block, Borders, Chart, Dataset, Gauge, GraphType, Paragraph, Tabs,
};
use ratatui::{Frame, Terminal};

use crate::analysis::{self, Analysis, DayView, FiveWindow};
use crate::seed;
use crate::store::Store;
use crate::usage;

const ACCENT: Color = Color::Cyan;

/// How the week panel is drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
enum WeekViz {
    /// Floating bars — one per 5-hour session, taller = more used that window.
    Sessions,
    /// A calendar of per-day 6-hour-block bars.
    Calendar,
}

impl WeekViz {
    fn toggle(self) -> WeekViz {
        match self {
            WeekViz::Sessions => WeekViz::Calendar,
            WeekViz::Calendar => WeekViz::Sessions,
        }
    }
    fn label(self) -> &'static str {
        match self {
            WeekViz::Sessions => "sessions",
            WeekViz::Calendar => "calendar",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Week,
    #[allow(dead_code)] // parked: re-enable by adding it back to `TABS`
    Trends,
}

/// The panels shown in the tab bar, in order. Trends is parked for now — add
/// `Tab::Trends` back here and the tab bar, ←/→ switching, and footer all light up again
/// (the `draw_trends` renderer stays wired in `draw_panel`).
const TABS: [Tab; 1] = [Tab::Week];

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Week => "Week",
            Tab::Trends => "Trends",
        }
    }
    fn index(self) -> usize {
        TABS.iter().position(|&t| t == self).unwrap_or(0)
    }
    /// Cycle to the next enabled panel. Parked with Trends — wire it to a key when the
    /// tab bar comes back (←/→ now drive the day cursor).
    #[allow(dead_code)]
    fn toggled(self) -> Tab {
        TABS[(self.index() + 1) % TABS.len()]
    }
}

struct App {
    store: Store,
    switcher: String,
    weeks: usize,
    accounts: Vec<String>,
    selected: usize,
    tab: Tab,
    /// Cached analysis for the selected account; rebuilt on select / refresh / snapshot.
    analysis: Option<Analysis>,
    /// Week shown, relative to the current calendar week (0 = current, negative = past;
    /// never positive — no future weeks).
    week_offset: i64,
    /// Highlighted day within the shown week (0..7, Sun..Sat).
    day_cursor: usize,
    /// Selected 5-hour session block, as an index into the shown week's windows
    /// (`week_window_idxs`). `None` = day navigation.
    block_cursor: Option<usize>,
    /// Whether the selected day/block detail view is open.
    detail: bool,
    /// How the week panel is drawn (burn-up line vs bar calendar).
    viz: WeekViz,
    /// Demo mode: read from a throwaway in-memory store seeded with dummy data.
    demo: bool,
    demo_store: Option<Store>,
    status: Option<String>,
    last_updated: DateTime<Local>,
    should_quit: bool,
}

impl App {
    fn new(store: Store, switcher: String, weeks: usize) -> Self {
        let mut app = App {
            store,
            switcher,
            weeks,
            accounts: Vec::new(),
            selected: 0,
            tab: Tab::Week,
            analysis: None,
            week_offset: 0,
            day_cursor: 0,
            block_cursor: None,
            detail: false,
            viz: WeekViz::Sessions,
            demo: false,
            demo_store: None,
            status: None,
            last_updated: Local::now(),
            should_quit: false,
        };
        app.reload();
        app.reset_view();
        app
    }

    /// Index of today within its week (Sun..Sat), or 0 if unknown.
    fn today_index(&self) -> usize {
        self.analysis
            .as_ref()
            .map(|a| a.today_date.weekday().num_days_from_sunday() as usize)
            .unwrap_or(0)
    }

    /// Snap back to the current week with the cursor on today, detail closed.
    fn reset_view(&mut self) {
        self.week_offset = 0;
        self.detail = false;
        self.block_cursor = None;
        self.day_cursor = self.today_index();
    }

    /// Indices (into `analysis.five_windows`) of the 5-hour sessions that start within
    /// the shown week, chronological.
    fn week_window_idxs(&self) -> Vec<usize> {
        let Some(a) = self.analysis.as_ref() else {
            return Vec::new();
        };
        let start = a.current_week_start() + Duration::days(self.week_offset * 7);
        let end = start + Duration::days(7);
        a.five_windows
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                let d = w.start.date_naive();
                d >= start && d < end
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The absolute `five_windows` index currently selected, if any.
    fn selected_window(&self) -> Option<usize> {
        self.block_cursor.and_then(|k| self.week_window_idxs().get(k).copied())
    }

    fn block_next(&mut self) {
        let n = self.week_window_idxs().len();
        if n > 0 {
            self.block_cursor = Some(self.block_cursor.map_or(0, |i| (i + 1) % n));
        }
    }

    fn block_prev(&mut self) {
        let n = self.week_window_idxs().len();
        if n > 0 {
            self.block_cursor = Some(self.block_cursor.map_or(n - 1, |i| (i + n - 1) % n));
        }
    }

    /// Start of the week currently shown (Sunday), or None if there's no analysis.
    fn shown_week_start(&self) -> Option<chrono::NaiveDate> {
        self.analysis
            .as_ref()
            .map(|a| a.current_week_start() + Duration::days(self.week_offset * 7))
    }

    /// Move the cursor left, wrapping to the previous week's Saturday at the left edge.
    fn cursor_left(&mut self) {
        if self.day_cursor > 0 {
            self.day_cursor -= 1;
        } else {
            self.week_offset -= 1;
            self.day_cursor = 6;
        }
    }

    /// Move the cursor right, wrapping to the next week's Sunday at the right edge — but
    /// never onto a future date.
    fn cursor_right(&mut self) {
        let (Some(start), Some(today)) =
            (self.shown_week_start(), self.analysis.as_ref().map(|a| a.today_date))
        else {
            return;
        };
        if self.day_cursor < 6 {
            if start + Duration::days(self.day_cursor as i64 + 1) <= today {
                self.day_cursor += 1;
            }
        } else if start + Duration::days(7) <= today {
            self.week_offset += 1;
            self.day_cursor = 0;
        }
    }

    /// The store the views read from: the in-memory demo store in demo mode, else real.
    fn active_store(&self) -> &Store {
        if self.demo {
            self.demo_store.as_ref().unwrap_or(&self.store)
        } else {
            &self.store
        }
    }

    /// Re-read the account list from the active store and rebuild the selected analysis.
    fn reload(&mut self) {
        let accounts = self.active_store().accounts().unwrap_or_default();
        self.accounts = accounts;
        if self.selected >= self.accounts.len() {
            self.selected = self.accounts.len().saturating_sub(1);
        }
        self.rebuild_analysis();
    }

    fn rebuild_analysis(&mut self) {
        let Some(acct) = self.accounts.get(self.selected).cloned() else {
            self.analysis = None;
            return;
        };
        let built = analysis::build(self.active_store(), &acct, "", self.weeks).ok();
        self.analysis = built;
    }

    /// Lazily build the demo store the first time it's needed.
    fn ensure_demo(&mut self) {
        if self.demo_store.is_none() {
            if let Ok(mut s) = Store::in_memory() {
                let _ = seed::run(&mut s, "demo", 35, true);
                self.demo_store = Some(s);
            }
        }
    }

    /// Toggle demo mode: a throwaway in-memory "demo" account seeded with dummy data, so
    /// the screens can be explored before any real snapshots exist.
    fn toggle_demo(&mut self) {
        self.demo = !self.demo;
        if self.demo {
            self.ensure_demo();
        }
        self.selected = 0;
        self.reload();
        self.reset_view();
        self.status = Some(if self.demo {
            "demo mode on · dummy data (press d to exit)".into()
        } else {
            "demo mode off · live data".into()
        });
    }

    fn select_next(&mut self) {
        if !self.accounts.is_empty() {
            self.selected = (self.selected + 1) % self.accounts.len();
            self.rebuild_analysis();
            self.reset_view();
        }
    }

    fn select_prev(&mut self) {
        if !self.accounts.is_empty() {
            self.selected = (self.selected + self.accounts.len() - 1) % self.accounts.len();
            self.rebuild_analysis();
            self.reset_view();
        }
    }

    /// Fetch a live usage sample via claude-switcher and record it, then reload.
    fn snapshot(&mut self) {
        if self.demo {
            self.status = Some("demo mode · snapshot disabled (press d to exit)".into());
            return;
        }
        match usage::fetch(&self.switcher).and_then(|acc| self.store.record(&acc)) {
            Ok(n) => {
                self.last_updated = Local::now();
                self.reload();
                self.status = Some(format!("snapshot · recorded {n} window sample(s)"));
            }
            Err(e) => self.status = Some(format!("snapshot failed: {e}")),
        }
    }
}

/// Entry point from `main` when no subcommand is given.
pub fn run(store: Store, switcher: String, weeks: usize) -> Result<()> {
    let mut app = App::new(store, switcher, weeks);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                handle_key(app, key);
            }
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }
    // Any keypress clears a lingering status message; an action below may set a new one.
    app.status = None;
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        // Esc backs out a level: zoom → block selection → day nav → quit.
        KeyCode::Esc => {
            if app.detail {
                app.detail = false;
            } else if app.block_cursor.is_some() {
                app.block_cursor = None;
            } else {
                app.should_quit = true;
            }
        }
        // ↑/↓ cycle the 5-hour session blocks overlaid on the line.
        KeyCode::Up | KeyCode::Char('k') => app.block_next(),
        KeyCode::Down | KeyCode::Char('j') => app.block_prev(),
        // ←/→ select the day (and drop any block selection).
        KeyCode::Left | KeyCode::Char('h') => {
            app.cursor_left();
            app.block_cursor = None;
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.cursor_right();
            app.block_cursor = None;
        }
        // Tab cycles accounts (only interesting with more than one).
        KeyCode::Tab => app.select_next(),
        KeyCode::BackTab => app.select_prev(),
        KeyCode::Enter => app.detail = true,
        // Jump back to today (current week, cursor on today).
        KeyCode::Char('t') | KeyCode::Char('T') => {
            app.reset_view();
            app.status = Some("jumped to today".into());
        }
        KeyCode::Char('g') | KeyCode::Char('G') => app.viz = app.viz.toggle(),
        KeyCode::Char('d') | KeyCode::Char('D') => app.toggle_demo(),
        KeyCode::Char('s') | KeyCode::Char('S') => app.snapshot(),
        KeyCode::Char('r') | KeyCode::Char('R') => app.reload(),
        _ => {}
    }
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn draw(f: &mut Frame, app: &App) {
    // The tab bar only earns its line when there's more than one panel to switch.
    let show_tabs = TABS.len() > 1;
    let mut constraints = vec![Constraint::Length(1)]; // title
    if show_tabs {
        constraints.push(Constraint::Length(1)); // tab bar
    }
    constraints.push(Constraint::Min(0)); // panel body
    constraints.push(Constraint::Length(1)); // footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.area());

    let mut i = 0;
    draw_header(f, chunks[i], app);
    i += 1;
    if show_tabs {
        draw_tabs(f, chunks[i], app);
        i += 1;
    }
    draw_panel(f, chunks[i], app);
    draw_footer(f, chunks[i + 1], app);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let brand = " claude-observer";
    let sub = " · usage patterns";
    let badge = if app.demo { "   DEMO · dummy data" } else { "" };
    let updated = format!("updated {} ", app.last_updated.format("%-I:%M %p"));
    let used =
        brand.chars().count() + sub.chars().count() + badge.chars().count() + updated.chars().count();
    let gap = (area.width as usize).saturating_sub(used);
    let mut spans = vec![
        Span::styled(brand, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(sub, dim()),
    ];
    if app.demo {
        spans.push(Span::styled(badge, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    }
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(updated, dim()));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The tab bar: the panels rendered as buttons, the active one filled with the accent
/// so it's obvious what ←/→ (or tab) is switching between.
fn draw_tabs(f: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<String> = TABS.iter().map(|t| format!(" {} ", t.label())).collect();
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .style(dim())
        .highlight_style(Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD))
        .divider(" ");
    // Indent one column so the buttons line up under the title's leading space.
    let inner = Rect { x: area.x + 1, width: area.width.saturating_sub(1), ..area };
    f.render_widget(tabs, inner);
}

fn draw_panel(f: &mut Frame, area: Rect, app: &App) {
    if app.accounts.is_empty() {
        let block = Block::default().borders(Borders::ALL).title(" no data yet ");
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(""),
                Line::from(Span::styled("  No samples yet.", Style::default().add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(Span::styled("  Press  s  to snapshot now, or run `claude-observer snapshot` on a timer.", dim())),
            ])),
            inner,
        );
        return;
    }

    let acct = &app.accounts[app.selected];
    let title = if app.accounts.len() > 1 {
        format!(" {acct}  ·  account {} of {} ", app.selected + 1, app.accounts.len())
    } else {
        format!(" {acct} ")
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    match (app.tab, app.analysis.as_ref()) {
        (_, None) => f.render_widget(
            Paragraph::new(Span::styled("  no samples for this account yet", dim())),
            inner,
        ),
        (Tab::Trends, Some(a)) => draw_trends(f, inner, a),
        (Tab::Week, Some(a)) => {
            let start = a.current_week_start() + Duration::days(app.week_offset * 7);
            let date = start + Duration::days(app.day_cursor as i64);
            let sel_window = app.selected_window();
            match (app.viz, app.detail) {
                // Zoomed: a selected session's breakdown wins over the day zoom.
                (_, true) => match sel_window {
                    Some(wi) => draw_block_breakdown(f, inner, &a.five_windows[wi]),
                    None if app.viz == WeekViz::Calendar => draw_day_detail(f, inner, &a.day_view(date)),
                    None => draw_day_burnup(f, inner, a, date),
                },
                (WeekViz::Sessions, false) => {
                    draw_burnup(f, inner, a, start, app.day_cursor, &app.week_window_idxs(), sel_window)
                }
                (WeekViz::Calendar, false) => draw_week(f, inner, a, start, app.day_cursor),
            }
        }
    }
}

/// An accent-bold section heading.
fn section(title: &str) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(
        title.to_string(),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )))
}

fn caption(text: String) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(text, dim())))
}

fn threshold_color(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::Red
    } else if pct >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn verdict(pace: f64) -> &'static str {
    if pace < 85.0 {
        "under"
    } else if pace <= 108.0 {
        "on pace"
    } else {
        "over"
    }
}

const DOW: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// A slim right-hand y-axis: the shared max at the top, 0 at the bottom (just above the
/// chart's own label row), so bar heights have a readable scale.
fn draw_y_axis(f: &mut Frame, area: Rect, ymax: u64) {
    let h = area.height as usize;
    let mut lines = vec![Line::from(""); h.max(1)];
    lines[0] = Line::from(Span::styled(format!(" {ymax:>3}%"), dim()));
    if h >= 2 {
        lines[h - 2] = Line::from(Span::styled("   0%", dim()));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// A right-hand y-axis for the 0-100% chart views: top label at the first row, bottom
/// label just above the chart's x-axis rows, mid in between.
fn draw_y_axis_right(f: &mut Frame, area: Rect, top: &str, mid: &str, bot: &str) {
    let h = area.height as usize;
    if h == 0 {
        return;
    }
    let bottom = h.saturating_sub(3); // last plot row, above the x-axis line + labels
    let mut lines = vec![Line::from(""); h];
    lines[0] = Line::from(Span::styled(format!(" {top}"), dim()));
    if bottom >= 1 {
        lines[bottom / 2] = Line::from(Span::styled(format!(" {mid}"), dim()));
        lines[bottom] = Line::from(Span::styled(format!(" {bot}"), dim()));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The week view: a gauge for the week's total allotment use, then a calendar row of
/// per-day clusters (four 6-hour blocks each, Sun→Sat). All bars share one scale (shown
/// on the y-axis) so they compare day-to-day. `cursor` is the highlighted day.
fn draw_week(f: &mut Frame, area: Rect, a: &Analysis, start: NaiveDate, cursor: usize) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // gauge
            Constraint::Length(1), // caption
            Constraint::Length(1), // spacer
            Constraint::Length(1), // heading
            Constraint::Min(0),    // per-day grid
        ])
        .split(area);

    let week = a.week_view(start);
    let end = start + Duration::days(6);
    let is_current = start == a.current_week_start();
    let total: f64 = week.iter().map(|d| d.total).sum();

    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(threshold_color(total)))
            .ratio((total / 100.0).clamp(0.0, 1.0))
            .label(format!("{total:.0}% of weekly allotment used")),
        rows[0],
    );
    let cap = if is_current {
        let resets = a
            .current_resets
            .map(|r| r.with_timezone(&Local).format("%a %-l%p").to_string())
            .unwrap_or_else(|| "—".into());
        let pace = a
            .current_pace
            .map(|p| format!("pace ~{p:.0}% ({})", verdict(p)))
            .unwrap_or_else(|| "pace —".into());
        format!("this week   ·   resets {resets}   ·   {pace}")
    } else {
        "past week".to_string()
    };
    f.render_widget(caption(cap), rows[1]);

    f.render_widget(
        section(&format!("{} – {}  ·  6-hour blocks", start.format("%b %-d"), end.format("%b %-d"))),
        rows[3],
    );
    draw_week_grid(f, rows[4], &week, cursor);
}

/// The per-day clusters. One shared `ymax` across every 6-hour block in the week keeps
/// heights comparable; the cursor day is bright with a reversed label, today is bold,
/// future days are greyed.
fn draw_week_grid(f: &mut Frame, area: Rect, week: &[DayView], cursor: usize) {
    let ymax = week.iter().flat_map(|d| d.blocks).fold(0.0, f64::max).max(1.0).round() as u64;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(5)])
        .split(area);
    draw_y_axis(f, cols[1], ymax);

    let mut chart = BarChart::default().bar_width(2).bar_gap(0).group_gap(2).max(ymax);
    for (i, d) in week.iter().enumerate() {
        let bar_style = if i == cursor {
            Style::default().fg(ACCENT)
        } else if d.is_future {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(ACCENT).add_modifier(Modifier::DIM)
        };
        let label_style = if i == cursor {
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else if d.is_today {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        let bars: Vec<Bar> = (0..4)
            .map(|b| {
                Bar::default()
                    .value(d.blocks[b].round() as u64)
                    .text_value(String::new())
                    .style(bar_style)
            })
            .collect();
        chart = chart.data(
            BarGroup::default()
                .label(Line::from(Span::styled(d.date.format("%a %-d").to_string(), label_style)))
                .bars(&bars),
        );
    }
    f.render_widget(chart, cols[0]);
}

/// The day-detail view: a gauge for the day's slice of the weekly allotment, its peak
/// 5-hour window, and an hourly chart of when tokens were burned.
fn draw_day_detail(f: &mut Frame, area: Rect, d: &DayView) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // heading
            Constraint::Length(1), // day gauge
            Constraint::Length(1), // caption
            Constraint::Length(1), // spacer
            Constraint::Length(1), // by-hour heading
            Constraint::Min(0),    // hourly chart
        ])
        .split(area);

    let tag = if d.is_today {
        "  ·  today"
    } else if d.is_future {
        "  ·  upcoming"
    } else {
        ""
    };
    f.render_widget(section(&format!("{}{tag}", d.date.format("%A, %b %-d"))), rows[0]);
    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(threshold_color(d.total)))
            .ratio((d.total / 100.0).clamp(0.0, 1.0))
            .label(format!("{:.0}% of weekly allotment used this day", d.total)),
        rows[1],
    );
    f.render_widget(caption(format!("peak 5-hour window {:.0}%", d.peak_5h)), rows[2]);

    f.render_widget(section("BY HOUR  ·  when tokens were burned (local)"), rows[4]);
    draw_hours(f, rows[5], &d.hours);
}

/// A 24-bar hourly chart with a shared y-axis; labels at 12a / 6a / 12p / 6p.
fn draw_hours(f: &mut Frame, area: Rect, hours: &[f64; 24]) {
    let ymax = hours.iter().cloned().fold(0.0, f64::max).max(1.0).round() as u64;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(5)])
        .split(area);
    draw_y_axis(f, cols[1], ymax);

    let bars: Vec<Bar> = (0..24)
        .map(|h| {
            let label = match h {
                0 => "12a",
                6 => "6a",
                12 => "12p",
                18 => "6p",
                _ => "",
            };
            Bar::default()
                .value(hours[h].round() as u64)
                .label(Line::from(label))
                .text_value(String::new())
                .style(Style::default().fg(ACCENT))
        })
        .collect();
    f.render_widget(
        BarChart::default().data(BarGroup::default().bars(&bars)).bar_width(3).bar_gap(0).max(ymax),
        cols[0],
    );
}

const WEEKDAYS_SUN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Minimum bar height (% pts) so a no-usage session still shows as a short bar, making
/// idle stretches read as a flat floor rather than gaps.
const MIN_BAR: f64 = 1.5;

/// Even day labels for a [0,7] x-axis. ratatui edge-aligns the first/last label, so
/// padding both ends with an empty entry leaves Sun..Sat on evenly-spaced interior ticks.
/// Days after `today` (future dates in the current week) are greyed.
fn week_x_labels(start: NaiveDate, today: NaiveDate) -> Vec<Line<'static>> {
    let mut labels = vec![Line::from("")];
    for (i, name) in WEEKDAYS_SUN.iter().enumerate() {
        let d = start + Duration::days(i as i64);
        let style = if d > today {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        labels.push(Line::from(Span::styled(*name, style)));
    }
    labels.push(Line::from(""));
    labels
}

/// Append a *floating* filled rectangle `[x0,x1]×[y0,y1]` as a dense point grid, for a
/// Scatter dataset (Braille) — a bar that doesn't have to start at the 0 baseline. The
/// grid density scales with how much of the axes (`xspan`/`yspan`) the box covers, so it
/// stays solid whether the box is a thin sliver or a big block.
fn push_rect(dst: &mut Vec<(f64, f64)>, x0: f64, x1: f64, y0: f64, y1: f64, xspan: f64, yspan: f64) {
    let nx = (((x1 - x0).abs() / xspan) * 300.0).clamp(6.0, 200.0) as usize;
    let ny = (((y1 - y0).abs() / yspan) * 180.0).clamp(4.0, 160.0) as usize;
    for i in 0..=nx {
        let x = x0 + (x1 - x0) * i as f64 / nx as f64;
        for j in 0..=ny {
            dst.push((x, y0 + (y1 - y0) * j as f64 / ny as f64));
        }
    }
}

/// The week view: a burn-up drawn as floating bars — one per 5-hour session, stacked so
/// each bar spans that period's slice of the cumulative (bottom = running total before,
/// top = after). The tops trace the burn-up toward 100%; thick bars = heavy sessions,
/// thin = light. The cursor day's bars are highlighted, a selected session (↑/↓) is
/// brightest, future days are greyed.
fn draw_burnup(
    f: &mut Frame,
    area: Rect,
    a: &Analysis,
    start: NaiveDate,
    cursor: usize,
    widxs: &[usize],
    sel: Option<usize>,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    let end = start + Duration::days(6);
    let is_current = start == a.current_week_start();
    f.render_widget(
        section(&format!("5-HOUR SESSIONS  ·  {} – {}", start.format("%b %-d"), end.format("%b %-d"))),
        rows[0],
    );
    // Caption: a selected session's details win; otherwise this-week / past-week status.
    let cap = if let Some(wi) = sel {
        let w = &a.five_windows[wi];
        let k = widxs.iter().position(|&x| x == wi).unwrap_or(0);
        format!(
            "session {}/{}  ·  {} – {}  ·  peak {:.0}%{}",
            k + 1,
            widxs.len(),
            w.start.format("%a %-l%p"),
            w.end.format("%-l%p"),
            w.peak,
            if w.peak >= 99.5 { "  (exhausted)" } else { "" }
        )
    } else if is_current {
        let resets = a
            .current_resets
            .map(|r| r.with_timezone(&Local).format("%a %-l%p").to_string())
            .unwrap_or_else(|| "—".into());
        let pace = a
            .current_pace
            .map(|p| format!("pace ~{p:.0}% ({})", verdict(p)))
            .unwrap_or_else(|| "pace —".into());
        format!("this week  ·  resets {resets}  ·  {pace}")
    } else {
        "past week".to_string()
    };
    f.render_widget(caption(cap), rows[1]);

    // Session-window bars, split by highlight: selected session, the cursor day's other
    // sessions, and the rest of the week.
    // Running cumulative (the burn-up curve) at hourly resolution.
    let wh = a.week_alloc_hours(start);
    let mut cum = vec![0.0f64; wh.len() + 1];
    for i in 0..wh.len() {
        cum[i + 1] = cum[i] + wh[i];
    }
    let cum_at = |hour: usize| cum[hour.min(wh.len())];

    // Each session is a floating box: x = its 5 hours (small gap so bars read discretely),
    // y = [cumulative before, cumulative after]. Idle sessions get a thin visible slab.
    let day_date = start + Duration::days(cursor as i64);
    let (mut rest, mut day, mut chosen) = (Vec::new(), Vec::new(), Vec::new());
    for &wi in widxs {
        let w = &a.five_windows[wi];
        let hs = ((w.start.date_naive() - start).num_days() * 24 + w.start.hour() as i64)
            .clamp(0, wh.len() as i64) as usize;
        let he = (hs + 5).min(wh.len());
        let (y0, y1) = (cum_at(hs), cum_at(he).max(cum_at(hs) + MIN_BAR));
        let (x0, x1) = (hs as f64 / 24.0, he as f64 / 24.0);
        let gap = (x1 - x0) * 0.12;
        let dst = if Some(wi) == sel {
            &mut chosen
        } else if w.start.date_naive() == day_date {
            &mut day
        } else {
            &mut rest
        };
        push_rect(dst, x0 + gap, x1 - gap, y0, y1, 7.0, 100.0);
    }

    let datasets = vec![
        Dataset::default().marker(Marker::Braille).graph_type(GraphType::Scatter).style(Style::default().fg(Color::DarkGray)).data(&rest),
        Dataset::default().marker(Marker::Braille).graph_type(GraphType::Scatter).style(Style::default().fg(ACCENT)).data(&day),
        Dataset::default().marker(Marker::Braille).graph_type(GraphType::Scatter).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)).data(&chosen),
    ];
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(6)])
        .split(rows[2]);
    let chart = Chart::new(datasets)
        .x_axis(Axis::default().style(dim()).bounds([0.0, 7.0]).labels(week_x_labels(start, a.today_date)))
        .y_axis(Axis::default().style(dim()).bounds([0.0, 100.0]).labels(Vec::<Line>::new()));
    f.render_widget(chart, cols[0]);
    draw_y_axis_right(f, cols[1], "100%", "50%", "0%");
}

/// Per-session breakdown: how utilization ramped across one 5-hour window.
fn draw_block_breakdown(f: &mut Frame, area: Rect, w: &FiveWindow) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    f.render_widget(
        section(&format!("5-HOUR SESSION  ·  {} – {}", w.start.format("%a %-l%p"), w.end.format("%-l%p"))),
        rows[0],
    );
    let ex = if w.peak >= 99.5 { "  ·  fully exhausted" } else { "" };
    f.render_widget(caption(format!("peak {:.0}%{ex}", w.peak)), rows[1]);

    let datasets = vec![Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(ACCENT))
        .data(&w.points)];
    let x_labels: Vec<Line> = ["0h", "1h", "2h", "3h", "4h", "5h"].iter().map(|s| Line::from(*s)).collect();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(6)])
        .split(rows[2]);
    let chart = Chart::new(datasets)
        .x_axis(Axis::default().style(dim()).bounds([0.0, 5.0]).labels(x_labels))
        .y_axis(Axis::default().style(dim()).bounds([0.0, 100.0]).labels(Vec::<Line>::new()));
    f.render_widget(chart, cols[0]);
    draw_y_axis_right(f, cols[1], "100%", "50%", "0%");
}

/// Day zoom: the day's burn-up as floating bars — one per 5-hour session, stacked up the
/// day's cumulative, over a 24-hour x-axis (y scaled to the day's own total).
fn draw_day_burnup(f: &mut Frame, area: Rect, a: &Analysis, date: NaiveDate) {
    let d = a.day_view(date);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    let tag = if d.is_today {
        "  ·  today"
    } else if d.is_future {
        "  ·  upcoming"
    } else {
        ""
    };
    f.render_widget(
        section(&format!("{}{tag}  ·  5-hour sessions", d.date.format("%A, %b %-d"))),
        rows[0],
    );
    f.render_widget(
        caption(format!(
            "consumed {:.0}% of allotment  ·  peak 5-hour window {:.0}%",
            d.total, d.peak_5h
        )),
        rows[1],
    );

    // Cumulative within the day (the day's burn-up), hourly.
    let dh = a.day_alloc(date);
    let mut cum = [0.0f64; 25];
    for h in 0..24 {
        cum[h + 1] = cum[h] + dh[h];
    }
    let ymax = cum[24].max(2.0 * MIN_BAR);

    // Each session overlapping the day as a floating box, stacked up the cumulative.
    let hour_of = |t: DateTime<Local>| {
        (t.date_naive() - date).num_days() as f64 * 24.0
            + t.time().num_seconds_from_midnight() as f64 / 3600.0
    };
    let mut bars = Vec::new();
    for w in &a.five_windows {
        let (sh, eh) = (hour_of(w.start), hour_of(w.end));
        if eh <= 0.0 || sh >= 24.0 {
            continue;
        }
        let hs = sh.max(0.0).floor() as usize;
        let he = (eh.min(24.0).ceil() as usize).min(24);
        let (y0, y1) = (cum[hs], cum[he].max(cum[hs] + MIN_BAR));
        let gap = (he - hs) as f64 * 0.12;
        push_rect(&mut bars, hs as f64 + gap, he as f64 - gap, y0, y1, 24.0, ymax);
    }
    let datasets = vec![Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Scatter)
        .style(Style::default().fg(ACCENT))
        .data(&bars)];
    let x_labels: Vec<Line> = ["12a", "6a", "12p", "6p", "12a"].iter().map(|s| Line::from(*s)).collect();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(6)])
        .split(rows[2]);
    let chart = Chart::new(datasets)
        .x_axis(Axis::default().style(dim()).bounds([0.0, 24.0]).labels(x_labels))
        .y_axis(Axis::default().style(dim()).bounds([0.0, ymax]).labels(Vec::<Line>::new()));
    f.render_widget(chart, cols[0]);
    draw_y_axis_right(f, cols[1], &format!("{ymax:.0}%"), &format!("{:.0}%", ymax / 2.0), "0%");
}

fn draw_trends(f: &mut Frame, area: Rect, a: &Analysis) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // weekly heading
            Constraint::Min(5),    // weekly bar chart
            Constraint::Length(1), // day-of-week heading
            Constraint::Min(5),    // day-of-week bar chart
            Constraint::Length(1), // weekday/weekend caption
            Constraint::Length(1), // monthly heading
            Constraint::Length(4), // monthly bar chart
        ])
        .split(area);

    // WEEKLY — % of allotment used, coloured by how close to the cap; free = 100 − used.
    f.render_widget(section("WEEKLY  ·  % of allotment used  (headroom to 100%)"), rows[0]);
    let wbars: Vec<Bar> = a
        .weeks
        .iter()
        .map(|w| {
            Bar::default()
                .value(w.used.round() as u64)
                .label(Line::from(w.label.rsplit(' ').next().unwrap_or("").to_string()))
                .text_value(format!("{:.0}", w.used))
                .style(Style::default().fg(threshold_color(w.used)))
        })
        .collect();
    f.render_widget(
        BarChart::default().data(BarGroup::default().bars(&wbars)).bar_width(4).bar_gap(1).max(100),
        rows[1],
    );

    // DAY OF WEEK — avg daily activity, scaled to its own peak so the shape reads.
    f.render_widget(section("DAY OF WEEK  ·  avg daily activity"), rows[2]);
    let dmax = a.dow.iter().cloned().fold(0.0, f64::max).max(1.0).round() as u64;
    let dbars: Vec<Bar> = (0..7)
        .map(|i| {
            Bar::default()
                .value(a.dow[i].round() as u64)
                .label(Line::from(DOW[i]))
                .text_value(format!("{:.0}", a.dow[i]))
                .style(Style::default().fg(ACCENT))
        })
        .collect();
    f.render_widget(
        BarChart::default().data(BarGroup::default().bars(&dbars)).bar_width(3).bar_gap(1).max(dmax),
        rows[3],
    );

    let avg = if a.weeks.is_empty() {
        0.0
    } else {
        a.weeks.iter().map(|w| w.used).sum::<f64>() / a.weeks.len() as f64
    };
    f.render_widget(
        caption(format!(
            "weekday {:.0}%   ·   weekend {:.0}%   ·   ~{:.0}% of allotment typically free",
            a.weekday_avg,
            a.weekend_avg,
            (100.0 - avg).max(0.0)
        )),
        rows[4],
    );

    f.render_widget(section("MONTHLY  ·  avg weekly used"), rows[5]);
    let mbars: Vec<Bar> = a
        .months
        .iter()
        .map(|(l, v)| {
            Bar::default()
                .value(v.round() as u64)
                .label(Line::from(l.clone()))
                .text_value(format!("{v:.0}"))
                .style(Style::default().fg(ACCENT))
        })
        .collect();
    f.render_widget(
        BarChart::default().data(BarGroup::default().bars(&mbars)).bar_width(5).bar_gap(2).max(100),
        rows[6],
    );
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let line = match &app.status {
        Some(s) => Line::from(Span::styled(format!(" {s}"), Style::default().fg(ACCENT))),
        None => {
            let account = if app.accounts.len() > 1 { "tab acct · " } else { "" };
            let nav = if app.detail {
                "esc back · ".to_string()
            } else {
                format!("←→ day · ↑↓ session · enter zoom · t today · g {} · ", app.viz.toggle().label())
            };
            let demo = if app.demo { "d live · " } else { "d demo · " };
            let keys = format!(" {nav}{account}{demo}s snap · q quit");
            Line::from(Span::styled(keys, dim()))
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;

    /// Flatten a rendered TestBackend buffer into per-row strings.
    fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buf = terminal.backend().buffer();
        let w = buf.area.width as usize;
        buf.content()
            .chunks(w)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect()
    }

    /// A seeded store on a throwaway db, so the TUI has real accounts + patterns.
    fn seeded_app() -> App {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        crate::seed::run(&mut store, "paul-nhost", 14, true).unwrap();
        crate::seed::run(&mut store, "work", 14, true).unwrap();
        App::new(store, "claude-switcher".into(), 8)
    }

    /// An app over an empty store — the fresh-install case (no snapshots yet).
    fn empty_app() -> App {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        App::new(store, "claude-switcher".into(), 8)
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn renders_burnup_by_default_and_g_toggles_bars() {
        let mut app = seeded_app();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let all = rows(&terminal).join("\n");

        assert!(all.contains("claude-observer"), "header brand present");
        assert!(!all.contains("Trends"), "trends tab is parked");
        // Default view is the burn-up line, with Sun–Sat on the x-axis.
        assert!(all.contains("5-HOUR SESSIONS"), "sessions bars are the default week view");
        assert!(all.contains("Sun") && all.contains("Sat"), "x-axis spans the week");
        assert!(all.contains("paul-nhost") && all.contains("account 1 of 2"), "account shown in title");
        assert!(all.contains("enter zoom"), "footer hotkeys present");

        // g flips to the bar calendar.
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.viz, WeekViz::Calendar);
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(rows(&terminal).join("\n").contains("6-hour blocks"), "g shows the bar calendar");
    }

    #[test]
    fn tab_moves_account() {
        let mut app = seeded_app();
        assert_eq!(app.selected, 0);
        assert_eq!(app.tab, Tab::Week);

        // Tab cycles the account; the title reflects the new one.
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.selected, 1, "tab moves to the next account");
        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(rows(&terminal).join("\n").contains("work"), "title shows the selected account");
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.selected, 0, "shift-tab moves back");
    }

    #[test]
    fn up_selects_session_and_enter_zooms_breakdown() {
        let mut app = seeded_app();
        assert!(app.block_cursor.is_none());

        press(&mut app, KeyCode::Up);
        assert_eq!(app.block_cursor, Some(0), "up selects the first 5-hour session");
        press(&mut app, KeyCode::Enter);
        assert!(app.detail, "enter zooms");

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(rows(&terminal).join("\n").contains("5-HOUR SESSION"), "enter on a session shows its breakdown");

        // esc backs out a level at a time: zoom → session selection → day nav.
        press(&mut app, KeyCode::Esc);
        assert!(!app.detail && app.block_cursor.is_some(), "esc leaves the zoom, keeps the session");
        press(&mut app, KeyCode::Esc);
        assert!(app.block_cursor.is_none(), "esc clears the session selection");

        // ←/→ also drop a session selection (back to day nav).
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Left);
        assert!(app.block_cursor.is_none(), "left drops the session selection");
    }

    #[test]
    fn enter_opens_day_detail_and_esc_returns() {
        let mut app = seeded_app();
        assert!(!app.detail);

        // ←/→ move the day cursor; enter drills into that day.
        let start = app.day_cursor;
        press(&mut app, KeyCode::Left);
        assert_eq!(app.day_cursor, start.saturating_sub(1), "left moves the day cursor");
        press(&mut app, KeyCode::Enter);
        assert!(app.detail, "enter opens the day detail");

        let mut terminal = Terminal::new(TestBackend::new(90, 22)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let all = rows(&terminal).join("\n");
        assert!(all.contains("5-hour sessions"), "detail zooms into the day's sessions");
        assert!(all.contains("consumed") && all.contains("of allotment"), "detail shows the day's stats");
        assert!(all.contains("esc back"), "footer offers a way back");

        // Esc returns to the week view (does not quit).
        press(&mut app, KeyCode::Esc);
        assert!(!app.detail && !app.should_quit, "esc backs out of detail");
        // A second esc from the week view quits.
        press(&mut app, KeyCode::Esc);
        assert!(app.should_quit);
    }

    #[test]
    fn d_toggles_demo_mode() {
        let mut app = empty_app();
        assert!(app.accounts.is_empty(), "no real accounts on a fresh install");

        press(&mut app, KeyCode::Char('d'));
        assert!(app.demo, "d turns demo mode on");
        assert!(!app.accounts.is_empty(), "demo provides a preview account");
        assert!(app.analysis.is_some(), "the demo account has dummy data");

        let mut terminal = Terminal::new(TestBackend::new(90, 22)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let all = rows(&terminal).join("\n");
        assert!(all.contains("DEMO"), "the demo badge is shown");
        assert!(all.contains("5-HOUR SESSIONS"), "the week view has data to render");

        press(&mut app, KeyCode::Char('d'));
        assert!(!app.demo && app.accounts.is_empty(), "toggling off returns to live data");
    }

    #[test]
    fn left_at_week_start_wraps_to_previous_week() {
        let mut app = seeded_app();
        assert_eq!(app.week_offset, 0);
        let presses = app.day_cursor + 1; // walk to Sunday, then one past the left edge
        for _ in 0..presses {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.week_offset, -1, "left at Sunday goes to the previous week");
        assert_eq!(app.day_cursor, 6, "landing on that week's Saturday");
    }

    #[test]
    fn cannot_scroll_into_the_future() {
        let mut app = seeded_app();
        let today = app.today_index();
        assert_eq!(app.day_cursor, today, "cursor starts on today");
        for _ in 0..8 {
            press(&mut app, KeyCode::Right);
        }
        assert_eq!(app.week_offset, 0, "never scrolls into a future week");
        assert_eq!(app.day_cursor, today, "cursor stops at today");
    }

    #[test]
    fn q_quits() {
        let mut app = seeded_app();
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn t_jumps_to_today() {
        let mut app = seeded_app();
        // Wander off: back several weeks, select a session, open the zoom.
        for _ in 0..10 {
            press(&mut app, KeyCode::Left);
        }
        assert!(app.week_offset < 0, "navigated into a past week");
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.week_offset, 0, "t returns to the current week");
        assert_eq!(app.day_cursor, app.today_index(), "t puts the cursor on today");
        assert!(app.block_cursor.is_none() && !app.detail, "t clears any session/zoom");
    }





}
