# Decay ladder

This page records the live verification of the task-decay ladder. The
JavaScript implementations live in bob-navigation-hotkeys
(`050-decay-planner.js`, `052-decay-ladder-step-planner.js`,
`060-decay-card-modal.js`, `195-rotten-keep.js`,
`530-plugin-freshness-refresh-and-decay.js`, `548-decay-card-writers.js`)
and bob-ledger-tools (`090-freshness-config.js`). Node suites cover the
planner, writers, and Rotten Log grammar. The real-Obsidian walk is
bob-plugins `scripts/obsidian-decay-ladder-e2e.cjs`.

The ladder is WIP (`[/]`) → Next (`[*]`) → Ready (`[ ]`) → P1 → … → P4.
Each rung allows a configured number of due Alt+F keeps (this check uses
2), then the next due keep opens one rung-aware decay card. Enter steps
exactly one rung; Alt+F stays and restarts the count; D drops. Every
keep, stay, step, and drop writes one rung-qualified Rotten Log entry.
WIP keeps log to the Rotten Log. P4 Enter rolls again at P4. Counted
batches skip at-limit targets. Nothing moves on a timer.

## What the e2e drives

Following `scripts/obsidian-frees-marks-e2e.cjs` (isolated scratch vault,
`xvfb-run`, real Obsidian, never the live vault), the decay-ladder e2e
seeds one note with six tasks and a scratch `bob/config.yml`:

```yaml
freshness:
  interval: 1
  pending_interval: 1
  next_interval: 1
  decay:
    pending_keeps: 2
    next_keeps: 2
    ready_keeps: 2
    enter: P1
properties:
  - name: priority
    values: priority
    schedules: scheduled
    decay:
      rolls: 2
```

It pins `freshnessTodayText` and `laneReleaseDateText` across simulated
review days, patches `Math.random` to `0` so P-level windows are
deterministic, and drives `refreshTaskFreshness` (no opening Alt+F chord,
so Enter works immediately). Tasks plugin `taskFormat: dataview` so
`[id::]` / `[keeps::]` parse.

Walk, with exact task fields and Rotten Log bodies asserted after each
write:

| Task | Gestures | Expected |
| --- | --- | --- |
| WIP | keep (reason) → keep (blank=🤷) → card → Enter step → Ctrl+Z undo → D drop | `[/]` keeps 1 then 2; step to `[*]` with `🍂 WIP → Next · after 2 keeps`; undo restores; drop writes `🍂 WIP → dropped · after 2 keeps`. No Work Log. |
| Next | keep → keep → card Alt+F stay + reason → keep again | stay clears `[keeps::]` and writes `stayed in Next after 2 keeps`; a later keep counts as 1×. |
| Ready | keep → keep → card → Enter | steps to P1 (`[priority:: high]`), scheduled in the P1 window (3–8 days). |
| P4 | keep → keep → card → Enter | stays `[priority:: lowest]`, rolls a new P4 date, log `🍂 rolled again at P4 · after 2 keeps`. |
| skip-a, skip-b | counted batch (`countExplicit`, `additionalTaskCount: 1`) at `[keeps:: 2]` | no write; notice `2 need a decision`. |

WIP, Ready, and P4 cards are captured in light and dark themes.

## Offline self-test

```text
node scripts/obsidian-decay-ladder-e2e.cjs --self-test
```

Also wired as `scripts/test-obsidian-decay-ladder-e2e.cjs` in bob-plugins
`npm test`. The self-test never launches Obsidian. It checks the scratch
fixture (2 keeps on every rung, `enter: P1`, `rolls: 2`), keep / stay /
step / drop Rotten Log bodies, card-probe title and recommended step
label, skip-notice grammar, P1/P4 windows, and that the driver walks
Alt+F, the card, undo, and counted batch skip.

Verified 2026-11-03: `--self-test` complete on this host.

## Live verification (real-Obsidian e2e)

```text
xvfb-run -a node scripts/obsidian-decay-ladder-e2e.cjs [--out DIR] [-e PATH] [-r REPORT]
```

This host has no Obsidian Electron app (`/snap/bin/obsidian` missing) and
no `xvfb-run`, so the live pass did not run here. Exit 2 from the driver
means unrun, not a product failure. Rerun on a Linux host with Obsidian
and Xvfb, or on the Mac with the installed app:

- every keep, stay, step, drop, and undo row in the table above matches
  the exact task line and Rotten Log body
- WIP / Ready / P4 cards screenshot in light and dark
- counted batch skip writes nothing and shows `2 need a decision`

**Pending (Bryan, in Obsidian)**

- live Linux `xvfb-run` pass and screenshot review against the card
  anatomy (title `Kept 2× in/at …`, recommended Enter row, Stay, Drop)
- Mac smoke: Alt+F keep, card Enter / Alt+F / D, and Ctrl+Z undo on a
  real vault task
- iOS glyph rendering of 🥀 / 🍂 / 🤷
