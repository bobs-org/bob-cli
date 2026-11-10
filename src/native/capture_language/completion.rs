//! Cursor completion for capture markers.

use super::bare_name::NameSpelling;
use super::dependencies::*;
use super::draft::*;
use super::editor_classify::*;
use super::editor_model::*;
use super::editor_parse::*;
use super::editor_pomodoro::*;
use super::item::{
    named_start_name_offset, session_equals_token, start_timing_len,
    EqualsToken,
};
use super::line::*;
use super::markers::*;
use super::model::*;
use super::project_tasks::*;
use super::relations::{scan_line_relations, ScannedRelation};
use super::tokens::*;
use serde::Serialize;
use std::collections::HashSet;

/// Which discovery source a completion request should query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionContext {
    Route,
    Section,
    PomodoroBlockId,
    TaskBlockId,
    ProjectTaskBlockId,
    /// A first-level `- ^id` / `- :id` move bullet in a sealed project note.
    ProjectTaskMove,
    PomodoroName,
    PomodoroStartName,
    PomodoroPastName,
    PomodoroPastSegment,
    Task,
    /// A `#<type>` task-type marker at either item edge.
    TaskType,
    /// An edge `%` capture directive (`%c`, `%n`, `%t`, or URL-only `%l`).
    Directive,
    Drawer,
    ActiveTask,
    TaskLink,
    /// A whole-draft `/query` Go To search over the `:` task catalog.
    /// The query runs from just after the sigil to the cursor and the
    /// replacement covers the whole token, sigil included, so refetching
    /// at `replacement.start` returns the unfiltered list.
    TaskGoto,
    /// A bare-plus task search across the capture-target catalog. Accept
    /// inserts a complete `@route+block-id` parent marker.
    TaskParent,
    /// An `&` prerequisite modifier: vault-wide task search whose accept
    /// inserts a Bob-authored `&note:block-id` replacement. The candidate
    /// scan lands in the discovery phase; the lexical field (query plus the
    /// exact sigil-inclusive replacement range) is contract.
    TaskDependency,
    /// A `*` blocked-task modifier: vault-wide open-task search whose accept
    /// inserts a Bob-authored `*note:block-id` replacement.
    BlockedTask,
    /// A `~` relation target picker: the current task links to the candidate.
    RelationTo,
    /// A `~~` relation target picker: the candidate links to the current task.
    RelationFrom,
    /// A relation label suggestion field after `|`.
    RelationLabel,
    /// A whole-item `!` token: vault-wide open-task search whose accept
    /// inserts a Bob-authored `!note:block-id` replacement. The query is
    /// decoded like `&`; the replacement covers the whole token, sigil
    /// included, so refetching at `replacement.start` returns the
    /// unfiltered snapshot.
    TaskComplete,
    /// A `=/` split base field: today's open merged Pomodoros.
    PomodoroSplitBase,
    /// A `=/` split `#type` field: the target's merged-in types.
    PomodoroSplitType,
    /// A `=*` merge source field: today's open Pomodoros.
    PomodoroMergeSrc,
    /// A `=*` merge `#dest` field: untimed planned entries other than src.
    PomodoroMergeDest,
    WikilinkNote,
    WikilinkHeading,
    WikilinkBlock,
    /// A `((query` Task Wikilink field: vault-wide task search whose
    /// accept inserts a backend-formatted `[[target#^id]]` replacement.
    /// Draft-only trigger, never stored syntax.
    WikilinkTask,
    /// A first-level record field-name bullet (`- sta`, `- `).
    RecordField,
    /// A record field value, including an explicit next-missing-field request.
    RecordValue,
    /// An existing Record Note target after a configured `@dir/` route.
    RecordTarget,
}

/// The active marker component at one cursor position, ready for a
/// discovery scan and case-insensitive prefix/substring ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionField {
    pub(crate) context: CompletionContext,
    /// The already-resolved, lowercased route, when `context` needs one
    /// (`section`, `pomodoro_block_id`, `pomodoro_name`, `task`, and
    /// `drawer`).
    pub(crate) route: Option<String>,
    /// The already-typed block ID of a three-component
    /// `@route+id#section` or `@route:id#pomodoro` marker. Set only when
    /// `context` is `drawer` or `pomodoro_name`; always `None` for
    /// two-component contexts.
    pub(crate) block_id: Option<String>,
    /// The text already typed in this component, up to `cursor`.
    pub(crate) query: String,
    /// The half-open UTF-8 byte range of the whole component, which a
    /// completion replaces in full regardless of where the cursor sits
    /// inside it.
    pub(crate) replacement: (usize, usize),
    /// True for a bare `=name` or `x=name` field, so candidates whose
    /// slug cannot be bare get a `#slug` replacement.
    pub(crate) bare_name: bool,
}

/// Complete an edge `%` directive at the cursor. The shared directive lexer
/// owns which tokens claim the edge; this only exposes the token's typed
/// letter as a completion query while keeping replacement offsets in the
/// submitted draft.
fn directive_completion_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    use super::directive::DirectiveOccurrence;

    let occurrence = super::directive::collect_item(item).into_iter().find(
        |occurrence| {
            !matches!(occurrence, DirectiveOccurrence::Escaped { .. })
                && occurrence.span().start <= cursor
                && cursor <= occurrence.span().end
        },
    )?;
    let span = occurrence.span();
    let query_start = span.start.checked_add(1)?;
    let query_end = cursor.clamp(query_start, span.end);
    let query = item.source.get(query_start..query_end)?.to_string();

    Some(CompletionField {
        context: CompletionContext::Directive,
        route: None,
        block_id: None,
        query,
        replacement: (span.start, span.end),
        bare_name: false,
    })
}

pub(super) struct CompletionThird<'a> {
    pub(super) separator_len: usize,
    pub(super) part: &'a str,
    pub(super) context: CompletionContext,
}

pub(super) struct CompletionParts<'a> {
    pub(super) sigil_len: usize,
    pub(super) route_part: &'a str,
    pub(super) separator_len: usize,
    pub(super) right_part: &'a str,
    pub(super) right_context: Option<CompletionContext>,
    pub(super) third: Option<CompletionThird<'a>>,
}

/// Complete a solo leading `:` token: the query is the token text between
/// the sigil and the cursor (empty at or just after the sigil), and the
/// replacement covers the whole token including the sigil, because
/// accepting rewrites the query into the canonical `@route:block-id`
/// marker. The cursor may sit anywhere in `[token.start, token.end]`,
/// including just before the sigil so clients can refetch the full list at
/// `replacement.start`. Multi-token and multi-line items stay prose and
/// return `None`, exactly like the claim predicate.
pub(super) fn task_link_completion_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let token = task_link_query_token(item)?;
    if cursor < token.start || cursor > token.end {
        return None;
    }
    let relative = cursor - token.start;
    // A cursor just before the sigil refetches the full list with an empty
    // query; `get(1..0)` would be `None`, so spell that case directly.
    let query = if relative == 0 {
        String::new()
    } else {
        token.text.get(1..relative)?.to_string()
    };
    Some(CompletionField {
        context: CompletionContext::TaskLink,
        route: None,
        block_id: None,
        query,
        replacement: (token.start, token.end),
        bare_name: false,
    })
}

/// Complete a whole-draft `/query` Go To search: the query is the token
/// text between the sigil and the cursor (empty at or just after the
/// sigil), and the replacement covers the whole token including the sigil,
/// because refetching at `replacement.start` returns the unfiltered list
/// and Backspace on an empty filter can remove the `/`. The cursor may sit
/// anywhere in `[token.start, token.end]`, including just before the sigil
/// so clients can refetch the full list at `replacement.start`.
/// Multi-token drafts, multi-item drafts, and second-`/` tokens stay prose
/// and return `None`, exactly like the claim predicate.
pub(super) fn task_goto_completion_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let token = task_goto_query_token(item)?;
    if cursor < token.start || cursor > token.end {
        return None;
    }
    let relative = cursor - token.start;
    // A cursor just before the sigil refetches the full list with an empty
    // query; `get(1..0)` would be `None`, so spell that case directly.
    let query = if relative == 0 {
        String::new()
    } else {
        token.text.get(1..relative)?.to_string()
    };
    Some(CompletionField {
        context: CompletionContext::TaskGoto,
        route: None,
        block_id: None,
        query,
        replacement: (token.start, token.end),
        bare_name: false,
    })
}

/// Complete an `&` prerequisite modifier: the cursor inside a directive
/// complete or partial modifier (leading or trailing run, outside
/// protected spans) yields the `task_dependency` context. The query is the
/// text after the sigil up to the cursor (one opening quote stripped,
/// `\"`/`\\` decoded); the replacement covers exactly the whole modifier
/// including its sigil and quoted note, so accepting rewrites the query
/// into Bob's canonical `&note:block-id` form and refetching at
/// `replacement.start` returns the unfiltered snapshot. Literal mid-line
/// ampersands, `\&` escapes, and malformed modifiers yield nothing.
pub(super) fn dependency_completion_field_at(
    line: &RawLine<'_>,
    cursor: usize,
    parent: bool,
) -> Option<CompletionField> {
    relation_completion_field_at(
        line,
        cursor,
        parent,
        b'&',
        CompletionContext::TaskDependency,
    )
}

/// Complete a `*` blocked-task modifier with the same cursor and replacement
/// semantics as the prerequisite picker. The line extractor remains the
/// authority for leading/trailing run placement and Markdown guards.
pub(super) fn blocked_task_completion_field_at(
    line: &RawLine<'_>,
    cursor: usize,
    parent: bool,
) -> Option<CompletionField> {
    relation_completion_field_at(
        line,
        cursor,
        parent,
        b'*',
        CompletionContext::BlockedTask,
    )
}

/// Complete a `~` / `~~` relation target. The replacement ends before an
/// optional label, so choosing another task preserves a label already typed.
fn relation_target_completion_field_at(
    line: &RawLine<'_>,
    cursor: usize,
    parent: bool,
) -> Option<CompletionField> {
    let (_, found) = extract_line_dependencies(
        tokenize_line_with_spans(line),
        line.text,
        line.start,
        parent,
    );
    for scanned in scan_line_relations(line.text, line.start) {
        let (start, end, direction, sigil_len, directive_end) = match &scanned {
            ScannedRelation::Complete(entry) => {
                let target_end = entry
                    .separator
                    .map(|separator| separator.0)
                    .unwrap_or(entry.end);
                if cursor < entry.start || cursor > target_end {
                    continue;
                }
                (
                    entry.start,
                    target_end,
                    entry.direction,
                    entry.sigil_len(),
                    entry.end,
                )
            }
            ScannedRelation::PartialTarget { start, end, .. } => {
                if cursor < *start || cursor > *end {
                    continue;
                }
                let relative = *start - line.start;
                let sigil_len =
                    if line.text.as_bytes().get(relative + 1) == Some(&b'~') {
                        2
                    } else {
                        1
                    };
                (
                    *start,
                    *end,
                    if sigil_len == 2 {
                        RelationDirection::From
                    } else {
                        RelationDirection::To
                    },
                    sigil_len,
                    *end,
                )
            }
            ScannedRelation::PartialLabel { .. }
            | ScannedRelation::Invalid(_)
            | ScannedRelation::Escaped { .. } => continue,
        };
        let is_directive =
            found.relation_entries.iter().any(|entry| {
                entry.start == start && entry.end == directive_end
            }) || found.relation_target_partials.iter().any(
                |(partial_start, partial_end)| {
                    *partial_start == start
                        && *partial_end == end
                        && start <= cursor
                        && cursor <= end
                },
            );
        if !is_directive {
            continue;
        }
        let query_end = cursor.min(end);
        let query =
            if cursor <= start + sigil_len {
                String::new()
            } else {
                decode_dependency_query(line.text.get(
                    start + sigil_len - line.start..query_end - line.start,
                )?)
            };
        return Some(CompletionField {
            context: match direction {
                RelationDirection::To => CompletionContext::RelationTo,
                RelationDirection::From => CompletionContext::RelationFrom,
            },
            route: None,
            block_id: None,
            query,
            replacement: (start, end),
            bare_name: false,
        });
    }
    None
}

/// Complete the label part of a relation. The replacement owns the pipe and
/// the complete label token, so suggestions can use Bob's canonical
/// `|label` or `|"multi word"` spelling.
fn relation_label_completion_field_at(
    line: &RawLine<'_>,
    cursor: usize,
    parent: bool,
) -> Option<CompletionField> {
    let (_, found) = extract_line_dependencies(
        tokenize_line_with_spans(line),
        line.text,
        line.start,
        parent,
    );
    for scanned in scan_line_relations(line.text, line.start) {
        let (start, end, separator, hidden) = match &scanned {
            ScannedRelation::Complete(entry) => {
                let Some((separator, _)) = entry.separator else {
                    continue;
                };
                if cursor <= separator || cursor > entry.end {
                    continue;
                }
                (entry.start, entry.end, separator, entry.hidden)
            }
            ScannedRelation::PartialLabel {
                raw,
                start,
                end,
                hidden,
            } => {
                if cursor < *start || cursor > *end {
                    continue;
                }
                // The first pipe opens the separator: a `||` probe keeps
                // both pipes in the replacement so an accept preserves
                // the one-off spelling.
                let Some(pipe) = raw.find('|') else {
                    continue;
                };
                let separator = *start + pipe;
                if cursor <= separator {
                    continue;
                }
                (*start, *end, separator, *hidden)
            }
            _ => continue,
        };
        let is_directive = found
            .relation_entries
            .iter()
            .any(|entry| entry.start == start && entry.end == end)
            || found.relation_label_partials.iter().any(
                |(partial_start, partial_end)| {
                    *partial_start == start
                        && *partial_end == end
                        && start <= cursor
                        && cursor <= end
                },
            );
        if !is_directive {
            continue;
        }
        let query_end = cursor.min(end);
        let raw_query = line
            .text
            .get(separator + 1 - line.start..query_end - line.start)?;
        // A `||` probe reads past the first pipe: drop the second one
        // from the query so ranking sees the typed label text.
        let raw_query = hidden
            .then(|| raw_query.strip_prefix('|'))
            .flatten()
            .unwrap_or(raw_query);
        let raw_query = raw_query.strip_prefix('"').unwrap_or(raw_query);
        let query =
            raw_query.strip_suffix('"').unwrap_or(raw_query).to_string();
        return Some(CompletionField {
            context: CompletionContext::RelationLabel,
            route: None,
            block_id: None,
            query,
            replacement: (separator, end),
            bare_name: false,
        });
    }
    None
}

