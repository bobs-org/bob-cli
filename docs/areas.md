# Areas

An area is an ongoing sphere of responsibility with no end state. It is a
vault Markdown file with frontmatter `type: "[[area]]"`. Children name their
parent through `parent:`; every area note with sub-areas, and the root note
`[[area]]` resolves to, carries a bob-owned nested [`## Areas`](#the-catalog)
catalog that `bob areas list` and `bob areas check` report.

## The root

The root is the note that the bare `[[area]]` link resolves to through the
vault's note index (exact `area.md`, then a unique case-insensitive
basename), outside `done/` and excluded directories. It has no frontmatter
requirement. It is a terminal sentinel: its own `parent:` is never followed
and it is never a catalog row, even when a fixture types it
`type: "[[area]]"`.

- If nothing resolves, `bob areas` reports one `area_root_missing` warning.
- If several notes match, it reports one `area_root_ambiguous` warning.
- In either case no root catalog is planned, and top-level areas whose
  parent targets `area` get no per-area parent finding: the single root
  finding covers them.
- Bob never creates the root file. `type.md` and other type notes never
  host.

Today's vault uses untyped `area.md`.

## Hosts and membership

**Hosts** are the root when it has at least one top-level area, plus every
area note with at least one valid child edge.

The children are notes classified as areas (outside `done/`, excluding the
root itself) whose scalar `parent:` resolves to the root or to another
area. Each area produces at most one parent finding. All parent and cycle
findings are errors; none of them creates a row.

Aliased (`[[love.md|love]]`), path, and heading link forms resolve the way
the Projects catalog resolves them.

For a cycle, the parent edges of the cycle members are dropped. Edges from
non-members into a member remain, so a bad ancestor never suppresses valid
edges beneath it: that member still hosts its valid children.

**Cleanup.** An area note, project note, or the root that holds an owned
`[!areas]` region but is not a host has that region removed. Examples are a
former host whose last child left, or an area retyped as a project. The
heading goes only when its section is then blank. Notes of any other kind
are never touched: an `[!areas]` callout there is authored content.

## Findings

Hard errors:

| Code | Trigger |
| --- | --- |
| `area_parent_missing` | No `parent:` value |
| `area_parent_list` | List-form or duplicate `parent:` keys (more than one parent) |
| `area_parent_unresolved` | Target resolves to no note (suppressed for `area` when the root finding fired) |
| `area_parent_ambiguous` | Target matches several notes |
| `area_parent_self` | Target is the area itself |
| `area_parent_kind` | Target is neither the root nor an area note |
| `area_cycle` | The resolved area-to-area walk revisits a note or exceeds 16 hops |
| `area_catalog_damaged` | Malformed `> [!areas` opener; carries the opener's line number |

Soft warnings:

| Code | Trigger |
| --- | --- |
| `area_root_missing` | The bare `[[area]]` link does not resolve |
| `area_root_ambiguous` | The bare `[[area]]` link matches several notes |
| `area_parent_unquoted` | Parent otherwise resolves, but uses the unquoted `[[x]]` spelling |
| `area_summary_long` | Summary exceeds 140 characters |
| `area_summary_multiline` | Summary spans multiple lines |
| `area_catalog_stale` | Catalog is missing or differs from the planned rows |

A missing summary is never a finding. Body prose is never scraped.
`area_catalog_stale` covers both missing and stale catalogs, with distinct
messages. Findings never rewrite a file.

## The catalog

Every host lists its whole descendant area forest, nested. Each row is:

```text
> {two spaces per depth}- **[[link]]**[ — summary]
```

The callout is `> [!areas]`, then a lone `>`, then the rows. That is the
same shape the Notebooks and Projects callouts use.

```markdown
## Areas

> [!areas]
>
> - **[[body]]**
> - **[[cash]]**
>   - **[[bank]]**
>   - **[[recur]]**
```

- **Link.** The note stem, or `path-without-md|stem` when the stem alone is
  ambiguous.
- **Summary.** Optional one-line `summary:` rendered as inert text. Titles
  and hues are never stored.
- **Order.** Siblings sort A–Z by stem, ignoring case, with a path
  tie-break.
- **Nothing else.** No glyphs, counts, dates, titles, or hues. Everything
  visual is drawn by clients.

**Placement.** The first write uses the `AREAS` region spec: heading
`Areas`, callout `areas`, before the first real H2, else at EOF. Project
catalogs sit after an existing `## Areas` section. On a host that gets
every catalog at once, the first-write outline is Areas, then Projects,
then Notebooks, then Albums, then Tasks.

**Heading adoption.** If a host has no owned region but has a real
(unfenced, after frontmatter) H2 titled `Areas`, the first write puts only
the callout at the end of that section. The heading is never duplicated,
and the first such heading wins.

