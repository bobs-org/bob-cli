//! Core model types and shared token interfaces for the capture grammar.

use serde::Serialize;
use std::num::NonZeroUsize;

use crate::native::url_routing::{RefIntent, UrlRoutingPolicy};

/// Grammar-level parse options shared by `bob capture` and
/// `capture-parse`. `url_routing: None` turns reference claiming off (for
/// `-R`, a config opt-out, or tests); production capture passes the loaded
/// routing policy. Only the lexical URL-list split stays live without it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureParseOptions<'a> {
    pub(crate) url_routing: Option<&'a UrlRoutingPolicy>,
    /// A forced destination or clip flag (`-r -s -t -S -c`, `--task-ref`)
    /// keeps the item a task.
    pub(crate) explicit_destination: bool,
    /// Whether lone `@route` / `@route#section` headers split into capture
    /// lists. `false` for forced-destination parses, where `@tokens` stay
    /// literal. Template rows always pass `false` via the split call.
    pub(crate) recognize_lists: bool,
    /// A `@@` declaration in the draft keeps every item a task. Draft
    /// resolution fills this in from the split declarations.
    pub(crate) has_global_destination: bool,
}

impl<'a> CaptureParseOptions<'a> {
    /// Routing-off options: reference items never claim (tests only;
    /// production passes the loaded policy or explicit `None`).
    #[cfg(test)]
    pub(crate) fn routing_off() -> Self {
        Self {
            url_routing: None,
            explicit_destination: false,
            has_global_destination: false,
            recognize_lists: true,
        }
    }

    /// Copy these options with the draft's `@@` presence folded in.
    pub(crate) fn with_global(&self, has_global_destination: bool) -> Self {
        Self {
            has_global_destination: self.has_global_destination
                || has_global_destination,
            ..*self
        }
    }
}

/// Editor-side parse options. The live editor has no forced flags, so it
/// only needs the routing policy plus the draft's `@@` presence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EditorParseOptions<'a> {
    pub(crate) url_routing: Option<&'a UrlRoutingPolicy>,
    pub(crate) has_global_destination: bool,
}

