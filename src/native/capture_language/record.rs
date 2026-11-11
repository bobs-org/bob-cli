//! Shared record lexical classifier for capture record notes.
//!
//! Phase `record-language` (bob-cli-ir.5.1.1) owns this file: route-first
//! and title-first `@dir/` / `@dir/^stem` claiming, the immutable route
//! policy callers build outside the grammar, opaque field/note parsing,
//! marker refusals, and editor spans. The `record-parse` phase maps
//! [`RecordNoteIntent`] onto the records engine; `record-writes` stages
//! the planned Markdown. This file never reads the vault or the config
//! itself.
//!
//! A record item is claimed before transfer, dependency, directive,
//! ref-job, type, task-ID, and generic task-bullet parsing, in both the
//! strict and the editor paths. Claimed records never parse as tasks and
//! their bodies are never repaired afterward.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::draft::{classify_authored_line, AuthoredLineClass};
use super::editor_model::{
    Diagnostic, EditorMode, Need, Severity, Span, SpanKind,
};
use super::item::parsed_capture_item_outcome;
use super::item::CaptureItemParseError;
use super::line::normalize_task_text;
use super::markers::{
    is_priority_marker_token, is_route_token, parse_schedule_token,
};
use super::model::*;
use crate::native::capture_links::is_protected_markup_range;
use crate::native::records::schema::RecordsConfig;

/// Strict refusal for task markers outside opaque field values.
pub(crate) const RECORD_MARKER_UNSUPPORTED_CODE: &str =
    "record_marker_unsupported";
/// Strict refusal for a missing record title.
pub(crate) const RECORD_TITLE_CODE: &str = "record_title";
/// Strict refusal for a malformed explicit stem.
pub(crate) const RECORD_STEM_CODE: &str = "record_stem";
/// Strict refusal for malformed record tails and orphaned child lines.
pub(crate) const RECORD_SHAPE_CODE: &str = "record_shape";
/// Strict refusal for nested bullets under a field bullet.
pub(crate) const RECORD_FIELD_NESTED_CODE: &str = "record_field_nested";
/// Strict refusal for an existing non-record folder used as a record route.
pub(crate) const RECORD_FOLDER_NOT_RECORD_CODE: &str =
    "record_folder_not_record";
/// Strict refusal for a configured type id used as its folder.
pub(crate) const RECORD_TYPE_AS_FOLDER_CODE: &str = "record_type_as_folder";
/// Strict refusal for record candidates under a malformed records block.
pub(crate) const RECORDS_CONFIG_INVALID_CODE: &str = "records_config_invalid";
/// Additive assist contract for named-field addressing and `record.edit`.
/// Present on parse record objects and record completion metadata.
pub(crate) const RECORD_ASSIST_VERSION: u32 = 1;

/// Immutable record route policy, built outside the grammar (which never
/// touches the filesystem or the config). Callers memoize one policy per
/// invocation: cheap lexical preflight runs first, record-specific I/O
/// (a few `is_dir` stats for candidate dirs only) runs only when the
/// draft holds a record candidate, and the per-folder record index is
/// never read here.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecordRoutePolicy {
    /// Lower-cased configured directory → record type id.
    dirs: HashMap<String, String>,
    /// Lower-cased record type id → its directory.
    type_dirs: HashMap<String, String>,
    /// Config file path for diagnostics, when known.
    config_path: Option<String>,
    /// Malformed `records` block message, when the config failed to parse.
    config_error: Option<String>,
    /// Parsed schema retained for the native capture adapter. The grammar
    /// itself only consults the directory/type maps above.
    records_config: Option<RecordsConfig>,
    /// Vault root used to build this policy, for invocation-local indexing
    /// by the native adapter.
    vault_root: Option<PathBuf>,
    /// Lower-cased existing vault-relative directories that are not
    /// record folders.
    existing_dirs: HashSet<String>,
}

impl RecordRoutePolicy {
    /// No `records` block: every record candidate stays literal task text.
    pub(crate) fn absent() -> Self {
        Self::default()
    }

    /// A malformed `records` block: every record candidate refuses with
    /// `records_config_invalid`.
    pub(crate) fn invalid(config_path: &Path, message: String) -> Self {
        Self {
            config_path: Some(config_path.display().to_string()),
            config_error: Some(message),
            records_config: None,
            vault_root: None,
            ..Self::default()
        }
    }

    /// Build a policy from a parsed records config.
    pub(crate) fn from_config(
        config: &RecordsConfig,
        config_path: Option<&Path>,
    ) -> Self {
        let mut dirs = HashMap::new();
        let mut type_dirs = HashMap::new();
        for record_type in &config.types {
            dirs.insert(
                record_type.dir.to_ascii_lowercase(),
                record_type.id.clone(),
            );
            type_dirs.insert(
                record_type.id.to_ascii_lowercase(),
                record_type.dir.clone(),
            );
        }
        Self {
            dirs,
            type_dirs,
            config_path: config_path.map(|path| path.display().to_string()),
            config_error: None,
            records_config: Some(config.clone()),
            vault_root: None,
            existing_dirs: HashSet::new(),
        }
    }

    /// Attach the caller-stat existing directories (lower-cased,
    /// vault-relative) that are not record folders.
    pub(crate) fn with_existing(mut self, existing: HashSet<String>) -> Self {
        self.existing_dirs = existing;
        self
    }

    /// Directory the candidate claims, when it is a configured record
    /// folder. Callers check this before any filesystem inspection.
    pub(crate) fn configured_dir(&self, dir: &str) -> Option<&str> {
        self.dirs.get(dir).map(String::as_str)
    }

    pub(crate) fn records_config(&self) -> Option<&RecordsConfig> {
        self.records_config.as_ref()
    }

    pub(crate) fn config_error(&self) -> Option<&str> {
        self.config_error.as_deref()
    }

