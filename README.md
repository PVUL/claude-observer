# claude-observer

Monitor Claude account usage — the **5-hour** and **weekly** rolling windows — so no
allotment goes to waste. Shows how much of each window you've used, when it resets, and
your **pace** (are you on track to use ~100% before the reset, under-utilizing, or about
to hit the limit early?). Plays nice with [claude-switcher](https://github.com/PVUL/claude-switcher);
it can also stand alone.

## Why

On a Claude subscription the value is capped per rolling window, not per token — so the
goal is to use **100% of each 7-day allotment** (nothing wasted) while not getting
throttled mid-task in a 5-hour window. claude-observer makes those windows visible and,
over time, helps **plan work** to fill under-utilized capacity.

## Status — v0.1 (built)

- **`claude-observer status`** — per-account dashboard: 5h / 7d (and 7d-Opus) bars,
  % used / left, reset countdown, and a **pace projection** with a verdict
  (under-utilizing / on pace / over pace).
- **`claude-observer status --json`** — machine-readable, for the pi extension & scripts.
- **`claude-observer snapshot`** — record a sample into a local SQLite trend store
  (`$XDG_DATA_HOME/claude-observer/observer.db`). Meant to run on a timer.
- **`claude-observer history <account> [--window five_hour|seven_day]`** — recent samples.

Data source: `claude-switcher usage --json` (reuses its account auth). A standalone mode
that queries the Anthropic usage endpoint directly is on the roadmap.

## Roadmap

1. **Interactive TUI** (`ratatui`) — live dashboard: per-account windows, pace, and a
   trend sparkline from the snapshot store.
2. **pi extension** — ask pi "how much of my allotment have I used?" and get the 5h/7d
   picture + pace, in-session. (TS package, modeled on `pi-claude-switcher`.)
3. **Snapshot timer** — periodic `snapshot` (systemd) to build real trend history.
4. **Warmer** (opt-in, scheduled) — send minimal dummy tokens to trigger the 5-hour
   window early, so a big session starts against a fresh/aligned window. Never automatic.
5. **Per-ask token tracking** — attribute token spend (from session transcripts:
   input/output/cache per turn) to ask types, to learn typical cost by complexity.
6. **Work queue** — when a window is under-utilized, pick queued tasks to run that fit the
   remaining budget before reset, sized by the learned per-ask estimates.

## Build

Rust. `cargo build` (needs a C compiler for the bundled SQLite). On this box:
`nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#gcc -c cargo build`.
Will be packaged into pi-server (like claude-switcher) once past v0.1.
