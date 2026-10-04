---
keyword: Mac Menu Bar Pomodoro Indicator
aliases:
  - "mac pom"
---

The Hammerspoon status item in the macOS menu bar on Bryan's MacBook that shows the
current Pomodoro at a glance. It is a read-only consumer of
`bob pomodoro --show-stale`: it polls every 15 seconds, on wake and unlock, from its
Refresh menu item, and once when the countdown crosses zero; it ticks the countdown
locally in between and never writes the vault. A running session reads
`THEME (50m) · 🍅 12:34`, where THEME is the Pomodoro's ` — NAME` (`UNTITLED` when
unnamed). Once overdue, a `→ HH:MM` stop time joins the title and the `+MM:SS` count
turns red; from ten minutes overdue the count becomes an `OVERDUE` badge flashing red
at 1 Hz, and `--show-stale` holds it there rather than letting it read as idle. Empty
output shows a green `NO POMODORO` with no tomato, which flashes as a green pill for
its first minute on screen and then for one minute in every ten, restarting that
cycle whenever the label reappears or the Mac wakes or unlocks. A failed or
unparsable run hides the item. It is neither the `bob pomodoro tmux` status line nor
Bob Mac Capture's menu-bar item. It lives in the linked `chezmoi` repo under
`home/dot_hammerspoon/` (`init.lua` runtime, `pomodoro_countdown.lua` presentation
policy); that repo's README "Pomodoro menu bar" section is its spec, so coordinate
`bob pomodoro` output changes with it.
