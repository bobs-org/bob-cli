---
keyword: Mac Menu Bar Pomodoro Indicator
aliases:
  - "mac pom"
---

The Hammerspoon status item in the macOS menu bar on Bryan's MacBook that shows the
current Pomodoro at a glance. It is a read-only consumer of
`bob pomodoro --show-stale --worked`: it re-syncs when today's daily note changes and
polls every 60 seconds (15 seconds without the file watcher), on wake and unlock, from
its Refresh menu item, and once when the countdown crosses zero; it ticks the countdown
locally in between and never writes the vault. A running session reads
`NAME (50m) · 🍅 12:34`, where NAME is the Pomodoro's ` — NAME`. Each known,
non-exempt theme component prefixes NAME with a colored `■` using the hue
reported by `bob themes list -a -f json`; unknown or exempt themes and a failed
catalog call have no swatch. An unnamed session with linked tasks shows its
derived home label (`sase (50m) · 🍅 12:34`);
`UNTITLED` only appears when there is nothing to derive. Once overdue, a `→ HH:MM` stop time joins the title and the `+MM:SS` count
turns red; from ten minutes overdue the count becomes an `OVERDUE` badge flashing red
at 1 Hz, and `--show-stale` holds it there rather than letting it read as idle. Without
a session it shows today's worked total `<H>h<M>m` (bob's `[WORKED <minutes>m]`, closed
Pomodoros only) in green with no tomato, flashing as a green pill on Fibonacci-spaced
10-second steps that append `φ Nm`, re-anchored when the label reappears or the Mac
wakes or unlocks; empty output falls back to `NO POMODORO`. A failed or unparsable run
hides the item. It is neither the `bob pomodoro tmux` status line nor Bob Mac Capture's
menu-bar item. It lives in the linked `chezmoi` repo under `home/dot_hammerspoon/`
(`init.lua` runtime, `pomodoro_countdown.lua` presentation policy); that repo's README
"Pomodoro menu bar" section is its spec, so coordinate `bob pomodoro` output changes
with it.
