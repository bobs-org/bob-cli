# Changelog

All notable changes to Bob are documented here.

## [0.5.0](https://github.com/bobs-org/bob-cli/compare/v0.4.0...v0.5.0) - 2026-11-07

### Added

- *(gmail)* add confirmed send and note finalization
- *(gmail)* add durable send-recovery engine for compose drafts
- *(gmail)* add safe draft create and update flow
- *(gmail)* pull label mail and import attachments
- *(gmail)* link captured Gmail URLs without blocking capture
- *(gmail)* add compose render and attachment staging
- *(gmail)* swap Gmail URLs for labelled links, and backfill
- *(gmail)* add offline compose and reply notes
- *(gmail)* add search, peek, and remote counts
- *(gmail)* list saved mail offline and diagnose identity health
- *(gmail)* add transactional save and refresh
- *(gmail)* prepare thread with attachment_bytes transport
- *(gmail)* implement identity resolution and save policy
- *(capture)* execute atomic merge-in captures
- *(gmail)* implement mail note layout and refresh
- *(gmail)* repair renderer landing and finish hg.3.1
- *(capture)* complete merge-in feedback — pending context, copy, and stdin guard
- *(capture)* implement offer verdict surface with complete and cancel offers
- *(capture)* carry merge payload origins into task block previews
- *(gmail)* complete the pure message renderer and its acceptance corpus
- *(capture)* add merge-in source picker assistance
- *(capture)* add staged offer close descriptors
- *(gmail)* render HTML message bodies
- *(freshness)* order PRE and POST review stages
- *(gmail)* add bob gmail shell with config, login, logout, doctor
- *(periods)* support last-week, last-quarter, and last-year relative period ids
- *(gmail)* add inert output serialization boundary for gmail body
- *(capture)* add past-Pomodoro segment picker, dash rewrite, and submit guards
- *(gmail)* extract shared Google plumbing for bob-cli-hg.1
- *(capture)* flip relation direction with = anywhere in filter
- *(capture)* record pinned past-Pomodoro segments and ship grammar 14
- *(capture)* segment suffix lexer, pins, spans, and teaching errors
- *(capture)* add timeline catalog and pinned layout engine
- *(capture)* add relation dash rewrite aliases
- *(close)* implement explicit close-selection any-number with keep-clock
- *(clipboard)* add bob clipboard status
- *(gmail)* model gog threads and decode MIME bodies
- *(capture)* report clipboard history provider and entry ages
- *(gmail)* add offline Gmail URL identity parsing
- *(capture)* trust counted clipboard history only on live agreement
- *(types)* cover offclock lists in rename and split
- *(capture)* agree touched Pomodoro entries via shared applier
- *(capture)* add clipboard history providers (history boundary, Alfred and Clipy adapters)
- *(task-reconcile)* type-agreement pass in bob task reconcile
- *(clock)* add clock-type filter with strict offclock_types reader
- *(task-types)* shared agreement rule, close settle, and Rust ledger applier
- *(task-status-hooks)* coalesce credit-neutral Pomodoro sessions
- *(capture-pomodoros)* report max_task_link_count for x= close lists
- *(gcal)* refile moved meetings after Google updates
- *(plan)* leave off-clock Task Links out of the link budget
- *(capture)* alias / to # where a completion menu opens
- *(capture)* add glued Pomodoro start grammar
- *(capture)* alias comma to parent task picker
- *(periods)* rebuild brief Still open for ended periods
- *(capture)* add project-note move choice flow
- *(periods)* report brief truncation totals and clean words and aliases
- *(capture)* expand backslash hash alias in capture-rewrite
- *(capture)* alias move-bullet sigils and open the Move picker under offer seals
- *(capture)* enrich merge and move records, serialize merge offers, fix copy
- *(capture)* emit transfer landing blocks
- *(capture)* describe unfinished transfers in dry-run failures
- *(capture)* add route sigil alias rewrite
- *(capture)* list only closed tasks under capture-complete -C for task_goto
- *(capture)* support transfer task picker
- *(capture)* execute move_choice picks with mint, splice, offers and via
- *(capture)* complete past Pomodoro names
- *(capture)* add move_choice report, -m/--move-in and refusals
- *(capture)* add capture-complete -C/--closed-tasks for task_goto
- *(gcal)* complete past synced event tasks
- *(capture)* share one planner-true move consequence builder
- *(capture)* add transfer-assist completion, engine scoping, and rewrite
- *(capture)* optimize note-pull landing with move-aware ownership
- *(capture)* implement staged transfer merges
- *(capture)* implement transfer-moves planner with typed batch moves
- *(capture)* add transfer lexer and shell redirect guard
- *(capture)* add configurable merge relations
- *(task-merge)* ship bob task merge and bob task move -t
- *(capture)* execute adjacent task target runs
- *(task-merge)* complete planner integration
- *(capture)* complete and rewrite inside target runs
- *(capture)* repair project-note move landing integration
- *(capture)* show project-note moves and eligibility hints
- *(task)* add task split planner and CLI
- *(capture)* activate multi-target capture runs
- *(capture)* link moved tasks and integrate project offers
- *(capture)* combine editor target runs into one item with targets
- *(capture)* fan out selected target runs
- *(task-move)* shared transplant API, children classifier, drawer ensure, task locator
- *(capture)* execute ordered project-note moves
- *(capture)* add note-pull conformance evidence
- *(capture)* add shared target-run selection staging
- *(attachments)* record attachment origins and ship bob attachments
- *(pomodoro)* show last closed Pomodoro with --last
- *(capture)* present note-pull offers
- *(periods)* add agent reviews — brief, draft, and instruction refresh
- *(capture)* batch note-pull planning with ownership and ordering
- *(capture)* add ordered batch task moves
- *(capture!)* [**breaking**] lone @route pulls its note's ready tasks
- *(periods)* add period reports and stats ranges
- *(capture)* file clipboard notes directly under existing tasks
- *(periods)* adopt the period-note tree (migrate, list -p, nightly)
- *(periods)* add ISO period calendar, scaffolds, parser, ensure/list
- *(gcal)* read buffer reasons from description lines
- *(gcal)* report Google clock deprecations and document the completed contract
- *(gcal)* resolve inherited clocks for list and MEET stamps
- *(pomodoro-clock)* stamp the effective clock on capture writers
- *(gcal)* parse and render explicit clock tags
- *(clock-readers)* effective clock_source in agenda, settle, and stats drift
- *(capture)* reserve clock tags from type markers
- *(capture)* offer relation successors on Pending unlink
- *(pomodoro-clock)* resolve off-clock tags and note defaults
- *(capture)* emit ordered pull-in groups and relation human copy
- *(capture)* net relation offers across capture items
- *(capture)* plan parked successors on pomodoro close
- *(capture)* offer relation successors on managed Done
- *(capture)* share lazy relation source snapshots
- *(gcal)* follow Google description bullets until edited
- *(relations)* surface successor label effects
- *(gcal)* write event descriptions as Event Task bullets at birth
- *(gcal)* render calendar description bullets
- *(gcal)* filter vendor boilerplate from event descriptions
- *(capture)* merge the next planned Pomodoro by default
- *(capture)* place MEET continuation after last open pomodoro
- *(gcal)* parse calendar descriptions into structured lines
- *(gcal)* indent CANCEL LOG like the Task Card writer
- *(native)* add shared task_text_escape module for note escaping
- *(capture)* unnamed start joins the meeting in progress
- *(task-relations)* repair relation-successor ordering, identity, and recovery regressions
- *(task-relations)* merge relation offers into successor planning
- *(gcal)* sort event task links by meeting span
- *(capture)* round-trip spaced section selectors
- *(task-relations)* add pure relation-successor classifier
- *(task-relations)* add successor-label policy and shared copy
- *(task-relations)* document successor offer contract
- *(stats)* report worked and off-clock clock breakdowns
- *(capture)* add list block ID editor assists
- *(capture)* stamp session clock from counted Task Links
- *(stats)* evaluate Pomodoro metrics on worked, off-clock, and total clocks
- *(task-status-hooks)* settle ended off-clock event tasks into closed sessions
- *(capture)* retune shared block-ID suggestions from vault calibration
- *(off-clock)* add off-clock Pomodoro ledger support
- *(capture)* expose buffered calendar event agenda
- *(gcal)* buffer-aware sync spans, render, and docs
- *(capture)* honor event buffers in MEET runs and run-into-event warnings
- *(gcal)* parse event buffers from HTML-safe directive lines
- *(capture)* land keep-moving repairs with lazy routing and lane names
- *(capture)* bullet drawer completion menu, docs, and memory records
- *(capture)* narrow the closed Pomodoro name and shed the carried one on x
- *(capture)* several notes on one existing task
- *(capture)* net keep-moving batches, render human copy, and gate snapshot work
- *(types)* add merged-Pomodoro routing planners
- *(capture)* warn when a non-MEET session runs past an Event Task
- *(capture)* connect keep-moving planning and JSON contract
- *(capture)* start a MEET on its next event run
- *(plan)* report and print Today roots
- *(task-complete)* add leaf-project keep-moving successor engine
- *(capture)* add events to pomodoro task agenda
- *(capture)* log remaining Work Log targets with `- *` bullets
- *(capture)* resolve ^prj drawers against project-note H2 sections
- *(capture)* add directive completion and grammar revision
- *(capture)* enable @route:id@ project-note offer form end to end
- *(gkeep)* add bob gkeep status with bob-owned check gates
- *(capture)* plan project-note task offers through the colon-bullet writer
- *(gkeep)* persist host-local Keep status cache
- *(gcal)* reroute unedited Event Task roots when Google's route changes
- *(pomodoro-homes)* label home chips by root parent
- *(capture)* support plus block-ID separators
- *(gcal)* refresh the Events stamp on every new-minute sync
- *(gcal)* link new Event Tasks under MEET by phase
- *(gcal)* stamp each Events section with account and last change
- *(capture)* add task-wikilink ranker vectors and candidates tests
- *(gcal)* match real gog OAuth and finish guided login
- *(gcal)* guide Calendar login setup and recovery
- *(capture)* expose task-wikilink parse spans
- *(capture)* add task type completion context
- *(gcal)* bob-owned login sign-in with browser and paste flows
- *(capture)* retype existing tasks with t: and D8 relocation
- *(highlights)* link annotation tasks to reading task
- *(relations)* derive participle-by inverse display phrases
- *(task-mentions)* enable linked-from mentions by default
- *(capture)* stage task mention mirrors atomically
- *(capture)* complete type-marker grammar and writers
- *(capture)* enable t: type-marker writes for new tasks
- *(capture)* editor spans and transparent completion parsing
- *(capture)* parse task type markers
- complete task type workflow integration
- *(highlights)* pack short record fields to meet the page budget
- *(gcal)* three-way span updates, cancels, RSVP, series learning, list and doctor
- *(gcal)* add bob gcal sync
- *(task-move,capture)* gesture-retype keeps agreement on note moves and Pomodoro renames
- *(web-clip)* records and grid cleanup on print route
- *(gcal)* add calendar list command
- *(highlights)* improve grid table widths and row spacing
- *(highlights)* add configurable table layout for reference renders
- *(capture)* bare captures place links by task type
- *(gcal)* add bob gcal login, logout, doctor, and calendars
- *(capture)* close event tasks by meeting clock
- *(ref)* embed table-layout Lua pass for prose-table records
- *(task-relations)* project text mentions onto LINKED FROM
- *(types)* add task-type routing planners and type_mismatch lint
- *(task-relations)* parse and preserve text mentions
- *(types)* add bob types migrate for legacy theme spellings
- *(config)* add global default task type
- *(capture)* start template from trailing @@@ at any line end
- *(capture)* add wikilink_task (( completion contract
- *(types)* store task types as [type::] and task_type: with legacy theme reads
- *(types)* rename bob themes to bob types and adopt task-type words
- *(homes)* add [root::] field for as-if-residence attribution
- *(relations)* reverse relation-label renames with previewed edge reversal
- *(relations)* add complete-label CLI surfaces
- *(capture)* add task-theme capture writes
- *(capture)* land complete-label closeout with edge grouping, scoped warnings, and acceptance
- *(capture)* suggest task themes in completion and ref today
- *(themes)* rewrite note defaults and task fields on rename
- *(capture)* complete relation sources in the staged transaction
- *(themes)* publish PT13 pomodoro-theme contract and preservation regressions
- *(themes)* explain task suggestions and cache unknown note defaults
- *(relations)* add complete-label policy and verdicts
- *(relations)* add complete-label contract, reverse-rename, and shared vectors
- *(relations)* add cancel-source surfaces for capture-complete and relations list
- *(vault-sync)* quarantine displaced-plus-foreign-identity note clobbers
- *(capture)* cancel-label commits retire the source in the staged batch
- *(relations)* add cancel-label config parser and task_cancel engine
- *(themes)* add task theme suggestion model and field helpers
- *(themes)* scope Recent to configured themes and warn on unconfigured names
- *(relations)* add cancel-label contract, config docs, and shared vectors
- *(themes)* configured themes vocabulary, config-first hue rule, and catalog reports
- *(themes)* add per-entry split and skip re-adopted rename reports
- *(ref)* stage web overwrite sources instead of clearing
- *(web-ref)* multi-URL collections save one source and catalog
- *(web-ref)* single web articles save source.md and a catalog
- *(web-clip)* emit saved source views and code anchors

### Breaking

- *(capture)* [**breaking**] rewrite relation directions
- *(capture)* [**breaking**] glue start and past timing to Pomodoro names
- *(capture)* [**breaking**] complete merge-in grammar and teaching refusals
- *(capture)* [**breaking**] complete explicit close-selection coverage
- *(capture)* [**breaking**] counted %c history commands must list the live clipboard first
- *(capture)* [**breaking**] settle close names and retype tasks landing in created entries
- *(capture)* [**breaking**] glue past timing to the Pomodoro name
- *(plugins)* [**breaking**] accept committed dirty deploys via inclusion proof
- *(capture)* [**breaking**] whole-stack overlap guard and past edge repair
- *(capture)* [**breaking**] execute past Pomodoro stacks with explicit-entry record engine
- *(highlights)* [**breaking**] stamp minute-precision captured time in every ref create marker
- *(capture)* [**breaking**] record past Pomodoro grammar (x= lexer, capture-parse, refusal)
- *(capture)* [**breaking**] claim - ^^ move-choice marker with teaching errors
- *(capture)* [**breaking**] add transfer parsing contracts
- *(capture)* [**breaking**] complete target-run grammar and landing
- *(task-merge)* [**breaking**] retire sources with repointing and provenance
- *(capture)* [**breaking**] finish note-pull contract and landing
- *(capture)* [**breaking**] add project task move completion
- *(capture)* [**breaking**] parse move bullets in sealed project-note items
- *(freshness)* [**breaking**] bump freshness JSON to schema 15 with per-rung decay limits
- *(capture)* [**breaking**] repair list headers, discard ranges, and ID tuning
- *(stats)* [**breaking**] expose and document the Worked, Off-clock, and Total clocks
- *(stats)* [**breaking**] report clock-aware sessions and clock diagnostics
- *(capture)* [**breaking**] file each list bullet as its own capture
- *(capture)* [**breaking**] add =* merge syntax end to end
- *(capture)* [**breaking**] add =/ split syntax end to end
- *(capture)* [**breaking**] land named captures by merging types
- *(capture)* [**breaking**] stamp close Work Log entries
- *(capture)* [**breaking**] first-level `- #…` is a drawer shortcut (grammar 4)
- *(gcal)* [**breaking**] support #untracked calendar events end to end
- *(plan)* [**breaking**] retire the daily type cap and plan.strict
- *(pomodoro)* [**breaking**] omit untracked sessions from clocks and counts
- *(capture)* [**breaking**] full drawer completion menu and drawer vocabulary sweep
- *(capture)* [**breaking**] drawer writes, retype, and drawer JSON contract
- *(capture)* [**breaking**] glued #drawer capture with exact matching, on-demand creation, drawer menu, and capture-drawers
- *(capture)* [**breaking**] frame %t templates, retire @@template:, @@@, and the @@ template candidate
- *(capture)* [**breaking**] treat a bare edge # as an incomplete type marker
- *(capture)* [**breaking**] teach %n and %l directives, retire trailing # and URL %/@
- *(capture)* [**breaking**] add %c/%c<N> directive lexer and split clipboard reading
- *(capture)* [**breaking**] remove @@route+block-id
- *(capture)* [**breaking**] seal project notes with @ instead of +
- *(capture)* [**breaking**] replace t: type markers with #<type>
- *(ref)* [**breaking**] bob ref today places reading tasks by type
- *(capture)* [**breaking**] named Pomodoro captures keep the task type in agreement
- [**breaking**] retire GTD review buckets
- *(stats)* [**breaking**] keep every theme in JSON parts summaries
- *(assets)* [**breaking**] split embedded payloads into bob-obsidian-assets crate
- *(plan)* [**breaking**] make PLAN the review theme and add the <daily> root
- *(task-status-hooks)* [**breaking**] remove legacy_dependency_children from hooks JSON

### Fixed

- *(tests)* make macOS CI suites deterministic and green
- *(gmail)* read back gog 0.47.1 drafts and reply to your own sent mail
- *(gmail)* add compose to a read-only grant and parse empty headers
- *(gmail)* land the Gmail epic with sign-in hints, fresh guides, and no stale allowances
- *(gmail)* land compose epic without intermediate allowances
- *(capture)* repair successor verdict landing behavior
- *(capture)* land x= segment suffix docs and restore ~ in the shell-tilde refusal
- *(gmail)* close transactional save gaps
- *(task-complete)* repair successor verdict contracts and receipts
- *(capture)* repair shared offer-close contract and executable vectors
- *(capture)* restamp clocks after landing retypes
- *(memory)* complete the offclock_types decision record and isolate its gcal clock test
- *(consumption)* align reconcile and completion cleanup
- *(task-types)* land Pomodoro name agreement with green gates
- *(capture)* read the clipboard once per capture and bound clipboard commands
- *(capture)* restamp relocated pomodoros and align gcal clocks
- *(capture)* land past Pomodoros (x=) with name rules, exact rewrites, and green check
- *(highlights)* preserve fallback glyphs in generic ref PDFs
- *(periods)* attribute archived tasks and split multi-link minutes in brief
- *(capture)* keep note-pull groups on their requested source after moves
- *(capture)* drop unused AsRawFd import in shell guard
- *(capture)* stop transfer lexer claiming non-transfer arrows
- *(task-merge)* repair landing edge cases
- *(capture)* handle multi-target completion edge cases
- *(attachments)* handle wrapped paths and update catalog tests
- *(task-merge)* resolve payload preparation failures
- *(periods)* accept brief -f md and fill frontmatter-only period stubs
- *(periods)* insert missing daily parent on migrate; drop duplicate note_pull field
- *(capture)* initialize note pull in capture result
- *(capture)* remove duplicate note pull initializer
- *(capture)* initialize note pull result
- *(stats)* name effective clocks in drift copy and document offclock lints
- *(capture)* skip previous-daily relation sources by relative path
- *(capture)* restamp edited pomodoro lineups
- *(gcal)* keep description bullets put when the row no longer parses
- *(capture)* land merged Pomodoros with complete merge guards and docs
- *(gcal)* write title-free badge links, read location from Maps URL
- *(task-complete)* finish relation successor core
- *(capture)* keep implied merge dest identity through ledger edits
- *(capture)* round-trip section completion replacements to the picked heading
- *(stats)* repair clock drift lookup and date-aware overlap
- *(gcal)* finish event buffer epic closeout
- *(capture)* compose multi-note captures with relation closes and report the final parent
- *(capture)* keep agenda clippy, goldens, and gcal checks green
- *(capture)* refuse route-less sealed offers and keep offer mode while incomplete
- *(capture)* retype ID-less picker refs, bare-marker existing subject, near-duplicate override
- *(capture)* keep unpicked duplicate project offers after picks mint IDs
- *(capture)* finish project-offer landing repairs for bob-cli-ex.2.1
- *(gkeep)* bound adapter 429 retries and classify rate-limit 403s
- *(capture)* finish attached #<type> forms, refusal hints, and docs
- *(gcal)* repair Events stamp spacing and finish landing
- *(gcal)* keep a blank line under Events
- *(capture)* pin D8 relocation identity and named type-marker agreement
- *(ci)* restore release-tool and macOS lint/test jobs
- *(gcal)* reuse gog clients by piping normalized client JSON
- *(gcal)* land bob gcal with clippy fixes and integration docs
- *(ref)* reduce record spacing for table PDFs
- *(capture)* render rewrite help escapes literally
- *(capture)* skip URLs in block-ID suggestions
- *(homes)* accept spaced root names and keys per R11
- *(homes)* let the ^gtd link, not the PLAN name, decide review time
- *(themes)* accept trailing whitespace in rename and split verifiers
- *(highlights)* recognize manual heading numbers precisely
- *(themes)* repair task-theme scalars, placement, and unused-code gates
- *(capture)* align cancel-label output with §17 CL6/CL7
- *(vault-sync)* complete clobber-guard closeout quarantine
- *(ref)* explain copied reading tasks in scan failures
- *(themes)* set config_warning in the test-only Catalog::for_suggest
- *(homes)* label the daily root parent GTD
- *(themes)* preserve every byte outside renamed theme spans
- *(themes)* preserve split bytes, complete completion, and order journal re-adoption
- *(web-ref)* keep language-mermaid through Defuddle extraction
- *(ref)* stop pandoc numbering self-numbered Markdown headings
- *(web-ref)* catalog figure-wrapped code and keep anchor pages after a mismatch

### Performance

- *(capture)* narrow starts-after relation-source reads on capture close
- *(capture)* trim relation-successor overhead and re-measure apollo budgets
- *(install)* speed up just install with incremental local-install profile
- *(stats)* parallelize the index parse and reuse session scans

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