    pub(crate) fn config_path_display(&self) -> Option<&str> {
        self.config_path.as_deref()
    }

    pub(crate) fn vault_root(&self) -> Option<&Path> {
        self.vault_root.as_deref()
    }

    pub(crate) fn with_vault_root(mut self, vault_root: &Path) -> Self {
        self.vault_root = Some(vault_root.to_path_buf());
        self
    }
}

/// Pure lexical scan of a draft for record-candidate directories: the dir
/// part of every `@dir/...` token, lower-cased, in first-seen order.
/// Callers stat only these candidates and only when non-empty, so
/// ordinary task drafts perform no record-specific I/O.
pub(crate) fn candidate_dirs(raw_text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for token in raw_text.split_whitespace() {
        let Some(rest) = token.strip_prefix('@') else {
            continue;
        };
        if rest.starts_with('@') {
            continue;
        }
        let Some((dir, _)) = rest.split_once('/') else {
            continue;
        };
        if dir.is_empty() {
            continue;
        }
        let dir = dir.to_ascii_lowercase();
        if seen.insert(dir.clone()) {
            candidates.push(dir);
        }
    }
    candidates
}

/// Bare, plain `@route` tokens at the end of a physical line. These need
/// the records schema for the `@dir` collection near-miss warning, but they
/// never trigger a record-folder stat or index read.
pub(crate) fn candidate_collection_routes(raw_text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for line in raw_text.lines() {
        let Some(token) = line.split_whitespace().last() else {
            continue;
        };
        let Some(route) = token.strip_prefix('@') else {
            continue;
        };
        if route.starts_with('@') || !is_route_token(route) {
            continue;
        }
        let route = route.to_ascii_lowercase();
        if seen.insert(route.clone()) {
            candidates.push(route);
        }
    }
    candidates
}

/// One parsed `key:: value` field bullet: exact key/value text plus
/// absolute UTF-8 source ranges. Repeated list bullets retain order;
/// scalar duplicates are refused by the records engine, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordFieldInput {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) key_range: (usize, usize),
    pub(crate) value_range: (usize, usize),
}

/// One note line inside a [`RecordNoteGroup`]: exact text plus its
/// absolute UTF-8 source range and any nested note lines below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordNoteLine {
    pub(crate) text: String,
    pub(crate) range: (usize, usize),
    pub(crate) children: Vec<RecordNoteLine>,
}

/// One note group: an optional ALL-CAPS section header plus the ordered
/// first-level note lines below it (each with nested note lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordNoteGroup {
    pub(crate) section: Option<String>,
    pub(crate) section_range: Option<(usize, usize)>,
    pub(crate) lines: Vec<RecordNoteLine>,
}

/// Whether a record route creates a note or updates an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordAction {
    Create,
    Update,
}

/// Parsed record intent: creation title or update parent-line note, folder,
/// target stem, raw field inputs, grouped notes, and absolute UTF-8 ranges.
/// The `record-parse` adapter maps this onto the records engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordNoteIntent {
    pub(crate) action: RecordAction,
    /// Lower-cased configured record directory.
    pub(crate) dir: String,
    /// Creation title or update parent-line note (route token removed).
    pub(crate) title: String,
    /// Explicit stem from `@dir/^stem`, when typed.
    pub(crate) explicit_stem: Option<String>,
    /// Existing target stem from `@dir/stem`, when updating.
    pub(crate) update_stem: Option<String>,
    pub(crate) fields: Vec<RecordFieldInput>,
    pub(crate) notes: Vec<RecordNoteGroup>,
    pub(crate) route_range: (usize, usize),
    pub(crate) title_range: Option<(usize, usize)>,
    pub(crate) stem_range: Option<(usize, usize)>,
}

/// Lexical record claim result: either a parsed creation intent or a
/// record-path refusal (never a fallback to task parsing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordClaim {
    Note(RecordNoteIntent),
    Refused {
        code: &'static str,
        message: String,
        span: Option<(usize, usize)>,
    },
}

impl RecordClaim {
    fn refused(
        code: &'static str,
        message: String,
        span: Option<(usize, usize)>,
    ) -> Self {
        Self::Refused {
            code,
            message,
            span,
        }
    }
}

/// One whitespace-delimited token with its absolute UTF-8 byte range.
struct RecordToken<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

/// Split text into whitespace-delimited tokens with absolute ranges.
/// `base` is the absolute offset of `text[0]`.
fn split_record_tokens(text: &str, base: usize) -> Vec<RecordToken<'_>> {
    let mut tokens = Vec::new();
    let mut token_start: Option<usize> = None;
    for (relative, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                tokens.push(RecordToken {
                    text: &text[start..relative],
                    start: base + start,
                    end: base + relative,
                });
            }
        } else if token_start.is_none() {
            token_start = Some(relative);
        }
    }
    if let Some(start) = token_start.take() {
        tokens.push(RecordToken {
            text: &text[start..],
            start: base + start,
            end: base + text.len(),
        });
    }
    tokens
}

/// A parsed `@dir/...` route token: the lower-cased dir plus the optional
/// explicit stem (`@dir/^stem`), update stem (`@dir/stem`), and the
/// absolute range of the whole token.
struct RecordRouteToken {
    dir: String,
    stem: RecordRouteStem,
    range: (usize, usize),
}

enum RecordRouteStem {
    /// `@dir/`: creation without an explicit stem.
    None,
    /// `@dir/^stem`: creation with an explicit stem.
    Explicit(String, (usize, usize)),
    /// `@dir/stem`: existing-record update target (no caret); only the
    /// absolute range is kept (text re-slices from the source).
    Update((usize, usize)),
}

