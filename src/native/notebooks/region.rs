//! Strict byte-span handling for the Bob-owned `## Notebooks` region.
//!
//! The managed block is fenced by exactly one begin/end marker pair. Bob
//! owns the bytes between the markers; the position of the block, its
//! heading, and every other byte belong to the note's author. Edits are
//! computed as byte-range replacements over the original text, so a UTF-8
//! BOM, LF or CRLF line endings, and all surrounding bytes survive exactly.
//! Heading and marker detection ignore frontmatter and fenced examples.

use std::ops::Range;

use super::rules::track_fence;

pub(crate) const BEGIN_MARKER: &str = "<!-- bob:notebooks:begin v1 -->";
pub(crate) const END_MARKER: &str = "<!-- bob:notebooks:end -->";
const MARKER_OPENER: &str = "<!-- bob:notebooks:";
const HEADING_TEXT: &str = "Notebooks";

/// Why a host's managed region cannot be edited safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegionDamage {
    /// 1-based physical line of the first offending marker.
    pub(crate) line: usize,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditState {
    /// Required content, no managed region yet.
    Missing,
    /// The region differs from the expected bytes, or is removable.
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostOutcome {
    /// No expected content and no managed region.
    Nothing,
    /// The managed bytes already match.
    Current,
    /// One byte-span edit produces `after`.
    Update {
        state: EditState,
        after: String,
    },
    Damaged(RegionDamage),
}

#[derive(Debug, Clone, Copy)]
struct Line<'a> {
    content: &'a str,
    start: usize,
    /// Exclusive end including the line ending.
    end: usize,
}

impl Line<'_> {
    fn is_blank(&self) -> bool {
        self.content.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy)]
struct Heading {
    index: usize,
    level: usize,
}

struct Scanned<'a> {
    text: &'a str,
    lines: Vec<Line<'a>>,
    /// First line index after the frontmatter block.
    body_first: usize,
    fenced: Vec<bool>,
    eol: &'static str,
}

impl<'a> Scanned<'a> {
    fn new(text: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for piece in text.split_inclusive('\n') {
            let end = start + piece.len();
            let content = piece.strip_suffix('\n').unwrap_or(piece);
            let content = content.strip_suffix('\r').unwrap_or(content);
            lines.push(Line {
                content,
                start,
                end,
            });
            start = end;
        }
        let eol = if text
            .find('\n')
            .is_some_and(|at| at > 0 && text.as_bytes()[at - 1] == b'\r')
        {
            "\r\n"
        } else {
            "\n"
        };
        let body_first = frontmatter_end(&lines);
        let mut fenced = vec![false; lines.len()];
        let mut fence = None;
        for (index, line) in lines.iter().enumerate().skip(body_first) {
            fenced[index] = track_fence(line.content, &mut fence);
        }
        Self {
            text,
            lines,
            body_first,
            fenced,
            eol,
        }
    }

    fn real(&self, index: usize) -> bool {
        index >= self.body_first && !self.fenced[index]
    }

    fn heading(&self, index: usize) -> Option<(Heading, &'a str)> {
        if !self.real(index) {
            return None;
        }
        let (level, title) = atx_heading(self.lines[index].content)?;
        Some((Heading { index, level }, title))
    }

    fn headings(&self) -> impl Iterator<Item = (Heading, &'a str)> + '_ {
        (0..self.lines.len()).filter_map(|index| self.heading(index))
    }

    /// Locate the managed region: absent, one valid pair, or damaged.
    fn region(&self) -> Result<Option<Range<usize>>, RegionDamage> {
        let mut begins = Vec::new();
        let mut ends = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            if !self.real(index) {
                continue;
            }
            let trimmed = line.content.trim();
            if trimmed == BEGIN_MARKER {
                begins.push(index);
            } else if trimmed == END_MARKER {
                ends.push(index);
            } else if line.content.contains(MARKER_OPENER) {
                return Err(damage(
                    index,
                    "unsupported or malformed notebook catalog marker",
                ));
            }
        }
        match (begins.as_slice(), ends.as_slice()) {
            ([], []) => Ok(None),
            ([begin], [end]) if begin < end => Ok(Some(*begin..*end)),
            ([begin], [end]) => Err(damage(
                *end.min(begin),
                "notebook catalog markers are reversed",
            )),
            ([], [end, ..]) => Err(damage(
                *end,
                "notebook catalog end marker has no begin marker",
            )),
            ([begin, ..], []) => Err(damage(
                *begin,
                "notebook catalog begin marker has no end marker",
            )),
            _ => {
                let first = begins
                    .iter()
                    .chain(ends.iter())
                    .copied()
                    .min()
                    .unwrap_or(0);
                Err(damage(first, "notebook catalog markers are duplicated"))
            }
        }
    }

