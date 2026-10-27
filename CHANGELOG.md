# Changelog

All notable changes to Bob are documented here.

## [0.4.0](https://github.com/bobs-org/bob-cli/compare/v0.3.1...v0.4.0) - 2026-10-27

### Added

- *(ref)* add verified saved Markdown source attachment
- *(ref)* adopt orphan saved source stores
- *(ref)* verify every attach-source candidate and attach on exactly-one-pass
- *(ref)* coordinate annotated-overwrite archive moves under open PDFs
- *(plugins)* warn when a deployed plugin folder is not git-ignored or is tracked
- *(highlights-ref)* route capture replacement writers through guarded publish boundary
- *(ref)* add guarded stamped-PDF publication helper
- *(highlights-ref)* coordinate complete rename and id-repair transaction
- *(native)* add bounded save-and-move coordination
- *(ref)* add annotation-backed link recataloging (bob-cli-ct.3)
- *(ref)* add bob ref links read surface
- *(highlights-ref)* record URI link catalog with anchor pages in code.json
- *(ref)* land bob-cli-cc with code tests, store gaps, and human metadata
- *(stats)* add typical day to daily points
- *(task-deps)* retire legacy child dependency readers
- *(ref)* add bob ref attach-source with text-verified sources
- *(highlights-ref)* overwrite source store and doctor ref sources
- *(ref)* add saved code catalog read command
- *(cli)* honor color, progress, and exit receipts without a TTY
- *(highlights-ref)* write-once source store and source_markdown pointer
- *(catalog)* add hidden bob __catalog --protocol 1 handshake
- *(completion)* add theme and relation label providers
- *(highlights-ref)* build render-time code catalog
- *(highlights)* support coordinated annotated overwrite
- *(stats)* name more root parents in day-strip legend via short_label
- *(ref)* recover annotated overwrite transactions that renamed bundles
- *(ref)* plan annotated overwrite archives and highlight links
- *(homes)* let root: true make a project its own root parent
- *(highlights)* overwrite local and web routes with alias preservation
- *(highlights)* enable collection overwrite
- *(highlights)* enable single-URL overwrite across web capture routes
- *(highlights)* enable local-file and vault-snapshot overwrite
- *(ref)* apply staged overwrite in scoped transaction
- *(ref)* prepare safe in-place overwrite preflight
- *(highlights-ref)* durable overwrite-intent lifecycle with recovery
- *(highlights-ref)* add route guards for create/clip overwrite, redirect, and narrated hints
- *(highlights)* collection rereads use shared recapture preflight
- *(highlights)* derive birth earlier lineage and reading-aware ref task IDs
- *(highlights)* read-only family planning recapture with no reread writes
- *(highlights)* add force parser with reread/overwrite modes
- *(ref)* reading families in the library read model
- *(ref)* one already-captured refusal, before parent resolution
- *(stats)* add root_parent dimension to pomodoro homes and stats
- *(ref-doctor)* flag id/stem mismatches and duplicate ids with rename repair
- *(capture)* finish inbox routing with atomic snapshot and exact task identity
- *(capture)* add || one-off relation label syntax with hide-on-commit
- *(relations)* add verified label rename command
- *(capture)* route inbox answers through task-move engine
- *(capture-complete)* use cached relation-label catalog
- *(highlights-ref)* join annotation sections with #name tags
- *(relations)* shared preference journal, label catalog, and bob relations list/hide/unhide
- *(capture)* detect inbox route choices
- *(highlights-ref)* land annotation sections comment declarations with CI fix
- *(capture)* suggest recent themes with lossless names
- *(web-clip)* improve Gist author and title metadata
- *(themes)* add theme refs to capture-parse spans, agenda, plan, stats and capture output plus CLI and tmux swatches
- *(highlights)* section stubs, diagnostics, and route hints
- *(highlights)* sectioned annotation rendering with stable IDs
- *(themes)* add global theme rename transaction
- *(highlights)* parse reference sections
- *(themes)* add hide, unhide, and color journal writers
- *(capture)* support exact-name literals with docs and regression tests
- *(ref)* group bob ref show output by annotation sections
- *(highlights)* keep hard-wrapped hash lines in quotes and scope page headings by level
- *(themes)* add derived catalog and bob themes list
- *(highlights-ref)* parse sectioned managed regions
- *(highlights)* restore stale Highlights saves and report PDF write coordination
- *(time)* honor configured day timezone
- *(highlights-ref)* add scan JSON held fields, scan receipt, and Darwin notification
- *(highlights)* coordinate PDF marker writes with holds and human reporting
- *(native)* add bounded macOS file coordination
- *(highlights-ref)* stage PDF write policy and reading-activity probe
- *(rotten-log)* land Rotten Log consequence tests and docs

### Breaking

