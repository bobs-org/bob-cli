---
create_time: 2026-06-03
status: research
topic: Obsidian to Logseq migration tradeoffs
---
# Research: Obsidian to Logseq Migration Tradeoffs

## Question

Should Bryan migrate from Obsidian to Logseq, and what are the practical tradeoffs
for the current Bob notes workflow?

## Short Answer

Do **not** fully migrate the Bob vault from Obsidian to Logseq right now.

The main reason is not generic feature preference; it is product direction and
workflow risk. As of June 3, 2026, Logseq is splitting into two products:

- **Logseq OG** for file-based Markdown graphs. This is the closest match to an
  Obsidian vault, but Logseq says it will be maintained for security and Electron
  upgrades rather than new feature development.
- **Logseq** for database graphs. This is the main product going forward, but it
  changes the storage model to local SQLite, has DB-specific importer/export
  limits, and is still described by the upstream repo as beta/nightly territory
  where data loss is possible.

For Bob specifically, Obsidian currently wins on stability, existing automation,
Dataview/Tasks support, custom plugins, headless Sync through `ob`, and plain
Markdown/YAML portability. Logseq is worth a bounded pilot if the goal is an
outliner-first, block-reference-heavy workflow, but not as the primary source of
truth for `~/bob` yet.

## Local Context

From SASE memory:

- `~/bob/` is Bryan's Obsidian vault.
- Local workflows use `obsidian-headless` through the `ob` command to support
  Obsidian Sync without a full GUI Obsidian session.
- New Markdown notes under `~/bob/` should include a `parent` frontmatter field
  linking to another Markdown file.

Observed vault configuration on 2026-06-03:

- Obsidian Vim mode is enabled.
- Obsidian line numbers are enabled.
- Obsidian auto-updates internal links on rename.
- Enabled community plugins:
  - `dataview`
  - `obsidian-tasks-plugin`
  - `templater-obsidian`
  - `quickadd`
  - `task-status-cycler`
  - `mrj-jump-to-link`
  - `bob-navigation-hotkeys`
  - `bob-ledger-tools`
  - `block-id-prompt`
  - `obsidian-relative-line-numbers`
  - `note-refactor-obsidian`

Count-only scans of `~/bob`:

- About 5,402 Markdown notes.
- About 838 common media/PDF assets.
- About 5,377 Markdown notes begin with YAML frontmatter.
- About 112 Markdown files contain Dataview-style usage.
- About 855 Markdown files contain Markdown task checkboxes.
- About 3,753 Markdown files contain Obsidian block IDs or block-reference-like
  syntax.

Implication: this is not just a folder of generic Markdown files. It is a large
Obsidian-shaped system with metadata, queries, task conventions, custom plugins,
and Sync automation.

## Product Model

### Obsidian

Obsidian stores notes as Markdown-formatted plain text files in a local vault.
The vault is a normal folder, and Obsidian watches external file changes. Note
properties are stored as YAML frontmatter at the top of each Markdown file.

Obsidian is not open source, but the core app is free for personal, commercial,
non-profit, educational, and government use. Obsidian's license page says data is
local by default, Obsidian Sync data is encrypted, and users retain ownership of
their content.

The current Obsidian direction still reinforces the file model:

- **Properties** are YAML frontmatter.
- **Bases** creates database-like views over local Markdown files and their
  properties; base views are saved as `.base` files or embedded in Markdown code
  blocks.
- **Headless Sync** exists as an official open beta via `obsidian-headless` and
  `ob`, which directly matches Bob's current automation model.

### Logseq

Historically, Logseq was the open-source, local-first, outliner-first alternative
that worked over Markdown or Org files. The file graph uses a Logseq-flavored
Markdown model: pages, journals, blocks, indentation, `property:: value`,
`[[page]]` references, `((block-id))` references, embeds, queries, tasks, and
block metadata.

The important 2026 change is that Logseq announced a split:

- **Logseq OG**: file-based Markdown graphs for users who prefer local Markdown
  files. Existing Markdown users are not forced to migrate, but this branch is
  planned for maintenance and reliability rather than new features.
- **Logseq**: database graphs, the main version going forward, aimed at better
  sync, collaboration, performance, integrity checks, and future platform work.

