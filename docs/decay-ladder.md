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

Completed twice on Athena on 2026-11-03 with Obsidian 1.15.0 and Tasks
8.6.0. Each run passed all 48 checks, skipped none, and captured the six
WIP, Ready, and P4 card views in light and dark themes. The checks asserted
the task line and Rotten Log after each keep, stay, step, roll, drop, and
undo; verified the Ready P1 schedule window and P4 re-roll; and confirmed
that a counted batch skip changes neither task.

The runner copied the working bob-plugins tree and the Tasks plugin source
into a fresh Athena `/tmp` directory for each run, then used an isolated
scratch vault. It did not use Athena's repository checkout or the live vault.
The command run inside each copy was:

```text
BOB_E2E_TASKS_PLUGIN_DIR=<isolated Tasks plugin> xvfb-run -a node scripts/obsidian-decay-ladder-e2e.cjs --out <results/run-N> --report <results/report-N.json>
```

The harness now pins both `freshnessTodayText` and `laneReleaseDateText` to
the simulated review day, waits for the Tasks cache and task writes, and
accepts duplicate copies of the same batch notice toast. It dispatches card
answers to the focused control in Obsidian's app document. The e2e asserts
that each decay card's owner document is that same app document.

The screenshot review confirmed the recommended Enter row, rung title,
Stay and Drop actions, and readable light/dark contrast on all three cards.
The second run's WIP and Ready images and P4 dark image were byte-identical
to the first run; P4 light differed by a small capture-level pixel variation
without a visible layout or content change. The configured P-level label is
shown in the decay log, while `[priority::]` continues to store its configured
value.

The reports and six reviewed screenshots are registered as SASE artifacts on
`bob-cli-g9`. The separate Mac smoke and iOS glyph checks remain optional
device checks outside this real-Obsidian acceptance run.
