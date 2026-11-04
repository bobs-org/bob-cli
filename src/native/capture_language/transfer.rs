//! Shared span-aware transfer recognizer for `>` capture lines.
//!
//! A transfer moves one or more whole tasks (`@route+id` sources) to a bare
//! note (`@route`) or merges them into a task (`@route+id`), drawer
//! (`@route+id#drawer`), or section (`@route#section`). Both the execution
//! resolver (`draft.rs`) and the editor resolver (`editor_parse.rs`) must
//! claim the same parent lines before target-run fan-out, so this module
//! owns the single pure scanner they will call in the next phase.
//!
//! Both execution and editor parsing call this scanner before target-run
//! fan-out, so a claimed transfer cannot fall through to ordinary capture.

use super::markers::is_route_token;
use super::model::{RawLine, Token};
use super::pomodoro_literal::{lex_exact_name_after_hash, opens_exact_name};
use super::tokens::is_block_id;

/// One complete transfer source: `@route+id` with absolute byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferSource {
    pub(crate) marker: String,
    pub(crate) route: String,
    pub(crate) block_id: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// The standalone `>` arrow with absolute byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferArrow {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Which destination shape a transfer targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferDestinationKind {
    Note,
    Task,
    Drawer,
    Section,
}

/// One transfer destination with absolute spans and selector intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferDestination {
    pub(crate) marker: String,
    pub(crate) route: String,
    pub(crate) block_id: Option<String>,
    pub(crate) drawer: Option<String>,
    pub(crate) drawer_exact: bool,
    pub(crate) section: Option<String>,
    pub(crate) section_exact: bool,
    pub(crate) kind: TransferDestinationKind,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Which editor need an incomplete right-hand side still owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferNeed {
    Destination,
    BlockId,
    Drawer,
    Section,
}

impl TransferNeed {
    #[cfg(test)]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Destination => "transfer_destination",
            Self::BlockId => "block_id",
            Self::Drawer => "drawer",
            Self::Section => "section",
        }
    }
}

/// Pure scan outcome for one parent line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferScan {
    /// Not a transfer: ordinary prose, code, blockquote, or a line without
    /// a source run immediately followed by `>`.
    Unclaimed,
    /// `\>` at the arrow position: the line keeps a literal `>` with the
    /// backslash dropped. The following tokens stay authored text.
    Escaped {
        sources: Vec<TransferSource>,
        backslash_span: (usize, usize),
        arrow_span: (usize, usize),
    },
    /// A claimed transfer whose right-hand side is partial: the editor
    /// still owes the named need.
    Incomplete {
        sources: Vec<TransferSource>,
        arrow: TransferArrow,
        need: TransferNeed,
        span: (usize, usize),
        message: String,
    },
    /// A complete, well-formed transfer.
    Valid {
        sources: Vec<TransferSource>,
        arrow: TransferArrow,
        destination: TransferDestination,
    },
    /// A claimed transfer that breaks a lexical rule.
    Invalid {
        code: &'static str,
        message: String,
        span: (usize, usize),
    },
}

pub(crate) const TRANSFER_ARROW_SPACING_CODE: &str = "transfer_arrow_spacing";
pub(crate) const TRANSFER_EXTRA_TEXT_CODE: &str = "transfer_extra_text";
pub(crate) const TRANSFER_CHILDREN_CODE: &str = "transfer_children";
pub(crate) const TRANSFER_SOURCE_SUFFIX_CODE: &str = "transfer_source_suffix";
pub(crate) const TRANSFER_SOURCE_MARKER_CODE: &str = "transfer_source_marker";
pub(crate) const TRANSFER_DESTINATION_MARKER_CODE: &str =
    "transfer_destination_marker";
pub(crate) const TRANSFER_DESTINATION_IS_SOURCE_CODE: &str =
    "transfer_destination_is_source";
pub(crate) const TRANSFER_DUPLICATE_CODE: &str = "duplicate_capture_target";

pub(crate) const TRANSFER_ARROW_SPACING_MESSAGE: &str =
    "put spaces around >: @a+x > @c";
pub(crate) const TRANSFER_EXTRA_TEXT_MESSAGE: &str = "a > line takes one destination and nothing else; put notes in a separate @c+id item";
pub(crate) const TRANSFER_DESTINATION_MARKER_MESSAGE: &str = "transfer destinations must use @<route>, @<route>+<block-id>, @<route>+<block-id>#<drawer>, or @<route>#<section>";
pub(crate) const TRANSFER_CHILDREN_MESSAGE: &str =
    "a > line takes no child bullets; put notes in a separate @c+id item";