- *(highlights-ref)* [**breaking**] report dirty scan targets as per-PDF holds
- *(vault-sync)* [**breaking**] refuse online writes to a different vault
- *(plugins)* [**breaking**] refuse overwriting another checkout's uncommitted deploy
- *(ref)* [**breaking**] enable rereads for single files, URLs, and note snapshots
- *(homes)* [**breaking**] retire project rollup frontmatter
- *(capture)* [**breaking**] refuse unknown flags via capture_flag_guard

### Fixed

- *(ref)* keep birth source_markdown pointer on newborn notes
- *(refs)* preserve saved source pointers through ingest
- *(plugins)* record deploy stamp time in real UTC
- *(stats)* share the usable-day rule with typical day
- *(freshness)* refuse Pomodoro ledger rows at stamp placement
- *(ref)* repair StoreWriteParams.verified build break and restore green check
- *(ref)* land recapture epic with host-independent overwrite guards
- *(highlights-ref)* drop stale render_frozen_note args from source pointer test
- *(highlights)* resolve annotated overwrite landing findings
- *(vault)* keep note walkers out of .bob
- *(highlights)* hold replaced annotated captures
- *(highlights)* repair overwrite identity, narration, selection, and rollback proof
- *(highlights)* harden overwrite recovery, selection recheck, and failpoints
- *(highlights)* honor overwrite selection and refusal contracts
- *(ref)* name newest narrated reading and resolve exact-output overwrite paths
- *(ref)* contract docs, cleanup, and reread test hardening
- *(ref)* preserve family identity for rereads
- *(ref)* recheck locked reread intake occupancy
- *(ref)* number Markdown sections from the top heading level
- *(capture-complete)* order repeated completed start names newest-first
- *(ref-doctor)* accept the duplicate-avoiding _N id suffix
- *(ref-doctor)* fold reference ids ASCII-only and drop pandoc from the repair test
- *(highlights)* require pdftotext for diagram PDF assertions
- *(url-routing)* keep explicit non-default ports in dedupe and display URLs
- *(cli)* quiet EPIPE guard keeps usage-error exit 2 and panic output
- *(cli)* exit 0 quietly on EPIPE instead of panicking
- *(highlights-ref)* limit the %40 URL identity alias to Medium author paths
- *(relations)* constrain catalog sweep to owned files and older versions
- *(relations)* bound catalog cache and isolate its tests
- *(listen)* warn when a listen episode is queued instead of published
- *(pomodoro-themes)* bound cache and restore completion contracts
- *(stats)* mark unmet provisional booleans not yet
- *(capture)* resolve close headline after ledger edits
- *(highlights-ref)* normalize URL identity escapes for shared dedupe key
- *(ref)* cap tall Mermaid diagrams at 80% text height
- *(highlights-ref)* keep hard-wrapped quote lines inside color-grouped highlights
- *(highlights)* exclude color-grouped setext title-prefixed marker mirrors
- *(capture)* reuse completed multi-word pomodoro names without forking spelling
- *(highlights-ref)* keep heading-shaped wrap lines inside quotes
- *(task-status-hooks)* dedupe completed Task Link into occupied Pomodoro
- *(capture)* walk mixed short clusters before unknown-flag refusal
- *(ref)* keep the library spelling for PDFs reached through a symlink
- *(time)* run a DATE clock command in the configured timezone
- *(time)* date ref captures and Keep notes in the configured day timezone
- *(webclip)* embed static CID TrueType faces instead of Type 3
- *(ref)* resolve relative PDF paths before mapping
- *(highlights)* make the coordination acceptance self-test run without pandoc
- *(ref)* allocate unique IDs from output stems

### Performance

- *(query)* avoid unused Dataview index in native Tasks queries

## [0.3.1](https://github.com/bobs-org/bob-cli/compare/v0.3.0...v0.3.1) - 2026-10-23

### Added

- *(managed-log)* register Rotten Log kind with agenda and snapshot support
- *(ref)* expose annotation group pieces with seams in show
- *(highlights-ref)* render continuation pieces as tucked groups

### Fixed

- *(highlights-ref)* harden legacy migration coverage and grouping docs

## [0.3.0](https://github.com/bobs-org/bob-cli/compare/v0.2.0...v0.3.0) - 2026-10-22

### Added

- *(capture-related)* restore --exclude calibration pieces and regen docs
- *(capture-related)* calibrate similar-task bands from reviewed labels
- *(highlights-ref)* stabilize highlight IDs across sidecar order
- *(relations)* add pair planner, projection, and redundant warnings
- *(capture)* attach relation labels on typed pipe and expose relation in completions

### Breaking

- *(stats)* [**breaking**] make the net flow headline the window total so it matches its label

### Fixed

- *(ref)* launch discovered bundled Chromium for Mermaid

### Reverted

- *(relations)* back out bob-cli-am pair contract and planner

## [0.2.0] - 2026-10-20

### Added

- First versioned release of Bob and its companion command-line binaries.
