//! Strict byte-span handling for a bob-owned callout region in a host note.
//!
//! Bob owns exactly the `> [!{callout}]` callout block; the position of the
//! block, its heading, and every other byte belong to the note's author.
//! Edits are computed as byte-range replacements over the original text, so
//! a UTF-8 BOM, LF or CRLF line endings, and all surrounding bytes survive
//! exactly. Heading, callout, and legacy-marker detection ignore frontmatter
//! and fenced examples.

use std::ops::Range;
use std::path::PathBuf;

use super::FileEdit;
use crate::native::markdown::{closes_fence, fence_marker};

pub(crate) const LEGACY_BEGIN_MARKER: &str = "<!-- bob:notebooks:begin v1 -->";
pub(crate) const LEGACY_END_MARKER: &str = "<!-- bob:notebooks:end -->";
const LEGACY_MARKER_OPENER: &str = "<!-- bob:notebooks:";

/// How a missing region is first written into a host note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement<'a> {
    /// Immediately before the first real `## Tasks`, else at EOF.
    BeforeFirstTasks,
    /// At the end of an existing `## {section}` (before the next heading of
    /// level 2 or less). Otherwise the same as [`Self::BeforeFirstTasks`].
    #[cfg_attr(not(test), allow(dead_code))]
    AfterSectionThenTasks(&'a str),
    /// Immediately before the first real `##` heading after the frontmatter,
    /// else at EOF.
    BeforeFirstH2,
}

/// Legacy v1 begin/end HTML markers. Notebooks only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LegacySpec<'a> {
    pub(crate) begin: &'a str,
    pub(crate) end: &'a str,
    pub(crate) opener: &'a str,
    /// Noun used in damage messages, e.g. `"notebook catalog"`.
    pub(crate) noun: &'a str,
}

/// Parameters that distinguish one owned callout kind from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegionSpec<'a> {
    pub(crate) callout: &'a str,
    pub(crate) heading: &'a str,
    pub(crate) head_title: Option<&'a str>,
    pub(crate) placement: Placement<'a>,
    pub(crate) legacy: Option<LegacySpec<'a>>,
    /// Treat a malformed callout opener using this exact name as damaged
    /// instead of inserting a second generated region beside it.
    pub(crate) strict_bare: bool,
}

/// The notebooks catalog spec: `[!notebooks]`, `## Notebooks`, before Tasks,
/// with v1 marker migration.
pub(crate) const NOTEBOOKS: RegionSpec<'static> = RegionSpec {
    callout: "notebooks",
    heading: "Notebooks",
    head_title: None,
    placement: Placement::BeforeFirstTasks,
    legacy: Some(LegacySpec {
        begin: LEGACY_BEGIN_MARKER,
        end: LEGACY_END_MARKER,
        opener: LEGACY_MARKER_OPENER,
        noun: "notebook catalog",
    }),
    strict_bare: false,
};

/// The album-owner catalog spec: `[!albums]`, `## Albums`, after the
/// `## Notebooks` section (or before Tasks when that section is absent).
pub(crate) const ALBUMS: RegionSpec<'static> = RegionSpec {
    callout: "albums",
    heading: "Albums",
    head_title: None,
    placement: Placement::AfterSectionThenTasks("Notebooks"),
    legacy: None,
    strict_bare: true,
};

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