impl<'a> EditorParseOptions<'a> {
    /// Routing-off options: reference items never claim.
    pub(crate) fn routing_off() -> Self {
        Self {
            url_routing: None,
            has_global_destination: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    Task,
    /// A complete capture transfer. Execution refuses it until the staged
    /// transfer adapter lands.
    Transfer {
        sources: Vec<super::transfer::TransferSource>,
        arrow: super::transfer::TransferArrow,
        destination: super::transfer::TransferDestination,
    },
    /// A whole-item bare URL admitted by the routing policy, queued for
    /// the reading queue by the capture planner. The wrapped intent
    /// carries the classified URL plus whether `%l` requested companion
    /// audio.
    Ref(RefIntent),
    TaskWithBlockId {
        block_id: String,
    },
    Bullet {
        section_prefix: Option<String>,
        exact: bool,
    },
    Pomodoro {
        block_id: String,
        pomodoro_name: Option<String>,
        /// True when `pomodoro_name` came from a `#"NAME"` literal: resolve
        /// by key equality, never by slug or prefix.
        pomodoro_name_is_literal: bool,
        start: Option<PomodoroStartSpec>,
        close: Option<PomodoroCloseSpec>,
    },
    /// `@<route>^<block-id>@` (create the note only) or
    /// `@<route>^<block-id>@#<pomodoro>` (also pick the Pomodoro that
    /// ` :<id>` Task Links go under): create the project note
    /// `<route>_<block-id>.md`. `pomodoro_name: None` links ` :` tasks
    /// into the current/next Pomodoro; `Some` names the Pomodoro the ` :`
    /// links go under (created as a named future Pomodoro when missing).
    /// The `^prj` task itself is never linked.
    ProjectNote {
        block_id: String,
        pomodoro_name: Option<String>,
        /// True when `pomodoro_name` came from a `#"NAME"` literal.
        pomodoro_name_is_literal: bool,
        form: ProjectNoteForm,
    },
    SubBullet {
        target: SubBulletTarget,
        section: Option<DrawerSelector>,
    },
    PomodoroNote,
    /// A marker-only `@route+block-id` or `@route+block-id!` item: update
    /// the existing task's status instead of capturing a child bullet.
    TaskToggle {
        block_id: String,
        intent: TaskToggleIntent,
    },
    /// A whole-item `+N`/`-N` Pomodoro duration adjustment (for example
    /// `+5` extends today's current timed Pomodoro by 25 minutes). The item
    /// must contain only the signed count; any extra text, marker, or child
    /// line is an invalid adjustment, never a task. The count is optional
    /// and defaults to 1, so a bare `+` or `-` is one unit.
    PomodoroAdjust {
        spec: PomodoroAdjustSpec,
    },
    /// A whole-item `++N`/`--N` Pomodoro session shift (for example `++3`
    /// moves today's running timed Pomodoro 15 minutes later). The item
    /// must contain only the operator; any extra text, marker, or child
    /// line is an invalid shift, never a task. The count is optional and
    /// defaults to 1, so a bare `++` or `--` is one unit.
    PomodoroShift {
        spec: PomodoroShiftSpec,
    },
    /// A solo `@route:block-id[#pomodoro][=<X>]` or
    /// `^route:block-id[#pomodoro][=<X>]` Task Link: link the existing task
    /// into today's Pomodoro ledger (and optionally start that session).
    /// Valid only when the marker is the entire capture item.
    PomodoroLink {
        block_id: String,
        pomodoro_name: Option<String>,
        /// True when `pomodoro_name` came from a `#"NAME"` literal.
        pomodoro_name_is_literal: bool,
        start: Option<PomodoroStartSpec>,
        close: Option<PomodoroCloseSpec>,
        spelling: PomodoroLinkSpelling,
    },
    /// A whole-item `x` Pomodoro close, with optional Work Log bullets as
    /// child lines (`x` plus `- 1 wired the lexer` below it). Extra text on
    /// the close line is never a task.
    PomodoroClose {
        spec: PomodoroCloseSpec,
    },
    /// A whole-item `=`/`=<X>` Pomodoro start. The item must contain only
    /// the start token and have exactly one physical line; any extra text,
    /// marker, or child line on a counted token is an invalid start, never
    /// a task. The suffix mirrors the `se<X>` snippet timing.
    PomodoroStart {
        spec: PomodoroStartSpec,
        pomodoro_name: Option<String>,
        /// True when `pomodoro_name` came from a `#"NAME"` literal.
        pomodoro_name_is_literal: bool,
    },
    /// A whole-item `!note:block-id` task completion: complete the existing
    /// open task the vault-wide note locator names. The token must be the
    /// entire capture item; anything extra is a teaching error.
    TaskComplete {
        raw: String,
        note: String,
        block_id: String,
        quoted: bool,
    },
    /// A lone `@route` task-list header with no section, bullets, or other
    /// markers: pull that note's eligible tasks through the Pull-In
    /// protocol (`docs/task-dependencies.md` §12.20). The wrapped route is
    /// already lower-cased, matching task capture resolution.
    NotePull {
        route: String,
    },
    /// A whole-item `=/` Pomodoro split: split a merged Pomodoro's types
    /// into planned entries. The item must contain only the split token
    /// and have exactly one physical line; any extra text, markers, or
    /// child lines are refused with a teaching error, never a task.
    PomodoroSplit {
        spec: PomodoroSplitSpec,
    },
    /// A whole-item `=*` Pomodoro merge: absorb planned dest Pomodoros
    /// into a source. The item must contain only the merge token and
    /// have exactly one physical line; any extra text, markers, or
    /// child lines are refused with a teaching error, never a task.
    PomodoroMerge {
        spec: PomodoroMergeSpec,
    },
}

/// Which sigil spelled a solo Pomodoro-link item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroLinkSpelling {
    At,
    Caret,
}

/// Which sealed project-note form a `@<route>^<block-id>@` marker names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProjectNoteForm {
    Create,
    Offer,
}