The DB version stores graph data under `~/logseq/graphs/GRAPH-NAME`, with
`db.sqlite` for graph data and `assets/` for assets. It has a CLI and scripting
story, but upstream still describes the DB version as beta while newer mobile and
real-time collaboration work is alpha. The README recommends backups and test
graphs because data loss is possible.

## Migration Paths

### Path 1: Obsidian to Logseq OG

This is the lower-disruption path because both tools can read Markdown files.
Logseq can create a graph from an existing Markdown directory and can find files
in subfolders; it creates `journals`, `pages`, and `assets` folders for Logseq's
own conventions.

Pros:

- Closest to the current Bob storage model.
- Keeps notes in visible Markdown files.
- Open-source AGPL app.
- Better native fit for outliner workflows, daily journals, backlinks, block
  references, and block-level queries.

Cons:

- Logseq OG is no longer the feature-forward product.
- Logseq Markdown is not the same as plain Obsidian Markdown. Important features
  use Logseq-specific syntax such as `property:: value`, block IDs, block refs,
  embeds, and query blocks.
- Obsidian YAML frontmatter does not map cleanly to Logseq page properties.
  Logseq page properties are the first block of a page, not YAML delimited by
  `---`.
- Dataview queries need to be rewritten as Logseq simple queries or advanced
  Datalog queries.
- Obsidian Tasks conventions and custom task statuses need testing and likely
  conversion.
- Bob's custom Obsidian plugins would need to be rewritten or abandoned.
- Logseq plugins are desktop-only in the file version; mobile/browser plugin
  parity is not equivalent to Obsidian desktop.
- Sharing the same live folder between Obsidian and Logseq is risky because both
  tools can mutate structure, metadata, links, and attachments differently.

### Path 2: Obsidian to Logseq DB

This is the strategic Logseq path, but it is a real data-model migration.

Pros:

- Aligns with Logseq's main future product.
- Local-first database graph.
- Better target for future performance, sync, collaboration, properties, views,
  CLI automation, and integrity checks.
- DB nodes unify page/block behavior more deeply than file-based Logseq.
- There is an official file-to-DB importer.

Cons:

- It is no longer a pure Markdown vault. The primary source of truth is
  `db.sqlite` plus assets.
- The DB importer is best-effort and has documented limitations. For example, a
  block with multiple simple queries, advanced queries, embeds, or quotes only
  imports one of those structures.
- Standard Markdown export from DB does not preserve all graph data. The docs say
  "Export as standard Markdown" does not include block properties and is unlikely
  to ever export timestamps or all properties.
- Org mode is no longer supported in the DB version.
- Several old built-ins are changed or removed in DB: whiteboards removed,
  Excalidraw/draw removed as built-in, Zotero no longer built-in, flashcard data
  not compatible with the previous implementation.
- The current DB release path still includes nightly builds and beta warnings.
- Bob would lose the simple invariant that `~/bob` is ordinary Markdown files
  plus YAML frontmatter.

## Feature Tradeoffs

### Writing Model

Obsidian is page/file-first. It works well for long-form Markdown notes, source
mode editing, file-oriented scripts, and treating notes as regular documents.

Logseq is block/outliner-first. Indentation is a core semantic feature: nested
blocks create parent-child relationships used by navigation, block references,
and queries. This is powerful if the unit of thought is a bullet or block, but it
can feel constraining for free-form prose, source edits, and external Markdown
tools.

For Bob: moving to Logseq would reward turning more of the vault into structured
outlines. If the current notes are mostly files with YAML metadata, Dataview, and
Obsidian plugins, that is a significant workflow change rather than a drop-in
editor replacement.

### Metadata

Obsidian:

- Properties are YAML frontmatter.
- Property types include text, lists, numbers, checkboxes, dates, date-times, and
  tags.
- Internal links in properties are supported with `[[Link]]` syntax, usually
  quoted in YAML.

Logseq file graph:

- Properties use `property:: value`.
- Page properties are the first block of a page.
- Block properties are properties on any other block.
- Property names are case-insensitive, lower-cased, and underscores are renamed
  to hyphens.
