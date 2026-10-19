//! Continuation-marker grammar, grouping plan, and quote assembly.
use super::*;
use std::borrow::Cow;

const ELLIPSIS: char = '\u{2026}';
const EN_DASH: char = '\u{2013}';
const GAP_TOKEN_ELLIPSIS: &str = "<\u{2026}";
const GAP_TOKEN_DOTS: &str = "<...";
const GAP_JOIN: &str = " [\u{2026}] ";
const ASCII_GAP_JOIN: &str = " [...] ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContinuationToken {
    Join,
    Gap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Seam {
    Join,
    Gap,
}

/// Grammar result for one highlight's `task_source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MarkerScan {
    None,
    Escaped {
        task_source: String,
    },
    Marker {
        token: ContinuationToken,
        task_source: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ContinuationPlan {
    /// head order -> followers (order + seam before it), in sidecar order
    pub(super) followers: BTreeMap<usize, Vec<(usize, Seam)>>,
    /// absorbed follower order -> head order
    pub(super) head_of: BTreeMap<usize, usize>,
    /// effective (task_source, comment) for merged followers and escapes
    pub(super) effective: BTreeMap<usize, (Option<String>, Option<String>)>,
    /// orders of highlights whose marker could not merge, in sidecar order
    pub(super) dangling: Vec<usize>,
}

impl ContinuationPlan {
    pub(super) fn effective_annotation<'a>(
        &'a self,
        annotation: &'a SidecarAnnotation,
    ) -> Cow<'a, SidecarAnnotation> {
        match self.effective.get(&annotation.order) {
            Some((task_source, comment)) => {
                let mut rewritten = annotation.clone();
                rewritten.task_source = task_source.clone();
                rewritten.comment = comment.clone();
                Cow::Owned(rewritten)
            }
            None => Cow::Borrowed(annotation),
        }
    }
}

pub(super) fn scan_continuation_marker(task_source: &str) -> MarkerScan {
    let Some(line) = first_nonempty_line(task_source) else {
        return MarkerScan::None;
    };
    let (leading_ws, after_ws) = split_leading_whitespace(line.content);
    let (list_prefix, body) = split_list_prefix(after_ws);
    let prefix = format!("{leading_ws}{list_prefix}");

    if let Some((token, token_len)) = continuation_token_at(body) {
        let after = body[token_len..].trim_start();
        let task_source = reconstruct_task_source(
            task_source,
            &line,
            if after.is_empty() {
                None
            } else {
                Some(format!("{prefix}{after}"))
            },
        );
        return MarkerScan::Marker { token, task_source };
    }

    if let Some(rest) = body.strip_prefix('\\')
        && continuation_token_at(rest).is_some()
    {
        let task_source = reconstruct_task_source(
            task_source,
            &line,
            Some(format!("{prefix}{rest}")),
        )
        .expect("escaped marker keeps the unescaped first line");
        return MarkerScan::Escaped { task_source };
    }

    MarkerScan::None
}

pub(super) fn plan_continuations(
    annotations: &[SidecarAnnotation],
) -> ContinuationPlan {
    let mut plan = ContinuationPlan::default();
    let mut last_text: Option<&SidecarAnnotation> = None;

    for annotation in annotations {
        if !is_text_highlight(annotation) {
            continue;
        }
        let scan = annotation
            .task_source
            .as_deref()
            .map(scan_continuation_marker)
            .unwrap_or(MarkerScan::None);
        match scan {
            MarkerScan::Escaped { task_source } => {
                let comment = comment_from_task_source(Some(&task_source));
                let stored = (!is_blank(&task_source)).then_some(task_source);
                plan.effective.insert(annotation.order, (stored, comment));
            }
            MarkerScan::Marker { token, task_source } => {
                if let Some(previous) = last_text {
                    let head_order = plan
                        .head_of
                        .get(&previous.order)
                        .copied()
                        .unwrap_or(previous.order);
                    let seam = seam_between(previous, annotation, token);
                    plan.followers
                        .entry(head_order)
                        .or_default()
                        .push((annotation.order, seam));
                    plan.head_of.insert(annotation.order, head_order);
                    let comment =
                        comment_from_task_source(task_source.as_deref());
                    plan.effective
                        .insert(annotation.order, (task_source, comment));
                    last_text = Some(annotation);
                    continue;
                }
                plan.dangling.push(annotation.order);
            }
            MarkerScan::None => {}
        }
        last_text = Some(annotation);
    }
    plan
}

pub(super) fn assemble_group_quote(pieces: &[(&str, Seam)]) -> String {
    if pieces.is_empty() {
        return String::new();
    }
    let mut runs = Vec::new();
    let mut current = pieces[0].0.to_string();
    for (text, seam) in &pieces[1..] {
        match seam {
            Seam::Join => {
                current.push('\n');
                current.push_str(text);
            }
            Seam::Gap => {
                runs.push(beautify_annotation_text(&current));
                current = (*text).to_string();
            }
        }
    }
    runs.push(beautify_annotation_text(&current));
    join_quote_runs(&runs)
}

pub(super) fn group_page_label(labels: &[Option<&str>]) -> Option<String> {
    let head = *labels.first()?;
    let head = head?;
    let mut distinct = Vec::new();
    for label in labels {
        let Some(label) = *label else {
            return Some(head.to_string());
        };
        if !distinct.iter().any(|seen| *seen == label) {
            distinct.push(label);
        }
    }
    if distinct.len() == 1 {
        return Some(distinct[0].to_string());
    }
    let mut numbers = Vec::new();
    for label in &distinct {
        let Some(number) = label_page_number(label) else {
            return Some(head.to_string());
        };
        numbers.push(number);
    }
    numbers.sort_unstable();
    numbers.dedup();
    let compressed = compress_page_ranges(&numbers);
    let prefix = if uses_short_page_prefix(head) {
        "pp. "
    } else {
        "Pages "
    };
    Some(format!("{prefix}{compressed}"))
}