/// Whole-item `+N`/`-N` adjustment: a sign plus a positive ASCII-decimal
/// magnitude in 5-minute units (`+5` is five units, 25 minutes). The count
/// is optional and defaults to 1 (a bare `+` is one unit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroAdjustSpec {
    /// Trimmed signed token exactly as typed (for example `+5`).
    pub(crate) raw: String,
    /// `true` for `+N`, `false` for `-N`.
    pub(crate) plus: bool,
    /// Number of 5-minute units (always positive; `+0`/`-0` is rejected).
    pub(crate) units: u64,
}

/// Whole-item `++N`/`--N` shift: a doubled sign plus a positive
/// ASCII-decimal magnitude in 5-minute units (`++3` is three units,
/// 15 minutes). The count is optional and defaults to 1 (a bare `++` is
/// one unit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroShiftSpec {
    /// Trimmed operator token exactly as typed (for example `++3`).
    pub(crate) raw: String,
    /// `true` for `++N` (later), `false` for `--N` (earlier).
    pub(crate) later: bool,
    /// Number of 5-minute units (always positive; `++0`/`--0` is rejected).
    pub(crate) units: u64,
}

/// A whole-item Pomodoro session operator: one sign resizes (moves the
/// end), two identical signs shift (moves the whole session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionOperator {
    /// One sign: `+N` extends, `-N` shortens.
    Resize { plus: bool },
    /// Two signs: `++N` moves later, `--N` moves earlier.
    Shift { later: bool },
}

/// Typed close specification: a whole-item `x` close or a
/// `@<route>:<block-id>=x` link close suffix. The whole-item `x` is
/// lowercase only and the link suffix `x` is case-insensitive; `raw` preserves
/// what was typed.
///
/// A selection (`x[<N>][*<P>][!<M>][~<K>]`, with `*`/`!`/`~` in any order)
/// names numbered Task Links: `in_progress` is `None` when no `<N>` list was
/// typed (unlisted links keep their ledger outcome unless `*<P>` is present,
/// which activates selection mode like `<N>`) and `Some` (possibly empty for
/// `x0`) when one was; `park` holds the `*<P>` list, `complete` the `!<M>`
/// list, and `drop` the `~<K>` list, each empty when none was typed. `park_all`
/// and `complete_all` retain a present group with no numbers; execution
/// expands its outcome over the remaining numbered Task Links. All concrete
/// lists are sorted ascending. Plain `x` reports `in_progress: None` and
/// empty `park`/`complete`/`drop`, so version-tolerant readers see only
/// additive fields.
///
/// Work Log bullets (`- [<n>] <text>` child lines under
/// `x[<N>][*<P>][!<M>][~<K>]`) append `log` entries: each names a
/// numbered Task Link plus the literal entry text and its nested details,
/// in typed order. A `- * <text>` star bullet logs to every remaining
/// worked task. Numbered all-or-none excludes star bullets; unnumbered
/// bullets log in order to the close's worked tasks. The text is final;
/// the grammar phase fills it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroCloseSpec {
    /// Raw close token exactly as typed: the bare token (`x1*2!3~4`) for a
    /// whole-item close, or the `=`-prefixed suffix (`=x1*2!3~4`) for a link
    /// or task close.
    pub(crate) raw: String,
    /// Numbered links that stay in progress, or `None` when no `<N>` list
    /// was typed. `Some(vec![])` is an explicit `x0` ('none').
    pub(crate) in_progress: Option<Vec<u32>>,
    /// Numbered links to park (record work without carrying), empty when no
    /// `*<P>` list was typed. Parked links get normal In Progress work
    /// effects but are not carried to the next placeholder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) park: Vec<u32>,
    /// `true` when a present `*` group omitted its numbers and selects the
    /// remaining numbered Task Links at execution time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) park_all: bool,
    /// Numbered links to complete, empty when no `!<M>` list was typed.
    pub(crate) complete: Vec<u32>,
    /// `true` when a present `!` group omitted its numbers and selects the
    /// remaining numbered Task Links at execution time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) complete_all: bool,
    /// Numbered links to drop, empty when no `~<K>` list was typed.
    /// Dropped links are removed from the closed session: not carried to
    /// the next placeholder and never started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) drop: Vec<u32>,
    /// Typed Work Log entries in typed order, empty when no tail was typed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) log: Vec<CloseLogEntry>,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// One typed Work Log entry on a close: the numbered Task Link it logs to