pub(crate) const TRANSFER_DESTINATION_IS_SOURCE_MESSAGE: &str =
    "the destination task is also a source";

/// Scan one parent line for a transfer directive.
///
/// `parent` is the parent line with absolute offsets, `source` is the full
/// draft for protected-markup checks, and `has_children` reports whether
/// the item carries child lines. Returns `Unclaimed` for ordinary prose,
/// code, blockquotes, `@@` declarations, forced-literal callers (which skip
/// the call), and lines without a source run immediately followed by `>`.
pub(crate) fn scan_transfer_line(
    parent: &RawLine<'_>,
    source: &str,
    has_children: bool,
) -> TransferScan {
    let trimmed = parent.text.trim_start();
    if trimmed.is_empty() {
        return TransferScan::Unclaimed;
    }
    if trimmed.starts_with("@@") {
        return TransferScan::Unclaimed;
    }
    if trimmed.starts_with('>') {
        return TransferScan::Unclaimed;
    }
    let tokens = absolutize_and_merge(parent);
    if tokens.is_empty() {
        return TransferScan::Unclaimed;
    }
    if tokens.iter().any(|token| {
        crate::native::capture_links::is_protected_markup_range(
            source,
            token.start,
            token.end,
        )
    }) {
        return TransferScan::Unclaimed;
    }
    if let Some(escaped) = try_escaped(&tokens, source) {
        return escaped;
    }
    let Some(arrow_index) = find_standalone_arrow(&tokens) else {
        return try_glued_arrow(&tokens);
    };
    scan_with_arrow(&tokens, arrow_index, source, has_children)
}

fn absolutize_and_merge<'a>(parent: &RawLine<'a>) -> Vec<Token<'a>> {
    let rel = super::model::tokenize_with_spans(parent.text);
    let mut abs_tokens: Vec<Token<'_>> = Vec::new();
    for token in rel {
        abs_tokens.push(Token {
            text: token.text,
            start: parent.start + token.start,
            end: parent.start + token.end,
        });
    }
    super::pomodoro_literal::merge_leading_literal_token(parent, abs_tokens)
}

