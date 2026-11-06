//! Shared target-run selection for `@route+id` multi-target captures.
//!
//! This module owns the lexical target-run representation used by both
//! execution and editor parsing. Selection is lexical: any token accepted by
//! [`is_sub_bullet_marker_candidate`] is a member, including partial or
//! malformed members. Strict parsing (existing item semantics) and editor
//! classification decide per-member validity afterwards, so a malformed
//! member receives its existing strict error or editor needs instead of
//! being treated as prose.
//!
//! Both execution and editor parsing use this selector for public target-run
//! behavior. Lexical selection remains separate from strict member parsing.

use super::editor_model::{Diagnostic, Severity, SpanKind};
use super::markers::{
    is_escaped_priority_token, parse_priority_token,
    parse_retired_priority_token, parse_schedule_token,
};
use super::model::{RetiredPriority, TerminalMarkers, Token};
use super::tokens::{
    is_sub_bullet_marker_candidate, parse_sub_bullet_route_token,
};

/// One lexically selected target member: original marker text plus absolute
/// byte range and source order. Ranges always land on `char` boundaries
/// because the scanner walks `char_indices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetMember {
    pub(crate) marker_text: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) token_index: usize,
}

/// Where a selected run sits on its physical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetRunPosition {
    /// Run at the start of the parent line; remaining tokens form the note.
    Leading,
    /// Run at the end of a parent or child line with body text before it.
    Trailing,
    /// Entire parent line is targets (plus terminal markers).
    WholeLine,
}

/// One lexically selected target run in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetRunSelection {
    pub(crate) members: Vec<TargetMember>,
    pub(crate) position: TargetRunPosition,
}

/// Evidence for a `duplicate_capture_target` refusal: the repeated token's
/// complete byte range plus the normalized identity that collided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DuplicateTargetEvidence {
    pub(crate) repeated_text: String,
    pub(crate) block_id: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Evidence for a `split_capture_targets` refusal: the leading and trailing
/// offending tokens plus the trailing token's complete byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitTargetsEvidence {
    pub(crate) leading_text: String,
    pub(crate) trailing_text: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) const DUPLICATE_CAPTURE_TARGET_CODE: &str =
    "duplicate_capture_target";
pub(crate) const SPLIT_CAPTURE_TARGETS_CODE: &str = "split_capture_targets";

/// Lexical member predicate: mirrors the singleton's sub-bullet family.
/// Plain routes, section routes, colon/caret routes, `^route:id`, and
/// `!note:id` are not members.
pub(crate) fn is_target_member_token(text: &str) -> bool {
    is_sub_bullet_marker_candidate(text)
}

/// Normalized duplicate identity: lower-cased route plus exact block ID,
/// ignoring drawer (`#sec`) and toggle (`!`) suffixes. Returns `None` for
/// members that do not parse as sub-bullet markers (partial/malformed
/// members participate in selection but not in duplicate comparison).
pub(crate) fn target_identity(marker_text: &str) -> Option<(String, String)> {
    let token = parse_sub_bullet_route_token(marker_text).ok()?;
    let route = token.route?.to_ascii_lowercase();
    let block_id = match token.kind {
        super::model::CaptureKind::SubBullet { target, .. } => match target {
            super::model::SubBulletTarget::BlockId(id) => id,
            super::model::SubBulletTarget::Ref { .. } => return None,
        },
        super::model::CaptureKind::TaskToggle { block_id, .. } => block_id,
        _ => return None,
    };
    Some((route, block_id))
}

/// Shared duplicate message, used by both strict errors and editor
/// diagnostics.
pub(crate) fn duplicate_target_message(
    repeated: &str,
    block_id: &str,
) -> String {
    format!(
        "`{repeated}` names ^{block_id} twice on this line; name each task once"
    )
}

/// Shared split message, used by both strict errors and editor diagnostics.
pub(crate) fn split_targets_message(leading: &str, trailing: &str) -> String {
    format!(
        "`{leading}` and `{trailing}` are at opposite ends of the line; put every `@route+id` target in one run at the start or end"
    )
}