/// Locate the relation token whose label separator sits at `separator`:
/// the token's start, its direction, and whether the separator was `||`,
/// so editors never re-lex the draft. The separator is the first `|`
/// byte the `relation_label` replacement starts at. Quoted note
/// components are covered because the scan that produced the field
/// already classified them.
pub(crate) fn relation_label_token_at(
    raw_text: &str,
    separator: usize,
) -> Option<(usize, RelationDirection, bool)> {
    let line = split_physical_lines(raw_text)
        .into_iter()
        .find(|line| separator >= line.start && separator <= line.end)?;
    for scanned in scan_line_relations(line.text, line.start) {
        match &scanned {
            ScannedRelation::Complete(entry)
                if entry.separator.map(|(start, _)| start)
                    == Some(separator) =>
            {
                return Some((entry.start, entry.direction, entry.hidden));
            }
            ScannedRelation::PartialLabel {
                raw, start, hidden, ..
            } => {
                // The first pipe opens the separator; a `||`
                // replacement starts at the first pipe too.
                let pipe = raw.find('|')?;
                if *start + pipe == separator {
                    let direction = if raw.starts_with("~~") {
                        RelationDirection::From
                    } else {
                        RelationDirection::To
                    };
                    return Some((*start, direction, *hidden));
                }
            }
            _ => {}
        }
    }
    None
}

fn relation_completion_field_at(
    line: &RawLine<'_>,
    cursor: usize,
    parent: bool,
    sigil: u8,
    context: CompletionContext,
) -> Option<CompletionField> {
    if cursor < line.start || cursor > line.end {
        return None;
    }
    let scanned = if sigil == b'*' {
        scan_line_blocked_tasks(line.text, line.start)
    } else {
        scan_line_dependencies(line.text, line.start)
    };
    let target = scanned.iter().find(|scan| {
        !matches!(
            scan,
            ScannedDependency::Escaped { .. } | ScannedDependency::Invalid(_)
        ) && scan.start() <= cursor
            && cursor <= scan.end()
    })?;
    // The modifier must be directive (a marker-run member), not literal
    // mid-line prose that merely looks like one.
    let tokens = tokenize_line_with_spans(line);
    let (_, found) =
        extract_line_dependencies(tokens, line.text, line.start, parent);
    let (entries, partials) = if sigil == b'*' {
        (&found.blocked_entries, &found.blocked_partials)
    } else {
        (&found.entries, &found.partials)
    };
    let directive = entries
        .iter()
        .any(|entry| entry.start <= cursor && cursor <= entry.end)
        || partials
            .iter()
            .any(|(start, end)| *start <= cursor && cursor <= *end);
    if !directive {
        return None;
    }
    let (start, end) = (target.start(), target.end());
    // A cursor just before the sigil refetches the full list with an empty
    // query; slicing past the sigil would invert the range, so spell that
    // case directly (mirroring the `:` picker contract).
    let typed = if cursor == start {
        String::new()
    } else {
        let slice =
            line.text.get(start + 1 - line.start..cursor - line.start)?;
        decode_dependency_query(slice)
    };
    Some(CompletionField {
        context,
        route: None,
        block_id: None,
        query: typed,
        replacement: (start, end),
        bare_name: false,
    })
}

/// Complete a solo leading `!` token: the query runs from just after
/// the sigil to the cursor (decoded like `&`, empty at or just before
/// the sigil) and the replacement covers the whole token including the
/// sigil, because accepting rewrites the query into the canonical
/// `!note:block-id` marker. The cursor may sit anywhere in
/// `[token.start, token.end]`, including just before the sigil so
/// clients can refetch the full list at `replacement.start`. Claimed
/// invalid tokens offer nothing: they carry a teaching diagnostic, not
/// a picker. Multi-line items stay prose, exactly like the claim
/// predicate.
pub(super) fn task_complete_completion_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    match claim_bang_item(item)? {
        BangClaim::Query { raw, start, end } => {
            bang_completion_field(&raw, start, end, cursor)
        }
        BangClaim::Complete(dependency) => bang_completion_field(
            &dependency.raw,
            dependency.start,
            dependency.end,
            cursor,
        ),
        BangClaim::Invalid(_) => None,
    }
}

/// Claimed whole-item `!` complete tokens across a batch draft, in
/// source order: the item range plus the decoded note and block ID.
/// The picker-contract catalog consumes this instead of re-deriving
/// the claim; claimed invalid tokens, queries, and prose never appear
/// here.
pub(crate) fn claimed_task_complete_tokens(
    raw_text: &str,
) -> Vec<(usize, usize, String, String)> {
    split_capture_draft(raw_text)
        .items
        .iter()
        .filter_map(|item| match claim_bang_item(item)? {
            BangClaim::Complete(dependency) => Some((
                item.start,
                item.end,
                dependency.note.clone(),
                dependency.block_id.clone(),
            )),
            BangClaim::Query { .. } | BangClaim::Invalid(_) => None,
        })
        .collect()
}

fn bang_completion_field(
    raw: &str,
    start: usize,
    end: usize,
    cursor: usize,
) -> Option<CompletionField> {
    if cursor < start || cursor > end {
        return None;
    }
    // A cursor just before the sigil refetches the full list with an
    // empty query; slicing past the sigil would invert the range, so
    // spell that case directly (mirroring the `:` picker contract).
    let query = if cursor == start {
        String::new()
    } else {
        decode_dependency_query(raw.get(1..cursor - start)?)
    };
    Some(CompletionField {
        context: CompletionContext::TaskComplete,
        route: None,
        block_id: None,
        query,
        replacement: (start, end),
        bare_name: false,
    })
}

/// Complete a solo leading `^` token: the `route:block-id` part (up to the
/// cursor, so typed suffixes survive an accept) offers vault-wide active
/// tasks, while a cursor after `#` offers Pomodoro names for the resolved
/// route and block ID. Anything else on the parent line is prose, a cursor
/// inside `=<X>` offers nothing, and partial shapes on multi-line items
/// stay prose, so those return `None`.
pub(super) fn caret_completion_field_at(
    item: &CaptureItem<'_>,
    line: RawLine<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let tokens = tokenize_line_with_spans(&line);
    let first = tokens.first()?;
    if !first.text.starts_with('^') || tokens.len() != 1 {
        return None;
    }
    if cursor <= first.start || cursor > first.end {
        return None;
    }
    let relative = cursor - first.start;
    let shape = classify_caret_token(first.text);
    if let CaretTokenShape::Invalid(message) = &shape
        && message.contains("`#` is only for names a bare start cannot spell")
    {
        return retired_caret_hash_start_field(first, cursor);
    }
    match shape {
        CaretTokenShape::Prose
        | CaretTokenShape::Invalid(_)
        | CaretTokenShape::CloseInvalid { .. } => None,
        // A dangling separator still completes the link part it hangs off;
        // a cursor inside the typed suffix (or the separator) offers
        // nothing, so the typed lists survive an accept.
        CaretTokenShape::CloseIncomplete {
            route,
            block_id,
            link_end,
            suffix_offset,
            ..
        } => caret_link_field(
            first,
            route,
            block_id,
            link_end,
            Some((link_end, link_end)),
            false,
            cursor,
        )
        .filter(|_| cursor <= first.start + suffix_offset),
        CaretTokenShape::Partial { .. } => {
            if item.lines.len() != 1 {
                return None;
            }
            let query = first.text.get(1..relative)?.to_string();
            Some(CompletionField {
                context: CompletionContext::ActiveTask,
                route: None,
                block_id: None,
                query,
                replacement: (first.start + 1, first.end),
                bare_name: false,
            })
        }
        CaretTokenShape::NamePartial {
            route,
            block_id,
            link_end,
            name_range,
            start_offset,
            ..
        } => {
            if item.lines.len() != 1 {
                return None;
            }
            let (name_range, bare_name) = caret_suffix_name_completion_range(
                first,
                start_offset,
                Some(name_range),
            );
            let name_range = name_range?;
            caret_link_field(
                first,
                route,
                block_id,
                link_end,
                Some(name_range),
                bare_name,
                cursor,
            )
            .filter(|_| {
                start_offset.is_none_or(|offset| {
                    let name_start = first.start + name_range.0;
                    let name_end = first.start + name_range.1;
                    if name_start >= first.start + offset {
                        cursor <= name_end
                    } else {
                        cursor <= first.start + offset
                    }
                })
            })
        }
        CaretTokenShape::Complete {
            route,
            block_id,
            link_end,
            name_range,
            start_offset,
            bare_name,
            ..
        } => {
            let (completion_name_range, completion_bare_name) =
                caret_suffix_name_completion_range(
                    first,
                    start_offset,
                    name_range,
                );
            caret_link_field(
                first,
                route,
                block_id,
                link_end,
                completion_name_range.or(Some((link_end, link_end))),
                completion_bare_name || bare_name,
                cursor,
            )
            .filter(|_| {
                // A suffix name used to discard the link part: admit cursors in
                // the link part as well as the suffix name, while keeping
                // timing bytes (`=`, `=3`, or the trailing `3` in `=bugs3`)
                // non-completable. Replacement may still cover trailing glued
                // timing so acceptance keeps the typed count.
                if cursor <= first.start + link_end {
                    return true;
                }
                start_offset.is_none_or(|offset| {
                    if let Some((name_start, name_end)) = name_range
                        && first.start + name_start >= first.start + offset
                    {
                        cursor >= first.start + name_start
                            && cursor <= first.start + name_end
                    } else {
                        cursor <= first.start + offset
                    }
                })
            })
        }
    }
}

/// Extra trailing glued `<X>` bytes after a name when no leading timing is
/// present. Retired `@` is not glued timing.
fn trailing_glued_name_extra(after_name: &str, leading: &str) -> usize {
    if !leading.is_empty() || after_name.starts_with('@') {
        0
    } else {
        start_timing_len(after_name)
    }
}

/// Include a hash-start suffix's delimiter in its completion replacement.
/// This lets the candidate formatter keep `#` for names that need it and
/// remove it for names that can be written bare. Trailing glued timing
/// joins the replacement when there is no leading `<X>`.
fn caret_suffix_name_completion_range(
    token: &Token<'_>,
    start_offset: Option<usize>,
    name_range: Option<(usize, usize)>,
) -> (Option<(usize, usize)>, bool) {
    let (Some(offset), Some((name_start, name_end))) =
        (start_offset, name_range)
    else {
        return (name_range, false);
    };
    let prefix = token.text.get(offset + 1..name_start).unwrap_or("");
    let leading = prefix.strip_suffix('#').unwrap_or(prefix);
    let extra = trailing_glued_name_extra(
        token.text.get(name_end..).unwrap_or(""),
        leading,
    );
    let range_end = name_end + extra;
    if name_start <= offset
        || token.text.as_bytes().get(name_start.saturating_sub(1))
            != Some(&b'#')
        || token
            .text
            .get(name_start..name_end)
            .is_some_and(super::pomodoro_literal::opens_exact_name)
    {
        return (Some((name_start, range_end)), false);
    }
    (Some((name_start - 1, range_end)), true)
}

/// A bare-spellable hash name is intentionally invalid for execution, but
/// completion still needs to offer its candidate so acceptance can remove
/// the retired `#`.
fn retired_caret_hash_start_field(
    token: &Token<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let marker = token.text.strip_prefix('^')?;
    let (route_part, tail) = marker.split_once(':')?;
    let (block_id, raw_start) = tail.split_once('=')?;
    if !is_route_token(route_part)
        || !is_block_id(block_id)
        || block_id.contains('#')
    {
        return None;
    }
    let start_token = format!("={raw_start}");
    let Some(EqualsToken::Start {
        suffix,
        len,
        name: Some(name),
        name_spelling: Some(NameSpelling::Hash),
        name_is_literal: false,
        doubled: false,
        ..
    }) = session_equals_token(&start_token)
    else {
        return None;
    };
    if len != start_token.len() || !super::bare_name::can_spell_bare(&name) {
        return None;
    }
    let link_end = 1 + route_part.len() + 1 + block_id.len();
    let suffix_offset = link_end;
    let name_start = suffix_offset
        + named_start_name_offset(suffix.len(), NameSpelling::Hash);
    let name_range = (name_start - 1, name_start + name.len());
    caret_link_field(
        token,
        route_part.to_ascii_lowercase(),
        block_id.to_string(),
        link_end,
        Some(name_range),
        true,
        cursor,
    )
}

/// Build the completion field for the `route:block-id[#name]` portion of a
/// `^` token: inside the link part the query runs from just after `^` to
/// the cursor and the replacement ends before any `#`/`=`; at or after `#`
/// the query is the typed name and the replacement covers the name part, so
/// accepting a candidate preserves a typed `#name`/`=<X>` suffix. `link_end`
/// and `name_range` are token-relative offsets from `classify_caret_token`.
pub(super) fn caret_link_field(
    token: &Token<'_>,
    route: String,
    block_id: String,
    link_end: usize,
    name_range: Option<(usize, usize)>,
    bare_name: bool,
    cursor: usize,
) -> Option<CompletionField> {
    let link_end_abs = token.start + link_end;
    if cursor <= link_end_abs {
        let query = token.text.get(1..cursor - token.start)?.to_string();
        return Some(CompletionField {
            context: CompletionContext::ActiveTask,
            route: None,
            block_id: None,
            query,
            replacement: (token.start + 1, link_end_abs),
            bare_name: false,
        });
    }
    let (name_start, name_end) = name_range?;
    let name_start_abs = token.start + name_start;
    let name_end_abs = token.start + name_end;
    if cursor < name_start_abs || cursor > token.end {
        return None;
    }
    let content_start = if bare_name
        && token
            .text
            .get(name_start..name_end)
            .is_some_and(|text| text.starts_with('#'))
    {
        name_start + 1
    } else {
        name_start
    };
    // A partial literal completes on the literal's decoded text; the
    // replacement still covers the whole partial literal.
    if let Some(after_hash) = token.text.get(name_start..)
        && super::pomodoro_literal::opens_exact_name(after_hash)
    {
        let typed_before =
            after_hash.get(..cursor - token.start - name_start)?;
        let inner = typed_before
            .strip_prefix('"')
            .or_else(|| typed_before.strip_prefix('“'))
            .unwrap_or(typed_before);
        let query = super::pomodoro_literal::decode_partial_literal_text(inner);
        return Some(CompletionField {
            context: CompletionContext::PomodoroName,
            route: Some(route),
            block_id: Some(block_id),
            query,
            replacement: (name_start_abs, name_end_abs),
            bare_name,
        });
    }
    let query = token
        .text
        .get(content_start..cursor - token.start)?
        .to_string();
    Some(CompletionField {
        context: CompletionContext::PomodoroName,
        route: Some(route),
        block_id: Some(block_id),
        query,
        replacement: (name_start_abs, name_end_abs),
        bare_name,
    })
}