pub(super) fn is_text_highlight(annotation: &SidecarAnnotation) -> bool {
    annotation.kind == SidecarAnnotationKind::Highlight
        && !is_sidecar_marker_mirror(annotation)
}

fn seam_between(
    previous: &SidecarAnnotation,
    next: &SidecarAnnotation,
    token: ContinuationToken,
) -> Seam {
    if token == ContinuationToken::Gap {
        return Seam::Gap;
    }
    match (previous.page_index, next.page_index) {
        (Some(prev), Some(next)) if next == prev + 1 => Seam::Join,
        _ => Seam::Gap,
    }
}

fn comment_from_task_source(task_source: Option<&str>) -> Option<String> {
    let task_source = task_source?;
    let comment = strip_comment_list_markers(task_source);
    (!comment.is_empty()).then_some(comment)
}

fn continuation_token_at(body: &str) -> Option<(ContinuationToken, usize)> {
    if let Some(len) = token_len_if_boundary(body, GAP_TOKEN_ELLIPSIS) {
        return Some((ContinuationToken::Gap, len));
    }
    if let Some(len) = token_len_if_boundary(body, GAP_TOKEN_DOTS) {
        return Some((ContinuationToken::Gap, len));
    }
    if let Some(len) = token_len_if_boundary(body, "<") {
        return Some((ContinuationToken::Join, len));
    }
    None
}

fn token_len_if_boundary(body: &str, token: &str) -> Option<usize> {
    let rest = body.strip_prefix(token)?;
    match rest.chars().next() {
        None => Some(token.len()),
        Some(character) if character.is_whitespace() => Some(token.len()),
        Some(_) => None,
    }
}

struct FirstNonemptyLine<'a> {
    content: &'a str,
    prefix_end: usize,
    terminator: &'a str,
    suffix: &'a str,
}

fn first_nonempty_line(text: &str) -> Option<FirstNonemptyLine<'_>> {
    let mut offset = 0usize;
    while offset < text.len() {
        let rest = &text[offset..];
        let (content, terminator, consumed) = split_one_line(rest);
        let next = offset + consumed;
        if !content.trim().is_empty() {
            return Some(FirstNonemptyLine {
                content,
                prefix_end: offset,
                terminator,
                suffix: &text[next..],
            });
        }
        offset = next;
        if consumed == 0 {
            break;
        }
    }
    None
}

fn split_one_line(text: &str) -> (&str, &str, usize) {
    if let Some(index) = text.find('\n') {
        let including_nl = &text[..=index];
        if including_nl.ends_with("\r\n") {
            (&text[..index - 1], "\r\n", index + 1)
        } else {
            (&text[..index], "\n", index + 1)
        }
    } else {
        (text, "", text.len())
    }
}

fn split_leading_whitespace(line: &str) -> (&str, &str) {
    let trimmed = line.trim_start();
    let prefix_len = line.len() - trimmed.len();
    (&line[..prefix_len], trimmed)
}

fn split_list_prefix(after_ws: &str) -> (&str, &str) {
    match strip_unordered_list_marker(after_ws) {
        Some(item) => {
            let prefix_len = after_ws.len() - item.len();
            (&after_ws[..prefix_len], item)
        }
        None => ("", after_ws),
    }
}

fn reconstruct_task_source(
    original: &str,
    line: &FirstNonemptyLine<'_>,
    first_line: Option<String>,
) -> Option<String> {
    let prefix = &original[..line.prefix_end];
    let reconstructed = match first_line {
        None => format!("{prefix}{}", line.suffix),
        Some(first) => {
            let mut out = String::from(prefix);
            out.push_str(&first);
            if !line.suffix.is_empty() {
                out.push_str(line.terminator);
                out.push_str(line.suffix);
            }
            out
        }
    };
    (!is_blank(&reconstructed)).then_some(reconstructed)
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

fn join_quote_runs(runs: &[String]) -> String {
    let Some(first) = runs.first() else {
        return String::new();
    };
    let mut joined = first.clone();
    for run in &runs[1..] {
        if run_has_gap_suffix(&joined) || run_has_gap_prefix(run) {
            joined = format!("{} {run}", joined.trim_end());
        } else {
            joined.push_str(GAP_JOIN);
            joined.push_str(run);
        }
    }
    joined
}

fn run_has_gap_suffix(run: &str) -> bool {
    let trimmed = run.trim_end();
    trimmed.ends_with(&GAP_JOIN[1..GAP_JOIN.len() - 1])
        || trimmed.ends_with(&ASCII_GAP_JOIN[1..ASCII_GAP_JOIN.len() - 1])
}

fn run_has_gap_prefix(run: &str) -> bool {
    let trimmed = run.trim_start();
    trimmed.starts_with(&format!("[{ELLIPSIS}]"))
        || trimmed.starts_with("[...]")
}

fn compress_page_ranges(numbers: &[u32]) -> String {
    let Some((first, rest)) = numbers.split_first() else {
        return String::new();
    };
    let mut parts = Vec::new();
    let mut start = *first;
    let mut prev = *first;
    for &number in rest {
        if number == prev + 1 {
            prev = number;
            continue;
        }
        parts.push(format_page_run(start, prev));
        start = number;
        prev = number;
    }
    parts.push(format_page_run(start, prev));
    parts.join(", ")
}

fn format_page_run(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}{EN_DASH}{end}")
    }
}

fn uses_short_page_prefix(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower.starts_with("p. ") || lower.starts_with("p ")
}