/// Find the first duplicate within an ordered member list, comparing
/// normalized lower-case route plus exact block ID. Malformed members that
/// lack an identity never collide.
pub(crate) fn find_duplicate_target(
    members: &[TargetMember],
) -> Option<DuplicateTargetEvidence> {
    let mut seen: Vec<(String, String)> = Vec::new();
    for member in members {
        let Some((route, block_id)) = target_identity(&member.marker_text)
        else {
            continue;
        };
        if seen.iter().any(|(r, b)| r == &route && b == &block_id) {
            return Some(DuplicateTargetEvidence {
                repeated_text: member.marker_text.clone(),
                block_id,
                start: member.start,
                end: member.end,
            });
        }
        seen.push((route, block_id));
    }
    None
}

fn is_schedule_or_priority(text: &str) -> bool {
    parse_schedule_token(text).is_some()
        || parse_priority_token(text).is_some()
        || parse_retired_priority_token(text).is_some()
}

/// Whether a token can sit inside a target-run edge block: a target member
/// or a schedule/priority/retired marker. Escaped priorities, plain routes,
/// and prose break blocks. Forced destinations keep `@tokens` literal, so
/// callers pass `detect_route == false` through as no selection.
fn is_block_token(text: &str) -> bool {
    is_target_member_token(text) || is_schedule_or_priority(text)
}

/// Lexically select a target run from already dependency-stripped tokens
/// (type/directive masking already applied by the caller). Terminal
/// schedule/priority tokens must already be removed by
/// [`extract_terminal_markers_crossing_runs`]; any remaining
/// schedule/priority token is body-adjacent literal and breaks the run the
/// same way prose does.
///
/// Rules:
/// - `detect_route == false` (forced destinations) keeps `@tokens` literal.
/// - Entire parent line of members resolves per-member marker-only ops.
/// - Start of parent line consumes the run; the rest is the note.
/// - End of parent/child consumes the run only with preceding body text.
/// - Between prose words the tokens stay literal (no selection).
/// - A second route-bearing physical line is the caller's duplicate-marker
///   responsibility; a list-bullet local run keeps `list_bullet_marker`.
pub(crate) fn select_target_run(
    tokens: &[Token<'_>],
    leading: bool,
    detect_route: bool,
) -> Option<TargetRunSelection> {
    if !detect_route || tokens.is_empty() {
        return None;
    }
    // Collect maximal adjacent target runs (non-target tokens end the run,
    // so `@a+x @b:y note` and `@a+x @bob hi` stay single-target).
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if !is_target_member_token(tokens[index].text) {
            index += 1;
            continue;
        }
        let start = index;
        while index < tokens.len() && is_target_member_token(tokens[index].text)
        {
            index += 1;
        }
        runs.push((start, index));
    }
    if runs.is_empty() {
        return None;
    }
    // Whole line: every token is a member.
    if runs.len() == 1 && runs[0].0 == 0 && runs[0].1 == tokens.len() {
        let (start, end) = runs[0];
        return Some(TargetRunSelection {
            members: members_in(tokens, start, end),
            position: TargetRunPosition::WholeLine,
        });
    }
    // Split needs a leading run and a trailing run with prose between, even
    // when either edge holds only one member. Detection lives in
    // [`detect_target_refusals`]; selection itself never returns split.
    let starts_with_target =
        is_target_member_token(tokens.first().map(|t| t.text).unwrap_or(""));
    let ends_with_target =
        is_target_member_token(tokens.last().map(|t| t.text).unwrap_or(""));
    if starts_with_target && ends_with_target {
        // Leading + trailing with prose between is a refusal, not a
        // selection. Callers surface it via `detect_target_refusals`.
        return None;
    }
    // Leading run at the start of the parent line only.
    if leading
        && let Some((start, end)) = runs.first()
        && *start == 0
    {
        return Some(TargetRunSelection {
            members: members_in(tokens, *start, *end),
            position: TargetRunPosition::Leading,
        });
    }
    // Trailing run at the end with body text before it (parent or child).
    if let Some((start, end)) = runs.last()
        && *end == tokens.len()
        && *start > 0
    {
        return Some(TargetRunSelection {
            members: members_in(tokens, *start, *end),
            position: TargetRunPosition::Trailing,
        });
    }
    None
}

