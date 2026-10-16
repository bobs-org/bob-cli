//! Lossless same-note heading-link refresh.

use std::path::Path;

use super::findings::Finding;
use super::scan::{ClassifiedLine, Region};
use super::sections::{
    heading_identity, heading_plain_identity, strip_section_number,
    PlannedHeading,
};
use crate::native::highlights_ref::note_snapshot::{
    parse_link_target, LinkFragment,
};
use crate::native::vault_links::NoteIndex;

#[derive(Debug, Default)]
pub(crate) struct LinkPlan {
    pub(crate) line_replacements: Vec<Option<String>>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) refreshed: usize,
}

#[derive(Debug, Clone, Copy)]
struct LinkSpan {
    target_start: usize,
    target_end: usize,
    wikilink: bool,
}

#[derive(Debug, Clone, Copy)]
struct FragmentSpan {
    start: usize,
    end: usize,
    markdown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolveFailure {
    Broken,
    Ambiguous,
}

pub(crate) fn refresh(
    lines: &[ClassifiedLine<'_>],
    headings: &[PlannedHeading],
    path: &Path,
    numbering: bool,
) -> LinkPlan {
    let mut plan = LinkPlan {
        line_replacements: vec![None; lines.len()],
        ..LinkPlan::default()
    };
    for line in lines {
        if protects_links(line.region) {
            continue;
        }
        let mut updated = line.content.to_owned();
        let mut line_findings = Vec::new();
        let spans = link_spans(line.content);
        for span in spans.into_iter().rev() {
            let target = &line.content[span.target_start..span.target_end];
            let target_content = if span.wikilink {
                target.split_once('|').map_or(target, |(before, _)| before)
            } else {
                markdown_target(target)
            };
            let parsed = parse_link_target(target_content);
            let Some(fragment) = parsed.fragment.as_ref() else {
                continue;
            };
            let LinkFragment::Heading(fragment) = fragment else {
                continue;
            };
            if !is_self_note(&parsed.note, path) {
                continue;
            }
            let Some(fragment_span) =
                fragment_span(target_content, span.wikilink)
            else {
                continue;
            };
            let decoded = percent_decode(
                &target_content[fragment_span.start..fragment_span.end],
            );
            let target_headings = match resolve_heading(&decoded, headings) {
                Ok(headings) => headings,
                Err(ResolveFailure::Broken) => {
                    line_findings.push((
                        span.target_start,
                        Finding::warning(
                            "broken-heading-link",
                            line.index + 1,
                            format!(
                                "heading link #{fragment} matches no section"
                            ),
                            "Fix the link or the heading",
                        ),
                    ));
                    continue;
                }
                Err(ResolveFailure::Ambiguous) => {
                    line_findings.push((
                        span.target_start,
                        Finding::warning(
                            "ambiguous-heading-link",
                            line.index + 1,
                            format!("heading link #{fragment} matches multiple sections"),
                            "Fix the link or the heading",
                        ),
                    ));
                    continue;
                }
            };
            let raw_fragment =
                &target_content[fragment_span.start..fragment_span.end];
            let replacement = rewrite_fragment_path(
                raw_fragment,
                target_headings
                    .iter()
                    .map(|heading| {
                        heading.number.as_deref().filter(|_| numbering)
                    })
                    .collect(),
                fragment_span.markdown,
            );
            if replacement == raw_fragment {
                continue;
            }
            let target_replacement = format!(
                "{}{}{}",
                &target_content[..fragment_span.start],
                replacement,
                &target_content[fragment_span.end..]
            );
            let target_replacement = if span.wikilink {
                match target.find('|') {
                    Some(alias) => {
                        format!("{}{}", target_replacement, &target[alias..])
                    }
                    None => target_replacement,
                }
            } else {
                replace_markdown_target(target, &target_replacement)
            };
            updated.replace_range(
                span.target_start..span.target_end,
                &target_replacement,
            );
            plan.refreshed += 1;
        }
        if updated != line.content {
            plan.line_replacements[line.index] = Some(updated);
        }
        line_findings.sort_by_key(|(position, _)| *position);
        plan.findings
            .extend(line_findings.into_iter().map(|(_, finding)| finding));
    }
    plan
}

pub(crate) fn strip_number_tokens_for_words(text: &str) -> String {
    let mut output = text.to_owned();
    for span in link_spans(text).into_iter().rev() {
        let target = &text[span.target_start..span.target_end];
        let target_content = if span.wikilink {
            target.split_once('|').map_or(target, |(before, _)| before)
        } else {
            markdown_target(target)
        };
        let Some(fragment_span) = fragment_span(target_content, span.wikilink)
        else {
            continue;
        };
        let fragment = &target_content[fragment_span.start..fragment_span.end];
        let replacement = rewrite_fragment_path(
            fragment,
            vec![None; fragment.split('#').count()],
            fragment_span.markdown,
        );
        if replacement == fragment {
            continue;
        }
        let target_replacement = format!(
            "{}{}{}",
            &target_content[..fragment_span.start],
            replacement,
            &target_content[fragment_span.end..]
        );
        let target_replacement = if span.wikilink {
            match target.find('|') {
                Some(alias) => {
                    format!("{}{}", target_replacement, &target[alias..])
                }
                None => target_replacement,
            }
        } else {
            replace_markdown_target(target, &target_replacement)
        };
        output.replace_range(
            span.target_start..span.target_end,
            &target_replacement,
        );
    }
    output
}

fn protects_links(region: Region) -> bool {
    matches!(
        region,
        Region::Frontmatter
            | Region::Fence
            | Region::UnclosedFence
            | Region::Math
            | Region::PercentComment
            | Region::HtmlComment
            | Region::HtmlBlock
            | Region::IndentedCode
    )
}

fn link_spans(line: &str) -> Vec<LinkSpan> {
    let inline_mask =
        super::scan::inline_mask(line, super::scan::InlineMaskMode::Links);
    let mut masked = line.as_bytes().to_vec();
    for (byte, masked) in masked.iter_mut().zip(inline_mask) {
        if masked {
            *byte = b' ';
        }
    }
    let mut spans = Vec::new();
    let mut i = 0;
    while i + 1 < masked.len() {
        if super::scan::is_escaped(line.as_bytes(), i) {
            i += 1;
            continue;
        }
        if masked[i..].starts_with(b"![[")
            && let Some(relative_end) = find_bytes(&masked[i + 3..], b"]]")
        {
            let end = i + 3 + relative_end + 2;
            spans.push(LinkSpan {
                target_start: i + 3,
                target_end: end - 2,
                wikilink: true,
            });
            i = end;
            continue;
        }
        if masked[i..].starts_with(b"[[")
            && let Some(relative_end) = find_bytes(&masked[i + 2..], b"]]")
        {
            let end = i + 2 + relative_end + 2;
            spans.push(LinkSpan {
                target_start: i + 2,
                target_end: end - 2,
                wikilink: true,
            });
            i = end;
            continue;
        }
        let open = if masked[i..].starts_with(b"![") {
            i + 1
        } else if masked[i] == b'[' {
            i
        } else {
            i += 1;
            continue;
        };
        let Some(label_end) = find_unescaped(&masked, open + 1, b']') else {
            i += 1;
            continue;
        };
        if masked.get(label_end + 1) != Some(&b'(') {
            i = label_end + 1;
            continue;
        }
        let Some(target_end) = find_markdown_close(&masked, label_end + 2)
        else {
            i = label_end + 2;
            continue;
        };
        let mut target_start = label_end + 2;
        let mut actual_end = target_end;
        if masked.get(target_start) == Some(&b'<')
            && masked.get(actual_end.saturating_sub(1)) == Some(&b'>')
        {
            target_start += 1;
            actual_end -= 1;
        } else if let Some(space) = masked[target_start..actual_end]
            .iter()
            .position(u8::is_ascii_whitespace)
        {
            actual_end = target_start + space;
        }
        if target_start < actual_end {
            spans.push(LinkSpan {
                target_start,
                target_end: actual_end,
                wikilink: false,
            });
        }
        i = target_end + 1;
    }
    spans
}

fn find_markdown_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 1_usize;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = (i + 2).min(bytes.len());
            continue;
        }
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_unescaped(bytes: &[u8], start: usize, needle: u8) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = (i + 2).min(bytes.len());
        } else if bytes[i] == needle {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn markdown_target(target: &str) -> &str {
    let trimmed = target.trim();
    trimmed
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(trimmed)
}

fn replace_markdown_target(original: &str, replacement: &str) -> String {
    let trimmed = original.trim();
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        format!("<{}>", replacement)
    } else if let Some(space) = original.find(char::is_whitespace) {
        format!("{}{}", replacement, &original[space..])
    } else {
        replacement.to_owned()
    }
}

