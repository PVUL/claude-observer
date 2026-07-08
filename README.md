# claude-observer

The **usage-metrics / long-term-patterns** layer for Claude accounts. claude-observer
continuously records the 5-hour and weekly window utilization and turns that time-series
into **trends and patterns** — how much of each 7-day allotment you actually use vs. leave
on the table, when your windows are typically under-utilized, burn-rate over time, per-
account comparison.

**Scope, deliberately narrow:** this is *not* the live point-in-time gauge.
[claude-switcher](https://github.com/PVUL/claude-switcher)'s TUI already nails that;
claude-observer is the separate **history + analytics** tool that sits alongside it (may
merge later). It has its own interactive TUI, but for *exploring recorded patterns* —
not a real-time meter. It reuses claude-switcher's `usage --json` as its data feed.

## Why

On a Claude subscription the value is capped per rolling window, not per token — so the
goal is to use **100% of each 7-day allotment** (nothing wasted) while not getting
throttled mid-task in a 5-hour window. claude-observer makes those windows visible and,
over time, helps **plan work** to fill under-utilized capacity.

## Status — v0.1 (built)

The collection + inspection primitives are in place:

- **`claude-observer`** (no subcommand) — the interactive TUI. The **Week** view is the
  week's **5-hour session windows as floating bars** — taller = more of that window used
  (full height = fully exhausted; a short floor = idle). The % scale is on the right;
  future days in the current week are greyed. Keys:
  - `←`/`→` — select the day (at the edges, scroll to the previous/next week; never the
    future).
  - `↑`/`↓` — cycle the 5-hour session blocks; the selected one is highlighted.
  - `enter` — **zoom**: on a selected session → its intra-session breakdown; otherwise →
    the day's sessions. `esc` backs out a level at a time.
  - `t` — jump back to today. `g` — switch between the **sessions** bars and the **bar
    calendar** (per-day 6-hour blocks).
  - `tab` — cycle accounts (shown in the panel title). `s` snapshot, `d` toggle a
    **demo** preview account (dummy data, no snapshots needed), `q` quit.
  - (A Trends view — weekly / day-of-week / monthly bar charts — is built but parked.)
- **`claude-observer snapshot`** — record current window utilization into a local SQLite
  time-series (`$XDG_DATA_HOME/claude-observer/observer.db`). Runs on a timer — this is
  the tool's heartbeat; everything else analyzes what it collects.
- **`claude-observer history <account> [--window five_hour|seven_day]`** — raw samples.
- **`claude-observer status [--json]`** — a quick current read (bars, % used/left, reset
  countdown, pace projection). Kept lightweight/scriptable — NOT a live TUI (that's
  claude-switcher's job); mainly a spot-check and the `--json` feed.

Data source: `claude-switcher usage --json`.

## Roadmap — long-term metrics, not a live view

1. **Snapshot collector** — periodic `snapshot` (systemd timer on the box) so a real
   time-series accumulates. Prerequisite for everything below.
2. **Reports / analytics** (`claude-observer report`) — the actual product: weekly
   allotment used vs. wasted, utilization by day-of-week / time-of-day, 5-hour
   under-utilization patterns, burn-rate trends, per-account comparison.
3. **pi extension** — ask pi for the *patterns* ("how much did I leave unused last week?",
   "when am I typically idle?"), surfaced from the history — not a live gauge.
4. **Per-ask token tracking** — attribute token spend (from session transcripts:
   input/output/cache per turn) to ask types, to learn typical cost by complexity.
5. **Warmer** (opt-in, scheduled) — trigger the 5-hour window early before a big session.
6. **Work queue** — fill under-utilized windows with queued tasks sized by learned
   estimates.

_May fold into claude-switcher later; kept separate for now._

## Install

Rust (cargo) + a C compiler (for the bundled SQLite). One command builds it and puts
`claude-observer` on your `PATH`:

```sh
cargo install --path .   # compiles release + installs to ~/.cargo/bin
```

That's it — `claude-observer` is now runnable from anywhere (assuming `~/.cargo/bin` is
on your `PATH`, which rustup sets up by default; if a shell already open says "command
not found", run `rehash`). Re-run the same command to upgrade after pulling.

On the nix box (no global toolchain): prefix with a nix shell —
`nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#gcc -c cargo install --path .`.

For a dev build without installing, use `cargo build --release` and call the binary by
its path (`./target/release/claude-observer …`).

Will be packaged into pi-server (like claude-switcher) once past v0.1.

## Quickstart

No real data yet? Seed dummy samples and look at the screens — this path needs
**nothing external**:

```sh
claude-observer seed --account paul-nhost --reset   # ~35 days of fake samples
claude-observer                                     # interactive TUI (default)
claude-observer preview                             # static text render (for review)
claude-observer report                              # plain-text summary
```

Running with no subcommand launches the interactive TUI; `preview` renders the same
panels as plain framed text (`--screen overview|trends|both`) for scripts and review.

For **real** data you need [claude-switcher](https://github.com/PVUL/claude-switcher)
on your `PATH` — `snapshot` shells out to `claude-switcher usage --json` to read your
accounts. Then:

```sh
claude-observer snapshot                            # record one sample (run on a timer)
claude-observer history paul-nhost --window seven_day
claude-observer preview
```

Run `snapshot` on a schedule (systemd timer / cron / launchd) so a real time-series
accumulates — that's the tool's heartbeat; everything else just analyzes what it collects.

### Config

- **Data store** — SQLite at `$XDG_DATA_HOME/claude-observer/observer.db` (else
  `~/.local/share/claude-observer/observer.db`). Override with `--db <path>` or
  `CLAUDE_OBSERVER_DB`.
- **claude-switcher binary** — defaults to `claude-switcher` (found on `PATH`).
  Override with `--switcher <path>` or `CLAUDE_OBSERVER_SWITCHER`.

Run `claude-observer --help` (or `<cmd> --help`) for all flags.