- Property values can create page references unless quoted or configured.

Logseq DB:

- Properties become database graph properties with richer behavior.
- Importer detects property types for number, date, checkbox, URL, node, and
  text, but conversion needs validation.

For Bob: YAML frontmatter is deeply present. Any migration needs an explicit
metadata conversion policy for `parent`, `tags`, `aliases`, date fields, boolean
fields, and any plugin-specific properties.

### Queries

Obsidian Dataview treats the vault as a database over Markdown pages, YAML
frontmatter, inline fields, tasks, and file metadata. Bob has observed Dataview
usage in about 112 files.

Logseq has simple queries and advanced Datalog queries. The query model is
block-oriented and does not accept Dataview DQL. Logseq DB has a CLI/query story,
but it is DB-specific.

Migration cost: every Dataview block that matters should be inventoried and
rewritten. This is likely one of the highest practical costs.

### Tasks

Obsidian task checkboxes are plain Markdown, often enhanced by
`obsidian-tasks-plugin` and related plugins. Bob has task checkboxes in about 855
files.

Logseq has native task markers such as `TODO`, `DOING`, `DONE`, `LATER`, and
`NOW` in the file version. The DB importer remaps some statuses: `LATER` to
`Todo`, `IN-PROGRESS` and `NOW` to `Doing`, and `WAIT`/`WAITING` to `Backlog`.

Migration cost: task status, recurrence, due/scheduled dates, priorities,
queries, and any custom task-status-cycler behavior need a dedicated conversion
test.

### Block References

Logseq's native advantage is block-level thinking. Block references and embeds
are core concepts.

Bob already has widespread block-ID or block-reference-like syntax in about
3,753 Markdown files. That suggests a real fit with Logseq's model, but the
syntax and semantics differ. Obsidian block IDs such as `^id` are not the same as
Logseq block UUID references such as `((uuid))`.

Migration cost: inspect how Bob currently uses block IDs. If they are mostly
Obsidian anchors for local links, conversion may be manageable. If they drive
custom automation, conversion is higher risk.

### Plugins and Custom Automation

Obsidian's plugin ecosystem is stronger for Bob today because Bob already uses
Dataview, Tasks, Templater, QuickAdd, note refactor, and custom Bob plugins.
Obsidian community plugins have broad permissions; Obsidian's own docs warn that
community plugins can access files, connect to the internet, and install
programs.

Logseq has plugins and themes, but the file-graph plugin docs say plugins are
desktop-only. The DB docs say 65+ plugins support DB graphs and that the JS
plugin SDK has DB-specific support, but this is still a smaller and more
transitioning ecosystem.

Migration cost: `bob-navigation-hotkeys`, `bob-ledger-tools`, `block-id-prompt`,
Dataview workflows, and command-line integrations would need replacements.

### Sync and Headless Workflows

Obsidian has a direct current fit for Bob:

- `ob sync` can run one-shot or continuous sync.
- `ob sync-config` supports modes such as bidirectional, pull-only, and
  mirror-remote.
- Headless Sync is explicitly useful for CI, agents, and automated workflows.
- It requires an active Obsidian Sync subscription and is currently open beta.

Logseq Sync is also encrypted and local-first-oriented, but the file-graph docs
still describe it as beta, available through active Open Collective contributors,
and warn not to combine it with third-party sync services such as iCloud,
Syncthing, or Dropbox. Logseq DB has a CLI and sync work, but this does not look
like a drop-in replacement for Bob's existing `ob` workflow today.

Migration cost: replacing `ob sync` is a major blocker for moving the primary
vault.

### Cost

Obsidian:

- Core app: free without sign-up.
- Sync: $4/user/month billed annually, or $5/user/month billed monthly.
- Publish: $8/site/month billed annually, or $10/site/month billed monthly.
- Catalyst: $25 one-time.
- Commercial license: optional, $50/user/year.

Logseq:

- Core app: open source, AGPL-3.0.
- Logseq Open Collective lists a $5/month Backer tier and $15/month Sponsor tier.
  Current docs describe Sync as beta and available to active Open Collective
  contributors.

Net: cost is not the decisive factor. Official sync is roughly comparable at the
individual level, but Obsidian's paid Sync is more directly integrated with Bob's
current automation.