/// Lexed facts for the named-start (`=<X>#name` / `=name`) token whose
/// name part holds `cursor`: the raw `<X>` suffix, the name text, the
/// absolute name-part span, and whether the name is bare-spelled. Retired
/// doubled starts never offer completion. `None` when the cursor is not
/// inside such a name part: just after `#` (hash) or just after `=`
/// (bare) through the end of the name part. A cursor on `=<X>` or at the
/// `#` byte itself yields nothing. A hash-delimited selector includes `#`
/// in its replacement unless it is a quoted exact-name literal, which keeps
/// its typed delimiter. The lexer decides what counts as a named start; validity never matters
/// here, so near misses lex the same way valid tokens do.
fn named_start_name_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<(String, String, (usize, usize), bool)> {
    let parent = item.lines.first()?;
    let text = parent.raw.text;
    let trimmed = text.trim();
    let super::item::EqualsToken::Start {
        suffix,
        name: Some(name),
        name_spelling: Some(spelling),
        name_is_literal,
        doubled: false,
        timing,
        ..
    } = super::item::session_equals_token(trimmed)?
    else {
        return None;
    };
    let leading = text.len() - text.trim_start().len();
    let token_start = parent.raw.start + leading;
    let name_start = token_start
        + super::item::named_start_name_offset(suffix.len(), spelling);
    // A partial literal completes on the literal's decoded text, and its
    // replacement covers the whole partial literal including the quotes.
    if name_is_literal {
        let rel_start = name_start - token_start;
        let after_hash = trimmed.get(rel_start..).unwrap_or("");
        let lex = super::pomodoro_literal::lex_exact_name_after_hash(
            after_hash,
            after_hash.len(),
        )?;
        let name_end = name_start + lex.literal_len;
        if cursor < name_start || cursor > name_end {
            return None;
        }
        let typed_before = after_hash.get(..cursor - name_start)?;
        let inner = typed_before
            .strip_prefix('"')
            .or_else(|| typed_before.strip_prefix('“'))
            .unwrap_or(typed_before);
        let query = super::pomodoro_literal::decode_partial_literal_text(inner);
        return Some((suffix, query, (name_start, name_end), false));
    }
    let name_end = name_start + name.len();
    let hash_start = if spelling == super::bare_name::NameSpelling::Hash {
        name_start.saturating_sub(1)
    } else {
        name_start
    };
    let mut range_end = name_end;
    if let Some((offset, text)) = timing.as_ref()
        && !text.is_empty()
        && trimmed.as_bytes().get(*offset) != Some(&b'@')
    {
        range_end += trailing_glued_name_extra(text, &suffix);
    }
    if cursor < name_start || cursor > range_end {
        return None;
    }
    let query = if cursor <= name_end {
        name.get(..cursor - name_start)?.to_string()
    } else {
        name.clone()
    };
    Some((suffix, query, (hash_start, range_end), true))
}

/// Completion field for the name part of a `=<X>#name` / `=name` named
/// Pomodoro start. The cursor must lie inside the name part, or inside
/// trailing glued timing when there is no leading `<X>`: just after `#`
/// (hash) or just after `=` (bare) through the end of that range. A cursor
/// on leading timing or at the `#` byte itself yields nothing. The
/// replacement includes `#` for selector names so candidate formatting can
/// keep it only when the selected name needs it; quoted exact-name
/// literals keep their typed delimiter. The replacement covers the whole
/// name part and any trailing glued timing regardless of where the cursor
/// sits inside it.
fn pomodoro_start_name_field(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    // `named_start_name_at` already truncates the query at the cursor
    // (decoded for literals, raw otherwise).
    let (_, query, (name_start, name_end), bare_name) =
        named_start_name_at(item, cursor)?;
    Some(CompletionField {
        context: CompletionContext::PomodoroStartName,
        route: None,
        block_id: None,
        query,
        replacement: (name_start, name_end),
        bare_name,
    })
}

/// Completion field for the segment suffix at the end of `E` (`~`, `~n`,
/// `~n+`, `~~`). The cursor must lie inside the suffix bytes (from its `~`
/// through digits, `+`, or a second `~`, before `!` or the second `=`). The
/// replacement covers the suffix range only; the query is the text after
/// the first `~` up to the cursor.
fn pomodoro_past_segment_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let parent = item.lines.first()?;
    let text = parent.raw.text.trim_start();
    let token_len = super::item::past_token_len(text)?;
    let token = &text[..token_len];
    let token_start = parent.raw.start + parent.raw.text.len() - text.len();
    let split = super::past::split_past_token(token)?;
    let (seg_start_rel, seg_end_rel) =
        match super::past::validate_past_e(token, &split) {
            super::past::PastEOutcome::Valid(valid) => valid.segment_range?,
            _ => return None,
        };
    let seg_start = token_start + seg_start_rel;
    let seg_end = token_start + seg_end_rel;
    if cursor < seg_start || cursor > seg_end {
        return None;
    }
    let suffix = token.get(seg_start_rel..seg_end_rel)?;
    // Query is the text after the first `~` up to the cursor.
    let typed_len = cursor.saturating_sub(seg_start + 1);
    let query = suffix
        .get(1..)?
        .get(..typed_len.min(suffix.len().saturating_sub(1)))?
        .to_string();
    Some(CompletionField {
        context: CompletionContext::PomodoroPastSegment,
        route: None,
        block_id: None,
        query,
        replacement: (seg_start, seg_end),
        bare_name: false,
    })
}

/// The name in `x=<E>` owns only its own bytes, including a quoted
/// literal but excluding counts, repair, timing, and close selections. A
/// leading count still leaves a name field (`x=3bu|`), and a plain `#`
/// name keeps its `#` in the replacement so accepting a bare-spellable
/// row drops it (`x=#bo|` plus `robot` gives `x=robot`).
fn pomodoro_past_name_field(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let parent = item.lines.first()?;
    let text = parent.raw.text.trim_start();
    let token_len = super::item::past_token_len(text)?;
    let token = &text[..token_len];
    let token_start = parent.raw.start + parent.raw.text.len() - text.len();
    let after_eq = &token[2..];
    let digits = after_eq.bytes().take_while(u8::is_ascii_digit).count();
    let after_digits = &after_eq[digits..];
    if let Some(after_hash) = after_digits.strip_prefix('#') {
        let literal = super::pomodoro_literal::opens_exact_name(after_hash);
        let name_len = if literal {
            super::pomodoro_literal::lex_exact_name_after_hash(
                after_hash,
                after_hash.len(),
            )?
            .literal_len
        } else {
            let extent = super::item::past_name_extent(after_hash);
            after_hash[..extent].find('*').unwrap_or(extent)
        };
        // A plain `#` name keeps its `#` in the replacement unless it is
        // a quoted literal, which keeps its typed delimiter instead.
        let hash_len = usize::from(!literal);
        let start = token_start + 2 + digits + usize::from(literal);
        let end = start + hash_len + name_len;
        if cursor < start || cursor > end {
            return None;
        }
        // A cursor on the `#` byte itself yields nothing.
        if !literal && cursor == start {
            return None;
        }
        let query = if literal {
            exact_name_query(
                &after_hash[..name_len],
                cursor - start - hash_len,
                true,
            )
        } else {
            after_hash.get(..cursor - start - hash_len)?.to_string()
        };
        return Some(CompletionField {
            context: CompletionContext::PomodoroPastName,
            route: None,
            block_id: None,
            query,
            replacement: (start, end),
            bare_name: !literal,
        });
    }
    // A bare name may follow a leading count (`x=3bu|`); the field covers
    // the bare name only, never the trailing count.
    if after_digits.is_empty() {
        let start = token_start + 2 + digits;
        if cursor != start {
            return None;
        }
        return Some(CompletionField {
            context: CompletionContext::PomodoroPastName,
            route: None,
            block_id: None,
            query: String::new(),
            replacement: (start, start),
            bare_name: true,
        });
    }
    let name_len = super::bare_name::bare_start_name_len(after_digits)?;
    let start = token_start + 2 + digits;
    let name_end = start + name_len;
    let leading = if digits == 0 { "" } else { &after_eq[..digits] };
    let extra = trailing_glued_name_extra(&after_digits[name_len..], leading);
    let range_end = name_end + extra;
    if cursor < start || cursor > range_end {
        return None;
    }
    let query = if cursor <= name_end {
        after_digits.get(..cursor - start)?.to_string()
    } else {
        after_digits.get(..name_len)?.to_string()
    };
    Some(CompletionField {
        context: CompletionContext::PomodoroPastName,
        route: None,
        block_id: None,
        query,
        replacement: (start, range_end),
        bare_name: true,
    })
}

/// Completion field for a `=/` split base or `#type` selector. The cursor
/// must lie inside the base (just after `=/` through the end of the base)
/// or inside a selector name (just after `#` through the end of that
/// selector). A cursor on `=/` or at a `#` byte itself yields nothing, and
/// neither sigil is inside the replacement. The replacement covers the
/// whole base or selector part regardless of where the cursor sits inside
/// it, preserving surrounding selectors, quotes, and chain siblings.
fn pomodoro_split_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let parent = item.lines.first()?;
    let text = parent.raw.text;
    let trimmed = text.trim();
    let super::item::EqualsToken::Split {
        base,
        selectors,
        len,
    } = super::item::session_equals_token(trimmed)?
    else {
        return None;
    };
    let _ = len;
    let leading = text.len() - text.trim_start().len();
    let token_start = parent.raw.start + leading;
    // Base range: just after `=/` through the end of the base (empty when
    // omitted). A cursor there completes the base.
    let base_start = token_start + 2;
    let base_end = base_start + base.as_deref().unwrap_or_default().len();
    if cursor >= base_start && cursor <= base_end {
        // When a `#` selector starts exactly at the cursor (e.g. `=/#fix`
        // with the cursor before `#`), the cursor is on a selector sigil,
        // not in the base.
        let first = trimmed.split_whitespace().next().unwrap_or(trimmed);
        let rel = cursor - token_start;
        if first.as_bytes().get(rel) == Some(&b'#') {
            return None;
        }
        let query = base
            .as_deref()
            .unwrap_or_default()
            .get(..cursor - base_start)
            .unwrap_or_default()
            .to_string();
        return Some(CompletionField {
            context: CompletionContext::PomodoroSplitBase,
            route: None,
            block_id: None,
            query,
            replacement: (base_start, base_end),
            bare_name: true,
        });
    }
    // Selector ranges: walk `#` parts in order, tracking absolute offsets.
    // Plain names cover their text; literals cover their quotes.
    let mut rel = (base_end - token_start) as usize;
    // Re-derive selector extents from the first whitespace-delimited token
    // so quoted literals with spaces stay aligned.
    let first = trimmed.split_whitespace().next().unwrap_or(trimmed);
    // `rel` currently points just after the base (relative to token start).
    // Walk each selector in order.
    let mut selector_index = 0;
    while rel < first.len() && first.as_bytes()[rel] == b'#' {
        let after_hash = &first[rel + 1..];
        let selector = selectors.get(selector_index)?;
        let name_start_abs = token_start + rel + 1;
        let name_end_abs = if selector.literal {
            if let Some(lex) =
                super::pomodoro_literal::lex_exact_name_after_hash(
                    after_hash,
                    after_hash.len(),
                )
            {
                if lex.complete {
                    name_start_abs + lex.literal_len
                } else {
                    // Unterminated literal: the name runs to the token end.
                    token_start + first.len()
                }
            } else {
                name_start_abs
            }
        } else {
            name_start_abs + selector.text.len()
        };
        if cursor > name_start_abs - 1
            && cursor <= name_end_abs
            && cursor >= name_start_abs
        {
            // Cursor just after `#` through the end of the name.
            let typed_len = cursor - name_start_abs;
            let query = if selector.literal {
                super::pomodoro_literal::lex_exact_name_after_hash(
                    after_hash,
                    after_hash.len(),
                )
                .map(|_lex| {
                    // Truncate the decoded query at the cursor: decode the
                    // typed prefix the same way the literal lexer does.
                    let typed = &after_hash[..typed_len.min(after_hash.len())];
                    super::pomodoro_literal::decode_partial_literal_text(
                        typed
                            .strip_prefix('"')
                            .or_else(|| typed.strip_prefix('“'))
                            .unwrap_or(typed),
                    )
                })
                .unwrap_or_default()
            } else {
                selector
                    .text
                    .get(..typed_len)
                    .unwrap_or_default()
                    .to_string()
            };
            // An empty decoded query still completes (unfiltered list).
            return Some(CompletionField {
                context: CompletionContext::PomodoroSplitType,
                route: None,
                block_id: None,
                query,
                replacement: (name_start_abs, name_end_abs),
                bare_name: false,
            });
        }
        // A cursor exactly on `#` yields nothing.
        if cursor == name_start_abs - 1 {
            return None;
        }
        // Advance past this selector.
        if selector.literal {
            if let Some(lex) =
                super::pomodoro_literal::lex_exact_name_after_hash(
                    after_hash,
                    after_hash.len(),
                )
            {
                rel += 1 + lex.literal_len;
            } else {
                rel += 1;
            }
        } else {
            rel += 1 + selector.text.len();
        }
        selector_index += 1;
    }
    None
}

/// Completion field for a `=*` merge source or `#dest` selector. The
/// cursor just after `=*` (including a dest-less token) completes the
/// source; a cursor just after `#` through a dest name completes that
/// dest. Neither sigil is inside the replacement.
fn pomodoro_merge_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let parent = item.lines.first()?;
    let text = parent.raw.text;
    let trimmed = text.trim();
    let super::item::EqualsToken::Merge { src, dests, len } =
        super::item::session_equals_token(trimmed)?
    else {
        return None;
    };
    let _ = len;
    let leading = text.len() - text.trim_start().len();
    let token_start = parent.raw.start + leading;
    let src_start = token_start + 2;
    let first = trimmed.split_whitespace().next().unwrap_or(trimmed);
    let src_len = if src.as_deref().is_some_and(|name| !name.is_empty()) {
        let after_sigil = first.get(2..).unwrap_or("");
        if super::pomodoro_literal::opens_exact_name(after_sigil)
            && let Some(lex) =
                super::pomodoro_literal::lex_exact_name_after_hash(
                    after_sigil,
                    after_sigil.len(),
                )
        {
            lex.literal_len
        } else {
            src.as_deref().unwrap_or_default().len()
        }
    } else {
        0
    };
    let src_end = src_start + src_len;
    if cursor >= src_start && cursor <= src_end {
        let rel = cursor - token_start;
        if first.as_bytes().get(rel) == Some(&b'#') {
            // fall through to dest handling
        } else {
            let query = src
                .as_deref()
                .unwrap_or_default()
                .get(..cursor.saturating_sub(src_start))
                .unwrap_or_default()
                .to_string();
            return Some(CompletionField {
                context: CompletionContext::PomodoroMergeSrc,
                route: None,
                block_id: None,
                query,
                replacement: (src_start, src_end),
                bare_name: true,
            });
        }
    }
    let mut rel = src_end - token_start;
    let mut dest_index = 0;
    while rel < first.len() && first.as_bytes()[rel] == b'#' {
        let after_hash = &first[rel + 1..];
        let dest = dests.get(dest_index)?;
        let name_start_abs = token_start + rel + 1;
        let name_end_abs = if dest.literal {
            if let Some(lex) =
                super::pomodoro_literal::lex_exact_name_after_hash(
                    after_hash,
                    after_hash.len(),
                )
            {
                if lex.complete {
                    name_start_abs + lex.literal_len
                } else {
                    token_start + first.len()
                }
            } else {
                name_start_abs
            }
        } else {
            name_start_abs + dest.text.len()
        };
        if cursor >= name_start_abs && cursor <= name_end_abs {
            let typed_len = cursor - name_start_abs;
            let query = if dest.literal {
                super::pomodoro_literal::lex_exact_name_after_hash(
                    after_hash,
                    after_hash.len(),
                )
                .map(|_lex| {
                    let typed = &after_hash[..typed_len.min(after_hash.len())];
                    super::pomodoro_literal::decode_partial_literal_text(
                        typed
                            .strip_prefix('"')
                            .or_else(|| typed.strip_prefix('“'))
                            .unwrap_or(typed),
                    )
                })
                .unwrap_or_default()
            } else {
                dest.text.get(..typed_len).unwrap_or_default().to_string()
            };
            return Some(CompletionField {
                context: CompletionContext::PomodoroMergeDest,
                route: None,
                block_id: None,
                query,
                replacement: (name_start_abs, name_end_abs),
                bare_name: false,
            });
        }
        if cursor == name_start_abs - 1 {
            return None;
        }
        if dest.literal {
            if let Some(lex) =
                super::pomodoro_literal::lex_exact_name_after_hash(
                    after_hash,
                    after_hash.len(),
                )
            {
                rel += 1 + lex.literal_len;
            } else {
                rel += 1;
            }
        } else {
            rel += 1 + dest.text.len();
        }
        dest_index += 1;
    }
    None
}

