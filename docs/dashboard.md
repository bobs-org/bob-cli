# Dashboard navigation and child pages

The Dashboard (`dash.md`, alias Dashboard) is the calm place to choose
work, review its health, and open supporting collection and task pages.
Its navigation sits immediately after `# Dash`, above `## Tasks`, as
three labeled rows in exact reading and keyboard order: Work, Review,
Browse.

| Group | Badges, left to right | Purpose |
| --- | --- | --- |
| Work | TODAY · PENDING · NEXT · READY | Today's plan and the actionable lanes |
| Review | NEW · ROTTEN · CROWDED | Intake, aging work, crowded notes |
| Browse | PROJECTS · REFERENCES · BLOCKED | Open supporting collection and task pages |

Grouping is navigation only. It never changes the freshness walk, lane
semantics, caps, or review sequence, and the Projects collection badge
is not the freshness PROJECTS tier.

## Task badge warnings

PENDING and NEXT show the dashboard section counts, which exclude TODAY.
Their warning color follows the same visible count: a badge turns red only
when its section count is strictly above its cap. A section at the cap stays
normal-colored. Tooltips and accessibility labels still show the whole-lane
count, TODAY count, and any whole-lane excess. For example, NEXT `10/15` stays
normal-colored when the whole lane is `17/15` because seven tasks are in TODAY.
Daily `bob-plan` PENDING/NEXT badges use the same live section counts and
warning color, including on older daily notes. CLI warnings and navigation
notices continue to check the whole lane, including TODAY. Daily lint lines
name that whole-lane pressure explicitly.

BLOCKED is informational and has no cap. Its count stays available for
navigation and uses the same informational accent as PROJECTS and
REFERENCES regardless of its size.

## Child pages

Root-level notes with hierarchy from frontmatter `parent: "[[dash]]"`:

- `dash_projects.md` (alias Dashboard Projects): `# Projects`,
  `[[dash|← Dashboard]]`, one short active/waiting description, and
  `![[projects.base#🚀 Active & Waiting]]`.
- `dash_references.md` (alias Dashboard References): `# References`,
  `[[dash|← Dashboard]]`, one short reading-queue description, and
  `![[refs.base#🔖 Reading Queue]]`.

Neither child carries `type: [[project]]`, and no project or reference
note moves. The two `.base` files keep all views, filters, columns,
formulas, grouping, and sorting. The badge always describes the child's
default view, even if a visit switches the embedded table elsewhere.

## Count scopes

- PROJECTS links to `dash_projects`: project notes in the Active &
  Waiting view, counted once per path. Tooltip:
  `N projects in Active & Waiting. Open Projects.`
- REFERENCES links to `dash_references`: Markdown reference notes in
  the Reading Queue, counted once per path. Tooltip:
  `N references in Reading Queue (next, wip, ready). Open References.`

A successfully evaluated empty view is `0`. Incomplete metadata, a
failed source read, or an unrecognized view contract is `–` with a
short reason in the tooltip and accessibility label. One unavailable
collection never hides the other collection or any task badge.
Collection size is informational and never turns red.

## Source and view contract guard

`api.dashboardCollections` (namespace v1, top-level api stays v3)
mirrors the two default views without a general Bases interpreter.
Project type must resolve through Obsidian's link resolver to
`project.md`; a list type never counts. Project status follows Bases
`containsAny` (scalar substring, list exact elements). References use
exact case-sensitive status equality, the exact `ref/` prefix, and the
Markdown extension. No task-only Today, hide, freshness, schedule, or
dependency filters apply to either collection.

The plugin parses both `.base` files with the existing Obsidian
`parseYaml` dependency and validates only membership-affecting
structure, including any result limit, tolerating presentation-only
changes. A changed filter or renamed view marks that collection
unavailable instead of advertising a stale count. Predicate and
contract support must be extended together for a future filter change.
Base definitions load asynchronously outside the synchronous
`snapshot()`; metadata and vault events refresh open badges after a
short debounce, stale async completions are discarded, and widgets are
keyed by owning component and kind with cleanup on unload. There is no
per-badge polling, and `eval`, table DOM scraping, and private Bases
query APIs are never used.