/// Parse one whitespace-delimited token as a `@dir/...` route token.
/// Returns `None` when the token is not record-shaped at all.
fn parse_record_route_token(
    token: &RecordToken<'_>,
) -> Option<RecordRouteToken> {
    let rest = token.text.strip_prefix('@')?;
    if rest.starts_with('@') {
        return None;
    }
    let (dir, tail) = rest.split_once('/')?;
    if dir.is_empty() || !is_route_token(dir) {
        return None;
    }
    let stem = if tail.is_empty() {
        RecordRouteStem::None
    } else if let Some(stem) = tail.strip_prefix('^') {
        // Keep malformed caret stems in the record path so the record
        // diagnostic explains the allowed alphabet. Other nested route
        // tails (for example `@areas/a+f` or `@note#heading`) stay task
        // syntax and are not claimed here.
        let stem_start = token.start + 1 + dir.len() + 1 + 1;
        RecordRouteStem::Explicit(
            stem.to_string(),
            (stem_start, stem_start + stem.len()),
        )
    } else {
        // Existing stems are not constrained by the explicit-new-stem
        // alphabet. Keep task-route suffixes literal, but allow Unicode,
        // underscores, punctuation, and other filename characters; the
        // update resolver rejects unsafe path components.
        if !is_update_stem_token(tail) {
            return None;
        }
        let stem_start = token.start + 1 + dir.len() + 1;
        RecordRouteStem::Update((stem_start, stem_start + tail.len()))
    };
    Some(RecordRouteToken {
        dir: dir.to_ascii_lowercase(),
        stem,
        range: (token.start, token.end),
    })
}

/// Whether an on-disk stem can be addressed as `@dir/stem` in the capture
/// grammar. The update resolver still owns safety; this only mirrors the
/// lexical stem alphabet so completion and rewrites never offer a row that
/// no longer parses as that target (for example a stem with spaces).
pub(crate) fn is_addressable_update_stem(stem: &str) -> bool {
    !stem.is_empty()
        && stem.chars().all(|character| {
            !character.is_whitespace()
                && !matches!(
                    character,
                    '\0' | '@'
                        | '^'
                        | '+'
                        | '#'
                        | ':'
                        | '!'
                        | '%'
                        | '~'
                        | '&'
                        | '*'
                        | '='
                        | ','
                )
        })
}

fn is_update_stem_token(stem: &str) -> bool {
    is_addressable_update_stem(stem)
}