/// For a `pomodoro_start_name` field, whether a swap would keep the
/// running ledger: `Some(true)` when the leading `<X>` suffix is empty
/// and no `@` part was typed, `Some(false)` for a fresh `<X>` timing.
/// `None` when the cursor is not inside a named-start name part, or when
/// an earlier item or earlier same-line session operator is in the draft.
pub(crate) fn pomodoro_start_override_at(
    raw_text: &str,
    cursor: usize,
) -> Option<bool> {
    if cursor_in_close_log_text(raw_text, cursor) {
        return None;
    }
    let draft = split_capture_draft(raw_text);
    let item_index = draft.items.iter().position(|item| {
        item.lines
            .iter()
            .any(|line| cursor >= line.raw.start && cursor <= line.raw.end)
    })?;
    let item = &draft.items[item_index];
    for earlier in &draft.items[..item_index] {
        let Some(parent) = earlier.lines.first() else {
            continue;
        };
        if super::item::is_session_chain_token(parent.raw.text.trim()) {
            return None;
        }
    }
    let (suffix, _, _, _) = named_start_name_at(item, cursor)?;
    let parent = item.lines.first()?;
    let text = parent.raw.text;
    let leading = text.len() - text.trim_start().len();
    let named_token_start = parent.raw.start + leading;
    if tokenize_line_with_spans(&parent.raw).iter().any(|token| {
        token.end <= named_token_start
            && super::item::is_session_chain_token(token.text)
    }) {
        return None;
    }
    let trimmed = parent.raw.text.trim();
    let super::item::EqualsToken::Start { timing, .. } =
        super::item::session_equals_token(trimmed)?
    else {
        return None;
    };
    Some(suffix.is_empty() && timing.is_none())
}

/// Which transfer component the cursor sits in, for the engine to scope
/// candidates. Sources complete movable-or-mergeable tasks; a complete bare
/// destination also carries the `+ merge into a task` hint; merge targets
/// exclude the typed sources and carry a merge consequence; sections add a
/// creation row and a near-duplicate refusal on top of the note's headings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferCompletionRole {
    Source {
        route: String,
    },
    DestinationRoute,
    DestinationNote {
        route: String,
    },
    MergeTarget {
        route: String,
        exclude: Vec<(String, String)>,
    },
    VaultMergeTarget {
        exclude: Vec<(String, String)>,
    },
    Drawer {
        route: String,
        block_id: String,
    },
    Section {
        route: String,
    },
}

/// A merge-in source slot borrows the ordinary note-scoped and vault-wide
/// task pickers, with the typed sources excluded from both catalogs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeInCompletionRole {
    pub(crate) exclude: Vec<(String, String)>,
}

struct MergeInCompletionSlot<'a> {
    token: Option<Token<'a>>,
    exclude: Vec<(String, String)>,
}

/// The merge-in role at `cursor`, when it sits in an `<` source slot.
/// The slot's fields remain the ordinary `Route`, `Task`, and `TaskParent`
/// contexts; this role only scopes their rows.
pub(crate) fn merge_in_completion_role(
    raw_text: &str,
    cursor: usize,
) -> Option<MergeInCompletionRole> {
    let draft = split_capture_draft(raw_text);
    let item = draft.items.iter().find(|item| {
        item.lines.first().is_some_and(|line| {
            cursor >= line.raw.start && cursor <= line.raw.end
        })
    })?;
    let parent = item.lines.first()?.raw;
    let slot = merge_in_completion_slot(item, parent, cursor)?;
    slot.token.map(|_| MergeInCompletionRole {
        exclude: slot.exclude,
    })
}

/// Return the ordinary completion field inside a claimed merge-in source
/// slot. `Some(None)` means the `<` clause owns the caret (arrow or gap),
/// preventing target-run completion from claiming source text.
fn merge_in_completion_field_at(
    item: &CaptureItem<'_>,
    line: RawLine<'_>,
    cursor: usize,
) -> Option<Option<CompletionField>> {
    let slot = merge_in_completion_slot(item, line, cursor)?;
    Some(slot.token.and_then(|token| {
        if token.text.starts_with('+') {
            let relative = cursor.saturating_sub(token.start);
            let query = if relative <= 1 {
                String::new()
            } else {
                token.text.get(1..relative)?.to_string()
            };
            Some(CompletionField {
                context: CompletionContext::TaskParent,
                route: None,
                block_id: None,
                query,
                replacement: (token.start, token.end),
                bare_name: false,
            })
        } else {
            marker_field_at_cursor(&token, cursor)
        }
    }))
}

/// Locate a token owned by the `<` clause and the already-typed complete
/// sources. The source list is the scanner's result for valid and incomplete
/// clauses; malformed clauses still claim their text after the first
/// standalone arrow so they cannot fall through to normal route runs.
fn merge_in_completion_slot<'i, 's>(
    item: &'i CaptureItem<'s>,
    line: RawLine<'s>,
    cursor: usize,
) -> Option<MergeInCompletionSlot<'s>> {
    if cursor < line.start || cursor > line.end {
        return None;
    }
    let scan = super::merge_in::scan_merge_in_line(
        &line,
        item.source,
        item.lines.len() > 1,
    );
    let (arrow_start, arrow_end, sources) = match scan {
        super::merge_in::MergeInScan::Unclaimed => return None,
        super::merge_in::MergeInScan::Escaped { arrow_span, .. } => {
            (arrow_span.0, arrow_span.1, Vec::new())
        }
        super::merge_in::MergeInScan::Incomplete { sources, arrow, .. }
        | super::merge_in::MergeInScan::Valid { sources, arrow } => (
            arrow.start,
            arrow.end,
            sources
                .into_iter()
                .map(|source| (source.route, source.block_id))
                .collect(),
        ),
        super::merge_in::MergeInScan::Invalid { .. } => {
            let arrow = tokenize_line_with_spans(&line)
                .into_iter()
                .find(|token| token.text == "<")?;
            (arrow.start, arrow.end, Vec::new())
        }
    };
    if cursor < arrow_start {
        return None;
    }

    let token = tokenize_line_with_spans(&line).into_iter().find(|token| {
        token.start >= arrow_end
            && token.start <= cursor
            && cursor <= token.end
            && (token.text.starts_with('@') || token.text.starts_with('+'))
            && !crate::native::capture_links::is_protected_markup_range(
                item.source,
                token.start,
                token.end,
            )
    });
    Some(MergeInCompletionSlot {
        token,
        exclude: sources,
    })
}

/// The transfer role at `cursor`, when the cursor sits on a
/// transfer-claimed parent line in a completable component. Returns `None`
/// when the line is not a transfer or the cursor owns no component, so the
/// engine keeps its existing candidates. The cursor mapping mirrors
/// [`transfer_completion_field_at`]: both scan the same parent line, so a
/// role exists exactly when that function returns a field.
pub(crate) fn transfer_completion_role(
    raw_text: &str,
    cursor: usize,
) -> Option<TransferCompletionRole> {
    let draft = split_capture_draft(raw_text);
    let item = draft.items.iter().find(|item| {
        item.lines
            .iter()
            .any(|line| cursor >= line.raw.start && cursor <= line.raw.end)
    })?;
    if item.lines.first()?.raw.start
        != item
            .lines
            .iter()
            .find(|line| cursor >= line.raw.start && cursor <= line.raw.end)?
            .raw
            .start
    {
        return None;
    }
    let parent = item.lines.first()?.raw;
    if cursor < parent.start || cursor > parent.end {
        return None;
    }
    let scan = super::transfer::scan_transfer_line(
        &parent,
        item.source,
        item.lines.len() > 1,
    );
    match scan {
        super::transfer::TransferScan::Valid {
            sources,
            arrow,
            destination,
        } => transfer_role_for_valid(cursor, &sources, &arrow, &destination),
        super::transfer::TransferScan::Incomplete {
            sources,
            arrow,
            need,
            ..
        } => {
            transfer_role_for_incomplete(parent, cursor, &sources, &arrow, need)
        }
        super::transfer::TransferScan::Invalid { .. } => {
            transfer_role_for_invalid(parent, cursor)
        }
        _ => None,
    }
}

/// Role for a cursor on a transfer-shaped line the lexer refused. Partial
/// sources (`@route+` still being typed) still narrow to that note's
/// tasks, and a destination fragment still scopes like its incomplete
/// twin; the parsed sources are unavailable, so complete `@route+id`
/// tokens before the arrow stand in for the exclusion list.
fn transfer_role_for_invalid(
    parent: RawLine<'_>,
    cursor: usize,
) -> Option<TransferCompletionRole> {
    let tokens = tokenize_line_with_spans(&parent);
    let arrow = tokens
        .iter()
        .find(|token| token.text == ">")
        .map(|token| token.start);
    let token = tokens.iter().find(|token| {
        cursor >= token.start
            && cursor <= token.end
            && token.text.starts_with('@')
    })?;
    let rest = token.text.get(1..)?;
    let end = rest.find(['+', '#', ':', '^']).unwrap_or(rest.len());
    let route = rest.get(..end)?;
    if !is_route_token(route) {
        return None;
    }
    let route = route.to_ascii_lowercase();
    let before_arrow = arrow.is_none_or(|arrow| token.start < arrow);
    if before_arrow {
        if !rest.contains('+') {
            return None;
        }
        return Some(TransferCompletionRole::Source { route });
    }
    if rest.contains('+') {
        let exclude = tokens
            .iter()
            .filter(|other| {
                arrow.is_none_or(|arrow| other.end <= arrow)
                    && other.text.starts_with('@')
            })
            .filter_map(|other| {
                super::transfer::split_complete_source(other.text)
            })
            .collect();
        return Some(TransferCompletionRole::MergeTarget { route, exclude });
    }
    if rest.contains('#') {
        return Some(TransferCompletionRole::Section { route });
    }
    Some(TransferCompletionRole::DestinationRoute)
}

/// Role for a cursor on a complete transfer line.
fn transfer_role_for_valid(
    cursor: usize,
    sources: &[super::transfer::TransferSource],
    arrow: &super::transfer::TransferArrow,
    destination: &super::transfer::TransferDestination,
) -> Option<TransferCompletionRole> {
    use super::transfer::TransferDestinationKind;
    if let Some(source) = sources
        .iter()
        .find(|source| cursor >= source.start && cursor <= source.end)
    {
        // The engine only scopes `Task` fields; a cursor in the route
        // part keeps the plain route list.
        return Some(TransferCompletionRole::Source {
            route: source.route.clone(),
        });
    }
    if cursor >= arrow.start && cursor <= arrow.end {
        return None;
    }
    if cursor >= destination.start && cursor <= destination.end {
        let exclude = sources
            .iter()
            .map(|source| (source.route.clone(), source.block_id.clone()))
            .collect();
        return match destination.kind {
            TransferDestinationKind::Note => {
                if cursor == destination.end {
                    Some(TransferCompletionRole::DestinationNote {
                        route: destination.route.clone(),
                    })
                } else {
                    Some(TransferCompletionRole::DestinationRoute)
                }
            }
            TransferDestinationKind::Task => {
                Some(TransferCompletionRole::MergeTarget {
                    route: destination.route.clone(),
                    exclude,
                })
            }
            TransferDestinationKind::Drawer => {
                Some(TransferCompletionRole::Drawer {
                    route: destination.route.clone(),
                    block_id: destination.block_id.clone().unwrap_or_default(),
                })
            }
            TransferDestinationKind::Section => {
                Some(TransferCompletionRole::Section {
                    route: destination.route.clone(),
                })
            }
        };
    }
    None
}

/// Role for a cursor on a partial transfer line.
fn transfer_role_for_incomplete(
    parent: RawLine<'_>,
    cursor: usize,
    sources: &[super::transfer::TransferSource],
    arrow: &super::transfer::TransferArrow,
    need: super::transfer::TransferNeed,
) -> Option<TransferCompletionRole> {
    if sources
        .iter()
        .any(|source| cursor >= source.start && cursor <= source.end)
    {
        let route = sources
            .iter()
            .find(|source| cursor >= source.start && cursor <= source.end)
            .map(|source| source.route.clone())?;
        return Some(TransferCompletionRole::Source { route });
    }
    let fragment = transfer_destination_fragment(parent, arrow.end)?;
    if cursor < fragment.0 || cursor > fragment.1 {
        return None;
    }
    if need == super::transfer::TransferNeed::Task {
        let (start, end) = transfer_destination_fragment(parent, arrow.end)?;
        if cursor < start || cursor > end {
            return None;
        }
        return Some(TransferCompletionRole::VaultMergeTarget {
            exclude: sources
                .iter()
                .map(|source| (source.route.clone(), source.block_id.clone()))
                .collect(),
        });
    }
    // A partial destination completes through the same components as a
    // whole marker: routes, task IDs, drawers, and sections.
    let text = parent.text.get(
        fragment.0.checked_sub(parent.start)?
            ..fragment.1.checked_sub(parent.start)?,
    )?;
    if !text.starts_with('@') {
        return None;
    }
    if let Some(plus) = text.find('+') {
        let route = text[1..plus].to_ascii_lowercase();
        if text[plus + 1..].contains('#') {
            return Some(TransferCompletionRole::Drawer {
                route,
                block_id: String::new(),
            });
        }
        return Some(TransferCompletionRole::MergeTarget {
            route,
            exclude: sources
                .iter()
                .map(|source| (source.route.clone(), source.block_id.clone()))
                .collect(),
        });
    }
    if text.contains('#') {
        return Some(TransferCompletionRole::Section {
            route: text[1..text.find('#').unwrap_or(text.len())]
                .to_ascii_lowercase(),
        });
    }
    Some(TransferCompletionRole::DestinationRoute)
}

/// Byte range of the destination fragment on an incomplete transfer line:
/// the first whitespace-delimited token after the arrow, or `None` when
/// nothing is typed there yet.
fn transfer_destination_fragment(
    parent: RawLine<'_>,
    arrow_end: usize,
) -> Option<(usize, usize)> {
    let mut start = arrow_end;
    while start < parent.end
        && parent
            .text
            .get(start.checked_sub(parent.start)?..)
            .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with('\t'))
    {
        start += 1;
    }
    if start >= parent.end {
        return None;
    }
    let mut end = start;
    while end < parent.end
        && parent
            .text
            .get(end.checked_sub(parent.start)?..)
            .is_some_and(|rest| {
                !(rest.starts_with(' ') || rest.starts_with('\t'))
            })
    {
        end += 1;
    }
    Some((start, end))
}