fn fragment_span(target: &str, wikilink: bool) -> Option<FragmentSpan> {
    let body_end = if wikilink {
        target.find('|').unwrap_or(target.len())
    } else {
        target.len()
    };
    let body = &target[..body_end];
    let hash = body.find('#')?;
    let start = hash + 1;
    if body[start..].starts_with('^') || start == body_end {
        return None;
    }
    Some(FragmentSpan {
        start,
        end: body_end,
        markdown: !wikilink,
    })
}

fn is_self_note(note: &str, path: &Path) -> bool {
    if note.trim().is_empty() {
        return true;
    }
    let normalize = |value: &str| {
        let value = value.trim().replace('\\', "/");
        let value = value.trim_start_matches("./");
        value
            .strip_suffix(".md")
            .or_else(|| value.strip_suffix(".MD"))
            .unwrap_or(value)
            .to_lowercase()
    };
    let note = normalize(&percent_decode(note));
    let path_text = path.to_string_lossy().replace('\\', "/");
    let path_without_extension = path_text
        .strip_suffix(".md")
        .or_else(|| path_text.strip_suffix(".MD"))
        .unwrap_or(&path_text);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    note == normalize(path_without_extension) || note == normalize(stem)
}

fn resolve_heading<'a>(
    fragment: &str,
    headings: &'a [PlannedHeading],
) -> Result<Vec<&'a PlannedHeading>, ResolveFailure> {
    let segments = fragment.split('#').collect::<Vec<_>>();
    if segments.iter().any(|segment| segment.trim().is_empty()) {
        return Err(ResolveFailure::Broken);
    }
    let mut candidates: Vec<usize> = (0..headings.len()).collect();
    let mut selected = Vec::with_capacity(segments.len());
    for (segment_index, segment) in segments.iter().enumerate() {
        let segment = percent_decode(segment);
        let exact = candidates
            .iter()
            .copied()
            .filter(|index| exact_heading(&headings[*index], &segment))
            .collect::<Vec<_>>();
        let found = if let Some(first) = exact.first() {
            *first
        } else {
            let matches = candidates
                .iter()
                .copied()
                .filter(|index| identity_heading(&headings[*index], &segment))
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => return Err(ResolveFailure::Broken),
                [only] => *only,
                _ => return Err(ResolveFailure::Ambiguous),
            }
        };
        selected.push(&headings[found]);
        if segment_index + 1 < segments.len() {
            let parent_level = headings[found].new_level;
            candidates = (found + 1..headings.len())
                .take_while(|index| headings[*index].new_level > parent_level)
                .collect();
        }
    }
    if selected.is_empty() {
        Err(ResolveFailure::Broken)
    } else {
        Ok(selected)
    }
}