/// Explicit stems use the block-ID alphabet: ASCII letters, digits,
/// hyphens. Mirrors the records engine without reimplementing it.
fn is_valid_record_stem(stem: &str) -> bool {
    !stem.is_empty()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Where the record route token sits on the parent line.
enum RecordRoutePosition {
    /// First token: `@dir/ <title>` or `@dir/^stem <title>`.
    RouteFirst,
    /// Last token: `<title> @dir/` or `<title> @dir/^stem`.
    TitleFirst,
}

/// Locate the record route token on the parent line: the first token
/// wins (route-first); otherwise the last token (title-first). A bare
/// `@dir/` is both. Middle tokens never claim, so prose mentioning a
/// route mid-line stays literal.
fn find_record_route_token(
    parent_text: &str,
    base: usize,
) -> Option<(RecordRouteToken, RecordRoutePosition)> {
    let tokens = split_record_tokens(parent_text, base);
    if tokens.is_empty() {
        return None;
    }
    let mut first = 0;
    while first < tokens.len() && is_edge_task_marker(tokens[first].text) {
        first += 1;
    }
    if first < tokens.len()
        && let Some(route) = parse_record_route_token(&tokens[first])
    {
        return Some((route, RecordRoutePosition::RouteFirst));
    }
    let mut last = tokens.len();
    while last > 0 && is_edge_task_marker(tokens[last - 1].text) {
        last -= 1;
    }
    if last > 0
        && let Some(route) = parse_record_route_token(&tokens[last - 1])
    {
        return Some((route, RecordRoutePosition::TitleFirst));
    }
    // A single-token parent is both first and last; the check above
    // already handled it as route-first.
    None
}

/// Recognized task markers may sit beside a record route at either edge.
/// They still cause a record refusal after the route is claimed; this
/// predicate only lets them expose the route to the shared classifier.
fn is_edge_task_marker(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if parse_schedule_token(token).is_some()
        || is_priority_marker_token(token)
        || classify_region_token(token).is_some()
    {
        return true;
    }
    if token.starts_with('#') {
        return super::type_marker::scan_line(token, 0)
            .iter()
            .any(|occurrence| occurrence.is_marker());
    }
    if token.starts_with('%') {
        let template_marker = super::directive::template_tokens(&[RawLine {
            text: token,
            start: 0,
            end: token.len(),
        }]);
        return !template_marker.is_empty()
            || matches!(token, "%c" | "%n" | "%l");
    }
    if matches!(token.as_bytes().first(), Some(b'&' | b'*' | b'~')) {
        return true;
    }
    false
}

/// What a record-shaped route token resolves to under the policy.
enum RouteResolution {
    /// A configured folder: claim creation.
    Configured,
    /// A teaching refusal (malformed config, type id, or existing
    /// non-record folder).
    Refused(RecordClaim),
    /// An unknown folder: stay literal task text.
    Literal,
}

/// Resolve a record-shaped route token against the policy: configured
/// folders claim creation, type ids and existing non-record folders
/// refuse with teaching copy, unknown folders stay literal, and
/// malformed config refuses every candidate.
fn resolve_record_route(
    route: &RecordRouteToken,
    policy: &RecordRoutePolicy,
) -> RouteResolution {
    if let Some(detail) = policy.config_error.as_deref() {
        let path = policy.config_path.as_deref().unwrap_or("<config>");
        return RouteResolution::Refused(RecordClaim::refused(
            RECORDS_CONFIG_INVALID_CODE,
            format!(
                "`@{}/` cannot claim a record folder: records config `{path}` \
                 is invalid: {detail}; fix the `records:` block and retry",
                route.dir,
            ),
            Some(route.range),
        ));
    }
    if policy.configured_dir(&route.dir).is_some() {
        return RouteResolution::Configured;
    }
    if let Some(actual) = policy.type_dirs.get(&route.dir) {
        return RouteResolution::Refused(RecordClaim::refused(
            RECORD_TYPE_AS_FOLDER_CODE,
            format!(
                "`@{}/` names a record type id, not a folder: write \
                 `@{actual}/` (or `<title> @{actual}/`) instead",
                route.dir,
            ),
            Some(route.range),
        ));
    }
    if policy.existing_dirs.contains(&route.dir) {
        return RouteResolution::Refused(RecordClaim::refused(
            RECORD_FOLDER_NOT_RECORD_CODE,
            format!(
                "`@{}/` is not a record folder: `{}/` exists but no record \
                 type writes there; drop the trailing `/` to capture a task",
                route.dir, route.dir,
            ),
            Some(route.range),
        ));
    }
    RouteResolution::Literal
}

/// Classify one capture item as a record creation or a record-path
/// refusal. Returns `None` when the item is not record-shaped (or no
/// policy is configured), so the caller continues with ordinary task
/// parsing and the text keeps its existing literal meaning.
pub(crate) fn classify_record_item(
    item: &CaptureItem<'_>,
    policy: Option<&RecordRoutePolicy>,
) -> Option<RecordClaim> {
    let policy = policy?;
    let parent = item.lines.first()?.raw;
    if normalize_task_text(parent.text).is_empty() {
        return None;
    }
    let (route, position) = find_record_route_token(parent.text, parent.start)?;
    match resolve_record_route(&route, policy) {
        RouteResolution::Refused(refusal) => return Some(refusal),
        RouteResolution::Literal => return None,
        RouteResolution::Configured => {}
    }
    let (action, explicit_stem, update_stem, stem_range) = match &route.stem {
        RecordRouteStem::None => (RecordAction::Create, None, None, None),
        RecordRouteStem::Explicit(stem, range) => {
            if !is_valid_record_stem(stem) {
                let message = if stem.is_empty() {
                    format!(
                        "`@{}/^` names no stem: write `@{}/^<stem>` \
                         (A-Z, a-z, 0-9, `-`) or drop the `^` to derive \
                         the stem from the title",
                        route.dir, route.dir,
                    )
                } else {
                    format!(
                        "explicit record stem `{stem}` must use only A-Z, \
                         a-z, 0-9, or `-`",
                    )
                };
                return Some(RecordClaim::refused(
                    RECORD_STEM_CODE,
                    message,
                    Some(route.range),
                ));
            }
            (RecordAction::Create, Some(stem.clone()), None, Some(*range))
        }
        RecordRouteStem::Update(range) => (
            RecordAction::Update,
            None,
            Some(item.source[range.0..range.1].to_owned()),
            Some(*range),
        ),
    };
    let (mut title, title_range) = parent_title(parent, &route, &position);
    if let Some((start, end)) = title_range {
        title = decode_title_escapes(item, title, start, end);
    }
    let mut fields = Vec::new();
    let mut field_value_ranges = Vec::new();
    let mut notes = Vec::new();
    let mut current_group: Option<RecordNoteGroup> = None;
    let mut has_first_level_owner = false;
    let mut last_first_level: Option<LastFirstLevel> = None;
    for line in item.lines.iter().skip(1) {
        if line.raw.text.trim().is_empty() {
            continue;
        }
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => {
                return Some(RecordClaim::refused(
                    RECORD_SHAPE_CODE,
                    format!(
                        "record in `{}/` takes first-level `-` bullets below \
                         the title; this line is not one",
                        route.dir,
                    ),
                    Some((line.raw.start, line.raw.end)),
                ));
            }
            AuthoredLineClass::Item(authored) => authored,
        };
        if authored.depth == AuthoredDepth::Nested && !has_first_level_owner {
            return Some(RecordClaim::refused(
                RECORD_SHAPE_CODE,
                format!(
                    "record in `{}/` has a nested bullet with no \
                     first-level bullet above it",
                    route.dir,
                ),
                Some((line.raw.start, line.raw.end)),
            ));
        }
        let body = authored.body;
        let body_start = authored.body_start;
        if authored.depth == AuthoredDepth::First {
            has_first_level_owner = true;
            if let Some(input) = parse_record_field(body, body_start) {
                field_value_ranges.push(input.value_range);
                fields.push(input);
                last_first_level = Some(LastFirstLevel::Field);
                continue;
            }
            if super::drawer_bullet::is_all_caps_drawer_title(body) {
                if let Some(group) = current_group.take() {
                    notes.push(group);
                }
                let trimmed = body.trim();
                let offset = body.find(trimmed).unwrap_or(0);
                current_group = Some(RecordNoteGroup {
                    section: Some(trimmed.to_string()),
                    section_range: Some((
                        body_start + offset,
                        body_start + offset + trimmed.len(),
                    )),
                    lines: Vec::new(),
                });
                last_first_level = Some(LastFirstLevel::Note);
                continue;
            }
            let group = current_group.get_or_insert_with(|| RecordNoteGroup {
                section: None,
                section_range: None,
                lines: Vec::new(),
            });
            group.lines.push(note_line(body, body_start, line.raw.end));
            last_first_level = Some(LastFirstLevel::Note);
        } else if last_first_level == Some(LastFirstLevel::Field) {
            let key = fields.last().map(|field| field.key.clone());
            return Some(RecordClaim::refused(
                RECORD_FIELD_NESTED_CODE,
                match key {
                    Some(key) => format!(
                        "record field `{key}` takes no nested bullets: \
                         move that text into the value (`{key}:: ...`) \
                         or into a note bullet",
                    ),
                    None => format!(
                        "record in `{}/` takes no nested bullets under \
                         a field: move that text into the value or \
                         into a note bullet",
                        route.dir,
                    ),
                },
                Some((line.raw.start, line.raw.end)),
            ));
        } else {
            let line = note_line(body, body_start, line.raw.end);
            if let Some(group) = current_group.as_mut() {
                if let Some(last) = group.lines.last_mut() {
                    last.children.push(line);
                } else {
                    group.lines.push(line);
                }
            } else {
                notes.push(RecordNoteGroup {
                    section: None,
                    section_range: None,
                    lines: vec![line],
                });
            }
        }
    }
    if let Some(group) = current_group.take() {
        notes.push(group);
    }
    let claim = RecordNoteIntent {
        action,
        dir: route.dir.clone(),
        title,
        explicit_stem,
        update_stem,
        fields,
        notes,
        route_range: route.range,
        title_range,
        stem_range,
    };
    if let Some(refusal) =
        refuse_record_markers(item, &claim, &field_value_ranges)
    {
        return Some(refusal);
    }
    Some(RecordClaim::Note(claim))
}

