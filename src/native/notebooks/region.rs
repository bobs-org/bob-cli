//! Strict byte-span handling for the Bob-owned `## Notebooks` region.
//!
//! Bob owns exactly the `> [!notebooks]` callout block; the position of the
//! block, its heading, and every other byte belong to the note's author.
//! Edits are computed as byte-range replacements over the original text, so
//! a UTF-8 BOM, LF or CRLF line endings, and all surrounding bytes survive
//! exactly. Heading, callout, and legacy-marker detection ignore frontmatter
//! and fenced examples.

use std::ops::Range;

use super::rules::track_fence;

pub(crate) const LEGACY_BEGIN_MARKER: &str = "<!-- bob:notebooks:begin v1 -->";
pub(crate) const LEGACY_END_MARKER: &str = "<!-- bob:notebooks:end -->";
const LEGACY_MARKER_OPENER: &str = "<!-- bob:notebooks:";
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

    /// Locate the single legacy v1 marker pair, if any. A well-formed
    /// pair is one owned region; any lone, reversed, duplicated, or
    /// unsupported marker is damage and is never guessed.
    fn legacy_pair(&self) -> Result<Option<(usize, usize)>, RegionDamage> {
        let mut begins = Vec::new();
        let mut ends = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            if !self.real(index) {
                continue;
            }
            let trimmed = line.content.trim();
            if trimmed == LEGACY_BEGIN_MARKER {
                begins.push(index);
            } else if trimmed == LEGACY_END_MARKER {
                ends.push(index);
            } else if line.content.contains(LEGACY_MARKER_OPENER) {
                return Err(damage(
                    index,
                    "unsupported or malformed notebook catalog marker; repair the old markers by hand",
                ));
            }
        }
        match (begins.as_slice(), ends.as_slice()) {
            ([], []) => Ok(None),
            ([begin], [end]) if begin < end => Ok(Some((*begin, *end))),
            ([begin], [end]) => Err(damage(
                *end.min(begin),
                "notebook catalog markers are reversed; repair the old markers by hand",
            )),
            ([], [end, ..]) => Err(damage(
                *end,
                "notebook catalog end marker has no begin marker; repair the old markers by hand",
            )),
            ([begin, ..], []) => Err(damage(
                *begin,
                "notebook catalog begin marker has no end marker; repair the old markers by hand",
            )),
            _ => {
                let first = begins
                    .iter()
                    .chain(ends.iter())
                    .copied()
                    .min()
                    .unwrap_or(0);
                Err(damage(
                    first,
                    "notebook catalog markers are duplicated; repair the old markers by hand",
                ))
            }
        }
    }

    /// Every bare `> [!notebooks]` callout block outside frontmatter and
    /// fenced code, in document order. Each entry is (start, end) line
    /// indices inclusive.
    fn bare_regions(&self) -> Vec<(usize, usize)> {
        let mut regions = Vec::new();
        let mut index = self.body_first;
        while index < self.lines.len() {
            if !self.real(index) || !is_callout_head(self.lines[index].content)
            {
                index += 1;
                continue;
            }
            let mut end = index;
            let mut next = index + 1;
            while next < self.lines.len()
                && self.real(next)
                && is_quote_line(self.lines[next].content)
            {
                end = next;
                next += 1;
            }
            regions.push((index, end));
            index = end + 1;
        }
        regions
    }

    /// All owned regions (one legacy pair and/or bare callouts) in document
    /// order. Each entry is (start_line, end_line, is_legacy) with end
    /// inclusive.
    fn owned_regions(&self) -> Result<Vec<(usize, usize, bool)>, RegionDamage> {
        let legacy = self.legacy_pair()?;
        let mut owned: Vec<(usize, usize, bool)> = Vec::new();
        if let Some((begin, end)) = legacy {
            owned.push((begin, end, true));
        }
        for (start, end) in self.bare_regions() {
            // A callout inside a legacy pair belongs to that pair.
            if let Some((begin, end_legacy)) = legacy
                && start >= begin
                && end <= end_legacy
            {
                continue;
            }
            owned.push((start, end, false));
        }
        owned.sort_by_key(|(start, _, _)| *start);
        Ok(owned)
    }

    fn first_tasks_heading(&self) -> Option<usize> {
        self.headings()
            .find(|(heading, title)| heading.level == 2 && *title == "Tasks")
            .map(|(heading, _)| heading.index)
    }

    /// Byte range covering whole lines start..=end inclusive.
    fn line_byte_range(&self, start: usize, end: usize) -> Range<usize> {
        self.lines[start].start..self.lines[end].end
    }
}