/// plus the literal, unescaped entry text and its nested detail lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CloseLogEntry {
    /// Numbered Task Link the entry is logged under. `None` means an
    /// unnumbered bullet under a close without `<N>`/`*<P>`, which
    /// `bob capture` resolves against the running session; omitted from
    /// JSON so `capture-parse` leaves it out until execution fills it.
    /// Star bullets keep `None` in `capture-parse` and expand to real
    /// indices in `bob capture`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) index: Option<u32>,
    /// Literal entry text, unescaped and final.
    pub(crate) text: String,
    /// Nested detail lines under the entry, in typed order; each is
    /// unescaped, final, whitespace-normalized text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) details: Vec<String>,
    /// `true` for a `- * <text>` star bullet that logs to every remaining
    /// worked task. Omitted from JSON when false. Always matches
    /// `origin == StarBullet`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) all: bool,
    /// Where the entry was typed: a child bullet or the close line itself.
    /// Skipped in JSON so `pomodoro_close.log` matches the bullet form.
    #[serde(skip)]
    pub(crate) origin: CloseLogOrigin,
}

/// Where a Work Log entry was typed. Bullets keep the existing messages;
/// inline entries word execution failures by origin (explicit number versus
/// default number).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum CloseLogOrigin {
    /// A `- <n> <text>` child bullet.
    #[default]
    Bullet,
    /// An unnumbered `- <text>` child bullet. `position` is the 1-based
    /// typed position among the close's entries; the task number comes
    /// from positional assignment, not from the bullet.
    PositionalBullet { position: u32 },
    /// A `- * <text>` child bullet that logs to every remaining worked
    /// task. `position` is the 1-based typed position among the close's
    /// entries.
    StarBullet { position: u32 },
    /// The close line's own text (`x wired the lexer`). `close_token` is
    /// the typed close token (`x`, `x1,3,4`); `explicit` is true when the
    /// entry typed its number; `default_index` is the lexical default used
    /// for escapes.
    Inline {
        close_token: String,
        explicit: bool,
        default_index: Option<u32>,
    },
}

impl PomodoroCloseSpec {
    /// A plain `x` spec: no selection lists.
    pub(crate) fn plain(raw: String) -> Self {
        Self {
            raw,
            in_progress: None,
            park: Vec::new(),
            park_all: false,
            complete: Vec::new(),
            complete_all: false,
            drop: Vec::new(),
            log: Vec::new(),
        }
    }

    /// `true` when the spec names at least one numbered Task Link.
    pub(crate) fn has_selection(&self) -> bool {
        self.in_progress.is_some()
            || !self.park.is_empty()
            || self.park_all
            || !self.complete.is_empty()
            || self.complete_all
            || !self.drop.is_empty()
    }

    /// `true` when the spec carries at least one typed Work Log entry.
    pub(crate) fn has_log(&self) -> bool {
        !self.log.is_empty()
    }
}

/// Session suffix on a `@<route>:<block-id>` marker: either a start
/// (`=<X>`) or a close (`=x`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionSuffix {
    Start(PomodoroStartSpec),
    Close(PomodoroCloseSpec),
}