/// Title text with the route token removed, plus its absolute range.
/// Route-first takes the text after the token; title-first takes the
/// text before it. Ranges follow the trimmed title bytes.
fn parent_title(
    parent: RawLine<'_>,
    route: &RecordRouteToken,
    position: &RecordRoutePosition,
) -> (String, Option<(usize, usize)>) {
    let (relative_start, relative_end) = match position {
        RecordRoutePosition::RouteFirst => {
            let token_end = route.range.1 - parent.start;
            (token_end, parent.text.len())
        }
        RecordRoutePosition::TitleFirst => {
            let token_start = route.range.0 - parent.start;
            (0, token_start)
        }
    };
    let raw = parent.text.get(relative_start..relative_end).unwrap_or("");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }
    let offset = raw.find(trimmed).unwrap_or(0);
    let start = parent.start + relative_start + offset;
    (trimmed.to_string(), Some((start, start + trimmed.len())))
}

/// Apply the same `\#` literal decoding the ordinary task parser applies,
/// but only inside the title. Field values keep their authored bytes.
fn decode_title_escapes(
    item: &CaptureItem<'_>,
    title: String,
    start: usize,
    end: usize,
) -> String {
    let mut removals = super::type_marker::collect_item(item)
        .into_iter()
        .filter_map(|occurrence| match occurrence {
            super::type_marker::TypeMarkerOccurrence::Escaped { span }
                if start <= span.start && span.end <= end =>
            {
                Some(span.start)
            }
            _ => None,
        })
        .filter_map(|absolute| {
            let relative = absolute.checked_sub(start)?;
            (title.as_bytes().get(relative) == Some(&b'\\')).then_some(relative)
        })
        .collect::<Vec<_>>();
    removals.sort_unstable_by(|left, right| right.cmp(left));
    let mut bytes = title.into_bytes();
    for relative in removals {
        bytes.remove(relative);
    }
    String::from_utf8(bytes).expect("removing ASCII escapes preserves UTF-8")
}

/// Parse `- key:: value` into a field input. Returns `None` when the
/// body is not field-shaped, so sections and notes keep their meaning.
/// Unknown keys stay field inputs so the records engine can teach
/// their correction. Nested content under a field is refused by the
/// caller, which sees the nested lines after this first-level bullet.
fn parse_record_field(
    body: &str,
    body_start: usize,
) -> Option<RecordFieldInput> {
    let (key, _) = body.split_once("::")?;
    // A single-colon `key: value` stays a note (the record-parse phase
    // adds its teaching hint); only `::` claims a field.
    let key_trimmed = key.trim();
    if key_trimmed.is_empty() || !is_record_field_key(key_trimmed) {
        return None;
    }
    // A second `::` inside the value stays value content.
    let value_start_offset = key.len() + 2;
    let value_raw = &body[value_start_offset.min(body.len())..];
    let value_trimmed = value_raw.trim_start();
    let value_offset = value_raw.len() - value_trimmed.len();
    let key_offset = key.find(key_trimmed).unwrap_or(0);
    Some(RecordFieldInput {
        key: key_trimmed.to_string(),
        value: value_trimmed.trim_end().to_string(),
        key_range: (
            body_start + key_offset,
            body_start + key_offset + key_trimmed.len(),
        ),
        value_range: (
            body_start + value_start_offset + value_offset,
            body_start
                + value_start_offset
                + value_offset
                + value_trimmed.trim_end().len(),
        ),
    })
}

/// Field keys preserve exact text but must be single tokens: ASCII
/// letters, digits, `_`, `-`. Anything else stays a note bullet.
fn is_record_field_key(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

fn note_line(
    body: &str,
    body_start: usize,
    _line_end: usize,
) -> RecordNoteLine {
    let trimmed = body.trim();
    let offset = body.find(trimmed).unwrap_or(0);
    RecordNoteLine {
        text: trimmed.to_string(),
        range: (body_start + offset, body_start + offset + trimmed.len()),
        children: Vec::new(),
    }
}

/// What the most recent first-level bullet was: a field (whose nested
/// lines refuse) or a note/section (whose nested lines attach).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastFirstLevel {
    Field,
    Note,
}

/// True when `span` sits inside one of the opaque field value ranges.
fn in_field_values(ranges: &[(usize, usize)], span: (usize, usize)) -> bool {
    ranges
        .iter()
        .any(|range| range.0 <= span.0 && span.1 <= range.1)
}

/// Collect the non-opaque scan regions: the title plus every note line,
/// nested note line, and section header. Field values never appear here,
/// so they stay opaque no matter which sigils, URLs, or wikilinks they
/// hold.
fn record_scan_regions(claim: &RecordNoteIntent) -> Vec<(String, usize)> {
    let mut regions = Vec::new();
    if let Some(range) = claim.title_range {
        regions.push((claim.title.clone(), range.0));
    }
    for group in &claim.notes {
        if let (Some(section), Some(range)) =
            (group.section.as_deref(), group.section_range)
        {
            regions.push((section.to_string(), range.0));
        }
        for line in &group.lines {
            regions.push((line.text.clone(), line.range.0));
            for child in &line.children {
                regions.push((child.text.clone(), child.range.0));
            }
        }
    }
    regions
}

