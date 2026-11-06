//! Shared `@route+id` source helpers for `>` transfers and `<` merge-ins.
//!
//! Both scanners claim complete `@route+id` sources with identical lexical
//! rules. This module owns the source predicate, the complete-source parser,
//! the per-token lexical classifier, and duplicate detection so the two
//! scanners cannot drift.

use super::markers::is_route_token;
use super::model::Token;
use super::tokens::is_block_id;

pub(crate) const SOURCE_SUFFIX_CODE: &str = "transfer_source_suffix";
pub(crate) const SOURCE_MARKER_CODE: &str = "transfer_source_marker";
pub(crate) const DUPLICATE_CODE: &str = "duplicate_capture_target";

/// One complete `@route+id` source with absolute byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSource {
    pub(crate) marker: String,
    pub(crate) route: String,
    pub(crate) block_id: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Lexical refusal independent of either scan enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceLexicalError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) span: (usize, usize),
}

pub(crate) fn is_source_like(text: &str) -> bool {
    if text.starts_with("@@") {
        return false;
    }
    if let Some(rest) = text.strip_prefix('@') {
        if rest.starts_with('!') {
            return false;
        }
        return rest.contains('+') || rest.contains(':') || rest.contains('^');
    }
    if let Some(rest) = text.strip_prefix('^') {
        return rest.contains(':');
    }
    false
}

pub(crate) fn parse_complete_source_text(
    text: &str,
) -> Option<(String, String)> {
    let rest = text.strip_prefix('@')?;
    if rest.starts_with('@') || rest.starts_with('!') {
        return None;
    }
    let (route, id) = rest.split_once('+')?;
    if route.is_empty() || !is_route_token(route) {
        return None;
    }
    if id.is_empty()
        || id.contains('#')
        || id.contains('!')
        || id.contains(':')
        || id.contains('^')
        || id.contains('@')
    {
        return None;
    }
    if !is_block_id(id) {
        return None;
    }
    Some((route.to_ascii_lowercase(), id.to_string()))
}

/// Split a complete `@route+id` marker into lowercased route and block ID.
pub(crate) fn split_complete_source(text: &str) -> Option<(String, String)> {
    parse_complete_source_text(text)
}

fn suffix_error(token: &Token<'_>, message: String) -> SourceLexicalError {
    SourceLexicalError {
        code: SOURCE_SUFFIX_CODE,
        message,
        span: (token.start, token.end),
    }
}

fn marker_error(token: &Token<'_>, message: String) -> SourceLexicalError {
    SourceLexicalError {
        code: SOURCE_MARKER_CODE,
        message,
        span: (token.start, token.end),
    }
}

pub(crate) fn parse_source_token(
    token: &Token<'_>,
) -> Result<TaskSource, SourceLexicalError> {
    if let Some((route, block_id)) = parse_complete_source_text(token.text) {
        return Ok(TaskSource {
            marker: token.text.to_string(),
            route,
            block_id,
            start: token.start,
            end: token.end,
        });
    }
    let span = (token.start, token.end);
    let text = token.text;
    if text.starts_with('@') {
        let rest = &text[1..];
        if let Some(plus) = rest.find('+') {
            let after = &rest[plus + 1..];
            if after.contains('#') {
                return Err(suffix_error(
                    token,
                    format!(
                        "sources are whole tasks; drop #drawer from {text}"
                    ),
                ));
            }
            if text.ends_with('!') {
                return Err(suffix_error(
                    token,
                    format!("sources are whole tasks; drop ! from {text}"),
                ));
            }
        }
        if rest.contains(':') || rest.contains('^') {
            return Err(SourceLexicalError {
                code: SOURCE_MARKER_CODE,
                message: format!(
                    "name tasks with @route+id; `{text}` would link it"
                ),
                span,
            });
        }
        if text.ends_with('+')
            || rest.contains('+')
            || rest.contains('@')
            || rest.contains('!')
        {
            return Err(marker_error(
                token,
                format!("name tasks with @route+id; `{text}` would link it"),
            ));
        }
    }
    if text.starts_with('^') {
        return Err(marker_error(
            token,
            format!("name tasks with @route+id; `{text}` would link it"),
        ));
    }
    Err(marker_error(
        token,
        format!("name tasks with @route+id; `{text}` would link it"),
    ))
}

pub(crate) fn find_lexical_duplicate(
    sources: &[TaskSource],
) -> Option<SourceLexicalError> {
    let mut seen: Vec<(String, String)> = Vec::new();
    for source in sources {
        let key = (source.route.clone(), source.block_id.clone());
        if seen.iter().any(|(r, b)| r == &key.0 && b == &key.1) {
            let message = super::target_runs::duplicate_target_message(
                &source.marker,
                &source.block_id,
            );
            return Some(SourceLexicalError {
                code: DUPLICATE_CODE,
                message,
                span: (source.start, source.end),
            });
        }
        seen.push(key);
    }
    None
}