/// Typed `@<route>:<block-id>[#<name>]=<X>` start specification, where `<X>`
/// mirrors the Obsidian bob-ledger-tools `se<X>` snippet suffix: empty,
/// unsigned ASCII digits, `-`, `-` followed by digits, or digits followed by
/// `-` and optionally digits. An omitted duration means five 5-minute units
/// (25 minutes); an omitted offset with `-` means one unit; no `-` means
/// zero offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroStartSpec {
    /// Raw `<X>` text after `=`, exactly as typed.
    pub(crate) raw: String,
    /// Number of 5-minute duration units.
    pub(crate) duration_units: u64,
    /// Number of 5-minute offset units.
    pub(crate) offset_units: u64,
    /// Queued lineup numbers to drop from the started session, empty when
    /// no `~<K>` list was typed. Every parser leaves it empty until the
    /// start-drop grammar lands; the lineup engine already applies it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) drop: Vec<u32>,
}

/// One `=/` split selector: a `#name` or `#"Literal"` type part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroSplitSelector {
    /// Decoded selector text (literal decoded, plain as typed).
    pub(crate) text: String,
    /// True when spelled as a quoted literal: whole-key match only.
    pub(crate) literal: bool,
}

/// Whole-item `=/` split specification: the optional bare base plus the
/// ordered `#type` selectors. `raw` is the trimmed token exactly as typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroSplitSpec {
    /// Trimmed split token exactly as typed (for example `=/build#fix`).
    pub(crate) raw: String,
    /// Optional bare base name after `=/` (for example `build` in
    /// `=/build#fix`). `None` selects the current Pomodoro.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base: Option<String>,
    /// Ordered split selectors after the base, in typed order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) selectors: Vec<PomodoroSplitSelector>,
}

/// Whole-item `=*` merge specification: the optional source plus the
/// ordered `#dest` selectors. An empty `dests` list means absorb the next
/// planned Pomodoro below the resolved source. `raw` is the trimmed token
/// exactly as typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroMergeSpec {
    /// Trimmed merge token exactly as typed (for example `=*build#fix`).
    pub(crate) raw: String,
    /// Optional source name after `=*`. `None` selects the current
    /// Pomodoro (`=*#fix`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) src: Option<String>,
    /// Ordered destination selectors after the source, in typed order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) dests: Vec<PomodoroSplitSelector>,
}

/// How a marker-only `@route+block-id` capture should change an existing task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskToggleIntent {
    /// Link-presence toggle: link when no matching Task Link sits under
    /// an open Pomodoro, unlink when one does. Only the terminal
    /// `@route+block-id!` spelling carries this intent.
    Toggle,
    /// One-way ensure-Next plus Task Link relocation. Both unsuffixed
    /// marker-only forms (`@route+block-id` and `@route+block-id#type`)
    /// carry this intent; an optional type marker retypes the task before
    /// choosing its destination.
    EnsureNext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubBulletTarget {
    BlockId(String),
    Ref { line: usize, digest: String },
}

/// Typed `@route+block-id#section` selector. A typed token is always
/// prefix-capable (`exact: false`); the forced picker option sets `exact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrawerSelector {
    pub(crate) text: String,
    pub(crate) exact: bool,
}

/// Where a reference item's parent token came from: an explicit `@route`
/// (leading, trailing, or forced `-r`), a `@@route` global declaration,
/// or the default inbox when no route was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefParentSource {
    Explicit,
    Global,
    Default,
}

impl RefParentSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Global => "global",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureText {
    pub(crate) body: String,
    pub(crate) clip: Option<ClipRequest>,
    pub(crate) route: Option<String>,
    pub(crate) kind: CaptureKind,
    pub(crate) scheduled_offset: Option<u64>,
    pub(crate) priority_level: Option<u64>,
    /// Normalized authored-child bodies plus their semantic depth, in source
    /// order, with their source marker and item-wide markers already removed.
    /// Empty when the item was an ordinary single-line capture.
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
    /// Complete `&note:block-id` modifiers on this item, in typed order.
    /// Empty for every input without dependency modifiers.
    pub(crate) dependencies: Vec<ParsedDependency>,
    /// Complete `*note:block-id` modifiers on this item, in typed order.
    pub(crate) blocked_tasks: Vec<ParsedDependency>,
    /// Complete `~` / `~~` modifiers on this item, in typed order.
    /// Empty for every input without relation modifiers.
    pub(crate) relations: Vec<ParsedRelation>,
    /// Ownership of this item's modifiers: a task this capture creates
    /// (`new_task`) or an explicitly selected existing task
    /// (`existing_task`). `None` when the item carries no modifiers, or
    /// when no dependent was named yet (draft resolution finishes it
    /// after `@@` inheritance, or reports the ownerless diagnostic).
    pub(crate) dependency_target: Option<ParsedDependencyTarget>,
    /// Explicit `#<type>` marker on this item's subject task. Planning
    /// resolves it against the selected destination and includes the type
    /// write in the capture transaction.
    pub(crate) type_marker: Option<super::type_marker::TypeMarker>,
}