fn exact_heading(heading: &PlannedHeading, fragment: &str) -> bool {
    let raw = if let Some(written) = &heading.written {
        format!("{written} {}", heading.title)
    } else {
        heading.title.clone()
    };
    normalize(&raw) == normalize(fragment)
}

fn identity_heading(heading: &PlannedHeading, fragment: &str) -> bool {
    let raw = if let Some(written) = &heading.written {
        format!("{written} {}", heading.title)
    } else {
        heading.title.clone()
    };
    heading_identity(&raw) == heading_identity(fragment)
        || heading_plain_identity(&raw) == heading_plain_identity(fragment)
}

fn normalize(text: &str) -> String {
    super::sections::normalize_identity(text)
}

fn rewrite_fragment(
    fragment: &str,
    number: Option<&str>,
    markdown: bool,
) -> String {
    let (decoded, raw_ends) = percent_decode_with_ends(fragment);
    let stripped = strip_section_number(&decoded);
    let Some(token) = stripped.token else {
        return number.map_or_else(
            || fragment.to_owned(),
            |number| {
                let separator = if markdown { "%20" } else { " " };
                format!("{number}{separator}{fragment}")
            },
        );
    };
    let token_len = token.len();
    let raw_token_end = raw_ends
        .get(token_len.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    if let Some(number) = number {
        if token == number {
            return fragment.to_owned();
        }
        format!("{number}{}", &fragment[raw_token_end..])
    } else {
        let after_token = &decoded[token_len..];
        let trimmed = after_token.trim_start();
        let consumed = after_token.len() - trimmed.len();
        let decoded_end = token_len + consumed;
        let raw_end = raw_ends
            .get(decoded_end.saturating_sub(1))
            .copied()
            .unwrap_or(raw_token_end);
        fragment[raw_end..].to_owned()
    }
}

fn rewrite_fragment_path(
    fragment: &str,
    numbers: Vec<Option<&str>>,
    markdown: bool,
) -> String {
    fragment
        .split('#')
        .zip(numbers)
        .map(|(segment, number)| rewrite_fragment(segment, number, markdown))
        .collect::<Vec<_>>()
        .join("#")
}

fn percent_decode(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let raw = text.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let (Some(high), Some(low)) = (hex(raw[i + 1]), hex(raw[i + 2]))
        {
            bytes.push(high * 16 + low);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| text.to_owned())
}

fn percent_decode_with_ends(text: &str) -> (String, Vec<usize>) {
    let raw = text.as_bytes();
    let mut decoded = Vec::with_capacity(raw.len());
    let mut ends = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let (Some(high), Some(low)) = (hex(raw[i + 1]), hex(raw[i + 2]))
        {
            decoded.push(high * 16 + low);
            ends.push(i + 3);
            i += 3;
        } else {
            decoded.push(raw[i]);
            ends.push(i + 1);
            i += 1;
        }
    }
    let text = String::from_utf8(decoded).unwrap_or_else(|_| text.to_owned());
    (text, ends)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// One article whose heading fragments other notes may need rewritten.
pub(crate) struct InboundArticle<'a> {
    pub index: usize,
    pub path: &'a Path,
    pub headings: &'a [PlannedHeading],
    pub numbering: bool,
}