**Ownership.** Bob owns exactly the `> [!areas]` callout bytes. The heading,
its position after the first write, and every other byte belong to the
note. Duplicate well-formed regions self-heal by keeping the first. A
malformed `> [!areas` opener is damage: that host is left byte-identical
and reported, while other hosts still apply. The BOM and CRLF endings
survive.

**States.** `missing` (required content, no region), `stale` (differing or
removable content), `current`, or `damaged`.

**Section protection.** `## Areas` is never a section-bullet target or
completion row: `@love#areas` refuses with `areas_section_generated`.

## Commands

`bob areas` lives in the Vault section of `bob --help`, alphabetically
between `albums` and `articles`. Bare `bob areas` defaults to the read-only
`list`.

```text
bob areas [list] [-b DIR] [-f human|json]
bob areas check [-b DIR] [-f human|json]
```

- Every long option has a short alias; options stay alphabetical.
- Findings do not make `list` nonzero. `check` exits 1 on any error
  (including a damaged catalog), 0 for warning-only or clean reports, and
  2 on usage errors.
- Real I/O failures are reported, never converted into a misleading clean
  scan.
- `list` and `check` are read-only: vault bytes, index, and HEAD are
  unchanged.

| Command | Exit |
| --- | --- |
| `list` | 0 |
| `check` | 1 on any error (including a damaged catalog), else 0 |

Human `list` prints `Areas · N areas · M catalogs`, the tree rooted at the
root's stem drawn with `├─ └─ │` connectors, then unplaced areas. Each row
shows a `●` in the area's hue (muted is dim) and the bold stem, then the
dim explicit title when `titled`, the dim ` — summary`, and dim chips
`N projects` and `● today` (direct counts, hidden at zero). The footer is
one of: `✓ N catalogs current`; `! N catalogs stale · run bob areas sync`;
or a red damaged line `path:line [code] message`. A findings summary
points at `bob areas check`.

Human `check` groups findings by path: errors as `✗`, warnings as `!`, each
`path:line [code] message`. Then a summary `N areas · E errors · W
warnings`, or `✓ areas match the contract` when clean.

Output respects `NO_COLOR` and piped output.

## JSON

Every envelope carries `schema_version: 1` and emits no ANSI escapes or
human preamble. List and check share one report shape; `check` uses
`"command": "check"`. `ok` is false when any error-severity finding exists.

```json
{
  "schema_version": 1,
  "ok": true,
  "command": "list",
  "root": { "path": "area.md", "stem": "area", "state": "found" },
  "areas": [
    {
      "path": "cash.md",
      "stem": "cash",
      "link": "cash",
      "title": "Cash",
      "titled": false,
      "summary": null,
      "parent": "area.md",
      "parent_raw": "[[area]]",
      "depth": 1,
      "top": "cash",
      "hue": "magenta",
      "hue_source": "color",
      "children": ["bank.md"],
      "projects": { "direct": 2, "subtree": 2 },
      "today": { "direct": 0, "subtree": 1 },
      "issues": []
    }
  ],
  "catalogs": [
    {
      "host": "cash",
      "path": "cash.md",
      "kind": "area",
      "state": "current",
      "rows": [
        { "path": "bank.md", "link": "bank", "depth": 0 }
      ],
      "issues": []
    }
  ],
  "findings": [],
  "counts": {
    "areas": 19,
    "top_level": 7,
    "hosts": 5,
    "errors": 0,
    "warnings": 0,
    "catalogs": { "current": 5, "missing": 0, "stale": 0, "damaged": 0 }
  }
}
```

- `root.state` is `found`, `missing`, or `ambiguous`; `root.path` is null
  unless the state is `found`.
- `areas[]` is tree order (depth-first from the root, siblings A–Z),
  followed by unplaced areas A–Z. `depth` is 1 for top-level areas;
  `depth` and `top` are null when there is no valid chain to the root.
  `parent` is the resolved path of a valid parent, else null.
- `title` comes from the same title derivation notebooks use. `titled` is
  true only when that title came from an H1 or an alias rather than the
  stem.
- `hue` and `hue_source` come from Pomodoro home resolution.
  `hue_source` is `color`, `muted`, or `hash`. Clients render the hue bob
  reports. The root has no hue.
- `projects.direct` counts non-terminal project notes whose nearest area
  on their parent walk is this area (through projects, notebooks, and
  albums, 16-hop cap). `subtree` adds descendant areas.
- `today` counts distinct Today tasks from `bob plan`'s daily-note
  pipeline. Each task is attributed to the nearest area on the home walk,
  including a valid `[root::]` override. A missing daily note yields
  zeros. Today data is report-only and never written.
- `catalogs[]` has one record per host or cleanup note, sorted by path.
  `kind` is `root`, `area`, or `project`. `rows[].depth` is relative to
  the host (0 is a direct child). `counts.hosts` counts catalogs with at
  least one row.

See `docs/reference/vault-format.md` for note types and
`docs/reference/json-outputs.md` for the report schema.