/// One complete `~` / `~~` modifier on an execution item, in typed
/// order. Mirrors [`RelationEntry`](super::editor_model::RelationEntry)
/// without spans: the writer resolves `note`/`block_id` against staged
/// vault contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRelation {
    /// Exact typed text, sigil included.
    pub(crate) raw: String,
    /// `to` for `~`, `from` for `~~`.
    pub(crate) direction: super::editor_model::RelationDirection,
    /// Vault-relative note identity (quotes removed, escapes decoded).
    pub(crate) note: String,
    /// Related task block ID.
    pub(crate) block_id: String,
    /// Whether the note was a `"quoted component"`.
    pub(crate) quoted: bool,
    /// True when the label separator was `||`.
    pub(crate) hidden: bool,
    /// Normalised label, when `|label` was typed.
    pub(crate) label: Option<String>,
}

/// One complete `&note:block-id` modifier on an execution item, in typed
/// order. Mirrors the editor [`DependencyEntry`](super::editor_model::DependencyEntry)
/// without spans: the writer resolves `note`/`block_id` against staged
/// vault contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedDependency {
    /// Exact typed text, sigil included.
    pub(crate) raw: String,
    /// Vault-relative note identity (quotes removed, escapes decoded).
    pub(crate) note: String,
    /// Prerequisite block ID.
    pub(crate) block_id: String,
    /// Whether the note was a `"quoted component"`.
    pub(crate) quoted: bool,
}

/// Which task owns an execution item's dependency modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedDependencyTarget {
    pub(crate) kind: ParsedDependencyTargetKind,
    /// Dependent's route, when one is known.
    pub(crate) route: Option<String>,
    /// Existing dependent's block ID. `None` for a task this capture
    /// creates (its line is located by insertion, not by ID).
    pub(crate) block_id: Option<String>,
    /// Whether the owner route came from an inherited `@@route`
    /// declaration rather than an explicit item marker.
    pub(crate) inherited: bool,
}

/// Ownership vocabulary for [`ParsedDependencyTarget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParsedDependencyTargetKind {
    NewTask,
    ExistingTask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthoredDepth {
    First,
    Nested,
}

impl AuthoredDepth {
    pub(crate) fn level(self) -> u8 {
        match self {
            Self::First => 1,
            Self::Nested => 2,
        }
    }

    pub(crate) fn indent_units(self) -> usize {
        usize::from(self.level())
    }
}

/// A project-note task bullet's trailing ` :id` / ` ^id` token. `link` is
/// `true` for `:` (name it, make it Next, and link it into the Pomodoro)
/// and `false` for `^` (name it only). Always `None` outside project-note
/// items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ProjectTaskId {
    pub(crate) block_id: String,
    pub(crate) link: bool,
}

/// A first-level `- ^<id>` / `- :<id>` move bullet in a sealed project-note
/// item: move that open task from the parent note into this slot (`:` also
/// makes it Next and links it). Always `None` outside project-note items
/// and on nested bullets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveBullet {
    /// The move sigil: `^` or `:`.
    pub(crate) sigil: char,
    /// The moved task's block ID. `None` while the move is unfinished (a
    /// lone sigil), serialized as JSON null.
    pub(crate) block_id: Option<String>,
    /// UTF-8 byte range covering the sigil plus ID in the draft.
    pub(crate) range: (usize, usize),
    /// How the move was authored. Only `Bullet` exists until `cli-execute`
    /// splices synthetic `Choice` moves.
    #[serde(skip)]
    pub(crate) via: MoveVia,
}

