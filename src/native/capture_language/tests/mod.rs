//! Shared test helpers for capture-language tests.

use super::alias_tip::AliasId;
use super::*;

fn editor(raw: &str) -> EditorParse {
    parse_for_editor(raw)
}

fn span_kinds(parse: &EditorParse) -> Vec<SpanKind> {
    parse.spans.iter().map(|span| span.kind).collect()
}

fn ranges(parse: &EditorParse) -> Vec<(usize, usize, SpanKind)> {
    parse
        .spans
        .iter()
        .map(|span| (span.start, span.end, span.kind))
        .collect()
}

fn codes(parse: &EditorParse) -> Vec<&'static str> {
    parse
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

type MarkerCase = (
    &'static str,
    EditorMode,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
    &'static [Need],
);

type CrossedMarkerCase = (
    &'static str,
    EditorMode,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
);

fn field(raw: &str, cursor: usize) -> Option<CompletionField> {
    completion_field_at(raw, cursor)
}

/// The one place a positive `expand_sigil_alias` assertion lives, so a new
/// alias fixture cannot skip the typing-tip reverse check. `rewrite` must be
/// the forward rewrite of `raw`: it fired the alias rule and stamped an alias
/// id. For a tipped id, the reverse check on the output must prove the same
/// id and the typed key; an untipped id must never be taught back.
fn assert_alias_applied(raw: &str, rewrite: &DraftRewrite) -> AliasId {
    assert_eq!(rewrite.rule, Some(RewriteRule::ExpandSigilAlias), "{raw:?}");
    let stamp = rewrite
        .alias
        .as_ref()
        .unwrap_or_else(|| panic!("{raw:?} fired an alias without an id"));
    let cursor = rewrite
        .cursor
        .unwrap_or_else(|| panic!("{raw:?} alias lost the cursor"));
    let tip = sigil_alias_tip(&rewrite.text, cursor);
    if stamp.id.tipped() {
        let tip = tip.unwrap_or_else(|| {
            panic!(
                "{raw:?} -> {:?} ({}) proves no tip",
                rewrite.text,
                stamp.id.code()
            )
        });
        assert_eq!(tip.id, stamp.id, "{raw:?}");
        assert_eq!(tip.sigil, stamp.sigil, "{raw:?}");
        assert!(tip.keys.contains(&stamp.key), "{raw:?}: {:?}", tip.keys);
        assert_eq!(tip.range, (cursor - 1, cursor), "{raw:?}");
    } else if let Some(tip) = tip {
        assert_ne!(tip.id, stamp.id, "{raw:?} taught an untipped alias");
    }
    stamp.id
}

fn line_texts(raw: &str) -> Vec<&str> {
    split_physical_lines(raw)
        .iter()
        .map(|line| line.text)
        .collect()
}

fn execute(raw: &str) -> Result<ParsedCaptureText, String> {
    parse_capture_text_with_options(
        raw,
        None,
        None,
        &CaptureParseOptions::routing_off(),
    )
}

fn execute_draft(raw: &str) -> Result<ParsedCaptureDraft, String> {
    parse_capture_draft_with_options(
        raw,
        None,
        None,
        &CaptureParseOptions::routing_off(),
    )
}

fn sub_bullet_bodies(sub_bullets: &[AuthoredSubBullet]) -> Vec<&str> {
    sub_bullets.iter().map(|item| item.body.as_str()).collect()
}

fn sub_bullet_depths(sub_bullets: &[AuthoredSubBullet]) -> Vec<u8> {
    sub_bullets.iter().map(|item| item.depth.level()).collect()
}

fn draft_items(raw: &str) -> Vec<(usize, usize, &str)> {
    split_capture_draft(raw)
        .items
        .iter()
        .map(|item| (item.index, item.line_start, &raw[item.start..item.end]))
        .collect()
}

mod alias_tip;
mod bare_close;
mod chain;
mod completion;
mod draft;
mod drawer_bullet;
mod editor_modes;
mod editor_spans;
mod exact_name;
mod globals;
mod goto;
mod grammar;
mod lists;
mod merge_in;
mod move_bullet;
mod notebook;
mod past;
mod pomodoro_merge;
mod record;
mod ref_grammar;
mod retired_close;
mod rewrite;
mod sized_close;
mod target_runs;
mod template;
mod transfer;
mod type_marker;
mod type_marker_editor;
