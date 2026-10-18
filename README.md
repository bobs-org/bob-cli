# Bob CLI

`bob-cli` installs the `bob` command for the Bob Obsidian vault and Pomodoro
workflow. The preferred interface is `bob <subcommand>`; `bob --help` is the
authoritative command index. Command implementations are native Rust by
default.

Legacy command names still exist as installed binaries for existing tmux,
shell, and automation callers. The Pomodoro and notification shell
implementations remain embedded as a targeted rollback path; see
[Compatibility shims](#compatibility-shims) for the exact mappings and fallback
behavior.

New to Bob? Follow [Getting started](docs/getting-started.md) for vault
settings, a first capture and Pomodoro session, and the commands that write or
sync files. The [guide index](docs/README.md) points to the detailed contracts.

## Contents

- [Installation](#installation)
- [Terms](#terms)
- [Daily workflow](#daily-workflow)
- [Vault layout](#vault-layout)
- [Commands](#commands)
- [Article notes](#article-notes)
- [Capture](#capture)
- [Query](#query)
- [Task maintenance](#task-maintenance)
- [Plan budget](#plan-budget)
- [Task freshness](#task-freshness)
- [Ready cap](#ready-cap)
- [Stats](#stats)
- [Projects](#projects)
- [Plugins](#plugins)
- [Gkeep](#gkeep)
- [Reference library (bob ref)](#reference-library-bob-ref)
- [Nightly maintenance](#nightly-maintenance)
- [Vault sync](#vault-sync)
- [Pomodoro](#pomodoro)
- [Compatibility shims](#compatibility-shims)
- [Runtime dependencies](#runtime-dependencies)
- [Environment](#environment)
- [Migration notes](#migration-notes)
- [Release checklist](#release-checklist)
- [Detailed command contracts](#detailed-command-contracts)

## Installation

Installation requires a current stable Rust toolchain with `cargo`. The default
vault location is `~/bob`; set `BOB_DIR` when the vault lives elsewhere.

With `just` installed, install from this checkout:

```bash
just install
```

That installs `bob` (and its shims) from source into
`${CARGO_INSTALL_ROOT:-${CARGO_HOME:-~/.cargo}}` — overriding cargo's
`install.root` config so the recipe refreshes exactly the binary it
installed — then runs `bob completion install` to install or refresh shell
completion for your shells. Pass shells explicitly (`just install zsh`)
to choose. If completion needs attention — an install that failed, or a
live verification that is `not registered`, `shadowed by …`, or
`bob is bound to …` (an `unverified` probe only warns) — the recipe says
so and exits 1 while leaving the installed binary in place. Never pass `--force` to these
installs: cargo already replaces the same package from any checkout, and
`--force` would only let it overwrite a different crate's `bob`.

### Install everything

Run `just install-all` to pull this checkout with `--ff-only`, install bob-cli,
then pull and deploy the sibling `../bob-plugins` checkout with
`bob plugins sync --no-pull --repo`. On macOS it also pulls and installs the
sibling `../bob-mac-capture` checkout. `just install-all` passes no signing
identity, so Bob Mac Capture resolves the stable "Bob Mac Capture Local" identity
from the login keychain when available and creates it from a GUI Terminal if needed;
an explicit `CODESIGN_IDENTITY` (including `-`) still wins. An ad-hoc install is
reported as a ⚠ warning because its Accessibility grant resets each install. An
install whose signature changed clears Bob Mac Capture's stale Accessibility grant.
From a session whose login keychain is locked (SSH, or a tmux server started over SSH),
the Bob Mac Capture step asks to unlock it before building. When the login keychain
password no longer matches the Mac login password, it names that cause and offers a
one-run unlock with the previous password. The lasting fix is
`security set-keychain-password ~/Library/Keychains/login.keychain-db`. Without a
terminal, either case fails before building with its fix.
When a sibling it would install is missing and the command
runs in a terminal, it asks `[y/N]` whether to clone it over SSH from
`git@github.com:bobs-org/<repo>.git` into that sibling path. It asks before the
bob-cli build starts. bob-plugins is offered only when the Obsidian vault exists,
and bob-mac-capture only on macOS. A declined clone, a non-interactive run, a
missing Obsidian vault, and the macOS-only app on other hosts are skipped. A
failed clone is a failed step. Pull failures warn and use the local checkout. A
failed step makes the command exit 1.

Keep the checkouts together, for example:

```text
~/projects/github/bobs-org/
├── bob-cli/
├── bob-plugins/
└── bob-mac-capture/
```

After a clean run, `just install-all` restarts a running Obsidian only when
`bob plugins sync` copied at least one file into the vault. It previews the
sync with `bob plugins sync --dry-run --format json`. On macOS it gracefully
quits and relaunches Obsidian, and the first run may ask for Automation
permission. Before the macOS quit, `just install-all` posts a Notification
Center banner titled `Restarting Obsidian` with subtitle `bob install-all`.
The banner titled `Obsidian needs a restart` is posted instead when a failed
step or a failed quit/relaunch leaves the new plugins unloaded. macOS files
the banner under Script Editor. The script never asks for notification
permission, and a denied or dropped banner does not change the restart. On
Linux it uses the Obsidian CLI `restart` when that CLI is registered at
`~/.local/bin/obsidian`. A stopped Obsidian is never launched. If a failed
step or a failed restart leaves the new plugins unloaded, the next run retries
the restart. The pending marker lives under
`~/.local/state/bob-cli/install-all/`.

For installation from the Git remote:

```bash
cargo install --git git@github.com:bobs-org/bob-cli.git --locked bob-cli && bob completion install
```

Automation and agents smoke-test an install without replacing an existing
user install:

```bash
just install-smoke
```

### Shell completion

Every `<TAB>` after `bob` is answered live by the `bob` on your `PATH`, so
completion always matches the installed binary: commands, options, and vault
values such as capture routes, tasks, and open Pomodoros. `just install`
keeps the small adapter file current with `bob completion install`, which
never edits your rc files. Inspect the state anytime with `bob completion`
(same as `bob completion status`), and read
[`docs/completion.md`](docs/completion.md) for the full contract.

After installation, verify the vault selection with read-only commands before
running a command that writes or pushes changes:

```bash
export BOB_DIR=/path/to/bob-vault
bob --help
bob capture-targets
bob projects list
```

Priority rolls (`$<N>`), `bob task reroll`, Highlights pre-scan hooks,
`bob gkeep`, `bob plan`, and `bob stats` read `~/.config/bob/config.yml`. Override
that path with `BOB_CONFIG_FILE` or `XDG_CONFIG_HOME`; see
[Environment](#environment).

## Terms

These words show up across every command. They are vault conventions, not
separate `bob` subcommands.

| Term | Meaning |
| --- | --- |
| Vault | The Obsidian folder Bob operates on (`~/bob` by default; `BOB_DIR` to override) |
| Route | A name using ASCII letters, digits, `_`, or `-`, lowercased to select a top-level note; `@groceries` writes `groceries.md` |
| Pomodoro | A checkbox in the daily note's `Pomodoros` section for one work session |
| Block ID | The trailing `^id` on a task line, used to link or nest under that task |
| Task link | A `[[note#^id]]` (or embed) pointing at a task. When that is the only content of a Pomodoro child bullet, it is that session's planned work |
| Task dependency link | A plain task block link on the dependent's managed `⛓️ **DEPENDS ON:**` child line; it names a prerequisite |
| Project / reference task | A project's `^prj` completion task or a reference's `#ref` reading task in its parent note; older references use an in-note `^ref` tracker |
| Freshness | The `[fresh:: YYYY-MM-DD]` date when a human last confirmed an open task; it drives review, separately from its status |
| Schedule Log | A managed `🗓️ **SCHEDULE LOG**` child that records each schedule change |
| Work Log | A managed `🛠️ **WORK LOG**` child that records work summaries |
| Cancel Log | A managed ❌ **CANCEL LOG** child (placed first) that records why a task was cancelled |

Ready, Next, and In Progress are task statuses, also called **lanes**. Pending
is the UI/report name for In Progress (`[/]`); it does not mean Blocked. The
dashboard's uppercase READY is a filtered backlog within the Ready lane,
excluding tasks awaiting NEW or ROTTEN review and work linked for Today.

## Daily workflow

Bob tracks a daily Pomodoro ledger inside an Obsidian vault and keeps that vault
synced through Git. Capture, linking, status, and nightly maintenance are
separate steps:

1. **Capture** with `bob capture` or Bob Mac Capture (the macOS panel that
   calls the same commands). Tasks land in `mac_inbox.md` unless an `@route`
   token sends them to another note; scheduled checkbox-bearing captures start
   Blocked (`[?]`). An eligible bare public link queues a background clip
   (`-R` keeps it a task). Append a standalone `%` to request companion audio
   (`https://example.com/article %`). The confirmation says reading queue.
   The background job files the note when it runs; a queued clip appears in
   `bob ref list` after the host's intake drain (`bob ref scan -i`).
2. **Review and choose work** with the read-only reports `bob plan`,
   `bob freshness`, `bob ready`, and `bob stats`; confirm or revise ordinary
   tasks in Obsidian, and complete due recurring or checklist rows. After
   time away, preview the overdue backlog with
   `bob task reroll --dry-run`, then `bob task reroll --seed <seed>` to re-roll it.
3. **Link and run today's work** on a Pomodoro in the daily note. Link with
   a marker-only `@route:id` capture (which raises Ready to Next), ensure an
   already-planned task with a bare `@route+id` capture, explicitly toggle a
   task with `@route+id!`, complete finished work with
   `bob capture '!sase:fix-flaky'`, or add a task link under a Pomodoro in Obsidian.
   Start the queued session with `bob capture '='`, then close it with an
   outcome such as `bob capture 'x'` or `bob capture 'x!'` (see
   [Capture](#capture)). Starting the timer leaves task statuses unchanged;
   closing records worked tasks as In Progress and completed tasks as Done.
   When a close completes planned work, it recovers newly unblocked
   tasks immediately and offers them as successor offers
   (`plan.link_unblocked: true` by default). Nothing is auto-linked;
   pick which tasks to pull in (Obsidian picker, Bob Mac Capture choice
   step, or `bob capture -p`). When no work remains under the project
   surfacing rules, the project's completion tracker (`^prj`) is offered
   the same way after `#hide` is removed. It does not complete the
   project. See [completion follow-ups](docs/task-dependencies.md#12-successor-offers).
   `bob pomodoro`, `bob pomodoro tmux`, and
   `bob pomodoro notify` read the ledger to report timing; they do not create
   links or start sessions.
4. **Reconcile** with `bob task reconcile` after manual ledger or task edits.
   Next and In Progress are sticky lanes: unlinking work does not release it.
   The command promotes linked work, derives Blocked from open dependencies,
   future schedules, and project prerequisites or dates on the `^prj` line.
   Project blocking passes through sub-projects: affected task lines,
   including an inherited-blocked sub-project's `^prj`, carry
   `#prj_is_blocked`. The command also cleans up the ledger.
5. **Nightly**, run `bob nightly` to reconcile the vault through Git, archive
   done and canceled tasks with `bob task archive`, report article notes that
   need formatting, and reconcile the vault again.

Read-only inspection (`bob query`, `bob stats`, `bob projects list`,
`bob plugins list --no-pull`, `bob ref doctor`) can run at any time.

## Vault layout

Paths below are relative to `BOB_DIR` (`~/bob` by default):

| Path | Role |
| --- | --- |
| `mac_inbox.md` | Default capture target |
| `gkeep_inbox.md` | Default target for `bob gkeep pull` (`gkeep.target` can name another vault-relative note) |
| `<route>.md` | Area or project note selected by an `@route` token |
| `<route>_<id>.md` | Project note created by `@route^id+`; hyphens in the block ID become underscores |
| `YYYY/YYYYMMDD.md` | Daily note; the `Pomodoros` section is the session ledger |
| `done/` | Archive notes written by `bob task archive` |
| `img/`, `file/` | Images and saved clipboard snippets written by `bob capture` |
| `_conflicts/` | Local copies of vault files `bob vault-sync` could not merge |
| `.obsidian/plugins/` | Installed community plugins, including Bob's custom plugins |
| `xlib/` | Host-local intake queue: PDFs kept with `-k` or queued when inline intake could not run |
| `lib/` | Library PDFs, filed by `bob ref create` and its producers, or by `bob ref scan` for queued items |
| `old_lib/` | Archival predecessor of `lib/`; tracked in the vault Git repo, which is now the vault's only sync channel |
| `ref/` | Generated Highlights reference notes |

Daily-note selection uses `BOB_DAY_FILE` when set, otherwise
`<bob-dir>/YYYY/YYYYMMDD.md` for the local date (or `BOB_NOW`).

## Commands

Bob's workflow commands are:

### Daily workflow

| Command | Purpose |
| --- | --- |
| [`capture`](#capture) | Capture tasks, bullets, or bare public links; manage Pomodoro sessions |
| [`freshness`](#task-freshness) | Walk the tiered freshness review queue |
| [`plan`](#plan-budget) | Show today's plan budget, Today's tasks, and the NEXT/PENDING lanes |
| [`pomodoro`](#pomodoro) | Show Pomodoro status, print the tmux line, or notify on completion |
| [`ready`](#ready-cap) | Show each area/project note's Ready lane against the per-note cap |
| [`stats`](#stats) | Show derived daily metrics as a glance, detail chart, or catalog |

### Tasks and projects

| Command | Purpose |
| --- | --- |
| [`projects`](#projects) | Inspect and synchronize project lifecycle tasks |
| [`task`](#task-maintenance) | Vault-wide task maintenance: reconcile, reroll, archive |

### Vault

| Command | Purpose |
| --- | --- |
| [`articles`](#article-notes) | Format and inspect article notes with bob-owned section numbers |
| [`nightly`](#nightly-maintenance) | Run nightly maintenance: vault-sync, task archive, articles, vault-sync |
| [`query`](#query) | Run Dataview or Tasks queries against the vault |
| [`ref`](#reference-library-bob-ref) | Find, list, read, rename, and plan references for today; sync Highlights PDFs into them |
| [`vault-sync`](#vault-sync) | Reconcile the vault through Git (default: run) or show status |

## Article notes

`bob articles` lists, formats, outlines, and adopts Markdown notes marked
`type: "[[article]]"`. It writes section numbers into headings while preserving
the words. Use `bob articles adopt NOTE` to preview marking a note, `-w` to
write, `bob articles format --check` in scripts, and
[`docs/articles.md`](docs/articles.md) for the command and JSON contracts.

### Integrations

| Command | Purpose |
| --- | --- |
| [`gkeep`](#gkeep) | Drain the Google Keep inbox into tasks or inline reference clips |

### Setup

| Command | Purpose |
| --- | --- |
| `completion` | Install and inspect shell completion for bob |
| [`plugins`](#plugins) | List and deploy Bob's custom Obsidian plugins |

### Capture protocol

The eleven `capture-*` rows form the Capture protocol section. Bob Mac Capture
uses these JSON endpoints as a separate integration protocol.

| Command | Purpose |
| --- | --- |
| `capture-complete` | Complete capture markers, task/dependency pickers, or wikilinks at the cursor |
| `capture-parse` | Preview what in-progress capture text and wikilinks mean |
| `capture-pomodoro-name` | Assign a canonical name to an unnamed Pomodoro |
| `capture-pomodoros` | List today's Pomodoro ledger entries; `--tasks` adds the resolved agenda |
| `capture-rewrite` | Apply the capture grammar's automatic draft rewrites (`@route:id`/`@route^id` toggle and bare `@@` absorption) |
| `capture-sections` | List the non-`Tasks` headings in a routed note |
| `capture-targets` | List inbox, area, and non-terminal project capture routes |
| `capture-task-id` | Assign a user-authored block ID to an open capture task |
| `capture-task-sections` | List the ALL-CAPS child sections of a capture task |
| `capture-tasks` | List the open tasks in a routed note |
| `capture-templates` | List and manage the bob-owned saved-template shelf |

Use `bob <command> --help` for concise usage. The sections below summarize each
workflow and link to the detailed command contract where one exists.

Old top-level spellings remain permanent silent aliases; see
[Migration notes](#migration-notes).

## Capture

```bash
bob capture [OPTIONS] [--] [TEXT]...
```

Captures tasks, ordinary Markdown bullets, task sub-bullets, or bare public
links without opening desktop Obsidian. Tasks default to `mac_inbox.md`.
An eligible bare public URL queues a background reference job. The
confirmation says the link is queued for the reading queue. The background
job files the note when it runs; a queued clip appears in `bob ref list`
after the host's intake drain (`bob ref scan -i`). Use `-R, --no-ref` to keep a link as
an inbox task. Extra prose or an explicit destination also keeps it a task.
Hosts without a dot (`http://go/x`, `localhost`), IP literals, and the
default excluded hosts (`google.com`, `googleplex.com`, `youtube.com`,
`youtu.be`, `github.com`, `x.com`, `twitter.com`, and their subdomains) stay
tasks. See
[saving links](docs/capture.md#saving-links-to-your-reading-queue).

`TEXT` may be several physical lines: each item’s first nonblank line is the
parent, later authored bullets become children, and blank lines split a batch
of items. A first-line `@@template:` header turns the following item into a
template; after a blank line, each row supplies one capture, except a
`{0}`-only template takes one capture per space-separated value. A `@@route` or
`@@route+id` token anywhere in the draft is a
draft-wide destination declaration, not body text; item-local `@...` markers
still win, and `bob capture` warns when a local marker shadows the declaration
typed on that same item. The whole batch is planned before anything is written.

| Marker | Meaning |
| --- | --- |
| Bare public URL | Queue a reference for background capture when the routing policy admits the host; `--no-ref` keeps it a task |
| `URL %` | Queue an eligible reference and request companion audio. The marker must be a separate token immediately after the URL; `URL @` is an alias |
| `URL % @route` | Same, with the reference note's `parent` set to `[[route]]`. `URL @ @route` is an alias |
| `@@route` | Shared task destination, anywhere in the draft, for otherwise-unrouted items |
| `@@route+id` | Shared parent-task destination, anywhere in the draft, for otherwise-unrouted items |
| `@@template:` | Expand one capture per row from a `{N}` template block |
| `@route` | Task in `<route>.md` |
| `@route#Section` | Ordinary bullet under a matching non-`Tasks` heading |
| `@route#` | Ordinary bullet under any non-`Tasks` heading |
| `@route^id` | Ordinary task with a user-authored block ID |
| `@route:id` | Next-status task plus a Pomodoro task link; scheduled tasks start Blocked |
| `@route:id#pomodoro` | Same, linked under a matching named open Pomodoro or a new named future Pomodoro |
| `@route:id=<X>` | Same, and atomically start the session; `<X>` mirrors the `se<X>` snippet (empty is 25 minutes) |
| `@route:id#pomodoro=<X>` | Same under the named Pomodoro, starting that session |
| `@route:id[#pomodoro][=<X>]` with no other text | Link the existing task into today's ledger (no new task); a task Blocked only by its own prerequisites drops them first; `=<X>` starts the resolved session |
| `^route:id[#pomodoro][=<X>]` with no other text | Identical execution; `^` completes only In Progress and Next tasks |
| `:<query>` with no other text | Task-link picker over Ready, Blocked, Next, and In Progress tasks in routable inbox, area, and non-terminal project notes; accepting an identified task inserts `@route:id`; the query itself cannot be captured |
| `+query` or terminal prose `+` | Parent-task picker over the same task catalog; accepting inserts `@route+id` |
| `&note:id` | Add a prerequisite to the captured task; `&projects/foo:bar` supports nested notes, and `&"Shopping List":bar` supports spaces |
| `&note:id @route+id` with no body text | Add a prerequisite to an existing dependent task |
| `^route:id &note:id` or `&note:id ^route:id` | Add a prerequisite to the active-task owner; `^route:id` is a bare alias here and does not write a Pomodoro Task Link |
| `&note:id +query` | Open the parent-task picker to choose the existing task that will depend on `note.md#^id` |
| `+query &note:id` | Choose the existing dependent task first, then add its prerequisite |
| `*note:id` | Make the named open task depend on the captured (or explicitly selected) task; nested notes use `*projects/foo:bar`, quoted notes use `*"Shopping List":bar`. Quote `*` on the shell (`bob capture 'Ask Kelly *job:cleanup'`): zsh globs it |
| `*note:id @route+id` with no body text | Make the named task depend on that existing owner |
| `^route:id *note:id` or `*note:id ^route:id` | Make the named open task depend on the active-task owner; `^route:id` does not write a Pomodoro Task Link |
| `*note:id +query` | Open the parent-task picker to choose the existing task that will block the named task |
| `+query *note:id` | Choose the existing blocking task first, then name the task it will block |
| `!note:block-id` with no other text | Complete that existing open task. Quote `!` on shells that history-expand it (`bob capture '!sase:fix-flaky'`). Nested paths and quoted note names work (`!projects/foo:bar`, `!"Shopping List":milk`). Pomodoro Task Links to the completed task are retired. A Blocked dependent returns to Ready when it names that task directly, has no other open dependency, and has no future scheduled date |
| `+[N]` / `-[N]` | Adjust today's current timed Pomodoro by N five-minute units (`+5` extends 25m, `-` shortens 5m; the count defaults to 1); the item must contain only the signed count |
| `++[N]` / `--[N]` | Shift today's running timed Pomodoro N five-minute units later/earlier, keeping its duration (`++3` moves 15m later, `--` moves 5m earlier; the count defaults to 1); the item must contain only the operator |
| `=` / `=<X>` | Start today's next future Pomodoro with `se<X>` timing, or restart the running session with fresh timing; an override shows a warning before the write (quote in zsh) |
| `=<X>#pomodoro` | Start the named Pomodoro (`=#deep-work` is 25 minutes, `=3#bugs` is 15 minutes, `=#bugs@3` means `=3#bugs`); while another session runs, swap in a different name, keeping its ledger when `<X>` is empty or using fresh timing when provided (quote in zsh) |
| `=[<X>][#<name>]~<K>` | Start or override without the queued Task Links in `<K>` (`=~2`, `=3~2,4`, `=#bugs~2`); `~` drops from the session that ends up running (quote in zsh) |
| `==` / `==<X>` / `==[<X>]#pomodoro` | Retired: `` `==` is retired: `=` restarts or swaps the running Pomodoro by itself; write `=` ``; change the doubled prefix to one `=` to get the corresponding start (quote in zsh) |
| `x[<N>][*<P>][!<M>][~<K>][@<N>]` | Close today's running timed Pomodoro (lowercase `x` only, `*`/`!`/`~`/`@` in any order; `<N>` may instead be typed last as `@<N>`, so `x!1@2` means `x2!1`); `<N>` keeps links in progress, `*<P>` parks them without carrying forward, `!<M>` completes them, `~<K>` drops them while keeping their lane; a lone `0` in `<N>` means none; one Work Log entry may sit on the close line, several use child bullets. Quote `!`, `*`, and `~`. |
| `x*` / `x!` | Close parking/completing every numbered Task Link; an empty `*` or `!` selects the remainder after explicit assignments (`x*!2` parks all except task 2 and completes task 2) |
| `x [<n>] <entry>` | One Work Log entry on the close line (`x wired the lexer` logs to task 1, `x1,3 3 fixed the flake` logs to task 3) |
| `+2 x`, `x =`, `x =#bugs`, `x =~2`, `x wired the lexer =` | Same-line session-operator chain: whitespace-separated session tokens on one line run left to right like blank-line items (`x =~2` closes then starts without link 2; `x wired the lexer =` closes with the entry then starts) |
| `@route:id=x…` / `^route:id=x…` with no other text | Put that existing task into the running session, then close it (own-prerequisite Blocked targets drop those links first); the `=` is the suffix separator; numbers refer to the post-link lineup |
| `<text> @route:id=x…` | Create the new task in the running session, then close it |
| `@route^id+` | New project note `<route>_<id>.md`; the daily note and the parent note are left unchanged |
| `@route^id+#pomodoro` | Same, picking the Pomodoro that ` :<task-id>` Task Links go under (named open Pomodoro or new named future Pomodoro) |
| first-level bullet ending in ` :<task-id>` | Names the task, makes it Next, and links it into the current/next Pomodoro or the `#pomodoro` Pomodoro |
| first-level bullet ending in ` ^<task-id>` | Names the task only (never linked) |
| `@route:id+` | Retired project-note spelling; use `@route^id+` instead |
| `@route+id` | Child bullet under an existing task |
| `@route+id#section` | Child bullet under an ALL-CAPS section of that task |
| `@route+id` with no other text | Ensure the task is Next and relocate its existing open-Pomodoro Task Link to today's current/next Pomodoro; a task Blocked only by its own prerequisites drops them first |
| `@route+id#pomodoro` with no other text | Ensure Next and move that existing Task Link to the named Pomodoro, creating the named future Pomodoro if needed; a task Blocked only by its own prerequisites drops them first |
| `@route+id!` with no other text | Toggle that task's Pomodoro Task Link: link it when unlinked (Ready and own-prerequisite Blocked rise to Next; other Blocked stay unlinked) or unlink it when linked (lane unchanged) |
| trailing `#` | Plain-text note on a Pomodoro (no `@route`) |
| `s:<N>` | Schedule N days from today; checkbox-bearing captures start Blocked, including `s:0` |
| `$` / `$<N>` | Write priority level N (bare `$` = 1) and roll a scheduled date, so checkbox-bearing captures start Blocked |
| `%`, `%N`, `%header` | Attach clipboard content. When URL routing accepts `URL %` as a reference, `%` requests audio instead; `%N` and `%header` keep their clipboard meaning |

`#` is not one marker. A trailing bare `#` is a Pomodoro note; `@route#…` selects
a heading in that note; `@route+id#…` selects an ALL-CAPS child section of that
task once the item has body text, but selects a Pomodoro name when
`@route+id#…` is the whole item; `@route:id#…` selects a matching named open
Pomodoro or creates a named future Pomodoro; `=<X>#…` starts the named
Pomodoro now (open match in place, completed match as a new "again" session,
otherwise a new named session). A `#` in the middle of the body
stays ordinary text. The retired `@route::id` spelling is not accepted; use
`@route^id` for an ordinary task with a block ID.

`+` has two positions. `@route+id` is a child bullet, or an Ensure Next
request, under an existing task. `@route^id+` puts `+`
immediately after the block ID and creates `<route>_<id>.md` at the vault root,
replacing every `-` in the block ID with `_`. Capture leaves the parent note
unchanged; `bob projects sync` owns its Sub-projects line. The project's own
`^prj` task is never linked. End a first-level task bullet with ` :<task-id>`
to link it into the current/next Pomodoro or the `#pomodoro` Pomodoro, or with
` ^<task-id>` to name it only. The retired `@route:id+` spelling is not
accepted; use `@route^id+` instead. A `+` after a Pomodoro name, as in
`@sase:deep-fix#bugs+`,
stays part of the name. Route names may contain letters, digits, `_`, and `-`.
Block IDs may contain letters, digits, and `-`.

A bare `@route+id` marker-only capture is the default Ensure Next operation:
Ready `[ ]`, a task Blocked `[?]` only by its own open prerequisites, In
Progress `[/]`, and Next `[*]` all end as Next, and Bob moves the task's
single existing open-Pomodoro Task Link subtree to the implicit current/next
open Pomodoro. A project-blocked or `^prj` target stays unlinked. `@route+id#pomodoro` is the same Ensure
Next operation targeting a named open Pomodoro, or creating that named future
Pomodoro when no open match exists. Neither unsuffixed form toggles Next back
to Ready or creates a missing link. Repeating the request when the task is
already Next and the link is already at the destination is a no-op.

Use `@route+id!` to toggle the Pomodoro Task Link itself: an unlinked Ready
`[ ]` task, or a Blocked `[?]` task whose own open prerequisites capture can
drop, becomes Next `[*]` and gets `[[route#^id]]` under the implicit
current/next open Pomodoro; other Blocked targets stay unlinked. An unlinked
Next `[*]` or In Progress `[/]` task links with its status unchanged; a
linked task of any open status unlinks every matching link under every open
Pomodoro with its status unchanged. `!` cannot be combined with `#pomodoro`. Future schedules are retired
only when the link direction sets the task to Next and the task has exactly one
valid future `[scheduled::YYYY-MM-DD]`, and
a Schedule Log entry is written only if the task already has that log. Done,
canceled, missing, duplicate, and non-task IDs are rejected
before any note is written. Users upgrading from the initial `!`
implementation should note this reversal: the unsuffixed forms are now the
idempotent default, and `!` is the link/unlink escape hatch that never lowers
a lane.

Append `=<X>` to a Pomodoro-linked marker to start its session in the same
transaction: `bob capture 'Write outline @sase:outline=3'` starts a
15-minute session on the next open slot, and
`@sase:outline#deep-work=-2` starts the default 25-minute session with a
10-minute offset. The started entry moves ahead of every open Pomodoro, right
after the last completed one. `<X>` is exactly the suffix of the `se<X>` snippet — empty,
digits, `-`, `-digits`, or `digits-` with optional digits — and the range is
rounded to 5 minutes like the snippet. A timed open Pomodoro stops the
capture with a "finish the current Pomodoro first" error, and `=<X>` cannot
combine with `s:<N>` or `$<N>`.

Capture a whole item `+N` or `-N` to adjust today's current timed Pomodoro:
`bob capture +5` extends by 25 minutes, `bob capture -- -2` shortens by 10
minutes, and `printf '+5\n\nCall bank @Cash+\n' | bob capture` mixes an
adjustment with an ordinary item across blank lines. Session operators may
share one line (`bob capture '+2 x'` extends then closes; `bob capture 'x ='`
closes then starts). The item must contain
only the signed count; `Plan +5` stays prose, and forced route/task/clipboard
options are rejected on adjustments.

Capture a whole item `=`/`=<X>` to start today's next future Pomodoro
(`bob capture '='` starts 25 minutes, `bob capture '=3'` starts 15 minutes;
quote in zsh, which expands a leading `=word`). A trailing `~<K>` drop list
(`bob capture '=~2'`, `bob capture '=3#bugs~1'`) starts without those queued
Task Links: `~` drops, so `=~2` drops task 2 from the session you start the
way `x~2` drops task 2 from the session you stop.

Capture a whole item `x[<N>][*<P>][!<M>][~<K>][@<N>]` (lowercase `x` only) to close today's running timed
Pomodoro the way Obsidian's Ctrl+Enter completion does, plus an
auto-decrement that shortens an early-stopped session to the earliest
five-minute step at or after now: `bob capture 'x'` closes the session,
`bob capture 'x wired the lexer'` logs one entry to task 1,
`bob capture 'x2'` keeps only task 2 in progress, `bob capture 'x1!2'`
keeps task 1 in progress and completes task 2, `bob capture 'x0'` resets a note-free session to first future (or closes, deferring everything, when it owns a stand-alone note),
`bob capture 'x*'` parks every numbered Task Link without carrying it forward,
`bob capture 'x!'` completes every numbered Task Link,
`bob capture 'x1~2'` keeps task 1 in progress and drops task 2,
`bob capture '^bob:capture-stop=x'` puts that task into the running session
then closes it, and `printf -- '-2\n\nx\n' | bob capture` adjusts then
closes atomically (or `bob capture '-2 x'` on one line). Numbers count the session's Task Links in ledger order
from 1; on link forms they refer to the post-link lineup. Whole-item `=x`,
`=*`, and `=!` fail with a teaching error that names the `x…` rewrite.
See `docs/capture.md` for the numbering, outcomes, classification, carry, Work
Log, link-form, diagnostic, JSON, retired forms, and batch rules.

In an editor, `+query` or a trailing `+` after prose opens the parent-task
picker. A lone `+` opens that picker too; dismissing it leaves a valid
five-minute Pomodoro adjustment. Numeric `+5`, doubled `++`, and completed
`@route+id` markers keep their existing meanings.
With prose, the accepted `@route+id` appends a child bullet to that task;
alone it ensures Next instead. Accepting a picker row only edits the draft;
submitting the draft runs capture.

Add prerequisites with `bob capture 'Ship report @work^report &work:outline'`,
or edit an existing dependent with `bob capture '&work:outline @work+report'`.
The prerequisite must resolve to a unique task with a block ID. Open
prerequisites make the dependent Blocked. The `&` picker searches task-bearing
notes across the vault, including nested notes and completed history;
closed prerequisites do not block, and adding a dependency does not link the
dependent into a Pomodoro.

Block an existing task from the new (or selected) task with
`bob capture 'Ask Kelly *job:cleanup'`. That writes the edge on the named
task's Depends-On line, so `job#^cleanup` waits on Ask Kelly. The `*`
picker searches open editable tasks, ranked with today's linked tasks
first (the running entry's first). A new blocker is capped at Next.
Quote `*` on the shell; zsh globs it.

Incomplete `&`, `*`, and `+query` selectors must be resolved before capture. See
[dependency capture](docs/capture.md#adding-prerequisites-with-),
[blocking existing tasks](docs/capture.md#blocking-existing-tasks-with-), and
[task dependencies](docs/task-dependencies.md).

```bash
bob capture buy milk @groceries
bob capture '@dev^foobar' 'Some ordinary task.'
bob capture '@cash^goog-exit+' 'Finish the Google exit packet!'
bob capture '@dev:foobar' 'Some foobar task.'
bob capture '@dev:foobar#bugs' 'Some foobar task.'
bob capture 'Write outline @sase:outline=3'
bob capture +5
bob capture -- -2
bob capture '='
bob capture '=3'
bob capture 'x'
bob capture '^bob:capture-stop=x'
bob capture '@cash+goog-exit' 'Called Morgan Stanley today.'
bob capture '@cash+goog-exit'
bob capture '@cash+goog-exit!'
bob capture '@cash+goog-exit#coding'
bob capture 'remembered to bump the timeout #'
printf '@@foo\nFirst task\n\nSecond task @bar\n' | bob capture
```

Editor clients such as Bob Mac Capture call `bob capture --format json`,
`bob capture-parse`, `bob capture-rewrite`, and `bob capture-complete`.
`bob capture-rewrite` turns a `^` typed immediately after `@route:id` into
`@route^id` (and `:` after `@route^id` back into `@route:id`), and turns a
bare `@@` typed inside an item that already has a `@route` (or `@route+id`)
marker into `@@route` (or `@@route+id`), deleting the marker it absorbed.
Discovery helpers
(`capture-targets`, `capture-sections`, `capture-tasks`,
`capture-task-sections`, `capture-pomodoros`) feed those pickers.
`capture-task-id` assigns a user-authored block ID to an open task that still
lacks one. `capture-pomodoro-name` assigns a canonical ALL-CAPS name to an
unnamed Pomodoro in today's daily note.

Captures that change today's ledger also report the plan budget: a
top-level `plan_budget` with before/after theme and link meters, a
`→ under GOALS (next up)` destination line naming where the Task Link
landed (JSON adds a `role` of `current`, `next_up`, `named`, or
`created`), and a stderr warning only while the batch grows a meter
past its cap. With `plan.strict: true`, a batch that would create a new
named Pomodoro past the theme cap is refused atomically (exit 1,
`code: plan_theme_cap_exceeded`); session starts are never refused.

The full grammar, JSON contracts, and picker protocol live in
[`docs/capture.md`](docs/capture.md).

## Query

```bash
bob query --source '#project'
bob query --query 'LIST FROM #waiting'
bob query --format json --query-file queries/projects.dql
bob query --tasks 'status.type is TODO' --origin dash.md
bob query --format json --tasks-file queries/all.tasks
bob query --format markdown --tasks-note dash.md
```

Runs Dataview source expressions, DQL queries, and Obsidian Tasks queries from
the shell. The default native engine evaluates queries against the local
Markdown vault, so scripts do not need a running desktop Obsidian app. `paths`
output prints vault-relative Markdown paths, `json` output is stable for
scripts, and `markdown` output prints Dataview-rendered Markdown for supported
DQL results. Native Tasks support includes filters, Boolean expressions,
JavaScript `by function` instructions with Moment, sorting, grouping, limits,
layout instructions, Query File Defaults, placeholders, and rendered Markdown.
`--tasks-note` runs every fenced Tasks block with its note context and identifies
each result by heading.

This command does not reconcile the vault; freshness is handled by
`bob vault-sync` and the configured background or cron sync path. Use
`--engine obsidian` when you want exact
behavior from the live Dataview plugin in an open Obsidian vault. Tasks inputs
remain native-only, with an env-gated live renderer harness for parity checks.

The full command contract and live smoke-test steps live in
[`docs/dataview.md`](docs/dataview.md).

## Task maintenance

Every `bob task` command rewrites task lines across the whole vault.
Read-only reports (`plan`, `ready`, `freshness`) stay top-level.

### bob task reconcile

```bash
bob task reconcile [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-r|--retry-timeout SECONDS]
```

Run this after capturing or closing Pomodoro-linked work, and after
`bob projects sync` writes schedules. It makes today's Pomodoro ledger the
source of truth for Next / In Progress promotions and structural cleanup, and
uses the latest existing earlier daily note as a read-only recent-activity
source.

Direct block links under open Pomodoros promote Ready tasks to Next (`[*]`) and
leave In Progress (`[/]`) alone. Task dependency links on a dependent's managed
`⛓️ **DEPENDS ON:**` line plus R8 legacy dependency children inherit the
strongest parent rank. Before ranking, the command reconciles each
Depends-On line into `[dependsOn::]` / `[id::]` fields (R1–R10: project,
adopt, heal, canonicalize, warn; see
[`docs/task-dependencies.md`](docs/task-dependencies.md) and
[`docs/task-status-hooks.md`](docs/task-status-hooks.md#dependency-lines)).
Lanes are sticky: removing a link never lowers Next or
In Progress — only a Next task inside a canonical daily note (or the current
ledger) still clears back to Ready once unlinked and stale. Independently,
open Dataview dependencies and future `[scheduled:: YYYY-MM-DD]` dates mark a
task Blocked (`[?]`). A project's own blocking comes from its open `^prj`
task's Depends-On line and inline schedule: while either gives a reason,
every open resident of the note is Blocked and carries `#prj_is_blocked`
— as does every open task in its sub-project notes, recursively —
and every automatic Blocked-to-Ready recovery writes `[unblocked::]`. The command
also retires completed references, moves stray bullets onto the current
Pomodoro, repairs Pomodoro markers, de-duplicates the same task under later
open Pomodoros, removes list items that only point at canceled tasks, consumes
live Task Links to effectively Blocked tasks under today's open Pomodoros
(promoting open prerequisites first), deletes
childless current-daily Pomodoro entries, and
groups area/project `Tasks` sections into `Next & In Progress`, `Blocked`, and
`Done & Canceled` child headings, emitting only non-empty groups with a
linked status-count badge row while keeping Ready tasks in the intake. Human and JSON reports include the
dependency projection counts (`Dependencies:` plus the trailing `Summary:`
counts) as well as consumed blocked links, the project-blocked tag, and
unblocked-field writes with their own trailing `Summary:` counts.

```bash
bob task reconcile --dry-run
```

The command refuses to change files if the current daily note is missing, lacks
a `Pomodoros` section, or has multiple non-empty open timed Pomodoros. Live writes use
guarded snapshots, recovery copies, and a bounded quiet-period check for
structural regrouping; `--dry-run` computes the same plan without locking or
writing recovery state. Live runs also retry lock contention and other
transient failures with jittered backoff, bounded by `--retry-timeout`
(default 120s; `0` fails fast on the first attempt). The full sync, grouping,
link-resolution, exclusion, retry, output, and JSON contract lives in
[`docs/task-status-hooks.md`](docs/task-status-hooks.md).

### bob task reroll

```bash
bob task reroll [-d|--dry-run] [-f|--format human|json] [-l|--level LABEL]...
                [-o|--offline] [-r|--retry-timeout SECONDS] [-s|--seed SEED]
                [-u|--until DATE|+N]
```

Re-rolls every due prioritized task to its own random date inside that
task's configured priority window (P1 2–7 days through P4 91–365 days).
Each re-roll replaces the `scheduled` date, flips a future-dated Ready task
to Blocked, writes a 🎲 Schedule Log entry, and regroups eligible project
notes in the same write. P0 tasks, Next and In Progress tasks, tasks linked
from today's open Pomodoros, `^prj` lifecycle tasks, and `due`/`repeat`
tasks are always left alone. `--level` limits the roll to those labels.
`--until` treats tasks scheduled through that date as due and rolls windows
from it. An offset that cannot be represented on the calendar is a usage
error (exit 2): `bob task reroll: invalid --until "…": date out of range`.

Always preview first with `--dry-run`, then apply those exact dates with
the printed `--seed`, repeating `--level` and `--until` when you used them.
Omit `--seed` to use `BOB_PRIORITY_ROLL_SEED` or a generated seed. A live
run holds the shared vault-maintenance lock while it syncs, plans, writes,
commits exactly the rewritten notes as one `bob task reroll` commit, and
syncs again. `--offline` skips both sync cycles and still commits locally.
Undo with `git -C ~/bob revert <sha> && bob vault-sync`. The full contract
lives in [`docs/randomize.md`](docs/randomize.md).

### bob task archive

```bash
bob task archive [-t|--threshold N]
```

Scans the Bob vault for completed (`[x]`) and canceled (`[-]`) Markdown task
blocks containing `#task`, then moves blocks from notes that meet the threshold
into matching archive notes under `done/`. The default threshold is `10`; use a
smaller value for a targeted collection pass, such as `-t 1` in a
fixture vault.

Archive paths mirror the source note path and add `_done` to the file stem. For
example, `projects/foo.md` archives to `done/projects/foo_done.md`. Archive
notes are created with `parent` pointing at the original source note plus
`type: "[[done]]"`, such as `parent: "[[projects/foo]]"` and
`type: "[[done]]"`. Existing archive notes have `parent` and `type` frontmatter
inserted or repaired before new blocks are appended. Source notes that have a
matching archive note are linked back to it with `done_tasks`, such as
`done_tasks: "[[done/projects/foo_done]]"`. Existing archive notes under `done/`
are backfilled into source note frontmatter and archive metadata on future runs
even when no task blocks meet the threshold.

When task blocks with explicit Obsidian block ids are moved, links to those
blocks are repaired across vault Markdown notes. For example,
`[[projects/foo#^abc123]]`, `![[projects/foo#^abc123]]`, and aliases such as
`[[projects/foo#^abc123|follow-up]]` are rewritten to
`[[done/projects/foo_done#^abc123]]`. Moved block ids are de-duplicated within
their destination archive note before link repair. If `^abc123` already exists
in `done/projects/foo_done.md`, the moved id becomes the smallest available
suffix such as `^abc123-1`, and repaired links point at that final id. If
multiple moved blocks originally share the same id, their archived ids are still
made unique, but existing links to the original duplicate id are left unchanged
because the intended block is ambiguous. Only explicit `^block-id` targets can
be rewritten; heading links and tasks without block ids do not have a stable
target to repair.

Task dependency metadata has a separate vault-wide identity from its Obsidian
block link. A task at `projects/foo.md#^abc123` uses
`[id:: projects__foo__abc123]`, and dependents use the same value in
`[dependsOn:: projects__foo__abc123]`; the trailing block token remains
`^abc123`. When a task moves, the command rewrites its `[id::]` to the archive
path/final block ID and repairs exact dependency tokens across all planned
files. Metadata and link repair share the same atomic preview/write plan.

The command itself does not reconcile the full vault; `bob nightly` runs
`vault-sync` before and after invoking it. In a Git worktree, the command stages
only the files it touches, commits with a `bob task archive YYYY-MM-DD`
message, and pushes. Existing uncommitted changes in touched source, archive,
or link-repair files are included in that scoped commit after the command
rewrites those files. Non-Git vaults are left uncommitted.

## Plan budget

```bash
bob plan [-b|--bob-dir DIR] [-f|--format human|json]
```

Today is closed: GTD plus at most 3 themes. `bob plan` is read-only:
it reports the plan budget from today's daily-note ledger (distinct
open Pomodoro names besides the exempt ones, distinct Task Links
under them), Today's tasks with a dedicated Task Link under today's
open Pomodoros, and the NEXT/PENDING lane counts from the native
Tasks index, with lints and hints. Removing a Task Link never changes
a task's lane. The first open non-exempt entry is the highlight (★);
the timed entry is running (▶). A missing daily note or missing
Pomodoros section still reports `TODAY 0` and the lanes, and exits 0.
The full definition, JSON contract, lint codes, and conformance
examples live in [`docs/plan.md`](docs/plan.md).

## Task freshness

Task freshness records the date a human last confirmed that a task still
needs doing as written, in `[fresh:: YYYY-MM-DD]` (its "stamp"). Visible,
non-recurring Ready tasks are due for review when they have never been
confirmed (NEW), return from a deferral or automatic unblock (TICKLER), or
reach their review interval (ROTTEN). The interval defaults to 7 days and
can be overridden per task, per note, or in config. Supported keymaps and
`bob capture` edits stamp eligible existing tasks they rewrite; creation
and automation never confirm them. Recurring tasks are never stamped.
The definition, placement and state rules, and
conformance vectors live in [`docs/freshness.md`](docs/freshness.md).
The freshness mark display contract lives in §11 of that file.

```bash
bob freshness [-b|--bob-dir DIR] [-f|--format human|json] [-l|--limit N]
bob freshness list [-b|--bob-dir DIR] [-f|--format human|json] [-l|--limit N]
bob freshness seed [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-f|--format human|json]
```

Running `bob freshness` with no subcommand runs read-only `list`. The queue
is ordered PRE → NEW → PROJECTS → PENDING → NEXT → RECURRING → TICKLER →
ROTTEN → POST. Both list and seed use JSON schema 14. Counts cover the whole
vault even with `--limit`; `counts.walk` sums the nine `counts.by_tier`
values. Ready state totals and walk-tier totals have different scopes.

Ordinary Pending (In Progress) and Next tasks, including references, use
`freshness.pending_interval` and `freshness.next_interval` (default 1 day;
`false` disables that ordinary lane's walk). Due project completion trackers
(`^prj`) use PROJECTS and `freshness.project_interval`, falling back to the
task/note/default Ready interval. References use ordinary review groups:
Ready refs join NEW, TICKLER, or ROTTEN; Pending and Next refs use their lanes.
An ordinary Ready task with a stamp joins TICKLER when its scheduled date
arrives after that stamp, or its recorded `[unblocked::]` date is on or after
the stamp and no later than today. An unstamped task stays NEW. Hidden tasks,
including project and reference trackers, stay outside the queue;
`bob projects sync` manages when project trackers carry `#hide`.

A visible recurring occurrence joins RECURRING when the earliest of its
`scheduled`, `due`, and `start` dates arrives. Complete it in Obsidian so
Tasks creates the next occurrence, move its occurrence date into the future,
link it to Today, or cancel it. Ctrl+Alt+F does not resolve a recurring row.
Today-linked tasks, tasks in canonical daily notes (`YYYY/YYYYMMDD.md`),
blocked tasks, and future-scheduled tasks stay outside ordinary review.
Checklist rows tagged `#gtd #pre` or `#gtd #post` use PRE or POST instead;
they still walk when recurring, in daily notes, or linked to Today, and
resolve by completion. A hidden, blocked, or future-scheduled checklist
row stays out.

On the dashboard, NEW holds unconfirmed tasks, READY holds confirmed/exempt backlog, and rotten.md holds tickler tasks plus expired tasks. Repeated due-Ready confirmations accumulate [keeps:: N]; the approved-decay decision card is available immediately, subject to config and plugin capability.

Seed is a one-time migration, not daily maintenance. For tasks lacking a
valid freshness date, it staggers Ready candidates across seven date buckets
and stamps other eligible open tasks today. Existing older stamps plus new
candidates cause a refusal unless `--force`; valid stamps remain unchanged,
and a run with no candidates is a
no-op. `--dry-run` previews without writing; a writing run rechecks touched
files before the first write. See [seed guards and recovery](docs/freshness.md#7-bob-freshness)
for the limits of that check and partial-write recovery. Do not seed later
captures merely to clear their NEW review state.

## Ready cap

```bash
bob ready [NOTE] [-a|--all] [-b|--bob-dir DIR] [-n|--cap N] [-c|--check] [-f|--format human|json]
```

Every area/project note has a soft cap on its Ready lane
(`plan.max_ready_per_note`, default 5, per-note `ready_cap`
override). `bob ready` is read-only: it shows crowded notes with red
overflow bars, full notes at the cap, room notes as a compact list,
and the single-note worklist with `path:line` references. The cap counts NEW
and ROTTEN tasks too, and includes Today-linked Ready tasks; recurring tasks
and the `^prj` tracker are excluded. This differs from the dashboard's
freshness-gated READY backlog. `--all` expands room notes and includes empty,
exempt, and terminal notes. `--check`
exits 3 when any note is crowded; `--cap N` previews a different
default for one run. The full definition, JSON contract, and
conformance vectors live in [`docs/plan.md`](docs/plan.md).

## Stats

```bash
bob stats [overview] [-b|--bob-dir DIR] [-c|--compare] [-f|--format human|json|csv] [-g|--group auto|day|week|month] [-o|--offset N] [-r|--range RANGE]
bob stats show <METRIC>... | -a   [same options]
bob stats metrics [-b|--bob-dir DIR] [-f|--format human|json]
```

`bob stats` is read-only. Bare `bob stats` is the weekly glance of the
overview metrics. `show` draws one metric (or `-a` every visible metric)
as a detail chart. `metrics` lists the catalog. JSON is one line;
CSV is tidy `date,end,metric,value,display,quality,provisional,num,den`
rows. `BOB_NOW` pins today. The Obsidian Stats page (`dash_stats.md`,
STATS chip on the dashboard) draws the same JSON via a `bob-stats`
block; see [`docs/dashboard.md`](docs/dashboard.md#child-pages). Review
journal files under `.bob/stats/review/` sync with the vault
([`docs/vault-git-sync.md`](docs/vault-git-sync.md#stats-review-journal)).
The catalog, JSON contract, human layout, and known limits live in
[`docs/stats.md`](docs/stats.md).

## Projects

```bash
bob projects list [-b|--bob-dir DIR]
bob projects sync [-b|--bob-dir DIR] [-d|--dry-run]
```

Scans notes whose frontmatter declares `type: "[[project]]"`. `list` prints
frontmatter status, open `#task` counts, the `^prj` lifecycle task, and a dim
`rollup` marker for `rollup: subprojects`; invalid rollup values are warned
about without changing the exit status. `sync`
reconciles `status` from that task (`done`, `canceled`, or reopen to `wip`),
manages `#hide` so `^prj` surfaces on `dash.md` only when nothing else is open,
maintains the machine-owned Sub-projects ledger after Depends-On, hoists leftover
frontmatter `scheduled` onto `^prj`, and propagates that inline date onto ordinary
open tasks.

`sync` writes `#hide`, Sub-projects lines, inline schedules, and leftover
frontmatter hoists;
it does not change checkboxes. Run `bob task reconcile` afterward to derive
or recover `[?]` Blocked markers. The property picker in Bob Navigation
Hotkeys can propagate schedules and reconcile Blocked in the same editor
transaction. A close that empties a project also un-hides `^prj` at once
and may link it as Next
([`docs/task-dependencies.md`](docs/task-dependencies.md) §12.10). The
full project task contract lives in
[`docs/projects.md`](docs/projects.md).

## Plugins

```bash
bob plugins [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins list [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins sync [-B|--backup-dir DIR] [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-f|--format table|json] [-n|--no-pull] [-p|--plugin ID] [-r|--repo DIR]
```

Lists Bryan's custom Bob Obsidian plugins from the
[`bobs-org/bob-plugins`](https://github.com/bobs-org/bob-plugins) repo and
annotates each with live vault state. Running `bob plugins` with no subcommand
runs `list`. Managed files are `manifest.json`, `main.js`, and `styles.css`
when present; runtime files such as `data.json` are never touched.

The repo root resolves from `-r, --repo`, then `BOB_PLUGINS_DIR`, then
`~/projects/github/bobs-org/bob-plugins`. The vault root resolves from
`-b, --bob-dir`, then `BOB_DIR`, then `~/bob`. By default, `list` and `sync`
run a non-interactive `git pull` first; pass `-n, --no-pull` to skip it.
`sync` copies managed files into
`<bob-dir>/.obsidian/plugins/<id>/`, skips vault files with uncommitted Git
changes unless `-F, --force` is set, and writes timestamped backups first.

The full command contract lives in [`docs/plugins.md`](docs/plugins.md).

## Gkeep

```bash
bob gkeep [-a|--all] [-b|--bob-dir DIR] [-f|--format table|json] [-s|--source both|keep|vault]
bob gkeep list [-a|--all] [-b|--bob-dir DIR] [-f|--format table|json] [-s|--source both|keep|vault]
bob gkeep pull [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-i|--id REF]... [-p|--include-pinned] [-S|--include-shared] [-l|--limit N] [-n|--no-archive] [-C|--no-commit] [-R|--no-ref] [-P|--parent ROUTE] [-q|--quiet]
bob gkeep migrate-markers [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-C|--no-commit] [-q|--quiet]
bob gkeep migrate-tasks [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json] [-C|--no-commit] [-q|--quiet]
bob gkeep login [-e|--email EMAIL]
bob gkeep doctor [-b|--bob-dir DIR] [-f|--format human|json]
```

Imports Google Keep inbox notes into `gkeep_inbox.md` as Obsidian tasks
with no bookkeeping in the Markdown: tasks carry only the linked 💡, the
created field, labels, revision indication, and useful note children.
Import history lives in versioned `.bob/gkeep/imports/*.json` files inside
the vault, so vault Git sync carries it between machines (back up and sync
the vault; old binaries cannot read the new history, so upgrade every host
that pulls before relying on marker-free imports). Run the explicit offline
`bob gkeep migrate-markers` once to clean existing `%%gkeep:…%%` markers.
Eligible URL-only notes are clipped inline to intake PDFs during `pull`;
`bob ref scan` writes the notes `bob ref list` shows. `bob capture` queues
the same kind of link as a background job instead. `pull -R` keeps links as
tasks. For each URL-only note, a valid trailing `@route` chooses the clip's
parent; otherwise `-P` does, then a terminal prompt for that URL, then
`gkeep_inbox`. Prompts require terminal stdin and stderr and are skipped
with `--quiet` or JSON output. When there are reference candidates, an
invalid `-P` fails before any clip. A standalone `%` immediately after
that URL requests companion audio. Each note is archived in Keep only
after its current content is verified in the vault (unless `--no-archive`).
A pull that only clips skips
the inbox write and the Git commit. Unidentifiable URL-less edited tasks
keep `keep_id`/`keep_state` null rather than guessing another note's
identity. `bob gkeep migrate-tasks -d` previews full visual cleanup of
recognized generated Keep source children on open tasks; running it without
`-d` applies the offline migration while preserving import history. It keeps
closed tasks unchanged and reports ambiguous candidates. The older
`migrate-markers` command remains marker-only and includes all task statuses.
Running `bob gkeep` with no subcommand runs `list`: Keep notes
and vault tasks side by side with per-note pull states. `pull -d` previews
the exact Markdown, `pull -n` writes without archiving, and `pull` writes,
verifies, commits, then archives. `login` is the one-time token setup and
`doctor` diagnoses the whole chain.

The full command contract lives in [`docs/gkeep.md`](docs/gkeep.md).

## Reference library (bob ref)

```bash
bob ref create <TARGET>... [-A|--author NAME] [-a|--audio PATH] [-b|--bob-dir PATH] [-C|--closed] [-d|--dry-run] [-f|--force] [-H|--html FILE] [-i|--include-id] [-k|--keep-intake] [-l|--lib-dir PATH] [-L|--listen] [-M|--plain] [-N|--name STEM] [-n|--no-audio] [-o|--output PDF] [-P|--parent NOTE] [-p|--published DATE] [-r|--ref-dir PATH] [-S|--section HEADING]... [-s|--status STATUS] [-T|--title TITLE] [-t|--ref-type DIR] [-x|--xlib-dir PATH]
bob ref doctor [-b|--bob-dir PATH] [-l|--lib-dir PATH] [-n|--no-hooks] [-r|--ref-dir PATH] [-x|--xlib-dir PATH]
bob ref find <QUERY>... [-b|--bob-dir PATH] [-f|--format human|json|markdown] [-i|--include-intake] [-m|--min-score SCORE] [-r|--ref-dir PATH] [-x|--xlib-dir PATH]
bob ref jobs [list] [-a|--all] [-f|--format human|json]
bob ref jobs run [-q|--quiet]
bob ref list [-A|--all] [-b|--bob-dir PATH] [-f|--format human|json|markdown] [-g|--git-dates] [-n|--limit N] [-o|--origin external|agent-report] [-P|--parent NOTE] [-R|--reading-state STATE,...] [-r|--ref-dir PATH] [-S|--since DATE] [-s|--status STATUS,...] [-t|--ref-type TYPE,...]
bob ref migrate-tasks [-b|--bob-dir DIR] [-f|--format human|json|tsv] [-m|--map FILE] [-o|--offline] [-r|--ref-dir PATH] [-w|--write]
bob ref migrate-zorg [-b|--bob-dir PATH] [-f|--format human|json] [-o|--offline] [-r|--ref-dir PATH] [-w|--write]
bob ref marker <pdf> [-b|--bob-dir PATH] [-l|--lib-dir PATH] [-r|--ref-dir PATH] [-x|--xlib-dir PATH]
bob ref rename <REF> <NAME> [-b|--bob-dir PATH] [-d|--dry-run] [-f|--format human|json] [-l|--lib-dir PATH] [-o|--offline] [-r|--ref-dir PATH] [-x|--xlib-dir PATH]
bob ref scan [-b|--bob-dir PATH] [-d|--dry-run] [-f|--format human|json] [-i|--intake-only] [-j|--jobs N] [-l|--lib-dir PATH] [-n|--no-hooks] [-r|--ref-dir PATH] [-v|--verbose] [-w|--write-pdfs] [-x|--xlib-dir PATH]
bob ref show <REF>... [-b|--bob-dir PATH] [-c|--comments-only] [-f|--format human|json|markdown] [-N|--no-annotations] [-r|--ref-dir PATH]
bob ref sync <pdf> [-b|--bob-dir PATH] [-d|--dry-run] [-l|--lib-dir PATH] [-p|--prefer marker|frontmatter] [-r|--ref-dir PATH] [-w|--write-pdf] [-x|--xlib-dir PATH]
bob ref today <REF> [-b|--bob-dir PATH] [-d|--dry-run] [-e|--exact] [-f|--format human|json] [-p|--pomodoro NAME] [-P|--pomodoro-ref LINE:DIGEST] [-r|--ref-dir PATH]
```

Looks up and reads the reference library, imports material as Highlights-ready
PDFs, and syncs annotations into Obsidian reference notes. `create` files the
PDF, reference note, and reading task; `scan` drains the `xlib/` queue and
syncs annotations; `list` reads those notes. A queued or kept PDF appears in
`ref list` after the host's intake drain (`bob ref scan -i`). `bob ref clip`
remains a hidden alias of `create`.

- By default, `create <TARGET>...` files a vault note into
  `lib/notes/<note-stem>_<YYMMDD>.pdf`, other Markdown into
  `lib/chat/<stem>.pdf`, or a web article into `lib/blogs/<stem>.pdf`.
  Local PDFs, PDF URLs, and arXiv papers are stamped into
  `lib/papers/<stem>.pdf`. Two to twenty web article URLs produce one
  collection PDF. Vault notes infer an area/project parent from the note
  itself or its `parent:` links. Pass `-P, --parent` if none is eligible
  or for other targets.
  Snapshots omit closed tasks and condense logs. For vault notes, `-C` restores
  closed tasks in an appendix and full logs; repeatable `-S` selects snapshot
  sections; `-M` renders the note as ordinary Markdown.
  `-N, --name` sets the stem, `-T, --title` overrides the title, and
  `--ref-type` overrides the intake subdirectory. `-o, --output` supplies a
  complete PDF path and conflicts with `--ref-type` and `--name`.
  `-L, --listen` narrates with `highlights.listen_command` and binds companion
  audio; it conflicts with `-a` and `-n`. `-H, --html` replays a saved web
  page. Create files managed intake and library PDFs inline; `scan` drains
  queued PDFs and repairs notes or annotations. PDFs outside both directories
  need `bob ref sync <PDF>`. Creation refuses URLs already recorded
  in PDF-backed reference notes even with `--force`; that flag can replace
  the same queued intake target.
  See [target types, dependencies, and dedupe](docs/highlights-create.md) and
  [web article capture](docs/highlights-clip.md).
- `scan` runs the configured `highlights.pre_scan_hook` on writing runs, then
  moves pending PDFs from `xlib/<rel>` to `lib/<rel>` and recursively syncs
  the library, including image notes (screenshot paths pasted into highlight
  comments). Pass `-n, --no-hooks` on `scan` or `doctor`, or before the
  subcommand as `bob ref --no-hooks scan`, to ignore the hook. By
  default it does not write PDF markers; use `scan --dry-run --write-pdfs`,
  review, then
  `scan --write-pdfs`. `-v, --verbose` prints the detailed per-PDF plan instead
  of the concise report.
- `sync <pdf>` updates one reference note from the page-1 marker and sidecar.
- `marker <pdf>` inspects that marker without writing.
- `find <QUERY>...` looks up URLs, arXiv IDs, DOIs, paths, stems, or titles
  in the reference library with an `in_library`, `in_intake`, `possible`,
  or `not_found` verdict per query. `-` reads one query per line from
  stdin, `-i` also checks queued intake PDFs, and `-m` floors the title
  scores. The lookup contract lives in [`docs/ref.md`](docs/ref.md).
- `jobs` inspects links queued by `bob capture` (`list` is the
  bare read-only default; `run` clips pending jobs). A failed clip falls back
  to the inbox task plus a ⚠️ bullet. The lifecycle lives in
  [`docs/ref-jobs.md`](docs/ref-jobs.md).
- `list` shows the reading queue by default and filtered library views by
  reading state, status, type, origin, parent, or date, with `--limit 50`
  in every format (`-A` lifts it) and opt-in `-g` Git dates. Every row
  carries a `blocked` boolean overlay (true when its selected reading task
  is Blocked `[?]`); it never changes `status` or `reading_state`. Bob Mac
  Capture's Bob Refs panel reads it as a display overlay only.
- `show <REF>...` reads one reference note with its metadata, annotations,
  own notes, and tasks. `-c` keeps only commented annotations and
  standalone notes, `-N` keeps metadata and own notes only.
- `migrate-tasks` moves open ref tasks into parent notes, rewriting links and dependency ids. `--write` requires a Git worktree and commits the verified moves between two vault-sync cycles; `--offline` skips the sync cycles and keeps the commit local. See the [migration and rollback runbook](docs/ref.md#migrating-open-ref-tasks-bob-ref-migrate-tasks).
- `migrate-zorg` previews copies of old `status::` reading records into
  `ref/zorg/`, preserving the original records. `--write` requires a Git
  worktree and commits the verified notes between two vault-sync cycles;
  `--offline` skips the sync cycles and keeps the commit local. See the
  [migration and rollback runbook](docs/ref.md#migrating-zorg-era-records-bob-ref-migrate-zorg).
- `doctor` checks vault paths, intake, sidecars, markers, Git, pandoc, xelatex, LaTeX packages, `curl`,
  the web-clip chain (`uv`, adapter, browser, headed fallback), the
  `listen_command` row, and optional `ob` without writing. Pass
  `-n, --no-hooks` to skip the pre-scan hook check.

Generated notes live under `ref/`. Nested library PDFs such as
`lib/books/foo.pdf` write `ref/books/foo.md` with `type: "[[ref]]"` and
`ref_type: books`. New notes also get a local `created` timestamp; later syncs
preserve it and do not backfill older notes. On first sync, new references put
a `#ref` reading task in the parent note and embed it in the reference note
(v2). Older (v1) notes keep their in-note `^ref` tracker until migrated.
The reading task tracks lifecycle; `blocked` reports `[?]` separately.
Marker `status` values are `ready`, `next`, `wip`, `read`, `abandoned`, and
`legacy`.

The full contract and MacBook setup guide live in
[`docs/highlights-ref-sync.md`](docs/highlights-ref-sync.md); the web-capture
contract lives in [`docs/highlights-clip.md`](docs/highlights-clip.md), the
create-target contract in [`docs/highlights-create.md`](docs/highlights-create.md),
and the library-lookup contract in [`docs/ref.md`](docs/ref.md).

## Nightly maintenance

```bash
bob nightly
```

Runs the nightly Bob maintenance path. It acquires the shared vault-sync lock
(default `$XDG_RUNTIME_DIR/bob_sync.lock` when that variable names an existing
directory, otherwise `/tmp/bob_sync.lock`), then:

1. Runs `bob vault-sync` against the vault.
2. Runs `bob task archive` against the vault.
3. Runs a report-only `bob articles format --all --check` against the vault.
   Articles that need formatting are printed; the step still passes unless
   bob itself fails.
4. Runs `bob vault-sync` against the vault again.

A failed step is reported but does not prevent later steps from running. If
another Bob maintenance run already holds the lock, the command exits 0 after
printing that it is already active. `bob nightly` accepts no options other than
`-h, --help`.

## Vault sync

```bash
bob vault-sync [run] [-n|--dry-run] [-m|--message MESSAGE] [-q|--quiet]
bob vault-sync status [-j|--json]
```

Runs one Git reconcile cycle for the Bob vault. The default subcommand is
`run`, so `bob vault-sync --dry-run` is accepted. A cycle acquires the shared
maintenance lock, recovers interrupted merge/rebase/cherry-pick state, commits
local vault changes when present, fetches and merges `origin/master`, resolves
supported conflicts by writing local conflict copies under `_conflicts/`, and
pushes with bounded non-fast-forward retries.

The command refuses to stage any file at or above 95 MiB, warns for files at or
above 50 MiB, and writes a status record after each non-dry-run cycle. Use
`bob vault-sync status --json` for the machine-readable record containing the
last attempt/success timestamps, local and remote SHAs, committed-file count,
push retries, duration, conflict-copy paths, interrupted-merge recovery flag,
and last error.

If another maintenance command already holds the lock, `bob vault-sync run`
exits 0 silently.

The operational runbook for the two-machine Bob vault sync channel lives in
[`docs/vault-git-sync.md`](docs/vault-git-sync.md). Tracked machine data
under `.bob/` includes Keep import receipts and the Stats review journal
(`.bob/stats/review/*.jsonl`); see
[`docs/stats.md`](docs/stats.md#review-journal).

## Pomodoro

Bare `bob pomodoro` runs `bob pomodoro status`. `-h`/`--help` as the first
token shows the group help; `bob pomodoro status --help` shows the leaf help.

```bash
bob pomodoro [status] [-d|--debug] [-s|--show-stale] [-v|--verbose] [-w|--worked]
```

Prints the current Pomodoro ledger entry from today's Bob daily note, including
time remaining or recent overdue status. It defaults to
`$BOB_DIR/YYYY/YYYYMMDD.md`, or `~/bob/YYYY/YYYYMMDD.md` when `BOB_DIR` is
unset, unless `BOB_DAY_FILE` is set.
Ledger entries may use bold Markdown ranges such as
`(**0945-1015** [t:: 30m])`; command output remains plain, for example
`0945-1015 Review crate skeleton`. A named session prints that authored
name. An unnamed running session with linked tasks prints the derived
home chip after the range (`[<13m] 0950-1015 sase`); an unnamed session
without links prints the range alone.

The command exits successfully with no output when the daily note is missing,
has no open Pomodoro, or the open Pomodoro is more than nine minutes overdue.
Pass `-s` / `--show-stale` when a consumer needs to distinguish an old open
entry from no open entry; stale open Pomodoros keep the same normalized
`[OVERDUE by <minutes>m] HHMM-HHMM <task>` output shape as recent overdue
Pomodoros. `-d, --debug` and `-v, --verbose` enable debug tracing on stderr.

Pass `-w` / `--worked` when a consumer wants today's worked time instead of empty
output: whenever no status line would print, the command prints
`[WORKED <minutes>m]`. The total counts today's closed Pomodoros, using each
row's recorded `[t:: ]` duration first and its time range otherwise. It is the
same total as `capture-pomodoros --tasks`'s `completed_summary.minutes` and the
`bob stats` `worked` metric. A missing daily note reports `[WORKED 0m]`. The
option is native-only; with `BOB_CLI_USE_SCRIPT=1`, the script fallback exits 2.

```bash
bob pomodoro notify [-v] PRE_CHECK_SLEEP POST_NOTIFY_SLEEP
```

Polls Pomodoro status until the current entry is overdue, then sends a desktop
notification when `notify-send` is available and rings the terminal bell three
times. `PRE_CHECK_SLEEP` is the seconds to wait between status checks;
`POST_NOTIFY_SLEEP` is the seconds to wait after a notification before polling
again. Polling uses the same default status as `bob pomodoro` without
`--show-stale`: an entry more than nine minutes overdue looks like no open
Pomodoro, so start `bob pomodoro notify` while the session is still running or
only recently overdue. Loop status messages always go to stderr. `-v` /
`--verbose` may be repeated; extra debug tracing is emitted at `-vv`.

```bash
bob pomodoro tmux
```

Prints Pomodoro status in tmux status-line format: the regular status followed
by ` | `. Missing or stale Pomodoros produce no status. When today's daily
note has a Pomodoros section, the plan-budget meter `plan T/Tc · L/Lc` is
appended after the status (or alone when there is no status), wrapped in
`#[reverse]...#[noreverse]` when over the cap. With `BOB_CLI_USE_SCRIPT=1`
the script fallback stays budget-less.

## Compatibility shims

The installed legacy binaries map to the preferred interface as follows:

| Compatibility binary | Preferred command |
| --- | --- |
| `bob_notify` | `bob pomodoro notify` |
| `bob_pomodoro` | `bob pomodoro` |
| `tmux_bob_pomodoro` | `bob pomodoro tmux` |

By default they call the same native Rust implementations as the preferred
commands. With `BOB_CLI_USE_SCRIPT=1`, the notification and Pomodoro commands
and their shims delegate to their embedded shell assets. Native-only commands
ignore the fallback setting. Extracted assets are cached in a version-and-content-specific subdirectory of
`$XDG_CACHE_HOME/bob-cli/scripts/`. If `XDG_CACHE_HOME` is unset or empty, the
base is `$HOME/.cache`; if neither variable is available, Bob uses the system
temporary directory.

## Runtime dependencies

Native command execution does not require Bash or Perl. Forced shell fallback
with `BOB_CLI_USE_SCRIPT=1` requires Bash, and the Pomodoro-based fallback
scripts also require Perl.

The documented workflows use these external-tool integrations:

- `obsidian` CLI plus a running desktop Obsidian vault with the Dataview plugin
  only when using `bob query --engine obsidian`
- `git` for `bob vault-sync`, Git-backed `bob task archive`, plugin dirty-file
  checks, and the default `bob plugins` repository refresh; remote operations
  also need the credentials required by the configured remote
- `notify-send` for desktop notifications from `bob pomodoro notify`; Bob also
  rings the terminal bell whether or not `notify-send` is available
- platform clipboard tools for `bob capture` clipboard input: `pbpaste` on
  macOS; `wl-paste`, `xclip`, or `xsel` on Linux; or `tmux show-buffer` in a
  display-less tmux session (see `BOB_CLIPBOARD_CMD` below for the exact
  fallback order)
- `pandoc` and `xelatex` for `bob ref create` Markdown targets, plus the LaTeX packages `bob ref doctor` checks; override pandoc with
  `BOB_PANDOC_COMMAND`. Doctor prints the `tlmgr install` line for any missing packages
- `uv` and a discovered browser are optional for Mermaid fences in Markdown imports; without them, Mermaid prints as source with a visible note
- `curl` for `bob ref create` PDF URL and arXiv targets; override with
  `BOB_HIGHLIGHTS_CURL`
- A narration frontend for `bob ref create --listen`, configured as
  `highlights.listen_command` (`sase listen render ...`). Installing Bob does not install one. The SASE
  plugin (`sase plugin install listen`) provides
  `sase listen render {target} -e full -o {audio}`; a direct `sase-listen render ...`
  setting migrates automatically, and wrappers/shell programs are rejected
- `uv` plus a browser for `bob ref create` web article targets. `uv` fetches
  Python ≥3.10 and the pinned Playwright, Pillow, and nh3 on first run
  (override the whole spawn with `BOB_WEB_CLIP_ADAPTER`). Discovery uses
  Google Chrome at `/opt/google/chrome/chrome`, `/usr/bin/google-chrome`,
  `/usr/bin/google-chrome-stable`, or the macOS Google Chrome app, then
  Playwright's bundled Chromium. Set `BOB_CHROME` to another executable,
  including a system Chromium package. On Linux, `Xvfb` lets it retry
  bot-protected sites headed; on macOS the retry uses an off-screen window
- `bash` for the embedded shell fallback and for sourcing
  `~/.ssh-agent-thing`; the Pomodoro shell fallback additionally uses `perl`
- `uv` for `bob gkeep`: it fetches Python ≥3.10 and the pinned `gkeepapi` on
  first run (override the whole spawn with `BOB_GKEEP_ADAPTER`)
- `pass` for the default `bob gkeep` token store (`token_command` and
  `token_store_command` in the `gkeep:` config section)

No old chezmoi script files are required after installation. Cargo installs the
Rust binaries, and the binaries carry the script assets they need.

## Environment

`BOB_VAULT_SYNC_LOCK_FILE` overrides the lock path used by `bob vault-sync`,
`bob nightly`, live `bob task reconcile` runs, and `bob task reroll`.
The default path is the same shared `bob_sync.lock` path used by nightly
maintenance.

`BOB_VAULT_SYNC_STATE_FILE` overrides the JSON status record written and read
by `bob vault-sync`. The default is
`$XDG_STATE_HOME/bob-cli/vault-sync.json`, or
`$HOME/.local/state/bob-cli/vault-sync.json` when `XDG_STATE_HOME` is unset.

`BOB_CHROME` sets the browser executable for web article capture and Mermaid
rendering. When unset, discovery tries the Google Chrome paths above, then
Playwright's bundled Chromium.

`BOB_CLI_USE_SCRIPT=1` selects an embedded shell implementation where one is
available. See [Compatibility shims](#compatibility-shims) for the exact command
coverage and cache location.

`BOB_CAPTURE_TEMPLATES_FILE` overrides the exact saved-template shelf path
written after successful `@@template:` captures. The default is
`$XDG_STATE_HOME/bob-cli/capture/templates.json`, or
`$HOME/.local/state/bob-cli/capture/templates.json` when `XDG_STATE_HOME` is
unset.

`BOB_CLIPBOARD_CMD` is whitespace-split into a command and arguments and takes
priority over platform clipboard tools for `bob capture`. Without it, capture
uses `pbpaste` on macOS; on Linux it uses `wl-paste --no-newline --type text`
under Wayland or `xclip -selection clipboard -o` under X11, falling back to
`xsel --clipboard --output` when `xclip` is unavailable. A tmux session without
a display uses `tmux show-buffer`. Setting `BOB_CLIPBOARD_CMD` is also the
recommended deterministic automation and test hook.

`BOB_CLIPBOARD_HISTORY_CMD` is the portable clipboard-history provider for
counted captures above one. It is whitespace-split like `BOB_CLIPBOARD_CMD`,
receives the requested total count as its final argument, and must print a UTF-8
JSON array of complete clipboard strings ordered newest first. JSON framing
allows an entry to contain newlines. Bob reads the live clipboard separately,
removes at most the first equal history candidate, and then requires enough
older candidates to fulfill the exact count. A failed command, malformed JSON,
invalid entry, or insufficient result aborts the capture without vault writes.

Without that override, macOS reads Clipy's production `sqlite.db` history
read-only, validates the required schema, and reconstructs stored UTF-8 text
and file/URL assets rather than using Clipy's truncated display title. Other
platforms have no automatic history provider and report how to configure
`BOB_CLIPBOARD_HISTORY_CMD`; `%` and `%1` continue to use the portable live
clipboard source alone.

`BOB_CONFIG_FILE` sets the exact Bob config file. When unset, Bob uses
`$XDG_CONFIG_HOME/bob/config.yml`, then `~/.config/bob/config.yml`. That
file holds the priority windows for `$<N>` and `bob task reroll`,
`highlights.pre_scan_hook` for `bob ref scan` and
`bob ref doctor`, `highlights.listen_command` for
`bob ref create --listen`,
the `gkeep:` section for `bob gkeep`, and the
optional `plan:` and `freshness:` blocks. Plan caps default to 3 themes,
10 links, 15 NEXT tasks, 10 PENDING tasks, 100 dashboard READY tasks, and
5 Ready-lane tasks per area/project note. `bob plan` and `bob ready` exit 2
for invalid plan config. Capture and hooks warn and omit their budget report;
the tmux meter and Obsidian budget surfaces use defaults. Invalid freshness
config makes both `bob freshness` and `bob ready` exit 2. See
[plan configuration](docs/plan.md#config) and
[freshness configuration](docs/freshness.md#2-fields-overrides-and-config).

`COLUMNS`, when set to a positive integer, is the width `bob plugins list`
and `bob gkeep` use when they shorten human table text so each row fits.
Otherwise Bob uses 100 columns. `bob task reroll` lays its task lines out for
a fixed 100 columns.

`BOB_DATAVIEW_OBSIDIAN_COMMAND` overrides the executable used by
`bob query --engine obsidian`.

`BOB_DATAVIEW_VAULT` sets the default Obsidian vault name or ID forwarded to
`obsidian eval` by `bob query --engine obsidian`.

`BOB_DAY_FILE` sets the exact daily note path used by `bob pomodoro`,
`bob pomodoro tmux`, `bob pomodoro notify` (via the same status reader),
Pomodoro-linked and Pomodoro-note `bob capture` requests,
`bob task reconcile`, and `bob task reroll`. It also selects the ledger
read by `bob plan`, `bob freshness`, and `bob ready` when they evaluate
Today-linked work.
For `bob plan`, a dated daily-note filename also sets the report's evaluation
date; freshness and Ready use the current clock for their review date.

`BOB_DIR` sets the Bob vault directory. It defaults to `~/bob`.

`BOB_GKEEP_ADAPTER` is the path of an executable that speaks the Keep adapter
protocol and replaces `uv run --script …` for `bob gkeep`. It is the test
hook: tests point it at a fake adapter and configure everything else through
a temporary config file.

`BOB_HIGHLIGHTS_LIB_DIR` sets the Highlights PDF library directory used by
`bob ref`. It defaults to `lib` under `BOB_DIR`. Relative values are
resolved under the Bob vault; absolute paths and `~/...` paths are used as
configured.

`BOB_HIGHLIGHTS_PRE_SCAN_HOOK` overrides
`highlights.pre_scan_hook` from `~/.config/bob/config.yml` for
`bob ref scan` and `bob ref doctor`. Non-empty values run with
`sh -c` from `BOB_DIR` before intake; an empty value disables a configured
hook. `scan --dry-run` reports the hook it would run without executing it.
Pass `-n, --no-hooks` to ignore the hook from every source. The legacy
`BOB_HIGHLIGHTS_PRE_SCAN_COMMAND` variable is now an error. Bob exports
`BOB_HIGHLIGHTS_IN_PRE_SCAN_HOOK=1` to the hook child process.

`BOB_HIGHLIGHTS_CURL` replaces the `curl` program used by
`bob ref create` for PDF URL and arXiv targets. It is the test seam,
like `BOB_PANDOC_COMMAND`.

`BOB_HIGHLIGHTS_DIAGRAMS=off|0|false|no` skips Mermaid rendering in Markdown
imports and vault-note snapshots. Blocks print as source without a fallback
note; see [Mermaid diagrams](docs/highlights-create.md#mermaid-diagrams).

`BOB_HIGHLIGHTS_KEEP_WORKDIR=1` keeps the `bob ref create` scratch
directory for debugging and prints its path (the legacy
`BOB_WEB_CLIP_KEEP_WORKDIR=1` is also honored).

`BOB_HIGHLIGHTS_LISTEN_COMMAND` overrides `highlights.listen_command` in
`bob/config.yml` for `bob ref create --listen` across all target kinds (see
[create-target contract](docs/highlights-create.md#listen)).

`BOB_HIGHLIGHTS_REF_DIR` sets the generated reference note directory used by
`bob ref`. It defaults to `ref` under `BOB_DIR`.

`BOB_HIGHLIGHTS_XLIB_DIR` sets the Highlights PDF intake directory used by
`bob ref`. It defaults to `xlib` under `BOB_DIR`. `lib` and `xlib` must
be distinct, non-nested directories so intake cannot move PDFs inside the tree
being scanned.

`BOB_REF_JOBS_KICK=off|0|false` disables the detached worker started after
`bob capture` queues a link. Jobs remain durable; run `bob ref jobs run`
to process them. The spool lives under
`${XDG_STATE_HOME:-~/.local/state}/bob-cli/ref/jobs/`; see
[ref jobs](docs/ref-jobs.md) for recovery and the shared-spool limitation.

`BOB_NOW` overrides the local date and time used for Pomodoro status and default
daily-note selection by `bob pomodoro`, Pomodoro-linked capture, and
`bob task reconcile`. It pins today for `bob stats`. It also controls capture created/scheduled dates and
clipboard-snippet names, native Tasks-query date calculations, the default
`bob task archive YYYY-MM-DD` commit-message date, scheduled-project
visibility, plan/freshness/Ready evaluation, and the timestamped directory name
for plugin backups. Supported
formats are `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, and `YYYY-MM-DD HH:MM:SS`; `T`
may replace the space. Timezone names and UTC-offset suffixes are not accepted.
An unsupported value is ignored, after which Bob tries `DATE` and then the
system clock.

`BOB_PANDOC_COMMAND` overrides the pandoc executable used by
`bob ref create`.

`BOB_PLUGINS_DIR` sets the source repository used by `bob plugins`. It defaults
to `~/projects/github/bobs-org/bob-plugins`.

`BOB_PLUGIN_BACKUPS_DIR` sets the base directory for backups created before
`bob plugins sync` overwrites a vault plugin file. It defaults to
`~/.local/state/bob-cli/plugin-backups`.

`BOB_PRIORITY_ROLL_SEED` pins the `$<N>` scheduled-date roll to a decimal
integer seed so a `--dry-run` preview matches a real capture. Unset means each
capture rolls independently. `bob task reroll` also reads it as the default
base seed when `--seed` is omitted.

`BOB_WEB_CLIP_ADAPTER` is the path of an executable that speaks the web-clip
adapter protocol and replaces `uv run --script …` for web article capture,
collection composition, Mermaid rendering, and the `doctor` web-clip rows.
It is the test hook, like `BOB_GKEEP_ADAPTER`.

`BOB_WEB_CLIP_KEEP_WORKDIR=1` keeps the `bob ref create` web-article scratch
directory for debugging and prints its path.

`BOB_WEB_CLIP_TIMEOUT_SECS` sets the overall `bob ref create` web-article adapter
timeout in seconds. It defaults to 300; the first run may download the pinned
Python dependencies.

`DATE` preserves the legacy date override behavior, including the date used by
`bob capture` when `BOB_NOW` is unset. It can be a date command prefix such as
`date --utc`, or a timestamp in the same formats accepted by `BOB_NOW`.

`NO_COLOR` disables ANSI color in native human-readable output that would
otherwise be styled when stdout is a terminal.

`XDG_CACHE_HOME` is the cache root for extracted shell-fallback assets. See
[Compatibility shims](#compatibility-shims).

`XDG_CONFIG_HOME` is the base directory for the default
`$XDG_CONFIG_HOME/bob/config.yml` path used when `BOB_CONFIG_FILE` is unset.

`XDG_RUNTIME_DIR` is the preferred directory for the default
`bob_sync.lock` maintenance lock when that path exists as a directory.

## Migration notes

| Old spelling | Canonical path |
| --- | --- |
| `bob mark-next-tasks` | `bob task reconcile` |
| `bob move-done-tasks` | `bob task archive` |
| `bob notify` | `bob pomodoro notify` |
| `bob randomize` | `bob task reroll` |
| `bob task-status-hooks` | `bob task reconcile` |
| `bob task-status-setter` | `bob task reconcile` |
| `bob tmux-pomodoro` | `bob pomodoro tmux` |
| `bob highlights` | `bob ref` |
| `bob highlights-ref` | `bob ref` |
| `bob_pomodoro`, `bob_notify`, `tmux_bob_pomodoro` binaries | unchanged, same leaves |

Old spellings are permanent hidden aliases with byte-identical behavior and
no deprecation notices. New integrations should use the canonical names.

Reroll and archive commits are now labeled `bob task reroll` and
`bob task archive`; older history says `bob randomize` and
`bob move-done-tasks`.

The old top-level commands were renamed: `bob collect-done` is now
`bob task archive`, `bob dataview` is now `bob query`, `bob highlights-ref`
is now `bob ref` (and the `bob highlights` spelling stays as a permanent
alias). `bob sync`, `bob bulk-git-commit`, and the `bob_sync`
binary have been retired in favor of `bob vault-sync`. Those retired
top-level names are no longer registered.

The original script implementations remain embedded only as a rollback path.
New integrations should rely on the native Rust command behavior.

The retired `@<route>::<block-id>` capture spelling is no longer accepted; use
`@<route>^<block-id>` for an ordinary task with a requested block ID, and
`@<route>:<block-id>` for a Pomodoro-linked next task, optionally with
`#<pomodoro>` to select or create a named Pomodoro. Sub-bullet capture uses
`@<route>+<block-id>`.

## Release checklist

Run the package checks from a clean worktree:

```bash
just check
just check-scripts
just package-list
```

`just check` is the canonical verification gate: `cargo fmt --check`,
`cargo clippy --all-targets --all-features` with rustc unused and private-interface
warnings promoted to errors for bob-cli's targets, then
`cargo test --no-fail-fast`, which runs the lib, every `tests/*` binary,
and doctests even when one binary fails. `just test` runs the same
unmasked test step on its own. `just all` runs the same three checks through
separate recipes.

For documentation changes, build the Rust documentation and run doctests:

```bash
cargo doc --locked --no-deps
cargo test --locked --doc
git diff --check
```

There is no dedicated Markdown-check recipe. Review local Markdown links,
heading anchors, code fences, and command examples against the current CLI
help and implementation.

Run a local install smoke test:

```bash
just install-smoke
```

Run a tmux status smoke test after installing locally:

```bash
tmux display-message -p '#(bob pomodoro tmux)'
```

Before running `bob vault-sync` in a release smoke test, verify that `BOB_DIR`
points at the intended vault and that its Git remote can be pushed without
prompts. Before running `bob task archive` against the real vault,
verify that `~/bob` is the intended vault, inspect `git -C ~/bob status --short`,
and review any local edits that may be included when touched candidate files are
rewritten.

The default `bob query` smoke tests are local and headless. Before running
live Obsidian smoke tests, start desktop Obsidian, open the target vault, enable
Dataview, and use the explicit `--engine obsidian` examples in
[`docs/dataview.md`](docs/dataview.md).

For an end-to-end collection smoke test, install the local binary, run
`bob task archive` against `~/bob`, then verify that archive notes under
`~/bob/done` include `parent: "[[source]]"` for the original note and
`type: "[[done]]"`, source notes include matching `done_tasks` links and no
longer contain the collected blocks, Obsidian links to moved `^block-id` task
blocks point at `done/..._done#^block-id`, and the vault Git commit was pushed.

## Detailed command contracts

| Topic | Document |
| --- | --- |
| Capture grammar, JSON, and picker protocol | [`docs/capture.md`](docs/capture.md) |
| Article note formatting, vault reports, and ArticlePlan JSON | [`docs/articles.md`](docs/articles.md) |
| First capture, vault settings, command effects, and troubleshooting | [`docs/getting-started.md`](docs/getting-started.md) |
| Dashboard navigation and collection pages | [`docs/dashboard.md`](docs/dashboard.md) |
| Shell completion: runtime model, protocol 1, lifecycle, and troubleshooting | [`docs/completion.md`](docs/completion.md) |
| `bob query` Dataview and Tasks | [`docs/dataview.md`](docs/dataview.md) |
| Task freshness review lease, placement, evaluation, and display | [`docs/freshness.md`](docs/freshness.md) |
| Task date marks and calendar labels | [`docs/date-marks.md`](docs/date-marks.md) |
| Task tag marks for the #task tag | [`docs/task-tag-marks.md`](docs/task-tag-marks.md) |
| Today's plan, sticky lanes, dashboard READY, and per-note Ready caps | [`docs/plan.md`](docs/plan.md) |
| Daily Stats metrics, `bob stats`, JSON, the review journal, and the Obsidian Stats page | [`docs/stats.md`](docs/stats.md) |
| Derived Pomodoro homes, rollup, labels, chips, hues, and JSON contract | [`docs/pomodoro-homes.md`](docs/pomodoro-homes.md) |
| Highlights PDF intake and reference notes | [`docs/highlights-ref-sync.md`](docs/highlights-ref-sync.md) |
| `bob ref create` vault-note snapshots, Markdown, PDFs, and URL collections | [`docs/highlights-create.md`](docs/highlights-create.md) |
| Web article capture into Highlights intake PDFs | [`docs/highlights-clip.md`](docs/highlights-clip.md) |
| Reference lookup, reading queue, Today linking, renaming, and migration | [`docs/ref.md`](docs/ref.md) |
| Ref job spool, background worker, and fallback | [`docs/ref-jobs.md`](docs/ref-jobs.md) |
| Obsidian Sync folder exclusion runbook (historical) | [`docs/obsidian-sync-exclusions.md`](docs/obsidian-sync-exclusions.md) |
| Bob vault Git sync runbook | [`docs/vault-git-sync.md`](docs/vault-git-sync.md) |
| Custom plugin list and vault deploy | [`docs/plugins.md`](docs/plugins.md) |
| Project `^prj` lifecycle and schedules | [`docs/projects.md`](docs/projects.md) |
| Bulk re-roll of due prioritized tasks | [`docs/randomize.md`](docs/randomize.md) |
| Google Keep inbox drain into Obsidian tasks | [`docs/gkeep.md`](docs/gkeep.md) |
| Pomodoro-driven task status sync | [`docs/task-status-hooks.md`](docs/task-status-hooks.md) |
| Task dependency links: Depends-On line and project-dependency contract, plus conformance vectors | [`docs/task-dependencies.md`](docs/task-dependencies.md) |