    fn first_tasks_heading(&self) -> Option<usize> {
        self.headings()
            .find(|(heading, title)| heading.level == 2 && *title == "Tasks")
            .map(|(heading, _)| heading.index)
    }

    /// Byte range of the owned bytes between the markers.
    fn inner_range(&self, region: &Range<usize>) -> Range<usize> {
        self.lines[region.start].end..self.lines[region.end].start
    }
}

fn damage(index: usize, message: &str) -> RegionDamage {
    RegionDamage {
        line: index + 1,
        message: message.to_string(),
    }
}

/// First line index after a leading `---` frontmatter block, or 0 when the
/// note has none (a lone opening fence is not frontmatter).
fn frontmatter_end(lines: &[Line<'_>]) -> usize {
    let first = lines.first().map(|line| line.content);
    if first.map(|content| content.trim_start_matches('\u{feff}'))
        != Some("---")
    {
        return 0;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.content == "---")
        .map_or(0, |(index, _)| index + 1)
}

/// ATX heading level and trimmed title (closing hashes removed).
fn atx_heading(content: &str) -> Option<(usize, &str)> {
    let trimmed = content.trim_start_matches(' ');
    if content.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let title = rest.trim();
    let title = title.trim_end_matches('#').trim_end();
    Some((level, title))
}

/// The inner bytes the owned region must hold for `callout` lines.
pub(crate) fn inner_text(callout: &[String], eol: &str) -> String {
    let mut inner = String::from(eol);
    for line in callout {
        inner.push_str(line);
        inner.push_str(eol);
    }
    inner.push_str(eol);
    inner
}

fn full_block(callout: &[String], eol: &str) -> String {
    format!(
        "## {HEADING_TEXT}{eol}{eol}{BEGIN_MARKER}{eol}{}{END_MARKER}{eol}",
        inner_text(callout, eol)
    )
}

/// Plan the single edit that makes the host's managed region match
/// `callout`; an empty `callout` means the host should hold no region.
pub(crate) fn plan_host(text: &str, callout: &[String]) -> HostOutcome {
    let scanned = Scanned::new(text);
    let region = match scanned.region() {
        Ok(region) => region,
        Err(damage) => return HostOutcome::Damaged(damage),
    };
    match (region, callout.is_empty()) {
        (None, true) => HostOutcome::Nothing,
        (None, false) => HostOutcome::Update {
            state: EditState::Missing,
            after: apply(text, insertion(&scanned, callout)),
        },
        (Some(region), false) => {
            let inner = scanned.inner_range(&region);
            let expected = inner_text(callout, scanned.eol);
            if text[inner.clone()] == expected {
                HostOutcome::Current
            } else {
                HostOutcome::Update {
                    state: EditState::Stale,
                    after: apply(text, (inner, expected)),
                }
            }
        }
        (Some(region), true) => HostOutcome::Update {
            state: EditState::Stale,
            after: apply(text, removal(&scanned, &region)),
        },
    }
}

fn apply(text: &str, (range, replacement): (Range<usize>, String)) -> String {
    let mut after = String::with_capacity(text.len() + replacement.len());
    after.push_str(&text[..range.start]);
    after.push_str(&replacement);
    after.push_str(&text[range.end..]);
    after
}

/// First write: immediately before the first real `## Tasks`, else at EOF
/// separated by one blank line.
fn insertion(
    scanned: &Scanned<'_>,
    callout: &[String],
) -> (Range<usize>, String) {
    let eol = scanned.eol;
    let block = full_block(callout, eol);
    if let Some(index) = scanned.first_tasks_heading() {
        let at = scanned.lines[index].start;
        let after_blank = index > 0 && scanned.lines[index - 1].is_blank();
        let lead = if after_blank { "" } else { eol };
        return (at..at, format!("{lead}{block}{eol}"));
    }
    let text = scanned.text;
    let at = text.len();
    let mut lead = String::new();
    if !text.is_empty() {
        if !text.ends_with('\n') {
            lead.push_str(eol);
        }
        let blank_tail = scanned
            .lines
            .last()
            .is_some_and(|line| line.is_blank() && line.end == text.len())
            && scanned.lines.len() > scanned.body_first;
        if !blank_tail {
            lead.push_str(eol);
        }
    }
    (at..at, format!("{lead}{block}"))
}

/// Delete the region. Also drop its `## Notebooks` heading when that
/// section is then blank lines only; keep any authored prose.
fn removal(
    scanned: &Scanned<'_>,
    region: &Range<usize>,
) -> (Range<usize>, String) {
    let lines = &scanned.lines;
    let text_end = scanned.text.len();
    let mut start = lines[region.start].start;
    let mut end = lines[region.end].end;

    let heading = (0..region.start)
        .rev()
        .find_map(|index| scanned.heading(index));
    if let Some((heading, title)) = heading
        && heading.level == 2
        && title == HEADING_TEXT
    {
        let section_end = ((region.end + 1)..lines.len())
            .find(|index| {
                scanned
                    .heading(*index)
                    .is_some_and(|(next, _)| next.level <= 2)
            })
            .unwrap_or(lines.len());
        let only_blank = ((heading.index + 1)..section_end)
            .filter(|index| !region.contains(index) && *index != region.end)
            .all(|index| lines[index].is_blank());
        if only_blank {
            start = lines[heading.index].start;
            end = lines.get(section_end).map_or(text_end, |line| line.start);
            return (tidy_eof(scanned, start, end), String::new());
        }
    }

    let previous_blank = region.start == scanned.body_first
        || lines[region.start - 1].is_blank();
    if previous_blank {
        let mut next = region.end + 1;
        while next < lines.len() && lines[next].is_blank() {
            end = lines[next].end;
            next += 1;
        }
    }
    start = tidy_start(scanned, start, end);
    (start..end, String::new())
}

/// When a removal reaches EOF, also drop the one blank separator line the
/// original insertion added before it.
fn tidy_start(scanned: &Scanned<'_>, start: usize, end: usize) -> usize {
    if end != scanned.text.len() {
        return start;
    }
    let Some(index) = scanned.lines.iter().position(|line| line.start == start)
    else {
        return start;
    };
    if index > scanned.body_first && scanned.lines[index - 1].is_blank() {
        scanned.lines[index - 1].start
    } else {
        start
    }
}

fn tidy_eof(scanned: &Scanned<'_>, start: usize, end: usize) -> Range<usize> {
    tidy_start(scanned, start, end)..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callout(rows: &[&str]) -> Vec<String> {
        let mut lines = vec!["> [!notebooks]".to_string(), ">".to_string()];
        lines.extend(rows.iter().map(|row| format!("> {row}")));
        lines
    }

    fn block(rows: &[&str]) -> String {
        full_block(&callout(rows), "\n")
    }

    fn update(outcome: HostOutcome) -> (EditState, String) {
        match outcome {
            HostOutcome::Update { state, after } => (state, after),
            other => panic!("expected update, got {other:?}"),
        }
    }

    #[test]
    fn inserts_before_first_tasks_heading_with_blank_line() {
        let text =
            "---\ntype: \"[[area]]\"\n---\n\n# Dev\n\n## Tasks\n\n- [ ] x\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (state, after) = update(plan_host(text, &rows));
        assert_eq!(state, EditState::Missing);
        assert_eq!(
            after,
            format!(
                "---\ntype: \"[[area]]\"\n---\n\n# Dev\n\n{}\n## Tasks\n\n- [ ] x\n",
                block(&["- **[[a|A]]**"])
            )
        );
    }

    #[test]
    fn inserts_at_eof_with_one_blank_line() {
        let text = "---\ntype: \"[[area]]\"\n---\n\n# Dev\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert_eq!(after, format!("{text}\n{}", block(&["- **[[a|A]]**"])));
    }

    #[test]
    fn eof_without_final_newline_gets_separator() {
        let text = "---\ntype: \"[[area]]\"\n---\n# Dev";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert_eq!(after, format!("{text}\n\n{}", block(&["- **[[a|A]]**"])));
    }

    #[test]
    fn tasks_heading_inside_fence_or_frontmatter_is_ignored() {
        let text = "---\nnote: |\n  ## Tasks\n---\n\n```md\n## Tasks\n```\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert!(after.starts_with(text));
        assert!(after.ends_with(&block(&["- **[[a|A]]**"])));
    }

    #[test]
    fn in_place_rewrite_keeps_moved_block_and_surroundings() {
        let old = block(&["- **[[a|Old]]**"]);
        let text =
            format!("---\n---\n\n{old}\nprose after\n\n## Tasks\n\n- x\n");
        let rows = callout(&["- **[[a|New]]**"]);
        let (state, after) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(
            after,
            format!(
                "---\n---\n\n{}\nprose after\n\n## Tasks\n\n- x\n",
                block(&["- **[[a|New]]**"])
            )
        );
    }

    #[test]
    fn identical_block_is_current_wherever_it_sits() {
        let rows = callout(&["- **[[a|A]]**"]);
        let text =
            format!("---\n---\n\ntop\n\n{}\ntail\n", block(&["- **[[a|A]]**"]));
        assert_eq!(plan_host(&text, &rows), HostOutcome::Current);
    }

    #[test]
    fn deletion_removes_empty_section_and_heading() {
        let text = format!(
            "---\n---\n\n# Dev\n\n{}\n## Tasks\n\n- x\n",
            block(&["- **[[a|A]]**"])
        );
        let (state, after) = update(plan_host(&text, &[]));
        assert_eq!(state, EditState::Stale);
        assert_eq!(after, "---\n---\n\n# Dev\n\n## Tasks\n\n- x\n");
    }

    #[test]
    fn deletion_at_eof_restores_original_bytes() {
        let original = "---\n---\n\n# Dev\n";
        let with = format!("{original}\n{}", block(&["- **[[a|A]]**"]));
        let (_, after) = update(plan_host(&with, &[]));
        assert_eq!(after, original);
    }

    #[test]
    fn deletion_keeps_authored_prose_and_heading() {
        let text = format!(
            "---\n---\n\n## Notebooks\n\nMy own words.\n\n{}\n## Tasks\n",
            full_block(&callout(&["- **[[a|A]]**"]), "\n")
                .trim_start_matches("## Notebooks\n\n")
        );
        let (_, after) = update(plan_host(&text, &[]));
        assert_eq!(
            after,
            "---\n---\n\n## Notebooks\n\nMy own words.\n\n## Tasks\n"
        );
    }

    #[test]
    fn nothing_when_no_rows_and_no_region() {
        assert_eq!(plan_host("---\n---\n\n# Dev\n", &[]), HostOutcome::Nothing);
    }

    #[test]
    fn damaged_markers_refuse_with_line() {
        let rows = callout(&["- **[[a|A]]**"]);
        for (text, line) in [
            (format!("---\n---\n{BEGIN_MARKER}\nbody\n"), 3),
            (format!("---\n---\nbody\n{END_MARKER}\n"), 4),
            (format!("---\n---\n{END_MARKER}\n{BEGIN_MARKER}\n"), 3),
            (
                format!(
                    "---\n---\n{BEGIN_MARKER}\n{END_MARKER}\n{BEGIN_MARKER}\n{END_MARKER}\n"
                ),
                3,
            ),
            ("---\n---\n<!-- bob:notebooks:begin v2 -->\n".to_string(), 3),
        ] {
            match plan_host(&text, &rows) {
                HostOutcome::Damaged(damage) => assert_eq!(damage.line, line, "{text}"),
                other => panic!("expected damage for {text:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn fenced_marker_examples_are_inert() {
        let text = format!("---\n---\n\n```md\n{BEGIN_MARKER}\n```\n");
        let rows = callout(&["- **[[a|A]]**"]);
        let (state, _) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Missing);
    }

    #[test]
    fn crlf_and_bom_survive_every_edit() {
        let text =
            "\u{feff}---\r\ntype: \"[[area]]\"\r\n---\r\n\r\n# Dev\r\n\r\n## Tasks\r\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert!(after.starts_with("\u{feff}---\r\n"));
        assert!(!after.replace("\r\n", "").contains('\n'));
        // Second pass is current, then deletion round-trips the bytes.
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
        let (_, back) = update(plan_host(&after, &[]));
        assert_eq!(back, text);
    }
}