/// Transfer-aware completion for one parent line.
///
/// A claimed `>` line owns the whole line, so the normal target-run
/// selection cannot serve it: this maps the cursor to the component it
/// sits in and reuses [`marker_field_at_cursor`] for that component, so
/// sources complete routes and tasks, `> @` completes routes, `@c+`
/// completes tasks, `@c+id#` completes drawers, and `@c#` completes
/// sections.
///
/// Returns `None` when the line is no transfer (the caller continues with
/// ordinary completion). Returns `Some(None)` when the line is claimed but
/// the cursor sits on the arrow, in a gap, or past the destination, so the
/// line completes nothing instead of falling through to target runs.
fn transfer_completion_field_at(
    source: &str,
    line: RawLine<'_>,
    has_children: bool,
    cursor: usize,
) -> Option<Option<CompletionField>> {
    use super::transfer::{scan_transfer_line, TransferScan};
    if cursor < line.start || cursor > line.end {
        return None;
    }
    match scan_transfer_line(&line, source, has_children) {
        TransferScan::Unclaimed | TransferScan::Escaped { .. } => None,
        TransferScan::Invalid { .. } => {
            Some(transfer_invalid_field_at(line, cursor))
        }
        TransferScan::Incomplete { sources, arrow, .. } => {
            Some(transfer_partial_field_at(line, cursor, &sources, arrow.end))
        }
        TransferScan::Valid {
            sources,
            arrow,
            destination,
        } => Some(transfer_valid_field_at(
            line,
            cursor,
            &sources,
            &arrow,
            &destination,
        )),
    }
}

/// Completion on a transfer-shaped line the lexer refused: still complete
/// a source the cursor sits in, so a teaching error never hides the task
/// list. Anything else on the claimed line completes nothing.
fn transfer_invalid_field_at(
    line: RawLine<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let token = tokenize_line_with_spans(&line)
        .into_iter()
        .find(|token| cursor >= token.start && cursor <= token.end)?;
    if !token.text.starts_with('@') {
        return None;
    }
    marker_field_at_cursor(&token, cursor)
}

/// Completion on a partial transfer: a source completes through its own
/// marker, and the destination fragment after the arrow completes like a
/// whole marker. Gaps and the arrow itself complete nothing.
fn transfer_partial_field_at(
    line: RawLine<'_>,
    cursor: usize,
    sources: &[super::transfer::TransferSource],
    arrow_end: usize,
) -> Option<CompletionField> {
    if let Some(source) = sources
        .iter()
        .find(|source| cursor >= source.start && cursor <= source.end)
    {
        return marker_field_at_cursor(
            &Token {
                text: line.text.get(
                    source.start.checked_sub(line.start)?
                        ..source.end.checked_sub(line.start)?,
                )?,
                start: source.start,
                end: source.end,
            },
            cursor,
        );
    }
    let (start, end) = transfer_destination_fragment(line, arrow_end)?;
    if cursor < start || cursor > end {
        return None;
    }
    marker_field_at_cursor(
        &Token {
            text: line.text.get(
                start.checked_sub(line.start)?..end.checked_sub(line.start)?,
            )?,
            start,
            end,
        },
        cursor,
    )
}

/// Completion on a complete transfer: sources and the destination complete
/// through their own markers. The arrow, the gaps around it, and trailing
/// text past the destination complete nothing.
fn transfer_valid_field_at(
    line: RawLine<'_>,
    cursor: usize,
    sources: &[super::transfer::TransferSource],
    arrow: &super::transfer::TransferArrow,
    destination: &super::transfer::TransferDestination,
) -> Option<CompletionField> {
    if let Some(source) = sources
        .iter()
        .find(|source| cursor >= source.start && cursor <= source.end)
    {
        return marker_field_at_cursor(
            &Token {
                text: line.text.get(
                    source.start.checked_sub(line.start)?
                        ..source.end.checked_sub(line.start)?,
                )?,
                start: source.start,
                end: source.end,
            },
            cursor,
        );
    }
    if cursor >= destination.start && cursor <= destination.end {
        return marker_field_at_cursor(
            &Token {
                text: line.text.get(
                    destination.start.checked_sub(line.start)?
                        ..destination.end.checked_sub(line.start)?,
                )?,
                start: destination.start,
                end: destination.end,
            },
            cursor,
        );
    }
    let _ = arrow;
    None
}

/// Identify the completable marker component at `cursor`, reusing the same
/// tokenizer, terminal-marker extraction, and `@token` candidate detection
/// as [`parse_for_editor`]. Returns `None` when the cursor is not inside an
/// eligible leading or trailing `@` marker: plain body text, a token in the
/// middle of the input, and an unrecognized or invalid marker never produce
/// a completion field.
///
/// Multi-line drafts complete the physical line the cursor is on: only the
/// first (parent) line offers a leading marker, and a later line's source
/// indentation plus bullet marker itself is never completable, matching the
/// authored-bullet grammar `bob capture` and `bob capture-parse` execute
/// with.
pub(crate) fn completion_field_at(
    raw_text: &str,
    cursor: usize,
) -> Option<CompletionField> {
    // Work Log lines are literal text: no marker completion at all,
    // including the global `@@` path below. Note and heading wikilink
    // completion keeps working through the separate wikilink path.
    if cursor_in_close_log_text(raw_text, cursor) {
        return None;
    }
    if let Some(line) = split_physical_lines(raw_text)
        .into_iter()
        .find(|line| cursor >= line.start && cursor <= line.end)
    {
        let tokens = tokenize_line_with_spans(&line);
        if let Some(token) = tokens.iter().find(|token| {
            token.text.starts_with("@@")
                && cursor >= token.start
                && cursor <= token.end
        }) {
            return global_completion_field_at(token, cursor);
        }
    }

    // A first-level bullet's leading `#` names a drawer. It either opens
    // the drawer menu or is an empty success; it never reaches the
    // task-type or marker claims below.
    match drawer_bullet_probe(raw_text, cursor) {
        Some(Some(bullet)) => return Some(bullet.into_field()),
        Some(None) => return None,
        None => {}
    }

    let draft = split_capture_draft(raw_text);
    let items = draft.items;
    let header_item;
    let (item, line_index, line) = if let Some(found) =
        items.iter().find_map(|item| {
            item.lines
                .iter()
                .enumerate()
                .find(|(_, line)| {
                    cursor >= line.raw.start && cursor <= line.raw.end
                })
                .map(|(line_index, line)| (item, line_index, line.raw))
        }) {
        found
    } else {
        // A list header with bullets below belongs to no per-bullet item.
        // Complete it as a header-only leading line, matching an empty list.
        let (line_number, header_raw) = split_physical_lines(raw_text)
            .into_iter()
            .enumerate()
            .find(|(_, line)| cursor >= line.start && cursor <= line.end)?;
        detect_list_header(&header_raw)?;
        header_item = CaptureItem {
            source: raw_text,
            index: 0,
            start: header_raw.start,
            end: header_raw.end,
            line_start: line_number + 1,
            line_end: line_number + 1,
            lines: vec![ItemLine {
                raw: header_raw,
                line_number: line_number + 1,
            }],
            list: None,
        };
        (&header_item, 0, header_raw)
    };
    if let Some(field) = directive_completion_field_at(item, cursor) {
        return Some(field);
    }
    // Claim the whole past token before generic `#`/`@` completion can
    // mistake a name or duration for a task marker. Child logs stay literal.
    // The segment suffix owns its bytes; the name field keeps ending at `~`.
    if line_index == 0 && line.text.trim_start().starts_with("x=") {
        if let Some(field) = pomodoro_past_segment_field_at(item, cursor) {
            return Some(field);
        }
        return pomodoro_past_name_field(item, cursor);
    }
    // A `=/` split base or `#type` completes before generic task-type
    // masking can consume the split `#`, and before the session-action
    // early return below. This keeps split `#` selectors from becoming
    // task-type markers.
    if line_index == 0
        && let Some(field) = pomodoro_split_field_at(item, cursor)
    {
        return Some(field);
    }
    if line_index == 0
        && let Some(field) = pomodoro_merge_field_at(item, cursor)
    {
        return Some(field);
    }
    // A named start owns its `#` delimiter before generic task-type
    // completion can claim it. The full name replacement lets candidate
    // formatting preserve `#` only when the selected name needs it.
    if line_index == 0
        && let Some(field) = pomodoro_start_name_field(item, cursor)
    {
        return Some(field);
    }
    // Type markers and directives are transparent to route-run detection.
    // Their specialized completion claims have already run, so mask both
    // on a same-length view before selecting a run. Returned ranges still
    // index the original draft.
    let masked_source;
    let masked_item;
    let (item, line) = {
        if let Some(marker) = super::type_marker::completion_at(item, cursor) {
            return Some(CompletionField {
                context: CompletionContext::TaskType,
                route: None,
                block_id: None,
                query: marker.query,
                replacement: (marker.replacement.start, marker.replacement.end),
                bare_name: false,
            });
        }
        let type_occurrences = super::type_marker::collect_item(item);
        let directive_occurrences = super::directive::collect_item(item);
        if type_occurrences.is_empty() && directive_occurrences.is_empty() {
            (item, line)
        } else {
            let mut bytes = if type_occurrences.is_empty() {
                item.source.as_bytes().to_vec()
            } else {
                super::type_marker::masked_source(item, &type_occurrences)
                    .into_bytes()
            };
            super::directive::mask_bytes(&mut bytes, &directive_occurrences);
            masked_source = String::from_utf8(bytes)
                .expect("completion masking preserves UTF-8");
            masked_item = super::type_marker::masked_item(item, &masked_source);
            let masked_line = masked_item.lines[line_index].raw;
            (&masked_item, masked_line)
        }
    };
    // Merge-in sources borrow the ordinary marker and parent-task fields,
    // but the arrow owns its right side before generic target-run parsing.
    if line_index == 0
        && let Some(field) = merge_in_completion_field_at(item, line, cursor)
    {
        return field;
    }
    // Bare-plus task search is checked before the adjustment early return:
    // a literal `+` is both the existing +5m action and a task-picker
    // gesture, while numeric/double-plus operator tokens remain actions.
    if let Some(field) =
        parent_task_completion_field_at(item, line_index, cursor)
    {
        return Some(field);
    }
    // A whole-item `+[N]`/`-[N]` adjustment, `++[N]`/`--[N]` shift,
    // `x` close, or `=`/`=<X>` start is an action, never a routed
    // capture: it requests no route or task completion candidates. The
    // `=`-family parser covers both closes and starts.
    if parse_editor_adjust_item(item).is_some()
        || parse_editor_close_item(item).is_some()
    {
        return None;
    }
    // A first-level move bullet in a project-note creation item owns the
    // sigil before the trailing project-task-ID path can claim it. The
    // latter still handles task bullets and nested lines unchanged.
    if line_index > 0
        && let Some(field) = project_task_move_completion_field_at(
            item, line_index, line, cursor,
        )
    {
        return Some(field);
    }
    // A trailing task ID on a task-list bullet or project-note task bullet
    // completes the ID itself; a cursor elsewhere falls through to the `@`
    // marker path below.
    if (line_index > 0 || item.list.is_some())
        && let Some(field) =
            project_task_completion_field(item, line_index, line, cursor)
    {
        return Some(field);
    }
    let leading = line_index == 0;

    // A claimed `>` transfer line owns its whole parent line: the normal
    // target-run selection would treat the sources as a leading run and
    // leave every destination with no completion. Map the cursor to the
    // transfer component it sits in instead; a claimed line with the
    // cursor on the arrow or in a gap completes nothing.
    if leading
        && let Some(field) = transfer_completion_field_at(
            item.source,
            line,
            item.lines.len() > 1,
            cursor,
        )
    {
        return field;
    }

    // A whole-draft `/` token completes Go To searches instead of
    // offering `@` route completion. This runs before the `!`, `:`, and
    // `^` paths; all four only fire on the item's leading line and their
    // claims are disjoint.
    if leading && let Some(field) = task_goto_completion_field_at(item, cursor)
    {
        return Some(field);
    }

    // A solo leading `!` token completes vault-wide completable
    // tasks instead of offering `@` route completion. This runs before
    // the `:` and `^` paths; all three only fire on the item's leading
    // line and their claims are disjoint.
    if leading
        && let Some(field) = task_complete_completion_field_at(item, cursor)
    {
        return Some(field);
    }

    // A solo leading `:` token completes vault-wide linkable tasks
    // instead of offering `@` route completion: the query runs from just
    // after the sigil to the cursor and the replacement covers the whole
    // token including the sigil, so accepting rewrites the query into the
    // canonical link. This runs before the `^` path; both only fire on the
    // item's leading line.
    if leading && let Some(field) = task_link_completion_field_at(item, cursor)
    {
        return Some(field);
    }

    // A solo leading `^` token completes active tasks vault-wide instead of
    // offering `@` route completion: the cursor inside the `route:block-id`
    // part (including an empty part) requests the `active_task` context, a
    // cursor after `#` requests Pomodoro names, and a cursor inside `=<X>`
    // or `=x` requests nothing so a typed suffix survives an accept.
    if leading
        && let Some(field) = caret_completion_field_at(item, line, cursor)
    {
        return Some(field);
    }

    let scan_line = if leading {
        line
    } else {
        let AuthoredLineClass::Item(authored) = classify_authored_line(line)
        else {
            return None;
        };
        if authored.depth == AuthoredDepth::Nested
            && !has_previous_first_level_authored_item(&item.lines, line_index)
        {
            return None;
        }
        if cursor < authored.body_start {
            return None;
        }
        RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.end,
        }
    };

    // A cursor inside an `&note:block-id` modifier (complete or still
    // typed) completes vault-wide prerequisite tasks instead of offering
    // `@` route completion. This runs before marker completion so the
    // modifier's own range wins over any neighboring marker.
    if let Some(field) =
        dependency_completion_field_at(&scan_line, cursor, leading)
    {
        return Some(field);
    }
    // A cursor inside a `*note:block-id` modifier completes a blocked task
    // using the same owner context as `&` prerequisites.
    if let Some(field) =
        blocked_task_completion_field_at(&scan_line, cursor, leading)
    {
        return Some(field);
    }
    // Relation labels win over relation targets when the cursor is past `|`;
    // otherwise the target picker owns the sigil and task locator.
    if let Some(field) =
        relation_label_completion_field_at(&scan_line, cursor, leading)
    {
        return Some(field);
    }
    if let Some(field) =
        relation_target_completion_field_at(&scan_line, cursor, leading)
    {
        return Some(field);
    }
    let mut tokens = tokenize_line_with_spans(&scan_line);
    take_global_declarations(&mut tokens);
    // Leading/trailing dependency and blocked-task modifiers are not marker
    // candidates: strip them with the shared extractor (before terminal
    // markers, exactly like the editor parse) so an interleaved marker still
    // completes.
    let (stripped, _) = extract_line_dependencies(
        tokens,
        scan_line.text,
        scan_line.start,
        leading,
    );
    tokens = stripped;

    // Multi-target selection uses the same run-aware terminal extraction
    // as execution and the editor. Keep the historical singleton stream
    // below unless this selection actually covers a run or the cursor is
    // on the token extending its inner edge.
    let mut run_tokens = tokens.clone();
    super::target_runs::extract_terminal_markers_crossing_runs(&mut run_tokens);
    if let Some(index) =
        completion_target_run_marker_index(&run_tokens, leading, cursor)
    {
        let token = run_tokens[index];
        if cursor >= token.start && cursor <= token.end {
            return marker_field_at_cursor(&token, cursor);
        }
    }

    extract_terminal_markers(&mut tokens);

    let index = completion_marker_index(&tokens, leading, cursor)?;
    let token = tokens[index];
    if cursor < token.start || cursor > token.end {
        return None;
    }

    marker_field_at_cursor(&token, cursor)
}