/// Refuse task markers outside opaque field values with
/// `record_marker_unsupported`, naming the first marker found. Scalar
/// checks run in a fixed order (type, directive, dependency, region
/// shapes); the first hit wins so messages stay deterministic.
fn refuse_record_markers(
    item: &CaptureItem<'_>,
    claim: &RecordNoteIntent,
    field_value_ranges: &[(usize, usize)],
) -> Option<RecordClaim> {
    let unsupported = |marker: String, span: Option<(usize, usize)>| {
        RecordClaim::refused(
            RECORD_MARKER_UNSUPPORTED_CODE,
            format!(
                "`{marker}` is a task marker, and record notes take none \
                 outside field values: drop the marker or move that text \
                 into a `key:: value` field",
            ),
            span,
        )
    };
    // `#TYPE` markers, with quote/escape rules from the shared lexer.
    let occurrences = super::type_marker::collect_item(item);
    for occurrence in &occurrences {
        if !occurrence.is_marker() {
            continue;
        }
        if let super::type_marker::TypeMarkerOccurrence::Complete(marker) =
            occurrence
        {
            let span = (marker.span.start, marker.span.end);
            if in_field_values(field_value_ranges, span) {
                continue;
            }
            return Some(unsupported(format!("#{}", marker.name), Some(span)));
        }
    }
    // `%c` / `%n` / `%l` directives (plus unknown/invalid/incomplete
    // non-template directives), with the shared masking and escapes.
    // `%t` composes with record creation, so it never refuses.
    let type_masked_source =
        super::type_marker::masked_source(item, &occurrences);
    let type_masked_item =
        super::type_marker::masked_item(item, &type_masked_source);
    let directives = super::directive::collect_item(&type_masked_item);
    for directive in &directives {
        if !directive.is_marker() {
            continue;
        }
        if matches!(
            directive,
            super::directive::DirectiveOccurrence::Complete {
                kind: super::directive::DirectiveKind::Template,
                ..
            }
        ) {
            continue;
        }
        let span = directive.span();
        if in_field_values(field_value_ranges, (span.start, span.end)) {
            continue;
        }
        let raw = item.source.get(span.clone()).unwrap_or("%").to_string();
        return Some(unsupported(raw, Some((span.start, span.end))));
    }
    // `&` / `*` / `~` modifiers, with the shared protected-markup rules.
    let found = super::dependencies::scan_item_dependencies(item);
    let mut modifier_hits: Vec<(usize, String)> = Vec::new();
    for entry in found
        .entries
        .iter()
        .map(|entry| (entry.start, entry.end, entry.raw.clone()))
        .chain(
            found
                .blocked_entries
                .iter()
                .map(|entry| (entry.start, entry.end, entry.raw.clone())),
        )
        .chain(
            found
                .relation_entries
                .iter()
                .map(|entry| (entry.start, entry.end, entry.raw.clone())),
        )
    {
        if in_field_values(field_value_ranges, (entry.0, entry.1)) {
            continue;
        }
        modifier_hits.push((entry.0, entry.2));
    }
    for invalid in found
        .invalid
        .iter()
        .chain(found.blocked_invalid.iter())
        .chain(found.relation_invalid.iter())
    {
        if in_field_values(field_value_ranges, (invalid.start, invalid.end)) {
            continue;
        }
        modifier_hits.push((invalid.start, invalid.raw.clone()));
    }
    for partial in found
        .partials
        .iter()
        .chain(found.blocked_partials.iter())
        .chain(found.relation_target_partials.iter())
        .chain(found.relation_label_partials.iter())
    {
        if in_field_values(field_value_ranges, (partial.0, partial.1)) {
            continue;
        }
        let raw = item
            .source
            .get(partial.0..partial.1)
            .unwrap_or("&")
            .to_string();
        modifier_hits.push((partial.0, raw));
    }
    modifier_hits.sort_by_key(|hit| hit.0);
    if let Some((_, raw)) = modifier_hits.into_iter().next() {
        let span = item
            .source
            .find(&raw)
            .map(|start| (start, start + raw.len()));
        return Some(unsupported(raw, span));
    }
    // Schedule, priority, and selector/session shapes in the non-opaque
    // regions, with quote and escape rules for literal title text.
    let source = item.source;
    // The route can be exposed by a task marker before or after it. Scan
    // the whole parent line with only the route token blanked, so that
    // those markers cannot disappear when the title is extracted.
    if let Some(parent) = item.lines.first().map(|line| line.raw) {
        let mut parent_without_route = parent.text.as_bytes().to_vec();
        let route_start = claim.route_range.0.saturating_sub(parent.start);
        let route_end = claim.route_range.1.saturating_sub(parent.start);
        if route_start <= route_end && route_end <= parent_without_route.len() {
            parent_without_route[route_start..route_end].fill(b' ');
            let parent_without_route = String::from_utf8(parent_without_route)
                .expect("route masking preserves UTF-8");
            if let Some((marker, range)) =
                scan_region_markers(&parent_without_route, parent.start, source)
            {
                return Some(unsupported(marker, Some(range)));
            }
        }
    }
    for (text, base) in record_scan_regions(claim) {
        if let Some((marker, range)) = scan_region_markers(&text, base, source)
        {
            return Some(unsupported(marker, Some(range)));
        }
    }
    None
}