/// True when `content` is a `> [!notebooks]` callout head: at most 3 spaces
/// of indentation, then `>`, then `[!notebooks]` case-insensitively, with an
/// optional `-`/`+` fold sign and title. Writes are always canonical
/// `> [!notebooks]`.
fn is_callout_head(content: &str) -> bool {
    let indent = content.chars().take_while(|ch| *ch == ' ').count();
    if indent > 3 {
        return false;
    }
    let rest = &content[indent..];
    let Some(after_gt) = rest.strip_prefix('>') else {
        return false;
    };
    let after_gt = after_gt.trim_start_matches([' ', '\t']);
    if !after_gt.starts_with('[') {
        return false;
    }
    let Some(close) = after_gt.find(']') else {
        return false;
    };
    let inner = after_gt[1..close].trim();
    if !inner.eq_ignore_ascii_case("!notebooks") {
        return false;
    }
    // After `]` allow optional spaces, one optional fold sign, then anything.
    // A `|alias` form is not a catalog head.
    let tail = after_gt[close + 1..].trim_start_matches([' ', '\t']);
    let tail = tail.strip_prefix(['-', '+']).unwrap_or(tail);
    // Empty tail or a title starting with space/text is fine; a `|` alias is not.
    !tail.trim_start().starts_with('|')
}