/// Selects only a multi-member target run or its one inner-edge extension.
/// A selected singleton continues through the legacy completion scanner so
/// adding run-aware terminal extraction cannot change its behavior.
fn completion_target_run_marker_index(
    tokens: &[Token<'_>],
    leading: bool,
    cursor: usize,
) -> Option<usize> {
    let run = super::target_runs::select_target_run(tokens, leading, true)?;
    let marker_index = completion_marker_index(tokens, leading, cursor)?;
    if run.members.len() >= 2
        || run
            .members
            .first()
            .is_some_and(|member| member.token_index != marker_index)
    {
        Some(marker_index)
    } else {
        None
    }
}

/// Build the vault-wide parent-task field for the shared terminal-plus
/// classifier. Its replacement owns the complete `+query` token so a
/// candidate can replace the gesture with `@route+block-id`.
fn parent_task_completion_field_at(
    item: &CaptureItem<'_>,
    line_index: usize,
    cursor: usize,
) -> Option<CompletionField> {
    let selector =
        parent_task_selector_tokens(item)
            .into_iter()
            .find(|selector| {
                selector.line_index == line_index
                    && cursor >= selector.token.start
                    && cursor <= selector.token.end
            })?;
    let relative = cursor - selector.token.start;
    let query = if relative <= 1 {
        String::new()
    } else {
        selector.token.text.get(1..relative)?.to_string()
    };
    Some(CompletionField {
        context: CompletionContext::TaskParent,
        route: None,
        block_id: None,
        query,
        replacement: (selector.token.start, selector.token.end),
        bare_name: false,
    })
}

/// Trailing ` :id` / ` ^id` completion on a task bullet, or on one of its
/// nested lines. Project-note task bullets use the same contract under
/// `project_note_parity`.
fn project_task_completion_field(
    item: &CaptureItem<'_>,
    line_index: usize,
    line: RawLine<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let info = project_task_token_at(item, line_index, line, cursor)?;
    let query = info
        .token
        .text
        .get(1..cursor - info.token.start)?
        .to_string();
    Some(CompletionField {
        context: CompletionContext::ProjectTaskBlockId,
        route: Some(info.stem),
        block_id: None,
        query,
        replacement: (info.token.start + 1, info.token.end),
        bare_name: false,
    })
}

/// Completion context for a first-level move bullet in a project-note
/// creation item. The replacement owns only the ID; the sigil and bullet
/// remain authored by the draft.
fn project_task_move_completion_field_at(
    item: &CaptureItem<'_>,
    line_index: usize,
    line: RawLine<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    if line_index == 0 {
        return None;
    }
    let parsed = parse_editor_item(item).item;
    if !has_sealed_project_note_marker(&parsed) {
        return None;
    }
    let AuthoredLineClass::Item(authored) = classify_authored_line(line) else {
        return None;
    };
    if authored.depth != AuthoredDepth::First {
        return None;
    }
    let hit = super::move_bullet::scan_move_bullet(
        authored.body,
        authored.body_start,
        item.source,
    )?;
    let (start, end, id_start, id_end) = match hit {
        super::move_bullet::MoveBulletHit::Complete {
            id_span, span, ..
        } => (span.start, span.end, id_span.0, id_span.1),
        super::move_bullet::MoveBulletHit::Unfinished { span, .. } => {
            (span.start, span.end, span.end, span.end)
        }
        super::move_bullet::MoveBulletHit::Choice { .. }
        | super::move_bullet::MoveBulletHit::Invalid { .. } => return None,
    };
    if cursor < start || cursor > end {
        return None;
    }
    let query_end = cursor.clamp(id_start, id_end);
    let query = item.source.get(id_start..query_end)?.to_string();
    Some(CompletionField {
        context: CompletionContext::ProjectTaskMove,
        route: parsed.route,
        block_id: None,
        query,
        replacement: (id_start, id_end),
        bare_name: false,
    })
}

pub(crate) fn has_sealed_project_note_marker(item: &EditorItemParse) -> bool {
    item.spans.iter().any(|span| {
        (span.kind == SpanKind::ProjectNoteMarker
            && span.project_note == Some(ProjectNoteForm::Create))
            || (span.kind == SpanKind::ProjectNoteOfferMarker
                && span.project_note == Some(ProjectNoteForm::Offer))
    })
}

/// Draft details shared by the completion response and its picker
/// descriptor. Ranges are UTF-8 byte offsets into the original draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTaskMoveDetail {
    pub(crate) parent_route: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) destination: Option<String>,
    pub(crate) pomodoro_name: Option<String>,
    pub(crate) sigil: char,
    pub(crate) marker_range: (usize, usize),
    pub(crate) continuation: String,
    pub(crate) item_index: usize,
    pub(crate) current_line: usize,
    pub(crate) project_start: Option<String>,
    pub(crate) existing_moves: Vec<ProjectTaskMoveOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTaskMoveOccurrence {
    pub(crate) block_id: String,
    pub(crate) line: usize,
    pub(crate) item_index: usize,
}

/// Resolve the project-note context behind a previously detected
/// `project_task_move` field.
pub(crate) fn project_task_move_detail(
    raw_text: &str,
    cursor: usize,
    replacement: (usize, usize),
) -> Option<ProjectTaskMoveDetail> {
    let draft = split_capture_draft(raw_text);
    let (item, line_index, line) = draft.items.iter().find_map(|item| {
        item.lines
            .iter()
            .enumerate()
            .find(|(_, line)| {
                cursor >= line.raw.start && cursor <= line.raw.end
            })
            .map(|(line_index, line)| (item, line_index, line.raw))
    })?;
    let field =
        project_task_move_completion_field_at(item, line_index, line, cursor)?;
    if field.replacement != replacement {
        return None;
    }
    let parsed = parse_editor_item(item).item;
    if !has_sealed_project_note_marker(&parsed) {
        return None;
    }
    let AuthoredLineClass::Item(authored) = classify_authored_line(line) else {
        return None;
    };
    let hit = super::move_bullet::scan_move_bullet(
        authored.body,
        authored.body_start,
        raw_text,
    )?;
    let (sigil, marker_range) = match hit {
        super::move_bullet::MoveBulletHit::Complete { sigil, span, .. }
        | super::move_bullet::MoveBulletHit::Unfinished { sigil, span } => {
            (sigil, (span.start, span.end))
        }
        super::move_bullet::MoveBulletHit::Choice { .. }
        | super::move_bullet::MoveBulletHit::Invalid { .. } => return None,
    };
    let prefix_end = authored.body_start.checked_sub(line.start)?;
    let prefix = line.text.get(..prefix_end)?;
    let continuation = format!("\n{prefix}{sigil}");
    let parent_route = parsed.route.clone();
    let parent = parent_route
        .as_deref()
        .map(crate::native::capture::route_label);
    let destination = parent_route
        .as_deref()
        .zip(parsed.block_id.as_deref())
        .map(|(route, id)| {
            crate::native::capture_project_note::project_note_basename(
                route, id,
            )
        });
    let project_start = parsed.spans.iter().find_map(|span| {
        if span.kind != SpanKind::Schedule {
            return None;
        }
        let token = raw_text.get(span.start..span.end)?;
        let offset = token.strip_prefix("s:")?.parse::<i64>().ok()?;
        let today = crate::native::env::current_datetime().date();
        let date = today.checked_add_signed(chrono::Duration::days(offset))?;
        if date <= today {
            return None;
        }
        Some(date.format("%b %-d").to_string())
    });

    let mut existing_moves = Vec::new();
    if let Some(route) = parent_route.as_deref() {
        for candidate_item in &draft.items {
            let candidate = parse_editor_item(candidate_item).item;
            if candidate.route.as_deref() != Some(route)
                || !has_sealed_project_note_marker(&candidate)
            {
                continue;
            }
            for child in candidate_item.lines.iter().skip(1) {
                let AuthoredLineClass::Item(authored) =
                    classify_authored_line(child.raw)
                else {
                    continue;
                };
                if authored.depth != AuthoredDepth::First {
                    continue;
                }
                let Some(super::move_bullet::MoveBulletHit::Complete {
                    id,
                    ..
                }) = super::move_bullet::scan_move_bullet(
                    authored.body,
                    authored.body_start,
                    raw_text,
                )
                else {
                    continue;
                };
                existing_moves.push(ProjectTaskMoveOccurrence {
                    block_id: id,
                    line: child.line_number,
                    item_index: candidate_item.index,
                });
            }
        }
    }
    Some(ProjectTaskMoveDetail {
        parent_route,
        parent,
        destination,
        pomodoro_name: parsed.section,
        sigil,
        marker_range,
        continuation,
        item_index: item.index,
        current_line: item.lines.get(line_index)?.line_number,
        project_start,
        existing_moves,
    })
}

/// One trailing task-ID token plus the owning task information needed by
/// the picker contract.
struct ProjectTaskToken<'a> {
    token: Token<'a>,
    stem: String,
    owner_index: usize,
    owner_line: usize,
    owner_body: String,
    owner_id: Option<String>,
    owner_id_range: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectTaskIdToken {
    start: usize,
    end: usize,
    id: Option<String>,
}

/// Locate the trailing task-ID token at `cursor` on a task-list bullet or
/// nested line, or on a project-note task bullet and its nested lines. An
/// item left incomplete by a lone sigil still qualifies.
fn project_task_token_at<'a>(
    item: &CaptureItem<'a>,
    line_index: usize,
    line: RawLine<'a>,
    cursor: usize,
) -> Option<ProjectTaskToken<'a>> {
    let resolved = parse_editor_item(item).item;
    let is_list = item
        .list
        .as_ref()
        .is_some_and(|list| list.kind == ListKind::Tasks && !list.ownerless);
    let (stem, owner_index) = if is_list {
        (item.list.as_ref()?.route.clone(), 0)
    } else {
        let is_project_note = matches!(
            resolved.mode,
            EditorMode::ProjectNote | EditorMode::PomodoroProjectNote
        ) || (resolved.mode == EditorMode::Incomplete
            && resolved.needs == [Need::BlockId]);
        if !is_project_note || line_index == 0 {
            return None;
        }
        let (Some(route), Some(project_id)) =
            (resolved.route.as_ref(), resolved.block_id.as_ref())
        else {
            return None;
        };
        let owner = project_task_owner_index(item, line_index)?;
        (format!("{}_{}", route, project_id.replace('-', "_")), owner)
    };
    let line_view = task_bullet_line(item, line_index, line)?;
    if cursor < line_view.start {
        return None;
    }
    let child_tokens = tokenize_line_with_spans(&line_view);
    let child_parse = parse_editor_line(
        child_tokens,
        line_view.text,
        line_view.start,
        false,
        false,
    );
    let token = child_parse.body_tokens.last()?;
    // The lexer decides what counts as a trailing task-ID token; prose
    // (`:)`, `10:30`) yields nothing.
    lex_project_task_id(token.text)?;
    if cursor < token.start + 1 || cursor > token.end {
        return None;
    }
    let owner_line = item.lines.get(owner_index)?;
    let owner_view = task_bullet_line(item, owner_index, owner_line.raw)?;
    let (owner_body, owner_ids) = clean_task_bullet_line(owner_view);
    let owner_id_token = owner_ids.iter().find(|id| {
        id.id.is_some()
            && !(owner_index == line_index
                && id.start == token.start
                && id.end == token.end)
    });
    let owner_id = owner_id_token
        .and_then(|id| id.id.clone())
        .or_else(|| owner_ids.iter().find_map(|id| id.id.clone()));
    let owner_id_range = owner_id_token.map(|id| (id.start, id.end));
    Some(ProjectTaskToken {
        token: *token,
        stem,
        owner_index,
        owner_line: owner_line.line_number,
        owner_body,
        owner_id,
        owner_id_range,
    })
}

fn project_task_owner_index(
    item: &CaptureItem<'_>,
    line_index: usize,
) -> Option<usize> {
    let mut owner = None;
    for (index, line) in
        item.lines.iter().enumerate().take(line_index + 1).skip(1)
    {
        match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => return None,
            AuthoredLineClass::Item(authored) => match authored.depth {
                AuthoredDepth::First => owner = Some(index),
                AuthoredDepth::Nested if owner.is_some() => {}
                _ => return None,
            },
        }
    }
    owner
}

/// Return the marker-free authored body of a task bullet. List owner lines
/// have already lost their bullet prefix in the zero-copy split; project
/// note lines still carry their Markdown bullet.
fn task_bullet_line<'a>(
    item: &CaptureItem<'a>,
    line_index: usize,
    raw: RawLine<'a>,
) -> Option<RawLine<'a>> {
    if item.list.is_some() && line_index == 0 {
        return Some(raw);
    }
    let AuthoredLineClass::Item(authored) = classify_authored_line(raw) else {
        return None;
    };
    matches!(authored.depth, AuthoredDepth::First | AuthoredDepth::Nested)
        .then_some(RawLine {
            text: authored.body,
            start: authored.body_start,
            end: raw.end,
        })
}

/// Clean task text and all accepted task-ID tokens from one authored line.
/// Removing IDs before parsing lets the picker replace an existing owner ID
/// without leaking it into the suggested body.
fn clean_task_bullet_line<'a>(
    line: RawLine<'a>,
) -> (String, Vec<ProjectTaskIdToken>) {
    let original_tokens = tokenize_line_with_spans(&line);
    let (mut original_tokens, _) = extract_line_dependencies(
        original_tokens,
        line.text,
        line.start,
        false,
    );
    let mut ids = Vec::new();
    while let Some(id) = strip_trailing_task_id(&mut original_tokens) {
        let id_value = match id.lex {
            TaskIdLex::Valid { id, .. } => Some(id),
            TaskIdLex::Lone { .. } | TaskIdLex::Invalid { .. } => None,
        };
        ids.push(ProjectTaskIdToken {
            start: id.token.start,
            end: id.token.end,
            id: id_value,
        });
    }
    let masked = super::type_marker::masked_line(&line);
    let masked_line = RawLine {
        text: &masked,
        start: line.start,
        end: line.end,
    };
    let tokens = tokenize_line_with_spans(&masked_line);
    let (mut remaining, _) = extract_line_dependencies(
        tokens,
        masked_line.text,
        masked_line.start,
        false,
    );
    remaining.retain(|token| {
        !ids.iter()
            .any(|id| id.start == token.start && id.end == token.end)
    });
    let parse = parse_editor_line(
        remaining,
        masked_line.text,
        masked_line.start,
        false,
        false,
    );
    (clean_project_task_body(&parse.body), ids)
}

