//! Core model types and shared token interfaces for the capture grammar.

use serde::Serialize;
use std::num::NonZeroUsize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    Task,
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
        start: Option<PomodoroStartSpec>,
        close: Option<PomodoroCloseSpec>,
    },
    /// `@<route>^<block-id>+` (create the note only) or
    /// `@<route>^<block-id>+#<pomodoro>` (also pick the Pomodoro that
    /// ` :<id>` Task Links go under): create the project note
    /// `<route>_<block-id>.md`. `pomodoro_name: None` links ` :` tasks
    /// into the current/next Pomodoro; `Some` names the Pomodoro the ` :`
    /// links go under (created as a named future Pomodoro when missing).
    /// The `^prj` task itself is never linked.
    ProjectNote {
        block_id: String,
        pomodoro_name: Option<String>,
    },
    SubBullet {
        target: SubBulletTarget,
        section: Option<TaskSectionSelector>,
    },
    PomodoroNote,
    /// A bare `@route+block-id`, `@route+block-id!`, or
    /// `@route+block-id#pomodoro` marker with no other text on the item:
    /// update the existing task's status instead of capturing a new
    /// sub-bullet under it.
    TaskToggle {
        block_id: String,
        pomodoro_name: Option<String>,
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
        start: Option<PomodoroStartSpec>,
        close: Option<PomodoroCloseSpec>,
        spelling: PomodoroLinkSpelling,
    },
    /// A whole-item `=x` Pomodoro close. The item must contain only the
    /// close token; any extra text, marker, or child line is an invalid
    /// close, never a task.
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
    },
}

/// Which sigil spelled a solo Pomodoro-link item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroLinkSpelling {
    At,
    Caret,
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

/// Typed `@<route>:<block-id>=x` close specification. `x` is
/// case-insensitive; `raw` preserves what was typed.
///
/// A selection (`=x<N>`, `=x!<M>`, `=x<N>!<M>`) names numbered Task Links:
/// `in_progress` is `None` when no `<N>` list was typed (unlisted links keep
/// their ledger outcome) and `Some` (possibly empty for `=x0`) when one was;
/// `complete` holds the `!<M>` list, empty when none was typed. Both lists
/// are sorted ascending. Plain `=x` reports `in_progress: None` and an empty
/// `complete`, so version-tolerant readers see only additive fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PomodoroCloseSpec {
    /// Raw close token exactly as typed: the `=`-prefixed token (`=x1,3!2`)
    /// for a whole-item close, or the `=`-prefixed suffix for a link close.
    pub(crate) raw: String,
    /// Numbered links that stay in progress, or `None` when no `<N>` list
    /// was typed. `Some(vec![])` is an explicit `=x0` ("none").
    pub(crate) in_progress: Option<Vec<u32>>,
    /// Numbered links to complete, empty when no `!<M>` list was typed.
    pub(crate) complete: Vec<u32>,
}

impl PomodoroCloseSpec {
    /// A plain `=x` spec: no selection lists.
    pub(crate) fn plain(raw: String) -> Self {
        Self {
            raw,
            in_progress: None,
            complete: Vec::new(),
        }
    }

    /// `true` when the spec names at least one numbered Task Link.
    pub(crate) fn has_selection(&self) -> bool {
        self.in_progress.is_some() || !self.complete.is_empty()
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
}

/// How a marker-only `@route+block-id` capture should change an existing task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskToggleIntent {
    /// Two-way Ready/Blocked <-> Next toggle. Only the terminal
    /// `@route+block-id!` spelling carries this intent.
    Toggle,
    /// One-way ensure-Next plus Task Link relocation. Both unsuffixed
    /// marker-only forms (`@route+block-id` and `@route+block-id#pomodoro`)
    /// carry this intent; the optional Pomodoro name selects the destination.
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
pub(crate) struct TaskSectionSelector {
    pub(crate) text: String,
    pub(crate) exact: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthoredSubBullet {
    pub(crate) body: String,
    pub(crate) depth: AuthoredDepth,
    pub(crate) task_id: Option<ProjectTaskId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TerminalMarkers {
    pub(crate) clip: Option<ClipRequest>,
    pub(crate) scheduled_offset: Option<u64>,
    pub(crate) priority_level: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipRequest {
    Current { header: Option<String> },
    History { count: NonZeroUsize },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureItem<'a> {
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) lines: Vec<ItemLine<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureItem {
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) parsed: ParsedCaptureText,
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
    pub(crate) block_id: Option<String>,
    pub(crate) kind: CaptureKind,
}

impl ParsedGlobalDestination {
    pub(crate) fn mode_label(&self) -> &'static str {
        match self.kind {
            CaptureKind::SubBullet { .. } => "sub_bullet",
            _ => "task",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureDraft {
    pub(crate) global: Option<ParsedGlobalDestination>,
    pub(crate) items: Vec<ParsedCaptureItem>,
    pub(crate) warnings: Vec<String>,
}