#[derive(Debug, Default)]
pub(crate) struct InboundLineResult {
    pub updated: Option<String>,
    pub findings: Vec<(usize, Finding)>,
    pub refreshed: usize,
}

/// Refresh heading fragments in a foreign note that resolve to `articles`.
/// Same-note links and protected regions are left alone.
pub(crate) fn rewrite_inbound_line(
    content: &str,
    region: Region,
    current_path: &Path,
    line: usize,
    index: &NoteIndex,
    articles: &[InboundArticle<'_>],
) -> InboundLineResult {
    let mut result = InboundLineResult::default();
    if protects_links(region) || articles.is_empty() {
        return result;
    }
    let mut updated = content.to_owned();
    let mut line_findings = Vec::new();
    let spans = link_spans(content);
    for span in spans.into_iter().rev() {
        let target = &content[span.target_start..span.target_end];
        let target_content = if span.wikilink {
            target.split_once('|').map_or(target, |(before, _)| before)
        } else {
            markdown_target(target)
        };
        let parsed = parse_link_target(target_content);
        let Some(fragment) = parsed.fragment.as_ref() else {
            continue;
        };
        let LinkFragment::Heading(_) = fragment else {
            continue;
        };
        if parsed.note.trim().is_empty() {
            continue;
        }
        let note = percent_decode(&parsed.note);
        let Some(resolved) = index.resolve(Some(current_path), &note) else {
            continue;
        };
        if resolved == current_path {
            continue;
        }
        let Some(article) = articles
            .iter()
            .find(|article| article.path == resolved.as_path())
        else {
            continue;
        };
        let Some(fragment_span) = fragment_span(target_content, span.wikilink)
        else {
            continue;
        };
        let decoded = percent_decode(
            &target_content[fragment_span.start..fragment_span.end],
        );
        let target_headings = match resolve_heading(&decoded, article.headings)
        {
            Ok(headings) => headings,
            Err(failure) => {
                let loc = source_loc(current_path, line);
                let message = match failure {
                    ResolveFailure::Broken => format!(
                        "{loc} links to a heading this article no longer has"
                    ),
                    ResolveFailure::Ambiguous => format!(
                        "{loc} links to a heading that matches multiple sections"
                    ),
                };
                line_findings.push((
                    span.target_start,
                    article.index,
                    Finding::warning(
                        "inbound-heading-link",
                        1,
                        message,
                        "Repoint the link",
                    ),
                ));
                continue;
            }
        };
        let raw_fragment =
            &target_content[fragment_span.start..fragment_span.end];
        let replacement = rewrite_fragment_path(
            raw_fragment,
            target_headings
                .iter()
                .map(|heading| {
                    heading.number.as_deref().filter(|_| article.numbering)
                })
                .collect(),
            fragment_span.markdown,
        );
        if replacement == raw_fragment {
            continue;
        }
        let target_replacement = format!(
            "{}{}{}",
            &target_content[..fragment_span.start],
            replacement,
            &target_content[fragment_span.end..]
        );
        let target_replacement = if span.wikilink {
            match target.find('|') {
                Some(alias) => {
                    format!("{}{}", target_replacement, &target[alias..])
                }
                None => target_replacement,
            }
        } else {
            replace_markdown_target(target, &target_replacement)
        };
        updated.replace_range(
            span.target_start..span.target_end,
            &target_replacement,
        );
        result.refreshed += 1;
    }
    if updated != content {
        result.updated = Some(updated);
    }
    line_findings.sort_by_key(|(position, _, _)| *position);
    result.findings.extend(
        line_findings
            .into_iter()
            .map(|(_, article, finding)| (article, finding)),
    );
    result
}

pub(crate) fn source_loc(path: &Path, line: usize) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if line == 0 {
        path
    } else {
        format!("{path}:{line}")
    }
}