/// One already-typed project task ID elsewhere in the same item, for the
/// `project_task_block_id` used list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTaskUsedId {
    pub(crate) id: String,
    pub(crate) line: usize,
    pub(crate) text: String,
    pub(crate) task: bool,
    pub(crate) status_symbol: Option<char>,
    pub(crate) status_name: Option<String>,
}

/// Detail behind the `project_task_block_id` object: destination, owner,
/// active marker, splice, clean task body, and IDs already in scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTaskBlockIdDetail {
    pub(crate) stem: String,
    pub(crate) is_list: bool,
    pub(crate) marker: char,
    pub(crate) marker_range: (usize, usize),
    pub(crate) body: String,
    pub(crate) owner_line: usize,
    pub(crate) owner_id: Option<String>,
    pub(crate) splice: ProjectTaskSplice,
    pub(crate) used: Vec<ProjectTaskUsedId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTaskSplice {
    pub(crate) range: (usize, usize),
    pub(crate) before: String,
    pub(crate) after: String,
}

/// Collect the `project_task_block_id` block-ID detail at `cursor`.
/// `replacement` is the ID span the detection step reported; a caller
/// passing a foreign range gets nothing rather than a mismatched object.
pub(crate) fn project_task_block_id_detail(
    raw_text: &str,
    cursor: usize,
    replacement: (usize, usize),
) -> Option<ProjectTaskBlockIdDetail> {
    let draft = split_capture_draft(raw_text);
    let (item, line_index, line) = draft.items.iter().find_map(|item| {
        item.lines
            .iter()
            .enumerate()
            .find(|(_, line)| {
                cursor >= line.raw.start && cursor <= line.raw.end
            })
            .map(|(line_index, line)| (item, line_index, line.raw))
    })?;
    let info = project_task_token_at(item, line_index, line, cursor)?;
    if replacement != (info.token.start + 1, info.token.end) {
        return None;
    }
    let marker = info.token.text.chars().next()?;
    if marker != ':' && marker != '^' {
        return None;
    }
    let list = item.list.as_ref();
    let mut used = Vec::new();
    if let Some(list) = list {
        let mut seen = HashSet::new();
        for sibling in &draft.items {
            let Some(sibling_list) = sibling.list.as_ref() else {
                continue;
            };
            if sibling_list.ownerless
                || sibling_list.header_start != list.header_start
                || sibling_list.position == list.position
            {
                continue;
            }
            let parsed = parse_editor_item(sibling).item;
            let Some(id) = parsed.block_id else {
                continue;
            };
            if seen.insert(id.clone()) {
                used.push(ProjectTaskUsedId {
                    id,
                    line: sibling.line_start,
                    text: clean_project_task_body(&parsed.body),
                    task: true,
                    status_symbol: None,
                    status_name: None,
                });
            }
        }
    } else {
        let resolved = parse_editor_item(item).item;
        let parent_line_number =
            item.lines.first().map(|line| line.line_number).unwrap_or(1);
        used.push(ProjectTaskUsedId {
            id: "prj".to_string(),
            line: parent_line_number,
            text: clean_project_task_body(&resolved.body),
            task: true,
            status_symbol: None,
            status_name: None,
        });
        // Run the shared per-item pass in source order so duplicate
        // accounting matches the editor spans. The owner's own ID is
        // excluded whether the cursor is on the task or a nested line.
        let mut pass = ProjectTaskPass::new();
        for (index, child) in item.lines.iter().enumerate().skip(1) {
            let AuthoredLineClass::Item(authored) =
                classify_authored_line(child.raw)
            else {
                continue;
            };
            let Some(child_line) = task_bullet_line(item, index, child.raw)
            else {
                continue;
            };
            let child_tokens = tokenize_line_with_spans(&child_line);
            let child_parse = parse_editor_line(
                child_tokens,
                child_line.text,
                child_line.start,
                false,
                false,
            );
            let last_id = child_parse.body_tokens.last().and_then(|token| {
                lex_project_task_id(token.text)
                    .map(|lex| StrippedTaskId { token: *token, lex })
            });
            if let ChildTaskOutcome::Accept { id, .. } = pass.check_child(
                last_id,
                &child_parse.body,
                authored.depth,
                child.line_number,
            ) && index != info.owner_index
            {
                let (text, _) = clean_task_bullet_line(child_line);
                used.push(ProjectTaskUsedId {
                    id,
                    line: child.line_number,
                    text,
                    task: true,
                    status_symbol: None,
                    status_name: None,
                });
            }
        }
    }
    let splice =
        project_task_splice(raw_text, item, line_index, &info, marker)?;
    Some(ProjectTaskBlockIdDetail {
        stem: info.stem,
        is_list: list.is_some(),
        marker,
        marker_range: (info.token.start, info.token.end),
        body: info.owner_body,
        owner_line: info.owner_line,
        owner_id: info.owner_id,
        splice,
        used,
    })
}

fn project_task_splice(
    raw_text: &str,
    item: &CaptureItem<'_>,
    line_index: usize,
    info: &ProjectTaskToken<'_>,
    marker: char,
) -> Option<ProjectTaskSplice> {
    let id_start = info.token.start + 1;
    if let Some((owner_id_start, owner_id_end)) = info.owner_id_range {
        let between = raw_text.get(owner_id_end..info.token.start)?;
        let after = between.strip_suffix(' ').unwrap_or(between).to_string();
        return Some(ProjectTaskSplice {
            range: (owner_id_start, info.token.end),
            before: marker.to_string(),
            after,
        });
    }
    if line_index == info.owner_index {
        return Some(ProjectTaskSplice {
            range: (id_start, info.token.end),
            before: String::new(),
            after: String::new(),
        });
    }
    let owner_end = item.lines.get(info.owner_index)?.raw.end;
    let between = raw_text.get(owner_end..info.token.start)?;
    let after = between.strip_suffix(' ').unwrap_or(between).to_string();
    Some(ProjectTaskSplice {
        range: (owner_end, info.token.end),
        before: format!(" {marker}"),
        after,
    })
}

/// Bullet body without a leading checkbox, for suggestion input and used
/// text. Mirrors the renderer's checkbox shape through the shared helper.
fn clean_project_task_body(stripped: &str) -> String {
    let (_, rest) = split_leading_checkbox(stripped.trim_start());
    rest.trim().to_string()
}

pub(super) fn has_previous_first_level_authored_item(
    lines: &[ItemLine<'_>],
    current_line_index: usize,
) -> bool {
    lines[1..current_line_index].iter().any(|line| {
        let AuthoredLineClass::Item(authored) =
            classify_authored_line(line.raw)
        else {
            return false;
        };
        if authored.depth != AuthoredDepth::First {
            return false;
        }
        let normalized = normalize_task_text(authored.body);
        if normalized.is_empty() {
            return false;
        }
        let tokens = tokenize_with_spans(&normalized);
        !parse_editor_line(tokens, &normalized, 0, false, false)
            .body
            .is_empty()
    })
}

/// Mirror [`select_marker_token`]'s leading-then-trailing precedence, but
/// without its requires-body/single-token exclusion: a lone leading
/// `@route` fragment with no body text yet is still the token a user is
/// actively completing, even though `bob capture` would leave it literal.
/// `leading` is only ever set for the parent (first) physical line.
///
/// Target runs generalize the singleton choice: any member of the run phase
/// `grammar` selects completes, plus the `@`-token that extends a run at its
/// inner edge (the token right after a leading run, or right before a
/// trailing run). Per-token field parsing already uses the token's own route
/// and block ID, so the run only decides *which* token the cursor owns.
pub(super) fn completion_marker_index(
    tokens: &[Token<'_>],
    leading: bool,
    cursor: usize,
) -> Option<usize> {
    if let Some(run) =
        super::target_runs::select_target_run(tokens, leading, true)
    {
        for member in &run.members {
            if cursor >= member.start && cursor <= member.end {
                return Some(member.token_index);
            }
        }
        // An `@`-shaped neighbor at the run's inner edge extends the run
        // once completed: after a leading run, or before a trailing run.
        // Whole-line runs cover every token, so neither edge applies there
        // unless the run is a single member with room on one side (handled
        // by the position checks below).
        if matches!(
            run.position,
            super::target_runs::TargetRunPosition::Leading
        ) && let Some(last) = run.members.last()
        {
            let next = last.token_index + 1;
            if next < tokens.len()
                && let Some(token) = tokens.get(next)
                && token.text.starts_with('@')
                && cursor >= token.start
                && cursor <= token.end
            {
                return Some(next);
            }
        }
        if matches!(
            run.position,
            super::target_runs::TargetRunPosition::Trailing
        ) && let Some(first) = run.members.first()
            && first.token_index > 0
        {
            let prev = first.token_index - 1;
            if let Some(token) = tokens.get(prev)
                && token.text.starts_with('@')
                && cursor >= token.start
                && cursor <= token.end
            {
                return Some(prev);
            }
        }
        // A run owns this line: body text and middle tokens stay literal.
        return None;
    }
    if leading
        && let Some(first) = tokens.first()
        && classify_editor_token(first).is_some()
    {
        return Some(0);
    }

    if tokens.len() >= 2 {
        let last = tokens.len() - 1;
        if classify_editor_token(&tokens[last]).is_some() {
            return Some(last);
        }
    }

    None
}

/// Split one `@`-token the same way [`classify_route_token`],
/// [`classify_pomodoro_token`], and [`classify_sub_bullet_token`] do, then
/// resolve which component -- route or right-hand -- `cursor` sits in.
pub(super) fn marker_field_at_cursor(
    token: &Token<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let text = token.text;

    if is_sub_bullet_marker_candidate(text) {
        if let Some(prefix) = exact_explicit_toggle_prefix(text) {
            let bang_start = token.start + prefix.len();
            if cursor > bang_start {
                return None;
            }
        }
        let text = exact_explicit_toggle_prefix(text).unwrap_or(text);
        let marker = &text[1..];
        let (route_part, rest) =
            marker.split_once('+').expect("sub-bullet candidate");
        let (block_part, third) = match rest.split_once('#') {
            Some((block, section)) => (
                block,
                Some(CompletionThird {
                    separator_len: 1,
                    part: section,
                    context: CompletionContext::Drawer,
                }),
            ),
            None => (rest, None),
        };
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 1,
                route_part,
                separator_len: 1,
                right_part: block_part,
                right_context: Some(CompletionContext::Task),
                third,
            },
            cursor,
        );
    }

    if is_task_block_id_marker_candidate(text) {
        let marker = &text[1..];
        let (route_part, block_part) =
            marker.split_once('^').expect("task block-ID candidate");
        // A `#name` after `@route^id@` completes the Pomodoro name that
        // picks the ` :` links' Pomodoro; the replacement is just the name.
        let (block_part, third) = match block_part.split_once('#') {
            Some((block, name)) => (
                block,
                Some(CompletionThird {
                    separator_len: 1,
                    part: name,
                    context: CompletionContext::PomodoroName,
                }),
            ),
            None => (block_part, None),
        };
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 1,
                route_part,
                separator_len: 1,
                right_part: block_part,
                right_context: Some(CompletionContext::TaskBlockId),
                third,
            },
            cursor,
        );
    }

    if is_retired_double_colon_marker_candidate(text) {
        return None;
    }

    if is_pomodoro_marker_candidate(text)
        || is_incomplete_pomodoro_marker_candidate(text)
    {
        let legacy = text.starts_with("@!");
        let sigil_len = if legacy { 2 } else { 1 };
        let marker = &text[sigil_len..];
        let (route_part, rest, separator) = match marker.split_once(':') {
            Some((route, rest)) => (route, rest, true),
            None => (marker, "", false),
        };
        if let Some(field) =
            pomodoro_link_suffix_name_field(token, route_part, rest, cursor)
        {
            return Some(field);
        }
        // The additive `=<X>` start or `=x` close suffix is never a
        // completable component: strip it before splitting `#` so the
        // block and name parts — and their replacement ranges — end
        // before `=`. A cursor inside the suffix offers no completion;
        // accepting a name candidate must leave the typed suffix in place.
        let (rest, start_len) = match rest.split_once('=') {
            Some((before, suffix)) => (before, Some(1 + suffix.len())),
            None => (rest, None),
        };
        if let Some(start_len) = start_len
            && cursor > token.end - start_len
        {
            return None;
        }
        let (block_part, third) = match rest.split_once('#') {
            Some((block, name)) => (
                block,
                Some(CompletionThird {
                    separator_len: 1,
                    part: name,
                    context: CompletionContext::PomodoroName,
                }),
            ),
            None => (rest, None),
        };
        // The retired `:` project-note form (`@route:id+`, with or without
        // a session suffix) offers no completion.
        if block_part
            .strip_suffix('+')
            .is_some_and(|core| !core.is_empty())
        {
            return None;
        }
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len,
                route_part,
                separator_len: usize::from(separator),
                right_part: block_part,
                right_context: Some(CompletionContext::PomodoroBlockId),
                third,
            },
            cursor,
        );
    }

    let rest = text.strip_prefix('@')?;
    if let Some((route_part, prefix)) = rest.split_once('#') {
        // `#section^id`: a `^` after the section opens the block-ID
        // picker for a named note in that section. The section stays
        // outside the replacement so accepting an ID keeps it.
        if let Some((sec, block)) = prefix.split_once('^') {
            let sec_end = token.start + 1 + route_part.len() + 1 + sec.len();
            let caret = sec_end + 1;
            if cursor >= caret {
                if !is_route_token(route_part) {
                    return None;
                }
                let block_start = caret;
                let block_end = block_start + block.len();
                if cursor < block_start || cursor > block_end {
                    return None;
                }
                let split = cursor - block_start;
                return Some(CompletionField {
                    context: CompletionContext::TaskBlockId,
                    route: Some(route_part.to_ascii_lowercase()),
                    block_id: None,
                    query: block[..split].to_string(),
                    replacement: (block_start, block_end),
                    bare_name: false,
                });
            }
            // Cursor inside the section: fall through to Section below,
            // but with the `^id` suffix excluded from the range. The
            // caller (`completion_field_from_parts`) would otherwise
            // own the suffix; trim it here by narrowing `prefix` to the
            // section part. Since `right_end` derives from `prefix.len()`,
            // pass only the section text.
            return completion_field_from_parts(
                token,
                CompletionParts {
                    sigil_len: 1,
                    route_part,
                    separator_len: 1,
                    right_part: sec,
                    right_context: Some(CompletionContext::Section),
                    third: None,
                },
                cursor,
            );
        }
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 1,
                route_part,
                separator_len: 1,
                right_part: prefix,
                right_context: Some(CompletionContext::Section),
                third: None,
            },
            cursor,
        );
    }

    // A bare `@` or a still-typing `@fragment` with no separator yet: the
    // whole remainder is the route component, and there is no right-hand
    // component to fall into.
    completion_field_from_parts(
        token,
        CompletionParts {
            sigil_len: 1,
            route_part: rest,
            separator_len: 0,
            right_part: "",
            right_context: Some(CompletionContext::Route),
            third: None,
        },
        cursor,
    )
}