fn find_standalone_arrow(tokens: &[Token<'_>]) -> Option<usize> {
    tokens.iter().position(|token| token.text == ">")
}

fn try_escaped(tokens: &[Token<'_>], source: &str) -> Option<TransferScan> {
    let index = tokens.iter().position(|token| token.text == "\\>")?;
    let before = &tokens[..index];
    if before.is_empty() || !before.iter().all(|t| is_complete_source(t.text)) {
        return None;
    }
    let mut sources = Vec::new();
    for token in before {
        let Ok(parsed) = parse_complete_source(token) else {
            return None;
        };
        sources.push(parsed);
    }
    if sources.iter().any(|s| {
        crate::native::capture_links::is_protected_markup_range(
            source, s.start, s.end,
        )
    }) {
        return None;
    }
    let token = &tokens[index];
    if crate::native::capture_links::is_protected_markup_range(
        source,
        token.start,
        token.end,
    ) {
        return None;
    }
    Some(TransferScan::Escaped {
        sources,
        backslash_span: (token.start, token.start + 1),
        arrow_span: (token.start + 1, token.end),
    })
}

fn try_glued_arrow(tokens: &[Token<'_>]) -> TransferScan {
    for token in tokens {
        if token.text == "\\>" {
            continue;
        }
        if let Some(off) = token.text.find('>') {
            let abs = token.start + off;
            if is_escaped_gt(token.text, off) {
                continue;
            }
            if inside_quoted_literal(token.text, off) {
                continue;
            }
            return TransferScan::Invalid {
                code: TRANSFER_ARROW_SPACING_CODE,
                message: TRANSFER_ARROW_SPACING_MESSAGE.to_string(),
                span: (abs, abs + 1),
            };
        }
    }
    TransferScan::Unclaimed
}

fn is_escaped_gt(text: &str, off: usize) -> bool {
    off > 0 && text.as_bytes().get(off - 1) == Some(&b'\\')
}

fn inside_quoted_literal(text: &str, off: usize) -> bool {
    let Some(hash) = text.find('#') else {
        return false;
    };
    if off <= hash {
        return false;
    }
    let after = &text[hash + 1..];
    if !opens_exact_name(after) {
        return false;
    }
    let Some(lex) = lex_exact_name_after_hash(after, after.len()) else {
        return false;
    };
    let lit_start = hash + 1;
    let lit_end = lit_start + lex.literal_len;
    off >= lit_start && off < lit_end
}

fn scan_with_arrow(
    tokens: &[Token<'_>],
    arrow_index: usize,
    source: &str,
    has_children: bool,
) -> TransferScan {
    let arrow_token = &tokens[arrow_index];
    let arrow = TransferArrow {
        start: arrow_token.start,
        end: arrow_token.end,
    };
    let before = &tokens[..arrow_index];
    if before.is_empty() {
        return TransferScan::Unclaimed;
    }
    if !before.iter().all(|t| is_source_like(t.text)) {
        return TransferScan::Unclaimed;
    }
    let mut sources = Vec::new();
    for token in before {
        match parse_source_token(token) {
            Ok(parsed) => sources.push(parsed),
            Err(invalid) => return invalid,
        }
    }
    if crate::native::capture_links::is_protected_markup_range(
        source,
        arrow.start,
        arrow.end,
    ) {
        return TransferScan::Unclaimed;
    }
    if let Some(dup) = find_lexical_duplicate(&sources) {
        return dup;
    }
    let after = &tokens[arrow_index + 1..];
    if after.is_empty() {
        let span = (arrow.start, arrow.end);
        return TransferScan::Incomplete {
            sources,
            arrow,
            need: TransferNeed::Destination,
            span,
            message: "transfer needs a destination: @route, @route+id, @route+id#drawer, or @route#section"
                .to_string(),
        };
    }
    let dest_token = &after[0];
    if crate::native::capture_links::is_protected_markup_range(
        source,
        dest_token.start,
        dest_token.end,
    ) {
        return TransferScan::Unclaimed;
    }
    match parse_destination_token(dest_token) {
        Ok(destination) => {
            if after.len() > 1 {
                let start = after[1].start;
                let end = after.last().map(|t| t.end).unwrap_or(start);
                return TransferScan::Invalid {
                    code: TRANSFER_EXTRA_TEXT_CODE,
                    message: TRANSFER_EXTRA_TEXT_MESSAGE.to_string(),
                    span: (start, end),
                };
            }
            if has_children {
                let span = (destination.start, destination.end);
                return TransferScan::Invalid {
                    code: TRANSFER_CHILDREN_CODE,
                    message: TRANSFER_CHILDREN_MESSAGE.to_string(),
                    span,
                };
            }
            if let Some(conflict) =
                find_destination_is_source(&sources, &destination)
            {
                return conflict;
            }
            TransferScan::Valid {
                sources,
                arrow,
                destination,
            }
        }
        Err(TransferDestErr::Incomplete {
            need,
            span,
            message,
        }) => TransferScan::Incomplete {
            sources,
            arrow,
            need,
            span,
            message,
        },
        Err(TransferDestErr::Invalid {
            code,
            message,
            span,
        }) => TransferScan::Invalid {
            code,
            message,
            span,
        },
    }
}

fn is_source_like(text: &str) -> bool {
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

fn is_complete_source(text: &str) -> bool {
    parse_complete_source_text(text).is_some()
}

fn parse_complete_source_text(text: &str) -> Option<(String, String)> {
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

fn parse_complete_source(token: &Token<'_>) -> Result<TransferSource, ()> {
    let (route, block_id) = parse_complete_source_text(token.text).ok_or(())?;
    Ok(TransferSource {
        marker: token.text.to_string(),
        route,
        block_id,
        start: token.start,
        end: token.end,
    })
}

fn parse_source_token(
    token: &Token<'_>,
) -> Result<TransferSource, TransferScan> {
    if let Ok(parsed) = parse_complete_source(token) {
        return Ok(parsed);
    }
    let span = (token.start, token.end);
    let text = token.text;
    if text.starts_with('@') {
        let rest = &text[1..];
        if let Some(plus) = rest.find('+') {
            let after = &rest[plus + 1..];
            if after.contains('#') {
                return Err(TransferScan::Invalid {
                    code: TRANSFER_SOURCE_SUFFIX_CODE,
                    message: format!(
                        "sources are whole tasks; drop #drawer from {text}"
                    ),
                    span,
                });
            }
            if text.ends_with('!') {
                return Err(TransferScan::Invalid {
                    code: TRANSFER_SOURCE_SUFFIX_CODE,
                    message: format!(
                        "sources are whole tasks; drop ! from {text}"
                    ),
                    span,
                });
            }
        }
        if rest.contains(':') || rest.contains('^') {
            return Err(TransferScan::Invalid {
                code: TRANSFER_SOURCE_MARKER_CODE,
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
            return Err(TransferScan::Invalid {
                code: TRANSFER_SOURCE_MARKER_CODE,
                message: format!(
                    "name tasks with @route+id; `{text}` would link it"
                ),
                span,
            });
        }
    }
    if text.starts_with('^') {
        return Err(TransferScan::Invalid {
            code: TRANSFER_SOURCE_MARKER_CODE,
            message: format!(
                "name tasks with @route+id; `{text}` would link it"
            ),
            span,
        });
    }
    Err(TransferScan::Invalid {
        code: TRANSFER_SOURCE_MARKER_CODE,
        message: format!("name tasks with @route+id; `{text}` would link it"),
        span,
    })
}

fn find_lexical_duplicate(sources: &[TransferSource]) -> Option<TransferScan> {
    let mut seen: Vec<(String, String)> = Vec::new();
    for source in sources {
        let key = (source.route.clone(), source.block_id.clone());
        if seen.iter().any(|(r, b)| r == &key.0 && b == &key.1) {
            let message = super::target_runs::duplicate_target_message(
                &source.marker,
                &source.block_id,
            );
            return Some(TransferScan::Invalid {
                code: TRANSFER_DUPLICATE_CODE,
                message,
                span: (source.start, source.end),
            });
        }
        seen.push(key);
    }
    None
}

fn find_destination_is_source(
    sources: &[TransferSource],
    destination: &TransferDestination,
) -> Option<TransferScan> {
    let dest_id = destination.block_id.as_ref()?;
    if !matches!(
        destination.kind,
        TransferDestinationKind::Task | TransferDestinationKind::Drawer
    ) {
        return None;
    }
    let dest_route = destination.route.to_ascii_lowercase();
    let hit = sources.iter().any(|s| {
        s.route.to_ascii_lowercase() == dest_route && s.block_id == *dest_id
    });
    if hit {
        return Some(TransferScan::Invalid {
            code: TRANSFER_DESTINATION_IS_SOURCE_CODE,
            message: TRANSFER_DESTINATION_IS_SOURCE_MESSAGE.to_string(),
            span: (destination.start, destination.end),
        });
    }
    None
}

enum TransferDestErr {
    Incomplete {
        need: TransferNeed,
        span: (usize, usize),
        message: String,
    },
    Invalid {
        code: &'static str,
        message: String,
        span: (usize, usize),
    },
}

fn dest_invalid(token: &Token<'_>) -> TransferDestErr {
    TransferDestErr::Invalid {
        code: TRANSFER_DESTINATION_MARKER_CODE,
        message: TRANSFER_DESTINATION_MARKER_MESSAGE.to_string(),
        span: (token.start, token.end),
    }
}

fn parse_destination_token(
    token: &Token<'_>,
) -> Result<TransferDestination, TransferDestErr> {
    let text = token.text;
    let span = (token.start, token.end);
    if text.starts_with("@@") {
        return Err(dest_invalid(token));
    }
    let Some(rest) = text.strip_prefix('@') else {
        return Err(dest_invalid(token));
    };
    if rest.is_empty() {
        return Err(TransferDestErr::Incomplete {
            need: TransferNeed::Destination,
            span,
            message: "transfer destination needs a route: @route".to_string(),
        });
    }
    if rest.starts_with('!') || rest.starts_with(':') || rest.starts_with('^') {
        return Err(dest_invalid(token));
    }
    if let Some(plus) = rest.find('+') {
        let route = &rest[..plus];
        let after_plus = &rest[plus + 1..];
        if route.is_empty() || !is_route_token(route) {
            return Err(dest_invalid(token));
        }
        if after_plus.is_empty() {
            return Err(TransferDestErr::Incomplete {
                need: TransferNeed::BlockId,
                span,
                message:
                    "transfer destination needs a block ID: @route+<block-id>"
                        .to_string(),
            });
        }
        if after_plus.contains(':')
            || after_plus.contains('^')
            || after_plus.contains('@')
        {
            return Err(dest_invalid(token));
        }
        if let Some(hash) = after_plus.find('#') {
            let block_id = &after_plus[..hash];
            let drawer_part = &after_plus[hash + 1..];
            if block_id.is_empty() || !is_block_id(block_id) {
                if block_id.is_empty() {
                    return Err(TransferDestErr::Incomplete {
                        need: TransferNeed::BlockId,
                        span,
                        message:
                            "transfer destination needs a block ID: @route+<block-id>"
                                .to_string(),
                    });
                }
                return Err(dest_invalid(token));
            }
            if text.ends_with('!') {
                return Err(dest_invalid(token));
            }
            if drawer_part.is_empty() {
                return Err(TransferDestErr::Incomplete {
                    need: TransferNeed::Drawer,
                    span,
                    message:
                        "transfer destination needs a drawer: @route+id#<drawer>"
                            .to_string(),
                });
            }
            if drawer_part.contains('!') {
                return Err(dest_invalid(token));
            }
            if opens_exact_name(drawer_part) {
                let Some(lex) =
                    lex_exact_name_after_hash(drawer_part, drawer_part.len())
                else {
                    return Err(dest_invalid(token));
                };
                if !lex.complete {
                    return Err(TransferDestErr::Incomplete {
                        need: TransferNeed::Drawer,
                        span,
                        message:
                            "transfer destination needs a drawer: @route+id#<drawer>"
                                .to_string(),
                    });
                }
                let Some(canonical) =
                    crate::native::capture_drawers::normalize_drawer_literal(
                        &lex.decoded,
                    )
                else {
                    return Err(dest_invalid(token));
                };
                if drawer_part.len() != lex.literal_len {
                    return Err(dest_invalid(token));
                }
                return Ok(TransferDestination {
                    marker: text.to_string(),
                    route: route.to_ascii_lowercase(),
                    block_id: Some(block_id.to_string()),
                    drawer: Some(canonical),
                    drawer_exact: true,
                    section: None,
                    section_exact: false,
                    kind: TransferDestinationKind::Drawer,
                    start: token.start,
                    end: token.end,
                });
            }
            if !crate::native::capture_drawers::is_drawer_slug(drawer_part) {
                return Err(dest_invalid(token));
            }
            return Ok(TransferDestination {
                marker: text.to_string(),
                route: route.to_ascii_lowercase(),
                block_id: Some(block_id.to_string()),
                drawer: Some(drawer_part.to_string()),
                drawer_exact: false,
                section: None,
                section_exact: false,
                kind: TransferDestinationKind::Drawer,
                start: token.start,
                end: token.end,
            });
        }
        if after_plus.ends_with('!') {
            return Err(dest_invalid(token));
        }
        if !is_block_id(after_plus) {
            return Err(dest_invalid(token));
        }
        return Ok(TransferDestination {
            marker: text.to_string(),
            route: route.to_ascii_lowercase(),
            block_id: Some(after_plus.to_string()),
            drawer: None,
            drawer_exact: false,
            section: None,
            section_exact: false,
            kind: TransferDestinationKind::Task,
            start: token.start,
            end: token.end,
        });
    }
    if let Some(hash) = rest.find('#') {
        let route = &rest[..hash];
        let section_part = &rest[hash + 1..];
        if route.is_empty() || !is_route_token(route) {
            return Err(dest_invalid(token));
        }
        if section_part.is_empty() {
            return Err(TransferDestErr::Incomplete {
                need: TransferNeed::Section,
                span,
                message:
                    "transfer destination needs a section: @route#<section>"
                        .to_string(),
            });
        }
        if section_part.contains('@')
            || section_part.contains('+')
            || section_part.contains(':')
            || section_part.contains('^')
            || section_part.contains('!')
        {
            return Err(dest_invalid(token));
        }
        if opens_exact_name(section_part) {
            let Some(lex) =
                lex_exact_name_after_hash(section_part, section_part.len())
            else {
                return Err(dest_invalid(token));
            };
            if !lex.complete {
                return Err(TransferDestErr::Incomplete {
                    need: TransferNeed::Section,
                    span,
                    message:
                        "transfer destination needs a section: @route#<section>"
                            .to_string(),
                });
            }
            if section_part.len() != lex.literal_len {
                return Err(dest_invalid(token));
            }
            if lex.decoded.trim().is_empty() {
                return Err(dest_invalid(token));
            }
            return Ok(TransferDestination {
                marker: text.to_string(),
                route: route.to_ascii_lowercase(),
                block_id: None,
                drawer: None,
                drawer_exact: false,
                section: Some(lex.decoded),
                section_exact: true,
                kind: TransferDestinationKind::Section,
                start: token.start,
                end: token.end,
            });
        }
        return Ok(TransferDestination {
            marker: text.to_string(),
            route: route.to_ascii_lowercase(),
            block_id: None,
            drawer: None,
            drawer_exact: false,
            section: Some(section_part.to_string()),
            section_exact: false,
            kind: TransferDestinationKind::Section,
            start: token.start,
            end: token.end,
        });
    }
    if rest.contains(':')
        || rest.contains('^')
        || rest.contains('@')
        || rest.contains('!')
    {
        return Err(dest_invalid(token));
    }
    if !is_route_token(rest) {
        return Err(dest_invalid(token));
    }
    Ok(TransferDestination {
        marker: text.to_string(),
        route: rest.to_ascii_lowercase(),
        block_id: None,
        drawer: None,
        drawer_exact: false,
        section: None,
        section_exact: false,
        kind: TransferDestinationKind::Note,
        start: token.start,
        end: token.end,
    })
}