/// Mask template directives inside fields of configured record items before
/// `%t` analysis. Placeholders stay visible to the existing expander. The
/// masking is byte-preserving, so every template range and source map still
/// indexes the caller's original UTF-8 draft.
pub(crate) fn mask_field_values_for_template(
    raw: &str,
    policy: Option<&RecordRoutePolicy>,
) -> String {
    let Some(policy) = policy else {
        return raw.to_string();
    };
    let draft = super::draft::split_capture_draft_no_lists(raw);
    let mut bytes = raw.as_bytes().to_vec();
    for item in &draft.items {
        let Some(parent) = item.lines.first().map(|line| line.raw) else {
            continue;
        };
        let Some((route, _)) =
            find_record_route_token(parent.text, parent.start)
        else {
            continue;
        };
        if !matches!(
            resolve_record_route(&route, policy),
            RouteResolution::Configured
        ) {
            continue;
        }
        for line in item.lines.iter().skip(1) {
            let AuthoredLineClass::Item(authored) =
                classify_authored_line(line.raw)
            else {
                continue;
            };
            if authored.depth != AuthoredDepth::First {
                continue;
            }
            let Some(field) =
                parse_record_field(authored.body, authored.body_start)
            else {
                continue;
            };
            let (start, end) = field.value_range;
            if start <= end && end <= bytes.len() {
                let value = &raw[start..end];
                for (offset, _) in value.match_indices("%t") {
                    bytes[start + offset..start + offset + 2].fill(b' ');
                }
            }
        }
    }
    String::from_utf8(bytes).expect("field masking preserves UTF-8")
}

/// Quote state while scanning one region for marker shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionQuote {
    None,
    Straight,
    Curly,
}

/// Scan one non-opaque region for schedule/priority/selector/session
/// marker shapes. Returns the marker text plus its absolute range.
/// Quoted spans, `\`-escaped tokens, and Markdown-protected spans stay
/// literal; punctuation in ordinary prose is not a marker.
fn scan_region_markers(
    text: &str,
    base: usize,
    source: &str,
) -> Option<(String, (usize, usize))> {
    let mut quote = RegionQuote::None;
    let mut token_start: Option<usize> = None;
    let mut escaped = false;
    let mut chars: Vec<(usize, char)> = text.char_indices().collect();
    chars.push((text.len(), ' '));
    for (relative, ch) in chars {
        if escaped {
            escaped = false;
            if token_start.is_none() {
                token_start = Some(relative);
            }
            continue;
        }
        if ch == '\\' && quote == RegionQuote::None {
            escaped = true;
            continue;
        }
        match quote {
            RegionQuote::None => {
                if ch == '"' {
                    quote = RegionQuote::Straight;
                    token_start = None;
                    continue;
                }
                if ch == '\u{201c}' {
                    quote = RegionQuote::Curly;
                    token_start = None;
                    continue;
                }
            }
            RegionQuote::Straight => {
                if ch == '"' {
                    quote = RegionQuote::None;
                }
                continue;
            }
            RegionQuote::Curly => {
                if ch == '\u{201d}' {
                    quote = RegionQuote::None;
                }
                continue;
            }
        }
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                let token = &text[start..relative];
                let range = (base + start, base + relative);
                if !is_protected_markup_range(source, range.0, range.1)
                    && let Some(marker) = classify_region_token(token)
                {
                    return Some((marker, range));
                }
            }
            continue;
        }
        if token_start.is_none() {
            token_start = Some(relative);
        }
    }
    None
}

/// Classify one unquoted, unescaped, unprotected region token as a task
/// marker. Returns the marker text for the diagnostic. `None` is prose,
/// literal text, or a shape another scanner already owns (`#`, `%`,
/// `&`, `*`, `~` are claimed by the shared scanners above).
fn classify_region_token(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    // Schedule and priority edges.
    if parse_schedule_token(token).is_some() {
        return Some(token.to_string());
    }
    if is_priority_marker_token(token) {
        return Some(token.to_string());
    }
    let marker = |token: &str| Some(token.to_string());
    // `:` / `^` task selectors: word-only targets. Emoji (`:smile:`)
    // and emoticons (`:)`) stay prose.
    if let Some(target) = token.strip_prefix(':')
        && !target.is_empty()
        && target.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
    {
        return marker(token);
    }
    if let Some(target) = token.strip_prefix('^')
        && !target.is_empty()
        && target.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
    {
        return marker(token);
    }
    // `!` task selectors: mirror the bang claim (embeds, images, and
    // `!` runs stay prose).
    if let Some(rest) = token.strip_prefix('!')
        && !rest.is_empty()
        && !rest.starts_with('[')
        && !rest.starts_with('!')
    {
        return marker(token);
    }
    // `+` task selectors and session adjust/shift operators.
    if token.starts_with('+') && token.len() > 1 {
        return marker(token);
    }
    // `=` session starts.
    if token.starts_with('=') && token.len() > 1 {
        return marker(token);
    }
    // Whole-token session closes (`x`, `x2`, `x1!2`): only when the
    // token is nothing but a close shape.
    if is_session_close_token(token) {
        return marker(token);
    }
    // `/query` Go To searches.
    if token.starts_with('/')
        && token.len() > 1
        && token.bytes().filter(|byte| *byte == b'/').count() == 1
    {
        return marker(token);
    }
    None
}

/// Whether the token is exactly a session close shape (`x2`,
/// `x1*2!3~4`): a lowercase `x` plus close selectors. A bare `x` word
/// stays prose: only a whole-item `x` closes a Pomodoro, and a record
/// title is never a whole item.
fn is_session_close_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('x') else {
        return false;
    };
    !rest.is_empty()
        && rest.bytes().all(|byte| {
            byte.is_ascii_digit() || matches!(byte, b',' | b'*' | b'!' | b'~')
        })
}