fn members_in(
    tokens: &[Token<'_>],
    start: usize,
    end: usize,
) -> Vec<TargetMember> {
    tokens[start..end]
        .iter()
        .enumerate()
        .map(|(offset, token)| TargetMember {
            marker_text: token.text.to_string(),
            start: token.start,
            end: token.end,
            token_index: start + offset,
        })
        .collect()
}

/// Detect duplicate and split refusals on one physical line's
/// dependency-stripped tokens. Duplicate identity is limited to the selected
/// run; target-shaped prose elsewhere is ignored. A parent line with targets
/// at both edges is a split refusal, even when those edge targets repeat.
pub(crate) fn detect_target_refusals(
    tokens: &[Token<'_>],
    leading: bool,
    detect_route: bool,
) -> TargetRefusal {
    if !detect_route || tokens.is_empty() {
        return TargetRefusal::None;
    }
    // Gather target runs with their token spans.
    let mut runs: Vec<(usize, usize, Vec<TargetMember>)> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if !is_target_member_token(tokens[index].text) {
            index += 1;
            continue;
        }
        let start = index;
        while index < tokens.len() && is_target_member_token(tokens[index].text)
        {
            index += 1;
        }
        runs.push((start, index, members_in(tokens, start, index)));
    }
    if runs.is_empty() {
        return TargetRefusal::None;
    }
    let starts_with_target = runs.first().is_some_and(|(s, _, _)| *s == 0);
    let ends_with_target =
        runs.last().is_some_and(|(_, e, _)| *e == tokens.len());
    if leading && runs.len() > 1 && starts_with_target && ends_with_target {
        // Split is the approved parent-line refusal and takes precedence
        // over duplicate identity across the two edges.
        let leading_members = &runs.first().expect("leading").2;
        let trailing_members = &runs.last().expect("trailing").2;
        let leading_text = leading_members
            .first()
            .map(|m| m.marker_text.clone())
            .unwrap_or_default();
        let trailing =
            trailing_members.first().expect("trailing member").clone();
        return TargetRefusal::Split(SplitTargetsEvidence {
            leading_text,
            trailing_text: trailing.marker_text.clone(),
            start: trailing.start,
            end: trailing.end,
        });
    }
    let Some(selection) = select_target_run(tokens, leading, detect_route)
    else {
        return TargetRefusal::None;
    };
    find_duplicate_target(&selection.members)
        .map(TargetRefusal::Duplicate)
        .unwrap_or(TargetRefusal::None)
}

/// One line's refusal outcome: at most one of duplicate or split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetRefusal {
    None,
    Duplicate(DuplicateTargetEvidence),
    Split(SplitTargetsEvidence),
}

/// Strict adapter: shared evidence into a coded execution refusal.
pub(crate) fn strict_refusal_parts(
    refusal: &TargetRefusal,
) -> Option<(&'static str, String, (usize, usize))> {
    match refusal {
        TargetRefusal::None => None,
        TargetRefusal::Duplicate(evidence) => Some((
            DUPLICATE_CAPTURE_TARGET_CODE,
            duplicate_target_message(
                &evidence.repeated_text,
                &evidence.block_id,
            ),
            (evidence.start, evidence.end),
        )),
        TargetRefusal::Split(evidence) => Some((
            SPLIT_CAPTURE_TARGETS_CODE,
            split_targets_message(
                &evidence.leading_text,
                &evidence.trailing_text,
            ),
            (evidence.start, evidence.end),
        )),
    }
}

/// Editor adapter: shared evidence into a diagnostic with the same code
/// and message the strict path reports. Both adapters consume this evidence.
pub(crate) fn editor_refusal_diagnostic(
    refusal: &TargetRefusal,
) -> Option<Diagnostic> {
    let (code, message, (start, end)) = strict_refusal_parts(refusal)?;
    Some(Diagnostic {
        severity: Severity::Error,
        code,
        message,
        range: Some((start, end)),
        rewrite: None,
    })
}