/// [`plan_host`] plus the [`FileEdit`] that would install an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostRegionPlan {
    pub(crate) outcome: HostOutcome,
    pub(crate) edit: Option<FileEdit>,
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
            if let Some(open) = fence {
                fenced[index] = true;
                if closes_fence(line.content, open) {
                    fence = None;
                }
            } else if let Some(marker) = fence_marker(line.content) {
                fenced[index] = true;
                fence = Some(marker);
            }
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
    fn legacy_pair(
        &self,
        spec: &LegacySpec<'_>,
    ) -> Result<Option<(usize, usize)>, RegionDamage> {
        let mut begins = Vec::new();
        let mut ends = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            if !self.real(index) {
                continue;
            }
            let trimmed = line.content.trim();
            if trimmed == spec.begin {
                begins.push(index);
            } else if trimmed == spec.end {
                ends.push(index);
            } else if line.content.contains(spec.opener) {
                return Err(damage(
                    index,
                    &format!(
                        "unsupported or malformed {} marker; repair the old markers by hand",
                        spec.noun
                    ),
                ));
            }
        }
        match (begins.as_slice(), ends.as_slice()) {
            ([], []) => Ok(None),
            ([begin], [end]) if begin < end => Ok(Some((*begin, *end))),
            ([begin], [end]) => Err(damage(
                *end.min(begin),
                &format!(
                    "{} markers are reversed; repair the old markers by hand",
                    spec.noun
                ),
            )),
            ([], [end, ..]) => Err(damage(
                *end,
                &format!(
                    "{} end marker has no begin marker; repair the old markers by hand",
                    spec.noun
                ),
            )),
            ([begin, ..], []) => Err(damage(
                *begin,
                &format!(
                    "{} begin marker has no end marker; repair the old markers by hand",
                    spec.noun
                ),
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
                    &format!(
                        "{} markers are duplicated; repair the old markers by hand",
                        spec.noun
                    ),
                ))
            }
        }
    }

    /// Every bare `> [!{callout}]` callout block outside frontmatter and
    /// fenced code, in document order. Each entry is (start, end) line
    /// indices inclusive.
    fn bare_regions(&self, callout: &str) -> Vec<(usize, usize)> {
        let mut regions = Vec::new();
        let mut index = self.body_first;
        while index < self.lines.len() {
            if !self.real(index)
                || !is_callout_head(self.lines[index].content, callout)
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
    fn owned_regions(
        &self,
        spec: &RegionSpec<'_>,
    ) -> Result<Vec<(usize, usize, bool)>, RegionDamage> {
        if spec.strict_bare
            && let Some(index) = self.malformed_callout_head(spec.callout)
        {
            return Err(damage(
                index,
                &format!(
                    "malformed [!{}] callout head; repair the catalog by hand",
                    spec.callout
                ),
            ));
        }
        let legacy = match spec.legacy {
            Some(legacy) => self.legacy_pair(&legacy)?,
            None => None,
        };
        let mut owned: Vec<(usize, usize, bool)> = Vec::new();
        if let Some((begin, end)) = legacy {
            owned.push((begin, end, true));
        }
        for (start, end) in self.bare_regions(spec.callout) {
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

    /// Find a real, malformed opener for a strict callout. This is limited to
    /// an exact callout name, so `[!albums-card]` remains authored content.
    fn malformed_callout_head(&self, callout: &str) -> Option<usize> {
        (self.body_first..self.lines.len()).find(|index| {
            if !self.real(*index)
                || is_callout_head(self.lines[*index].content, callout)
            {
                return false;
            }
            let line = self.lines[*index].content;
            let indent = line.chars().take_while(|ch| *ch == ' ').count();
            if indent > 3 {
                return false;
            }
            let Some(after_gt) = line[indent..].strip_prefix('>') else {
                return false;
            };
            let after_gt = after_gt.trim_start_matches([' ', '\t']);
            let Some(after_marker) = after_gt.strip_prefix("[!") else {
                return false;
            };
            let name = after_marker
                .chars()
                .take_while(|ch| {
                    ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-'
                })
                .collect::<String>();
            name.eq_ignore_ascii_case(callout)
        })
    }

    fn first_tasks_heading(&self) -> Option<usize> {
        self.headings()
            .find(|(heading, title)| heading.level == 2 && *title == "Tasks")
            .map(|(heading, _)| heading.index)
    }

    fn first_h2(&self) -> Option<usize> {
        self.headings()
            .find(|(heading, _)| heading.level == 2)
            .map(|(heading, _)| heading.index)
    }

    /// Line index of the next heading of level 2 or less after `title`,
    /// or `Some(len)` when that section runs to EOF. `None` when the
    /// heading is absent.
    fn section_end(&self, title: &str) -> Option<usize> {
        let (heading, _) = self
            .headings()
            .find(|(heading, found)| heading.level == 2 && *found == title)?;
        let next = ((heading.index + 1)..self.lines.len()).find(|index| {
            self.heading(*index)
                .is_some_and(|(next, _)| next.level <= 2)
        });
        Some(next.unwrap_or(self.lines.len()))
    }

    /// Byte range covering whole lines start..=end inclusive.
    fn line_byte_range(&self, start: usize, end: usize) -> Range<usize> {
        self.lines[start].start..self.lines[end].end
    }
}

/// True when `content` is a `> [!{callout}]` callout head: at most 3 spaces
/// of indentation, then `>`, then `[!{callout}]` case-insensitively, with an
/// optional `-`/`+` fold sign and title. Writes are always canonical.
fn is_callout_head(content: &str, callout: &str) -> bool {
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
    let Some(name) = inner.strip_prefix('!') else {
        return false;
    };
    if !name.eq_ignore_ascii_case(callout) {
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

/// Canonical `> [!{callout}]` head, with the spec's title when present.
pub(crate) fn canonical_head(spec: &RegionSpec<'_>) -> String {
    match spec.head_title {
        Some(title) if !title.is_empty() => {
            format!("> [!{}] {title}", spec.callout)
        }
        _ => format!("> [!{}]", spec.callout),
    }
}

/// Head line plus `rows` (each already a callout body line, including a
/// leading `>`).
pub(crate) fn canonical_callout(
    spec: &RegionSpec<'_>,
    rows: &[String],
) -> Vec<String> {
    let mut lines = vec![canonical_head(spec)];
    lines.extend(rows.iter().cloned());
    lines
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

fn full_block(spec: &RegionSpec<'_>, callout: &[String], eol: &str) -> String {
    format!("## {}{eol}{eol}{}", spec.heading, bare_text(callout, eol))
}

/// Plan the edit that makes the host's owned callout match `callout`; an
/// empty `callout` means the host should hold no region. A well-formed
/// legacy v1 pair counts as one owned region and migrates to the bare
/// callout. More than one owned region self-heals by keeping the first and
/// deleting the rest (stale, never damaged).
pub(crate) fn plan_host(
    spec: &RegionSpec<'_>,
    text: &str,
    callout: &[String],
) -> HostOutcome {
    let scanned = Scanned::new(text);
    let owned = match scanned.owned_regions(spec) {
        Ok(owned) => owned,
        Err(damage) => return HostOutcome::Damaged(damage),
    };
    match (owned.as_slice(), callout.is_empty()) {
        ([], true) => HostOutcome::Nothing,
        ([], false) => HostOutcome::Update {
            state: EditState::Missing,
            after: apply(text, insertion(spec, &scanned, callout)),
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
            after: apply(text, removal(spec, &scanned, &(*start..*end))),
        },
        (regions, _) => {
            // Duplicates self-heal: keep the first, delete the rest.
            let mut edits: Vec<(Range<usize>, String)> = Vec::new();
            let (first_start, first_end, _) = regions[0];
            if callout.is_empty() {
                let (range, replacement) =
                    removal(spec, &scanned, &(first_start..first_end));
                edits.push((range, replacement));
            } else {
                let range = scanned.line_byte_range(first_start, first_end);
                let expected = bare_text(callout, scanned.eol);
                if text[range.clone()] != expected {
                    edits.push((range, expected));
                }
            }
            for (start, end, _) in &regions[1..] {
                let (range, replacement) =
                    removal(spec, &scanned, &(*start..*end));
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

/// Plan the host region and, when it is an update, the matching file edit.
pub(crate) fn plan_host_region(
    path: PathBuf,
    contents: &str,
    spec: &RegionSpec<'_>,
    callout: &[String],
) -> HostRegionPlan {
    let outcome = plan_host(spec, contents, callout);
    let edit = match &outcome {
        HostOutcome::Update { after, .. } => Some(FileEdit {
            path,
            before: contents.to_string(),
            after: after.clone(),
        }),
        _ => None,
    };
    HostRegionPlan { outcome, edit }
}

fn apply(text: &str, (range, replacement): (Range<usize>, String)) -> String {
    let mut after = String::with_capacity(text.len() + replacement.len());
    after.push_str(&text[..range.start]);
    after.push_str(&replacement);
    after.push_str(&text[range.end..]);
    after
}

fn insertion(
    spec: &RegionSpec<'_>,
    scanned: &Scanned<'_>,
    callout: &[String],
) -> (Range<usize>, String) {
    let at = match spec.placement {
        Placement::BeforeFirstTasks => scanned.first_tasks_heading(),
        Placement::BeforeFirstH2 => scanned.first_h2(),
        Placement::AfterSectionThenTasks(section) => {
            match scanned.section_end(section) {
                Some(end) if end < scanned.lines.len() => Some(end),
                Some(_) => None, // section runs to EOF
                None => scanned.first_tasks_heading(),
            }
        }
    };
    insert_at(spec, scanned, callout, at)
}

/// First write: immediately before `index`, else at EOF separated by one
/// blank line.
fn insert_at(
    spec: &RegionSpec<'_>,
    scanned: &Scanned<'_>,
    callout: &[String],
    index: Option<usize>,
) -> (Range<usize>, String) {
    let eol = scanned.eol;
    let block = full_block(spec, callout, eol);
    if let Some(index) = index {
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

/// Delete the region. Also drop its heading when that section is then
/// blank lines only; keep any authored prose.
fn removal(
    spec: &RegionSpec<'_>,
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
        && title == spec.heading
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

    const RECORDS: RegionSpec<'static> = RegionSpec {
        callout: "records",
        heading: "Records",
        head_title: Some("104 restaurants · 🔖 11 to try"),
        placement: Placement::BeforeFirstH2,
        legacy: None,
        strict_bare: false,
    };

    fn albums_rows() -> Vec<String> {
        canonical_callout(
            &ALBUMS,
            &[
                ">".to_string(),
                "> - 🎬 **[[movies|Movies]]** — Watch list.".to_string(),
            ],
        )
    }

    fn records_rows() -> Vec<String> {
        canonical_callout(
            &RECORDS,
            &[
                ">".to_string(),
                "> - [[eat/ani|Ani Ramen House]] — Ramen · Cranford"
                    .to_string(),
            ],
        )
    }

    fn update(outcome: HostOutcome) -> (EditState, String) {
        match outcome {
            HostOutcome::Update { state, after } => (state, after),
            other => panic!("expected update, got {other:?}"),
        }
    }

    fn block(spec: &RegionSpec<'_>, callout: &[String]) -> String {
        full_block(spec, callout, "\n")
    }

    #[test]
    fn second_callout_name_does_not_see_notebooks() {
        let text = "---\n---\n\n# Dev\n\n## Notebooks\n\n> [!notebooks]\n>\n> - **[[a|A]]**\n";
        let rows = albums_rows();
        let (state, after) = update(plan_host(&ALBUMS, text, &rows));
        assert_eq!(state, EditState::Missing);
        assert!(after.contains("> [!notebooks]"));
        assert!(after.contains("> [!albums]"));
        assert_eq!(after.matches("> [!albums]").count(), 1);
    }

    #[test]
    fn head_title_is_written_canonically() {
        assert_eq!(
            canonical_head(&RECORDS),
            "> [!records] 104 restaurants · 🔖 11 to try"
        );
        let text = "---\n---\n\n# Restaurants\n\nIntro.\n";
        let rows = records_rows();
        let (_, after) = update(plan_host(&RECORDS, text, &rows));
        assert!(after.contains("> [!records] 104 restaurants · 🔖 11 to try\n"));
        assert!(!after.contains("> [!records]\n"));
        assert_eq!(plan_host(&RECORDS, &after, &rows), HostOutcome::Current);
    }

    #[test]
    fn after_section_inserts_at_end_of_notebooks() {
        let text = "\
---
---

# Fun

## Notebooks

> [!notebooks]
>
> - **[[a|A]]**

## Something else

## Tasks
";
        let rows = albums_rows();
        let (state, after) = update(plan_host(&ALBUMS, text, &rows));
        assert_eq!(state, EditState::Missing);
        let albums = block(&ALBUMS, &rows);
        let notebooks_end = after.find("## Something else").unwrap();
        let albums_at = after.find(&albums).unwrap();
        assert!(albums_at < notebooks_end, "{after}");
        assert!(after.contains("## Something else"));
        assert!(after.contains("## Tasks"));
    }

    #[test]
    fn after_section_without_notebooks_falls_back_to_tasks() {
        let text =
            "---\ntype: \"[[area]]\"\n---\n\n# Fun\n\n## Tasks\n\n- [ ] x\n";
        let rows = albums_rows();
        let (state, after) = update(plan_host(&ALBUMS, text, &rows));
        assert_eq!(state, EditState::Missing);
        assert_eq!(
            after,
            format!(
                "---\ntype: \"[[area]]\"\n---\n\n# Fun\n\n{}\n## Tasks\n\n- [ ] x\n",
                block(&ALBUMS, &rows)
            )
        );
    }

    #[test]
    fn before_first_h2_inserts_ahead_of_notes() {
        let text =
            "---\n---\n\n# Restaurants\n\nIntro.\n\n## Notes\n\n- a note\n";
        let rows = records_rows();
        let (state, after) = update(plan_host(&RECORDS, text, &rows));
        assert_eq!(state, EditState::Missing);
        let records = block(&RECORDS, &rows);
        assert!(after.contains(&format!("{records}\n## Notes\n")));
        assert!(after.contains("# Restaurants"));
    }

    #[test]
    fn before_first_h2_without_h2_appends_at_eof() {
        let text = "---\n---\n\n# Restaurants\n";
        let rows = records_rows();
        let (_, after) = update(plan_host(&RECORDS, text, &rows));
        assert_eq!(after, format!("{text}\n{}", block(&RECORDS, &rows)));
    }

    #[test]
    fn fences_and_frontmatter_stay_inert_for_albums() {
        let rows = albums_rows();
        let fenced = "---\n---\n\n```md\n> [!albums]\n> - x\n```\n";
        let (state, after) = update(plan_host(&ALBUMS, fenced, &rows));
        assert_eq!(state, EditState::Missing);
        assert!(after.starts_with(fenced));
        let front = "---\n> [!albums]\n> - x\n---\n\n# Fun\n";
        let (state, _) = update(plan_host(&ALBUMS, front, &rows));
        assert_eq!(state, EditState::Missing);
    }

    #[test]
    fn malformed_album_callout_is_damaged_but_lookalike_is_authored() {
        let malformed =
            "---\n---\n\n# Fun\n\n## Albums\n\n> [!albums\n> - partial\n";
        assert!(matches!(
            plan_host(&ALBUMS, malformed, &albums_rows()),
            HostOutcome::Damaged(RegionDamage { line: 8, .. })
        ));

        let authored = "---\n---\n\n# Fun\n\n> [!albums-card]\n> authored\n";
        assert!(matches!(
            plan_host(&ALBUMS, authored, &albums_rows()),
            HostOutcome::Update { .. }
        ));
    }

    #[test]
    fn crlf_and_bom_survive_albums_edits() {
        let text =
            "\u{feff}---\r\ntype: \"[[area]]\"\r\n---\r\n\r\n# Fun\r\n\r\n## Tasks\r\n";
        let rows = albums_rows();
        let (_, after) = update(plan_host(&ALBUMS, text, &rows));
        assert!(after.starts_with("\u{feff}---\r\n"));
        assert!(!after.replace("\r\n", "").contains('\n'));
        assert_eq!(plan_host(&ALBUMS, &after, &rows), HostOutcome::Current);
        let (_, back) = update(plan_host(&ALBUMS, &after, &[]));
        assert_eq!(back, text);
    }

    #[test]
    fn albums_duplicates_self_heal_to_first() {
        let rows = albums_rows();
        let bare = rows.join("\n");
        let text = format!(
            "---\n---\n\n# Fun\n\n## Albums\n\n{bare}\n\n## Albums\n\n{bare}\n\n## Tasks\n"
        );
        let (state, after) = update(plan_host(&ALBUMS, &text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(after.matches("> [!albums]").count(), 1);
        assert_eq!(after.matches("## Albums").count(), 1);
        assert_eq!(plan_host(&ALBUMS, &after, &rows), HostOutcome::Current);
    }

    #[test]
    fn empty_subtree_deletes_albums_heading_and_keeps_notebooks() {
        let rows = albums_rows();
        let notebooks = "## Notebooks\n\n> [!notebooks]\n>\n> - **[[a|A]]**\n";
        let text = format!(
            "---\n---\n\n# Fun\n\n{notebooks}\n{}\n## Tasks\n",
            block(&ALBUMS, &rows)
        );
        let (state, after) = update(plan_host(&ALBUMS, &text, &[]));
        assert_eq!(state, EditState::Stale);
        assert!(!after.contains("> [!albums]"));
        assert!(!after.contains("## Albums"));
        assert!(after.contains("## Notebooks"));
        assert!(after.contains("> [!notebooks]"));
        assert_eq!(
            after,
            format!("---\n---\n\n# Fun\n\n{notebooks}\n## Tasks\n")
        );
    }

    #[test]
    fn folded_records_head_canonicalizes_title() {
        let rows = records_rows();
        let folded = "---\n---\n\n# Restaurants\n\n## Records\n\n> [!records]- 104 restaurants · 🔖 11 to try\n>\n> - [[eat/ani|Ani Ramen House]] — Ramen · Cranford\n";
        let (state, after) = update(plan_host(&RECORDS, folded, &rows));
        assert_eq!(state, EditState::Stale);
        assert!(after.contains("> [!records] 104 restaurants · 🔖 11 to try\n"));
        assert!(!after.contains("> [!records]-"));
        assert_eq!(plan_host(&RECORDS, &after, &rows), HostOutcome::Current);
    }
}
