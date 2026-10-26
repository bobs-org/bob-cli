# Command catalog protocol

`bob __catalog` is a hidden, versioned JSON handshake. Obsidian command
mode (and any other client) uses it to learn every useful bob command,
including arguments, write policy, and how to assemble argv. The binary
is the source of truth: the plugin never hard-codes a command list.

The endpoint is intercepted beside `__complete`, before alias rewriting
and clap, so it is absent from help, the docs CLI catalog, and shell
completion. See [Shell completion](completion.md) § Protocol 1 for the
sibling TAB handshake.

## Handshake

Request:

```text
bob __catalog --protocol <N>
```

`--protocol=N` is accepted too.

- **Supported protocol.** Prints one JSON document to stdout and exits
  0. It never reads the vault, the config, or caches, and never writes
  anything.
- **Unsupported protocol.** Prints
  `{"schema_version":1,"ok":false,"code":"unsupported_protocol","min":1,"max":1,"message":"…"}`
  to stdout and exits 2.
- **Missing or malformed request.** Prints usage to stderr and exits 2.
  This binary speaks protocol 1 only.

## Success document

```json
{
  "schema_version": 1,
  "ok": true,
  "protocol": 1,
  "bob_version": "0.3.1",
  "features": [],
  "sections": [{"id": "daily", "title": "Daily workflow"}],
  "roots": ["articles", "capture"],
  "aliases": [{"from": ["task-status-hooks"], "to": ["task", "reconcile"]}],
  "entries": [],
  "excluded": [{"path": ["task", "move"], "reason": "…"}]
}
```

`bob_version` is this binary's crate version. `features` is reserved
for environment knobs a later bob can advertise (`clicolor-force`,
`progress-env`, `run-receipt`); protocol 1 currently sends `[]`.
Clients ignore unknown feature names.

`sections` are `daily`, `tasks`, `vault`, `integrations`, and `setup`,
matching root help without the Capture protocol group. `roots` are
every public top-level name in those sections plus hidden root alias
names (`highlights`, `task-status-hooks`, …).

## Entry fields

Each `entries[]` object:

| Field | Rule |
| --- | --- |
| `id` | Unique kebab-case |
| `path` | Canonical public leaf (`["ref", "create"]`) |
| `fixed` | Args always passed (`["--intake-only"]`, `["-f","json"]`, `["="]`) |
| `title` | Human title from the static table |
| `about` | First line of the leaf's clap about |
| `section` | Catalog id of the root's `SUBCOMMANDS` section |
| `keywords` | Lowercase lookup words |
| `effect` | `reads` \| `writes` \| `syncs` (writes plus Git commit/push) |
| `network` | Whether the command may use the network |
| `presentation` | `report` \| `action` \| `inline` |
| `policy` | `run` \| `preview-first` \| `confirm` |
| `preview` | Args inserted for a dry run, or `null` |
| `confirm` | Required text when `policy` is `confirm`, else `null` |
| `budget_secs` | Client-side run budget |
| `args` | Positionals: `name`, `slot`, `required`, `repeat` |
| `exit_codes` | Optional `[{code, outcome, meaning}]` hints |
| `adapter` | Plugin adapter id, or `null` |
| `hosts` | `[]` means every host; otherwise e.g. `["macos"]` |
| `invalidates` | `tasks` \| `themes` \| `relations` \| `stats` \| `references` \| `plugins` |

`args[].slot` is one of `area-note`, `article-note`, `metric`, `pdf`,
`ref`, `ref-query`, `ref-target`, `relation-label`, `text`, `theme`,
`url`. The table authors only the slot; name, required, and repeat come
from clap positionals, defaulting the slot to `text`.

## Argv assembly

The client builds argv as `path + (preview ? preview : []) + fixed +
user args`. Preview args always come before fixed args, so
`capture --dry-run -- -` works.

## Compatibility

Additive fields keep `schema_version` 1. Clients ignore unknown fields.
An unknown `effect` is treated as `writes`, an unknown `presentation` as
`action`, an unknown `policy` as `confirm`, and an unknown `slot` as
`text`. Incompatible changes bump the protocol.

## Inclusion

Every useful public leaf ships as an `entries` row or an `excluded`
row. Capture-protocol leaves (`capture-*`, `capture-templates` members)
are always excluded. Hidden leaves such as `freshness seed` appear in
neither list.

A path appears in both lists only for entries with non-empty `fixed` or
a `url` slot (the named capture rows). Raw typed argv on an `excluded`
path is refused with that row's `reason`. Entries may still exist on an
excluded leaf through their own rows: the Pomodoro session operators
and `ref-queue` on `capture`.

A typed argv whose longest path prefix is an entry inherits that
entry's metadata. `aliases` rewrite `from` to `to` before that lookup,
so `task-status-hooks` runs as `task reconcile`.

## Coverage

Tests classify every public clap leaf (after `tree().build()`, skipping
hidden subcommands and `help`):

- Every catalog `path` is a public leaf.
- Every `preview` and `fixed` option exists on that leaf as a long or
  short flag.
- Every public leaf outside Capture protocol is an entry or excluded.
- Ids are unique kebab-case; keywords are lowercase.
- `confirm` implies confirm text; `preview-first` implies `preview`.
- `reads` implies policy `run` and no preview.
- Each `section` equals its root's `SUBCOMMANDS` section.
- Every slot is from the known set.
- `aliases` resolve to entries, excluded paths, or public prefixes.