/// How a move bullet was authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum MoveVia {
    /// An authored `- ^<id>` / `- :<id>` bullet.
    #[default]
    Bullet,
    /// A synthetic move spliced from a `- ^^` answer.
    Choice,
}

/// An exact `- ^^` choice marker in a sealed project-note item. The range
/// covers both carets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MoveChoiceBullet {
    /// UTF-8 byte range covering `^^` in the draft.
    pub(crate) range: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthoredSubBullet {
    pub(crate) body: String,
    pub(crate) depth: AuthoredDepth,
    pub(crate) task_id: Option<ProjectTaskId>,
    /// A first-level move bullet, when the shared move lexer claimed this
    /// line. Move bodies stay empty and their `task_id` stays `None`.
    pub(crate) move_task: Option<MoveBullet>,
    /// An exact first-level `- ^^` choice marker, when the shared move
    /// lexer claimed this line. Choice bodies stay empty and both
    /// `task_id` and `move_task` stay `None`.
    pub(crate) move_choice: Option<MoveChoiceBullet>,
    /// Leading `#slug` / `#"TITLE"` drawer shortcut, when this first-level
    /// bullet names a drawer. Literal ALL-CAPS drawer bullets gain this
    /// after grouping. Nested notes keep `None`.
    pub(crate) drawer: Option<DrawerSelector>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TerminalMarkers {
    pub(crate) clip: Option<ClipRequest>,
    pub(crate) scheduled_offset: Option<u64>,
    pub(crate) priority_level: Option<u64>,
    /// Retired `p:<digits>` token extracted from the terminal region.
    pub(crate) retired_priority: Option<RetiredPriority>,
    /// Consumed `\` bytes of `\$`/`\$<N>` escapes in the terminal region.
    pub(crate) priority_escapes: Vec<(usize, usize)>,
}

/// A typed retired `p:<N>` marker: the original token and its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetiredPriority {
    pub(crate) token: String,
    pub(crate) span: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipRequest {
    Current,
    History { count: NonZeroUsize },
}

impl ClipRequest {
    pub(crate) fn from_count(count: NonZeroUsize) -> Self {
        if count.get() == 1 {
            Self::Current
        } else {
            Self::History { count }
        }
    }

    pub(crate) fn count(&self) -> usize {
        match self {
            Self::Current => 1,
            Self::History { count } => count.get(),
        }
    }
}

pub(crate) struct RouteToken {
    pub(super) route: Option<String>,
    pub(super) kind: CaptureKind,
}

/// One whitespace-free token with UTF-8 byte offsets into the original,
/// un-normalized input. Spans are half-open `[start, end)` and always land
/// on `char` boundaries because the scanner walks `char_indices`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Token<'a> {
    pub(crate) text: &'a str,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// A token the marker walker can classify. `&str` tokens carry no position,
/// so the flat execution path discards the span list while the editor path
/// receives real byte ranges from [`Token`].
pub(crate) trait ParseToken {
    fn text(&self) -> &str;

    fn span(&self) -> Option<(usize, usize)> {
        None
    }
}

impl ParseToken for &str {
    fn text(&self) -> &str {
        self
    }
}

impl ParseToken for Token<'_> {
    fn text(&self) -> &str {
        self.text
    }

    fn span(&self) -> Option<(usize, usize)> {
        Some((self.start, self.end))
    }
}