/// True when `content` is a blockquote line: at most 3 spaces of
/// indentation, then `>`.
fn is_quote_line(content: &str) -> bool {
    let indent = content.chars().take_while(|ch| *ch == ' ').count();
    if indent > 3 {
        return false;
    }
    content[indent..].starts_with('>')
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

/// The bare callout bytes the owned region must hold for `callout`
/// lines: each line plus the document EOL, byte-stable.
pub(crate) fn bare_text(callout: &[String], eol: &str) -> String {
    if callout.is_empty() {
        return String::new();
    }
    let mut out = callout.join(eol);
    out.push_str(eol);
    out
}

fn full_block(callout: &[String], eol: &str) -> String {
    format!("## {HEADING_TEXT}{eol}{eol}{}", bare_text(callout, eol))
}

/// Plan the edit that makes the host's owned callout match `callout`; an
/// empty `callout` means the host should hold no region. A well-formed
/// legacy v1 pair counts as one owned region and migrates to the bare
/// callout. More than one owned region self-heals by keeping the first and
/// deleting the rest (stale, never damaged).
pub(crate) fn plan_host(text: &str, callout: &[String]) -> HostOutcome {
    let scanned = Scanned::new(text);
    let owned = match scanned.owned_regions() {
        Ok(owned) => owned,
        Err(damage) => return HostOutcome::Damaged(damage),
    };
    match (owned.as_slice(), callout.is_empty()) {
        ([], true) => HostOutcome::Nothing,
        ([], false) => HostOutcome::Update {
            state: EditState::Missing,
            after: apply(text, insertion(&scanned, callout)),
        },
        ([(start, end, _)], false) => {
            let range = scanned.line_byte_range(*start, *end);
            let expected = bare_text(callout, scanned.eol);
            if text[range.clone()] == expected {
                HostOutcome::Current
            } else {
                HostOutcome::Update {
                    state: EditState::Stale,
                    after: apply(text, (range, expected)),
                }
            }
        }
        ([(start, end, _)], true) => HostOutcome::Update {
            state: EditState::Stale,
            after: apply(text, removal(&scanned, &(*start..*end))),
        },
        (regions, _) => {
            // Duplicates self-heal: keep the first, delete the rest.
            let mut edits: Vec<(Range<usize>, String)> = Vec::new();
            let (first_start, first_end, _) = regions[0];
            if callout.is_empty() {
                let (range, replacement) =
                    removal(&scanned, &(first_start..first_end));
                edits.push((range, replacement));
            } else {
                let range = scanned.line_byte_range(first_start, first_end);
                let expected = bare_text(callout, scanned.eol);
                if text[range.clone()] != expected {
                    edits.push((range, expected));
                }
            }
            for (start, end, _) in &regions[1..] {
                let (range, replacement) = removal(&scanned, &(*start..*end));
                edits.push((range, replacement));
            }
            // If the first was already current and there were no other
            // edits, this would be Current; but >1 regions always need at
            // least the duplicate deletions.
            if edits.is_empty() {
                // First is current but duplicates remain: still stale.
                // Force the duplicate deletions (they must be non-empty).
                // This branch is unreachable because rest deletions always
                // produce an edit, but keep it for safety.
                return HostOutcome::Current;
            }
            edits.sort_by_key(|(range, _)| range.start);
            let mut after = text.to_string();
            for (range, replacement) in edits.into_iter().rev() {
                after.replace_range(range, &replacement);
            }
            HostOutcome::Update {
                state: EditState::Stale,
                after,
            }
        }
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
            (format!("---\n---\n{LEGACY_BEGIN_MARKER}\nbody\n"), 3),
            (format!("---\n---\nbody\n{LEGACY_END_MARKER}\n"), 4),
            (format!("---\n---\n{LEGACY_END_MARKER}\n{LEGACY_BEGIN_MARKER}\n"), 3),
            (
                format!(
                    "---\n---\n{LEGACY_BEGIN_MARKER}\n{LEGACY_END_MARKER}\n{LEGACY_BEGIN_MARKER}\n{LEGACY_END_MARKER}\n"
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
        let text = format!("---\n---\n\n```md\n{LEGACY_BEGIN_MARKER}\n```\n");
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

    #[test]
    fn legacy_pair_migrates_to_exact_bare_callout() {
        let rows = callout(&["- **[[a|A]]**"]);
        let legacy = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{LEGACY_BEGIN_MARKER}\n\n{}\n\n{LEGACY_END_MARKER}\n",
            rows.join("\n")
        );
        let (state, after) = update(plan_host(&legacy, &rows));
        assert_eq!(state, EditState::Stale);
        let expected = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{}\n",
            rows.join("\n")
        );
        assert_eq!(after, expected);
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
        assert!(!after.contains("bob:notebooks:"));
    }

    #[test]
    fn legacy_plus_bare_duplicate_heals_to_first() {
        let rows = callout(&["- **[[a|A]]**"]);
        let bare = rows.join("\n");
        let text = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{bare}\n\n## Notebooks\n\n{LEGACY_BEGIN_MARKER}\n\n{bare}\n\n{LEGACY_END_MARKER}\n\n## Tasks\n"
        );
        let (state, after) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(after.matches("> [!notebooks]").count(), 1);
        assert!(!after.contains("bob:notebooks:"));
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn two_bare_duplicates_heal_to_first() {
        let rows = callout(&["- **[[a|A]]**"]);
        let bare = rows.join("\n");
        let text = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{bare}\n\n## Notebooks\n\n{bare}\n\n## Tasks\n"
        );
        let (state, after) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(after.matches("> [!notebooks]").count(), 1);
        // Duplicate heading whose section went blank is dropped.
        assert_eq!(after.matches("## Notebooks").count(), 1);
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn folded_head_canonicalizes() {
        let rows = callout(&["- **[[a|A]]**"]);
        let folded = "---\n---\n\n# Dev\n\n## Notebooks\n\n> [!notebooks]-\n>\n> - **[[a|A]]**\n".to_string();
        let (state, after) = update(plan_host(&folded, &rows));
        assert_eq!(state, EditState::Stale);
        assert!(after.contains("> [!notebooks]\n"));
        assert!(!after.contains("> [!notebooks]-"));
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn callout_in_fence_or_frontmatter_stays_inert() {
        let rows = callout(&["- **[[a|A]]**"]);
        let fenced = "---\n---\n\n```md\n> [!notebooks]\n> - x\n```\n";
        let (state, after) = update(plan_host(fenced, &rows));
        assert_eq!(state, EditState::Missing);
        assert!(after.starts_with(fenced));
        let front = "---\n> [!notebooks]\n> - x\n---\n\n# Dev\n";
        let (state, _) = update(plan_host(front, &rows));
        assert_eq!(state, EditState::Missing);
    }

    #[test]
    fn quote_after_blank_does_not_join_region() {
        let rows = callout(&["- **[[a|A]]**"]);
        // A `>` line after a blank line is a second owned region, so the
        // first (partial) plus the orphan heal to one current block.
        let text =
            "---\n---\n\n## Notebooks\n\n> [!notebooks]\n>\n\n> - orphan\n";
        let (state, after) = update(plan_host(text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn bare_crlf_bom_round_trip() {
        let rows = callout(&["- **[[a|A]]**"]);
        let text = "\u{feff}---\r\n---\r\n\r\n# Dev\r\n\r\n## Notebooks\r\n\r\n> [!notebooks]\r\n>\r\n> - **[[a|A]]**\r\n";
        assert_eq!(plan_host(text, &rows), HostOutcome::Current);
        let (_, back) = update(plan_host(text, &[]));
        assert!(!back.contains("> [!notebooks]"));
        assert!(back.starts_with("\u{feff}---\r\n"));
    }
}
