//! Shared test helpers for capture-language tests.

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

fn line_texts(raw: &str) -> Vec<&str> {
    split_physical_lines(raw)
        .iter()
        .map(|line| line.text)
        .collect()
}

fn execute(raw: &str) -> Result<ParsedCaptureText, String> {
    parse_capture_text_with_clip_control(raw, None, None, true)
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

mod completion;
mod draft;
mod editor_modes;
mod editor_spans;
mod globals;
mod grammar;
mod rewrite;