/// Byte-preserving member view: mask every other run member with spaces so
/// absolute byte offsets never shift. Replacing each covered byte with
/// `0x20` keeps the source length identical and leaves valid UTF-8 (target
/// markers are ASCII; any drawer literal bytes become spaces). Authored
/// children and the original item identity are untouched by the caller;
/// masking never creates a second `@@` declaration.
pub(crate) fn masked_source_for_member(
    source: &str,
    members: &[TargetMember],
    keep_index: usize,
) -> String {
    let mut bytes = source.as_bytes().to_vec();
    for (index, member) in members.iter().enumerate() {
        if index == keep_index {
            continue;
        }
        if member.start >= member.end
            || member.end > bytes.len()
            || member.start > bytes.len()
        {
            continue;
        }
        for byte in &mut bytes[member.start..member.end] {
            *byte = b' ';
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}

/// Build a member view with the run's consumed schedule and priority tokens
/// masked as well. Their semantic values and spans travel in
/// `TargetRunContext`, so a member parser cannot reinterpret the tokens as
/// note text after the shared boundary has consumed them.
pub(crate) fn masked_source_for_member_with_terminals(
    source: &str,
    members: &[TargetMember],
    keep_index: usize,
    terminal_spans: &[(SpanKind, usize, usize)],
) -> String {
    let mut bytes =
        masked_source_for_member(source, members, keep_index).into_bytes();
    for (kind, start, end) in terminal_spans {
        if !matches!(kind, SpanKind::Schedule | SpanKind::Priority)
            || start >= end
            || *end > bytes.len()
        {
            continue;
        }
        for byte in &mut bytes[*start..*end] {
            *byte = b' ';
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}

/// Run-aware terminal extraction: collect schedule/priority markers on
/// either side of a target run or between its members.
///
/// `note s:3 @a+x @b+y`, `note @a+x @b+y s:3`, and `note @a+x s:3 @b+y`
/// select the same targets and schedule; priority (`$N`) behaves the same
/// for both `@` sub-bullet members. Existing stop-on-duplicate,
/// escaped-priority, retired-priority, and non-target-route behavior is
/// preserved: duplicates stop extraction, `\$` escapes stay literal with
/// their escape recorded, `p:<N>` extracts as retired, and plain routes
/// break blocks.
///
/// Only edge blocks (leading, trailing, or whole-line) contribute markers;
/// middle blocks between prose words stay literal. Callers run
/// [`select_target_run`] afterwards on the remaining tokens.
pub(crate) fn extract_terminal_markers_crossing_runs(
    tokens: &mut Vec<Token<'_>>,
) -> (TerminalMarkers, Vec<(SpanKind, usize, usize)>) {
    if tokens.is_empty() {
        return (TerminalMarkers::default(), Vec::new());
    }
    // Find the leading edge block (from 0) and trailing edge block (to
    // len) over block tokens (targets + schedule/priority/retired).
    let leading_end = tokens
        .iter()
        .position(|token| {
            !is_block_token(token.text)
                && !is_escaped_priority_token(token.text)
        })
        .unwrap_or(tokens.len());
    let mut trailing_start = tokens.len();
    while trailing_start > 0 {
        let text = tokens[trailing_start - 1].text;
        if is_block_token(text) {
            trailing_start -= 1;
        } else if is_escaped_priority_token(text) {
            break;
        } else {
            break;
        }
    }
    // No edge block with a target: fall back to literal (nothing to do).
    let leading_has_target = tokens[..leading_end]
        .iter()
        .any(|token| is_target_member_token(token.text));
    let trailing_has_target = tokens[trailing_start..]
        .iter()
        .any(|token| is_target_member_token(token.text));
    let whole_line = leading_end == tokens.len() && trailing_start == 0;
    // Middle-only blocks stay literal.
    if !whole_line && !leading_has_target && !trailing_has_target {
        return (TerminalMarkers::default(), Vec::new());
    }
    // Split lines have two edge blocks; extract from both so each side's
    // markers are still collected before the caller refuses split.
    // Otherwise extract from the single edge block that holds targets.
    let use_leading = whole_line || leading_has_target;
    let use_trailing = whole_line || trailing_has_target || leading_has_target;
    // For a single trailing run (`note ... targets`), only the trailing
    // block matters; for a single leading run, only the leading block.
    // Whole-line and split use both (they overlap on whole-line).
    let mut markers = TerminalMarkers::default();
    let mut spans = Vec::new();
    if whole_line {
        extract_markers_from_range(
            tokens,
            0,
            tokens.len(),
            &mut markers,
            &mut spans,
        );
        return (markers, spans);
    }
    let leading_is_edge = leading_end > 0 && leading_has_target;
    let trailing_is_edge = trailing_start < tokens.len() && trailing_has_target;
    if leading_is_edge && trailing_is_edge {
        // Split candidate: extract from both edges (prose sits between).
        extract_markers_from_range(
            tokens,
            0,
            leading_end,
            &mut markers,
            &mut spans,
        );
        // Trailing indices shifted by the leading removal; recompute the
        // trailing block on the mutated vector.
        let mut new_trailing_start = tokens.len();
        while new_trailing_start > 0
            && is_block_token(tokens[new_trailing_start - 1].text)
        {
            new_trailing_start -= 1;
        }
        extract_markers_from_range(
            tokens,
            new_trailing_start,
            tokens.len(),
            &mut markers,
            &mut spans,
        );
        let _ = (use_leading, use_trailing);
        return (markers, spans);
    }
    if leading_is_edge {
        extract_markers_from_range(
            tokens,
            0,
            leading_end,
            &mut markers,
            &mut spans,
        );
    } else if trailing_is_edge {
        extract_markers_from_range(
            tokens,
            trailing_start,
            tokens.len(),
            &mut markers,
            &mut spans,
        );
    }
    (markers, spans)
}

fn extract_markers_from_range(
    tokens: &mut Vec<Token<'_>>,
    start: usize,
    end: usize,
    markers: &mut TerminalMarkers,
    spans: &mut Vec<(SpanKind, usize, usize)>,
) {
    // Remove schedule/priority/retired markers inside [start, end),
    // preserving stop-on-duplicate and escaped handling. Walk backwards so
    // removals never shift unvisited indices.
    let mut index = end.min(tokens.len());
    let lower = start.min(index);
    while index > lower {
        let check = index - 1;
        if check >= tokens.len() {
            break;
        }
        let (text, span) =
            (tokens[check].text, (tokens[check].start, tokens[check].end));
        if is_target_member_token(text) {
            index -= 1;
            continue;
        }
        if is_escaped_priority_token(text) {
            markers.priority_escapes.push((span.0, span.0 + 1));
            break;
        }
        if let Some(offset) = parse_schedule_token(text) {
            if markers.scheduled_offset.is_some() {
                break;
            }
            markers.scheduled_offset = Some(offset);
            spans.push((SpanKind::Schedule, span.0, span.1));
            tokens.remove(check);
            index -= 1;
            continue;
        }
        if let Some(level) = parse_priority_token(text) {
            if markers.priority_level.is_some() {
                break;
            }
            markers.priority_level = Some(level);
            spans.push((SpanKind::Priority, span.0, span.1));
            tokens.remove(check);
            index -= 1;
            continue;
        }
        if parse_retired_priority_token(text).is_some() {
            if markers.retired_priority.is_some() {
                break;
            }
            markers.retired_priority = Some(RetiredPriority {
                token: text.to_string(),
                span: Some(span),
            });
            tokens.remove(check);
            index -= 1;
            continue;
        }
        break;
    }
}

/// Validate every selected boundary member with the existing strict
/// sub-bullet parser, so malformed members receive their established
/// strict error instead of staying literal prose.
pub(crate) fn validate_selected_members(
    members: &[TargetMember],
) -> Result<(), String> {
    for member in members {
        parse_sub_bullet_route_token(&member.marker_text)?;
    }
    Ok(())
}
