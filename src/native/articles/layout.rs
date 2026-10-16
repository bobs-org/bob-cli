//! Layout rules F1–F8, F10, and F11.

use super::scan::{ClassifiedLine, Document, Region};
use super::sections::PlannedHeading;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkLine {
    pub(crate) content: String,
    pub(crate) ending: String,
    pub(crate) region: Region,
    pub(crate) drop: bool,
    pub(crate) heading: bool,
    pub(crate) fence_open: bool,
    pub(crate) fence_close: bool,
    pub(crate) input_index: usize,
}

pub(crate) fn apply(
    document: &Document<'_>,
    headings: &[PlannedHeading],
) -> Vec<WorkLine> {
    let mut work = Vec::with_capacity(document.lines.len());
    let heading_by_line: Vec<Option<&PlannedHeading>> = {
        let mut map = vec![None; document.lines.len()];
        for heading in headings {
            if heading.line < map.len() {
                map[heading.line] = Some(heading);
            }
        }
        map
    };
    let unclosed_from = document.unclosed_fence;

    for (index, line) in document.lines.iter().enumerate() {
        if unclosed_from.is_some_and(|open| index > open) {
            work.push(WorkLine {
                content: line.content.to_string(),
                ending: line.ending.to_string(),
                region: Region::UnclosedFence,
                drop: false,
                heading: false,
                fence_open: false,
                fence_close: false,
                input_index: index,
            });
            continue;
        }

        if line.region == Region::SetextUnderline
            && let Some(heading) = heading_for_underline(headings, index)
            && !heading.setext_multiline
        {
            work.push(WorkLine {
                content: String::new(),
                ending: line.ending.to_string(),
                region: line.region,
                drop: true,
                heading: false,
                fence_open: false,
                fence_close: false,
                input_index: index,
            });
            continue;
        }

        if let Some(heading) = heading_by_line.get(index).copied().flatten()
            && !heading.setext_multiline
        {
            work.push(WorkLine {
                content: heading.rewritten.clone(),
                ending: line.ending.to_string(),
                region: Region::AtxHeading,
                drop: false,
                heading: true,
                fence_open: false,
                fence_close: false,
                input_index: index,
            });
            continue;
        }

        let protected = line.region.is_protected_layout();
        let content = if protected || line.region == Region::Frontmatter {
            line.content.to_string()
        } else {
            line.content.trim_end_matches([' ', '\t']).to_string()
        };
        let fence_open = is_fence_open(document, index);
        let fence_close = is_fence_close(document, index);
        work.push(WorkLine {
            content,
            ending: line.ending.to_string(),
            region: line.region,
            drop: false,
            heading: false,
            fence_open,
            fence_close,
            input_index: index,
        });
    }
    work
}

fn heading_for_underline(
    headings: &[PlannedHeading],
    underline: usize,
) -> Option<&PlannedHeading> {
    headings
        .iter()
        .find(|heading| heading.underline == Some(underline))
}

fn is_top_level_fence_line(line: &ClassifiedLine<'_>) -> bool {
    line.region == Region::Fence
        && !line.content.starts_with(' ')
        && !line.content.starts_with('\t')
}

fn is_fence_open(document: &Document<'_>, index: usize) -> bool {
    let line = &document.lines[index];
    if !is_top_level_fence_line(line) {
        return false;
    }
    index == 0 || document.lines[index - 1].region != Region::Fence
}

fn is_fence_close(document: &Document<'_>, index: usize) -> bool {
    let line = &document.lines[index];
    if !is_top_level_fence_line(line) || document.unclosed_fence == Some(index)
    {
        return false;
    }
    document
        .lines
        .get(index + 1)
        .is_none_or(|next| next.region != Region::Fence)
}

pub(crate) fn emit(
    document: &Document<'_>,
    work: &[WorkLine],
) -> Vec<(String, String)> {
    let default = document.default_ending;
    let fm_end = work
        .iter()
        .rposition(|line| line.region == Region::Frontmatter);
    let mut out: Vec<(String, String)> = Vec::new();

    if let Some(end) = fm_end {
        for line in &work[..=end] {
            out.push((line.content.clone(), ending_of(line, default)));
        }
        let body = &work[end + 1..];
        out.push((String::new(), default.to_string()));
        emit_body(&mut out, body, default, true);
    } else {
        emit_body(&mut out, work, default, false);
    }

    if document.unclosed_fence.is_none() {
        while out
            .last()
            .is_some_and(|(content, _)| content.is_empty() && out.len() > 1)
        {
            let fm_only = fm_end.is_some() && out.len() <= fm_end.unwrap() + 2;
            if fm_only {
                break;
            }
            out.pop();
        }
        if out.is_empty() {
            out.push((String::new(), default.to_string()));
        } else if out.last().is_some_and(|(_, ending)| ending.is_empty()) {
            let last = out.last_mut().expect("non-empty");
            last.1 = default.to_string();
        }
    }
    out
}