## Recommendation

Keep **Obsidian as the system of record** for `~/bob`.

Run Logseq only as a bounded experiment:

1. Create a copy of a representative subset of the vault, not the live `~/bob`
   directory.
2. Test Logseq OG first if the goal is to experience the outliner/block workflow
   over Markdown.
3. Test Logseq DB separately if the goal is to evaluate Logseq's future, but
   treat it as a test graph only.
4. Do not share one live folder between Obsidian and Logseq until a repeatable
   conversion and conflict policy exists.

Revisit full migration only after these are true:

- Logseq DB has a stable, non-nightly migration path with a clearer backup/export
  story.
- Bob's `ob sync` dependency has a Logseq equivalent or a new automation design.
- The top Dataview queries have Logseq replacements.
- Obsidian Tasks workflows have Logseq replacements.
- Bob's custom Obsidian plugins have been ported, replaced, or declared
  unnecessary.
- A sample migration preserves frontmatter metadata, links, block references,
  tasks, and attachments with acceptable loss.

## Migration Smoke Test

Use this checklist before any serious migration decision:

- Clone `~/bob` to a throwaway test graph.
- Test 20 random notes with YAML frontmatter.
- Test 20 notes with `parent` metadata or parent-style links.
- Test 20 notes with block IDs or block references.
- Test 20 notes with task checkboxes and task metadata.
- Test every note containing a Dataview block.
- Test attachment rendering for PDFs and images.
- Test internal links after renaming a page.
- Test daily notes and date formats.
- Test Vim-mode editing, relative line numbers, and keyboard navigation.
- Test mobile sync on every device Bryan actually uses.
- Test headless/CLI sync or replacement automation.
- Export back out of Logseq and compare what survives.

## Sources

- [Obsidian: How Obsidian stores data](https://obsidian.md/help/data-storage)
- [Obsidian: Properties](https://obsidian.md/help/properties)
- [Obsidian: Bases](https://obsidian.md/help/bases)
- [Obsidian: Headless Sync](https://obsidian.md/help/sync/headless)
- [Obsidian: Plugin security](https://obsidian.md/help/plugin-security)
- [Obsidian: Pricing](https://obsidian.md/pricing.html)
- [Obsidian: License overview](https://obsidian.md/license)
- [Logseq: Home page](https://logseq.com/)
- [Logseq announcement: splitting into Logseq OG and Logseq DB](https://logseq.io/page/b2ad9ce1-9cb7-4436-8083-54cb4516d324/df4dc09d-0a12-4c87-904e-22a9bf4c350a)
- [Logseq docs: create a graph using existing Markdown files](https://github.com/logseq/docs/blob/master/pages/How%20to%20create%20a%20Logseq%20graph%20using%20existing%20Markdown%20files.md)
- [Logseq docs: indentation](https://github.com/logseq/docs/blob/master/pages/What%20is%20indentation%20and%20why%20does%20it%20matter%253F.md)
- [Logseq docs: Markdown](https://github.com/logseq/docs/blob/master/pages/Markdown.md)
- [Logseq docs: Properties](https://github.com/logseq/docs/blob/master/pages/Properties.md)
- [Logseq docs: Queries](https://github.com/logseq/docs/blob/master/pages/Queries.md)
- [Logseq docs: Plugins](https://github.com/logseq/docs/blob/master/pages/Plugins.md)
- [Logseq docs: Sync](https://github.com/logseq/docs/blob/master/pages/Logseq%20Sync.md)
- [Logseq docs: Sync encryption](https://github.com/logseq/docs/blob/master/pages/Logseq%20Sync%20Encryption.md)
- [Logseq docs: DB version](https://github.com/logseq/docs/blob/master/db-version.md)
- [Logseq docs: DB version changes](https://github.com/logseq/docs/blob/master/db-version-changes.md)
- [Logseq GitHub repo](https://github.com/logseq/logseq)
- [Logseq GitHub releases](https://github.com/logseq/logseq/releases)
- [Logseq OG GitHub repo](https://github.com/logseq/og)
- [Logseq Open Collective](https://opencollective.com/logseq)