/// Split the original input into maximal runs of non-whitespace characters,
/// recording each run's UTF-8 byte range. Whitespace classification matches
/// [`normalize_task_text`], so both paths agree on token boundaries.
pub(crate) fn tokenize_with_spans(raw: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;

    for (index, character) in raw.char_indices() {
        if character.is_whitespace() {
            if let Some(begin) = start.take() {
                tokens.push(Token {
                    text: &raw[begin..index],
                    start: begin,
                    end: index,
                });
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }

    if let Some(begin) = start {
        tokens.push(Token {
            text: &raw[begin..],
            start: begin,
            end: raw.len(),
        });
    }

    tokens
}

/// One physical line of raw capture input, with UTF-8 byte offsets into the
/// original, un-normalized text. A physical line's `text` never includes
/// its terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawLine<'a> {
    pub(crate) text: &'a str,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ItemLine<'a> {
    pub(crate) raw: RawLine<'a>,
    pub(crate) line_number: usize,
}

/// Whether a capture list files tasks or section bullets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ListKind {
    Tasks,
    Notes,
}

impl ListKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tasks => "tasks",
            Self::Notes => "notes",
        }
    }
}

/// Per-bullet context for items split from a lone `@route` / `@route#section`
/// header. Every bullet block of one list shares route, section, header
/// position, and count; `position` is 1-based within the list (`0` marks an
/// ownerless (orphaned or invalid) leading block that errors instead of
/// capturing). `header_start`/`header_end` cover the header's physical line
/// and `ownerless` never cross the JSON API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ListContext {
    pub(crate) kind: ListKind,
    pub(crate) route: String,
    pub(crate) section: Option<String>,
    pub(crate) header_line: usize,
    pub(crate) position: usize,
    pub(crate) count: usize,
    #[serde(skip)]
    pub(crate) exact: bool,
    #[serde(skip)]
    pub(crate) header_start: usize,
    #[serde(skip)]
    pub(crate) header_end: usize,
    #[serde(skip)]
    pub(crate) ownerless: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureItem<'a> {
    /// Complete source draft, retained so lexical claims can consult
    /// Markdown protection across physical and blank-separated lines.
    pub(crate) source: &'a str,
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) lines: Vec<ItemLine<'a>>,
    /// List context when this item is one bullet block split from a lone
    /// `@route` / `@route#section` header. `None` for ordinary items.
    pub(crate) list: Option<ListContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureItem {
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) parsed: ParsedCaptureText,
    /// List context threaded from the split item, so planning and output
    /// report each bullet's list. `None` for ordinary items.
    pub(crate) list: Option<ListContext>,
    /// The row that produced this item when it came from a template draft.
    pub(crate) template_row: Option<TemplateRowSummary>,
    /// Lexical ref-parent source for `Ref` items (`explicit` for an inline
    /// `@route` or forced `-r`, `global` for a plain `@@route` upgrade,
    /// `default` otherwise). `None` for non-`Ref` items.
    pub(crate) ref_source: Option<RefParentSource>,
    /// Source context for one member expanded from an execution target run.
    /// `None` for singleton items and non-run item kinds.
    pub(crate) fanout: Option<FanoutContext>,
}

/// Identifies one parsed member of a capture target run. Positions are
/// one-based in source order, and `line` is the source line that contained
/// the run (or the originating row for template expansion).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FanoutContext {
    pub(crate) line: usize,
    pub(crate) position: usize,
    pub(crate) count: usize,
    pub(crate) marker: String,
}

/// Draft-wide `@@` declarations plus the real capture items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureDraft<'a> {
    pub(crate) declarations: Vec<GlobalDeclarationToken<'a>>,
    pub(crate) items: Vec<CaptureItem<'a>>,
}

/// One `@@...` declaration token plus the original physical line it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlobalDeclarationToken<'a> {
    pub(crate) token: Token<'a>,
    pub(crate) line_number: usize,
}

/// A strict, executable global destination inherited by items with no local
/// route/mode marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedGlobalDestination {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line: usize,
    pub(crate) route: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureDraft {
    pub(crate) global: Option<ParsedGlobalDestination>,
    pub(crate) items: Vec<ParsedCaptureItem>,
    pub(crate) warnings: Vec<String>,
    pub(crate) template: Option<super::template::TemplateSummary>,
}

/// Additive execution metadata for one item expanded from a template row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateRowSummary {
    pub(crate) row: usize,
    pub(crate) line: usize,
}