fn emit_body(
    out: &mut Vec<(String, String)>,
    body: &[WorkLine],
    default: &str,
    after_frontmatter: bool,
) {
    let mut i = 0;
    let mut first_body = true;
    let mut last_kind = LastKind::None;
    let mut last_is_blank = after_frontmatter && !out.is_empty();
    while i < body.len() {
        if body[i].drop {
            i += 1;
            continue;
        }
        if body[i].region == Region::UnclosedFence {
            ensure_spacing(
                out,
                last_kind,
                last_is_blank,
                &body[i],
                first_body,
                default,
            );
            while i < body.len() {
                if !body[i].drop {
                    out.push((
                        body[i].content.clone(),
                        ending_of(&body[i], default),
                    ));
                }
                i += 1;
            }
            break;
        }
        if body[i].region.is_protected_layout()
            && body[i].region != Region::Frontmatter
            && !body[i].fence_open
            && !body[i].fence_close
        {
            let mut j = i;
            while j < body.len()
                && body[j].region.is_protected_layout()
                && !body[j].fence_open
                && !body[j].fence_close
                && body[j].region != Region::UnclosedFence
            {
                j += 1;
            }
            for line in &body[i..j] {
                if line.drop {
                    continue;
                }
                out.push((line.content.clone(), ending_of(line, default)));
                first_body = false;
                last_kind = LastKind::Other;
                last_is_blank = is_blank_work(line);
            }
            i = j;
            continue;
        }
        if is_blank_work(&body[i]) {
            let mut j = i;
            while j < body.len() && (body[j].drop || is_blank_work(&body[j])) {
                j += 1;
            }
            let run = body[i..j]
                .iter()
                .filter(|line| !line.drop && is_blank_work(line))
                .count();
            let next = body[j..]
                .iter()
                .find(|line| !line.drop && !is_blank_work(line));
            let keep = match (last_kind, next) {
                (LastKind::List, Some(line)) if line.region.is_listy() => run,
                _ if first_body => 0,
                _ if run > 0 => 1,
                _ => 0,
            };
            for _ in 0..keep {
                out.push((String::new(), default.to_string()));
                first_body = false;
                last_is_blank = true;
            }
            i = j;
            continue;
        }

        ensure_spacing(
            out,
            last_kind,
            last_is_blank,
            &body[i],
            first_body,
            default,
        );
        out.push((body[i].content.clone(), ending_of(&body[i], default)));
        last_kind = kind_of(&body[i]);
        last_is_blank = false;
        first_body = false;
        i += 1;
    }
    let _ = after_frontmatter;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastKind {
    None,
    Heading,
    FenceOpen,
    FenceClose,
    List,
    Blank,
    Other,
}

fn kind_of(line: &WorkLine) -> LastKind {
    if line.heading {
        LastKind::Heading
    } else if line.fence_open {
        LastKind::FenceOpen
    } else if line.fence_close {
        LastKind::FenceClose
    } else if line.region.is_listy() {
        LastKind::List
    } else if is_blank_work(line) {
        LastKind::Blank
    } else {
        LastKind::Other
    }
}

fn ensure_spacing(
    out: &mut Vec<(String, String)>,
    last_kind: LastKind,
    last_is_blank: bool,
    next: &WorkLine,
    first_body: bool,
    default: &str,
) {
    if first_body {
        return;
    }
    let needs = next.heading
        || (next.fence_open && !next.region.is_listy())
        || last_kind == LastKind::Heading
        || last_kind == LastKind::FenceClose;
    if needs && !last_is_blank && !out.is_empty() {
        out.push((String::new(), default.to_string()));
    }
}

fn is_blank_work(line: &WorkLine) -> bool {
    line.region == Region::Blank
        || line.content.trim().is_empty() && !line.heading
}

fn ending_of(line: &WorkLine, default: &str) -> String {
    if line.ending.is_empty() {
        default.to_string()
    } else {
        line.ending.clone()
    }
}

pub(crate) fn join(
    bom: bool,
    lines: &[(String, String)],
    unclosed: bool,
    default: &str,
) -> String {
    let mut out = String::new();
    if bom {
        out.push('\u{feff}');
    }
    if lines.is_empty() {
        if !unclosed {
            out.push_str(default);
        }
        return out;
    }
    for (index, (content, ending)) in lines.iter().enumerate() {
        out.push_str(content);
        let last = index + 1 == lines.len();
        if last && unclosed {
            out.push_str(ending);
        } else {
            out.push_str(if ending.is_empty() { default } else { ending });
        }
    }
    out
}

pub(crate) fn layout_change_count(
    input: &[ClassifiedLine<'_>],
    output: &[(String, String)],
    headings: &[PlannedHeading],
) -> usize {
    let heading_lines: Vec<usize> = headings.iter().map(|h| h.line).collect();
    let old: Vec<&str> = input.iter().map(|l| l.content).collect();
    let new: Vec<&str> = output.iter().map(|(c, _)| c.as_str()).collect();
    let mut count = 0;
    for op in
        similar::capture_diff_slices(similar::Algorithm::Myers, &old, &new)
    {
        match op {
            similar::DiffOp::Equal { .. } => {}
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for offset in 0..old_len {
                    if !heading_lines.contains(&(old_index + offset)) {
                        count += 1;
                    }
                }
            }
            similar::DiffOp::Insert { new_len, .. } => count += new_len,
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_len,
                ..
            } => {
                let heading_hit = (0..old_len).any(|offset| {
                    heading_lines.contains(&(old_index + offset))
                });
                if heading_hit {
                    count +=
                        new_len.saturating_sub(old_len.max(1).min(new_len));
                } else {
                    count += old_len.max(new_len);
                }
            }
        }
    }
    count
}