/// Build the strict parse outcome for a claimed record creation: the
/// title becomes the body, the dir becomes the route, and notes stay
/// inside the intent (never in `sub_bullets`, so task-only trackers
/// ignore them).
fn strict_record_outcome<'a>(
    item: &CaptureItem<'a>,
    intent: RecordNoteIntent,
    route_text: &str,
) -> super::item::ParsedCaptureItemOutcome<'a> {
    parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: intent.title.clone(),
            clip: None,
            route: Some(intent.dir.clone()),
            kind: match intent.action {
                RecordAction::Create => CaptureKind::RecordNote { intent },
                RecordAction::Update => CaptureKind::RecordUpdate { intent },
            },
            scheduled_offset: None,
            priority_level: None,
            sub_bullets: Vec::new(),
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
            merge_in: None,
        },
        Vec::new(),
        Some(route_text.to_string()),
    )
}

/// Claim a record item on the strict path, before transfer, dependency,
/// directive, ref-job, type, task-ID, and generic task-bullet parsing.
/// Forced destinations (`--route` and equivalent explicit destination
/// paths) keep the text literal and never claim.
pub(super) fn claim_strict_record<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<
    Option<super::item::ParsedCaptureItemOutcome<'a>>,
    CaptureItemParseError,
> {
    if forced_route.is_some()
        || forced_section.is_some()
        || options.explicit_destination
    {
        return Ok(None);
    }
    let route_text = item
        .lines
        .first()
        .map(|line| line.raw.text.trim().to_string())
        .unwrap_or_default();
    match classify_record_item(item, options.record_policy) {
        None => Ok(None),
        Some(RecordClaim::Note(intent))
            if intent.action == RecordAction::Create
                && intent.title.trim().is_empty() =>
        {
            Err(CaptureItemParseError::coded(
                RECORD_TITLE_CODE,
                format!(
                    "record in `{}/` needs a title: write `@{}/ <title>` or \
                     `<title> @{}/`",
                    intent.dir, intent.dir, intent.dir,
                ),
            ))
        }
        Some(RecordClaim::Note(intent)) => {
            Ok(Some(strict_record_outcome(item, intent, &route_text)))
        }
        Some(RecordClaim::Refused {
            code,
            message,
            span,
        }) => Err(CaptureItemParseError::coded_at(code, message, span)),
    }
}

/// Editor view of a record claim: mode, needs, spans, and diagnostics.
/// A bare route or an empty field value reports a need (still
/// completable); every other refusal reports an error diagnostic with
/// the strict code so both surfaces share one vocabulary.
pub(crate) struct RecordEditorView {
    pub(crate) mode: EditorMode,
    pub(crate) body: String,
    pub(crate) route: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) spans: Vec<Span>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// Build the editor view for a record claim. A bare route reports a
/// `record_title` need with no diagnostic (still completable by typing
/// a title); empty field values report `record_value` needs the same
/// way. Every other refusal reports an error diagnostic with the
/// strict code so both surfaces share one vocabulary.
pub(crate) fn editor_view_for_claim(claim: &RecordClaim) -> RecordEditorView {
    match claim {
        RecordClaim::Note(intent) => {
            let mut needs = Vec::new();
            let mut spans = vec![Span {
                start: intent.route_range.0,
                end: intent.route_range.1,
                kind: SpanKind::RecordRoute,
                project_note: None,
                title: Some(intent.dir.clone()),
                key: None,
            }];
            if intent.action == RecordAction::Create
                && intent.title.trim().is_empty()
            {
                needs.push(Need::RecordTitle);
            }
            for field in &intent.fields {
                spans.push(Span {
                    start: field.key_range.0,
                    end: field.key_range.1,
                    kind: SpanKind::RecordFieldKey,
                    project_note: None,
                    title: None,
                    key: Some(field.key.clone()),
                });
                spans.push(Span {
                    start: field.value_range.0,
                    end: field.value_range.1,
                    kind: SpanKind::RecordFieldValue,
                    project_note: None,
                    title: None,
                    key: Some(field.key.clone()),
                });
                // On an update, an empty optional field is a legal key
                // removal, so the editor never needs `record_value` for it.
                // Required-empty updates still report the need through the
                // resolved-plan merge when `entry.missing` is non-empty.
                if intent.action == RecordAction::Create
                    && field.value.is_empty()
                    && !needs.contains(&Need::RecordValue)
                {
                    needs.push(Need::RecordValue);
                }
            }
            spans.sort_by_key(|span| (span.start, span.end));
            RecordEditorView {
                mode: match intent.action {
                    RecordAction::Create => EditorMode::RecordNote,
                    RecordAction::Update => EditorMode::RecordUpdate,
                },
                body: intent.title.clone(),
                route: Some(intent.dir.clone()),
                needs,
                spans,
                diagnostics: Vec::new(),
            }
        }
        RecordClaim::Refused {
            code,
            message,
            span,
        } => {
            if *code == RECORD_TITLE_CODE {
                return RecordEditorView {
                    mode: EditorMode::RecordNote,
                    body: String::new(),
                    route: None,
                    needs: vec![Need::RecordTitle],
                    spans: Vec::new(),
                    diagnostics: Vec::new(),
                };
            }
            RecordEditorView {
                mode: EditorMode::RecordNote,
                body: String::new(),
                route: None,
                needs: Vec::new(),
                spans: Vec::new(),
                diagnostics: vec![Diagnostic {
                    severity: Severity::Error,
                    code,
                    message: message.clone(),
                    range: *span,
                    rewrite: None,
                }],
            }
        }
    }
}

/// Claim a record item on the editor path, before transfer and merge
/// claims. Returns `None` when the item is not record-shaped, so the
/// editor keeps its existing behavior.
pub(crate) fn claim_editor_record(
    item: &CaptureItem<'_>,
    options: &EditorParseOptions<'_>,
) -> Option<RecordEditorView> {
    let claim = classify_record_item(item, options.record_policy)?;
    Some(editor_view_for_claim(&claim))
}