/// Completion for a name that follows a Pomodoro link's `=` suffix. The
/// start lexer supplies the name offset and spelling, while the route and
/// block ID still come from the link prefix. The replacement owns only the
/// name so a typed timing or hash delimiter survives accepting a candidate.
fn pomodoro_link_suffix_name_field(
    token: &Token<'_>,
    route_part: &str,
    rest: &str,
    cursor: usize,
) -> Option<CompletionField> {
    let (prefix, suffix) = rest.split_once('=')?;
    let (block_id, prefix_name) = match prefix.split_once('#') {
        Some((block, name)) => (block, Some(name)),
        None => (prefix, None),
    };
    if !is_block_id(block_id)
        || prefix_name.is_some_and(|name| !name.is_empty())
    {
        return None;
    }
    let start_token = format!("={suffix}");
    let EqualsToken::Start {
        suffix: timing_suffix,
        name: Some(name),
        name_spelling: Some(spelling),
        name_is_literal,
        drop,
        doubled: false,
        timing,
        ..
    } = session_equals_token(&start_token)?
    else {
        return None;
    };
    if drop.is_some() {
        return None;
    }
    let equals = token.text.find('=')?;
    let name_start =
        equals + named_start_name_offset(timing_suffix.len(), spelling);
    // A partial literal completes on the literal's decoded text, and its
    // replacement covers the whole partial literal including the quotes.
    if name_is_literal {
        let after_hash = token.text.get(name_start..).unwrap_or("");
        let lex = super::pomodoro_literal::lex_exact_name_after_hash(
            after_hash,
            after_hash.len(),
        )?;
        let name_end = name_start + lex.literal_len;
        if cursor < token.start + name_start || cursor > token.start + name_end
        {
            return None;
        }
        let typed_before =
            after_hash.get(..cursor - token.start - name_start)?;
        let inner = typed_before
            .strip_prefix('"')
            .or_else(|| typed_before.strip_prefix('“'))
            .unwrap_or(typed_before);
        let query = super::pomodoro_literal::decode_partial_literal_text(inner);
        return Some(CompletionField {
            context: CompletionContext::PomodoroName,
            route: Some(route_part.to_ascii_lowercase()),
            block_id: Some(block_id.to_string()),
            query,
            replacement: (token.start + name_start, token.start + name_end),
            bare_name: false,
        });
    }
    let name_end = name_start + name.len();
    let mut range_end = name_end;
    if let Some((offset, text)) = timing.as_ref()
        && !text.is_empty()
        && start_token.as_bytes().get(*offset) != Some(&b'@')
    {
        range_end += trailing_glued_name_extra(text, &timing_suffix);
    }
    if cursor < token.start + name_start || cursor > token.start + range_end {
        return None;
    }
    let query = if cursor <= token.start + name_end {
        token
            .text
            .get(name_start..cursor - token.start)?
            .to_string()
    } else {
        name.clone()
    };
    Some(CompletionField {
        context: CompletionContext::PomodoroName,
        route: Some(route_part.to_ascii_lowercase()),
        block_id: Some(block_id.to_string()),
        query,
        replacement: (
            token.start
                + if spelling == NameSpelling::Hash {
                    name_start.saturating_sub(1)
                } else {
                    name_start
                },
            token.start + range_end,
        ),
        // The replacement includes a typed `#` when present, so candidate
        // generation emits the shortest spelling and adds `#` only when the
        // selected name needs it. Trailing glued timing stays in the
        // replacement so acceptance can keep the typed count.
        bare_name: true,
    })
}

pub(super) fn global_completion_field_at(
    token: &Token<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    if cursor < token.start || cursor > token.end {
        return None;
    }
    let rest = token.text.strip_prefix("@@")?;
    if let Some(cut) = rest.find(['#', '^', ':']) {
        if cursor > token.start + 2 + cut {
            return None;
        }
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 2,
                route_part: &rest[..cut],
                separator_len: 0,
                right_part: "",
                right_context: Some(CompletionContext::Route),
                third: None,
            },
            cursor,
        );
    }
    if rest.contains('+') {
        return None;
    }
    completion_field_from_parts(
        token,
        CompletionParts {
            sigil_len: 2,
            route_part: rest,
            separator_len: 0,
            right_part: "",
            right_context: Some(CompletionContext::Route),
            third: None,
        },
        cursor,
    )
}

/// The decoded query of a quoted-literal name component. `part` is the
/// literal's text from its opening quote, `split` the cursor's byte offset
/// inside it. With `whole_when_closed`, a cursor past a closed literal
/// queries the whole decoded name instead of the closing quote with it.
fn exact_name_query(
    part: &str,
    split: usize,
    whole_when_closed: bool,
) -> String {
    let closed = whole_when_closed
        .then(|| {
            super::pomodoro_literal::lex_exact_name_after_hash(part, part.len())
        })
        .flatten()
        .filter(|lex| lex.complete && split >= lex.literal_len);
    match closed {
        Some(lex) => lex.decoded,
        None => {
            let raw_query = &part[..split];
            let inner = raw_query
                .strip_prefix('"')
                .or_else(|| raw_query.strip_prefix('“'))
                .unwrap_or(raw_query);
            super::pomodoro_literal::decode_partial_literal_text(inner)
        }
    }
}

/// A first-level bullet's drawer shortcut under the cursor: the bare `#`,
/// a `#slug`, or a quoted `#"TITLE"` literal, in a position where the
/// item's first-level bullets land directly under a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrawerBulletCompletion {
    pub(crate) target: super::drawer_bullet::DrawerBulletTarget,
    /// The decoded name typed so far, up to the cursor.
    pub(crate) query: String,
    /// The whole shortcut token, `#` through its end.
    pub(crate) replacement: (usize, usize),
    /// True when the token ends its line and the next physical line of the
    /// item is not a nested bullet, so accepting a row also opens the
    /// drawer's first nested `  - ` bullet.
    pub(crate) continuation: bool,
}

impl DrawerBulletCompletion {
    fn into_field(self) -> CompletionField {
        use super::drawer_bullet::DrawerBulletTarget;
        let (route, block_id) = match self.target {
            DrawerBulletTarget::ExistingTask { route, block_id } => {
                (route, Some(block_id))
            }
            DrawerBulletTarget::NewTask { route, .. } => (
                route.unwrap_or_else(|| {
                    crate::native::capture::inbox_route().to_string()
                }),
                None,
            ),
            DrawerBulletTarget::NewProjectNote {
                route, block_id, ..
            } => (
                crate::native::capture_project_note::project_note_basename(
                    &route, &block_id,
                )
                .trim_end_matches(".md")
                .to_string(),
                Some("prj".to_string()),
            ),
        };
        CompletionField {
            context: CompletionContext::Drawer,
            route: Some(route),
            block_id,
            query: self.query,
            replacement: self.replacement,
            bare_name: false,
        }
    }
}

/// Probe for a drawer shortcut at a first-level bullet start.
///
/// The outer `None` means the cursor is not in such a token, so other
/// completion claims run as usual. `Some(None)` means it is, but this
/// position cannot name a drawer: the field is an empty success and never
/// falls through to the task-type menu.
fn drawer_bullet_probe(
    raw_text: &str,
    cursor: usize,
) -> Option<Option<DrawerBulletCompletion>> {
    use super::drawer_bullet::{scan_leading, DrawerBulletHit};

    let draft = split_capture_draft(raw_text);
    let (item, line_index, line) = draft.items.iter().find_map(|item| {
        item.lines
            .iter()
            .enumerate()
            .find(|(_, line)| {
                cursor >= line.raw.start && cursor <= line.raw.end
            })
            .map(|(line_index, line)| (item, line_index, line.raw))
    })?;
    if line_index == 0 {
        return None;
    }
    let AuthoredLineClass::Item(authored) = classify_authored_line(line) else {
        return None;
    };
    if authored.depth != AuthoredDepth::First {
        return None;
    }
    let hit = scan_leading(authored.body, authored.body_start)?;
    if matches!(hit, DrawerBulletHit::Escaped { .. }) {
        return None;
    }
    let span = hit.span().clone();
    // The `#` itself is outside the claim, exactly like a glued selector.
    if cursor <= span.start || cursor > span.end {
        return None;
    }
    let Some(target) = super::editor_parse::editor_item_at(raw_text, cursor)
        .and_then(|parsed| parsed.drawer_bullet_target)
    else {
        return Some(None);
    };
    let name_start = span.start + 1;
    let part = raw_text.get(name_start..span.end)?;
    let split = cursor - name_start;
    let query = if super::pomodoro_literal::opens_exact_name(part) {
        exact_name_query(part, split, true)
    } else {
        part.get(..split)?.to_string()
    };
    let next_is_nested = item.lines.get(line_index + 1).is_some_and(|next| {
        matches!(
            classify_authored_line(next.raw),
            AuthoredLineClass::Item(next_line)
                if next_line.depth == AuthoredDepth::Nested
        )
    });
    Some(Some(DrawerBulletCompletion {
        target,
        query,
        replacement: (span.start, span.end),
        continuation: span.end == line.end && !next_is_nested,
    }))
}

/// The drawer menu a first-level bullet's leading `#` opens, or `None`
/// anywhere else (including a bullet-start `#` that cannot name a drawer).
pub(crate) fn drawer_bullet_completion_at(
    raw_text: &str,
    cursor: usize,
) -> Option<DrawerBulletCompletion> {
    drawer_bullet_probe(raw_text, cursor).flatten()
}

/// Build the completion field for one decomposed `@<route><sep><right>`
/// marker, given which side of the (possible) separator `cursor` lands on.
/// The route component spans exactly the route text, excluding the leading
/// sigil; the right component spans exactly its text, excluding the
/// separator. An optional third component is the same: `#` is never part of
/// a replacement range, so a cursor after `#` on `@route+id#` is a
/// zero-length `drawer` replacement at the insertion point. Each
/// component stays well-defined -- and empty -- when its text has not been
/// typed yet.
pub(super) fn completion_field_from_parts(
    token: &Token<'_>,
    parts: CompletionParts<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let route_start = token.start + parts.sigil_len;
    let route_end = route_start + parts.route_part.len();

    if parts.separator_len == 0 || cursor <= route_end {
        let split = cursor.clamp(route_start, route_end) - route_start;
        return Some(CompletionField {
            context: CompletionContext::Route,
            route: None,
            block_id: None,
            query: parts.route_part[..split].to_string(),
            replacement: (route_start, route_end),
            bare_name: false,
        });
    }

    let right_start = route_end + parts.separator_len;
    if cursor < right_start {
        return None;
    }

    let right_end = right_start + parts.right_part.len();
    // A single trailing `@` immediately after the block-ID part is the
    // project-note seal. It is never part of the block replacement: a
    // cursor inside the ID completes the ID, while a cursor just after the
    // seal (and before any `#`) is an empty success. A `+` inside a
    // Pomodoro name stays ordinary name charset.
    let right_is_block_id = matches!(
        parts.right_context,
        Some(CompletionContext::PomodoroBlockId)
            | Some(CompletionContext::TaskBlockId)
    );
    let (right_core, has_plus) = if right_is_block_id {
        match parts.right_part.strip_suffix('@') {
            Some(core) => (core, true),
            None => (parts.right_part, false),
        }
    } else {
        (parts.right_part, false)
    };
    let core_end = right_start + right_core.len();
    if has_plus {
        if cursor <= core_end {
            let right_context = parts.right_context?;
            if !is_route_token(parts.route_part) {
                return None;
            }
            let split = cursor.clamp(right_start, core_end) - right_start;
            return Some(CompletionField {
                context: right_context,
                route: Some(parts.route_part.to_ascii_lowercase()),
                block_id: None,
                query: right_core[..split].to_string(),
                replacement: (right_start, core_end),
                bare_name: false,
            });
        }
        if cursor <= right_end {
            return None;
        }
    }
    let in_right = parts.third.is_none() || cursor <= right_end;
    if in_right {
        // Past the first separator: complete the middle component when that
        // component is backed by a discovery source.
        let right_context = parts.right_context?;
        // The right-hand component only makes sense once the route it
        // belongs to already resolves.
        if !is_route_token(parts.route_part) {
            return None;
        }
        if has_plus {
            let split = cursor.clamp(right_start, core_end) - right_start;
            return Some(CompletionField {
                context: right_context,
                route: Some(parts.route_part.to_ascii_lowercase()),
                block_id: None,
                query: right_core[..split].to_string(),
                replacement: (right_start, core_end),
                bare_name: false,
            });
        }
        let split = cursor.clamp(right_start, right_end) - right_start;
        let query = if right_context == CompletionContext::Section
            && super::pomodoro_literal::opens_exact_name(parts.right_part)
        {
            exact_name_query(parts.right_part, split, true)
        } else {
            parts.right_part[..split].to_string()
        };
        return Some(CompletionField {
            context: right_context,
            route: Some(parts.route_part.to_ascii_lowercase()),
            block_id: None,
            query,
            replacement: (right_start, right_end),
            bare_name: false,
        });
    }

    let third = parts.third?;
    let third_start = right_end + third.separator_len;
    if cursor < third_start {
        return None;
    }
    if !is_route_token(parts.route_part) {
        return None;
    }
    let third_end = third_start + third.part.len();
    let split = cursor.clamp(third_start, third_end) - third_start;
    // A drawer replacement owns the glued `#` as well as the name, so a
    // row's `#slug` or `#"TITLE"` swaps the whole selector; a Pomodoro
    // name keeps its `#` outside the replacement.
    let replacement_start = if third.context == CompletionContext::Drawer {
        third_start - third.separator_len
    } else {
        third_start
    };
    // Strip the project-note seal from the block ID handed to Pomodoro-name
    // completion, so `@route^id@#name` reports `id` rather than `id@`.
    let third_block = if right_is_block_id {
        right_core
    } else {
        parts.right_part
    };
    // A partial literal completes on the literal's decoded text; the
    // replacement still covers the whole partial literal.
    let raw_query = &third.part[..split];
    let query = if matches!(
        third.context,
        CompletionContext::PomodoroName | CompletionContext::Drawer
    ) && super::pomodoro_literal::opens_exact_name(third.part)
    {
        // A drawer's cursor past a closed literal queries the whole
        // decoded name, never the closing quote with it.
        exact_name_query(
            third.part,
            split,
            third.context == CompletionContext::Drawer,
        )
    } else {
        raw_query.to_string()
    };
    Some(CompletionField {
        context: third.context,
        route: Some(parts.route_part.to_ascii_lowercase()),
        block_id: (!third_block.is_empty()).then(|| third_block.to_string()),
        query,
        replacement: (replacement_start, third_end),
        bare_name: false,
    })
}

// ---------------------------------------------------------------------------
// Rule A1-A6: `bob capture-rewrite`'s bare `@@` absorption
// ---------------------------------------------------------------------------
