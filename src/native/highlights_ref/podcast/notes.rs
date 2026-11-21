//! Show-notes segmentation.
//!
//! [`segment_notes`] splits a publisher's HTML show notes into ordered
//! [`NotesBlock`]s:
//!
//! - **Chapters**: an inline run of three or more consecutive block lines
//!   (`<p>`, `<li>`, or `<br>`-separated lines) that each read
//!   `[(]m:ss[)] [-–—:|] Title` or `h:mm:ss …`, with strictly increasing
//!   times, the first at most 10:00 and the last before the episode ends.
//!   A short label line right before the run ("Timestamps") is consumed.
//!   The block stays where the run was, so the publisher's order is kept.
//! - **Sponsors**: a run starting at a "Brought to you by" / "Sponsored
//!   by" / "Sponsors" line and ending at the next separator line or at the
//!   end of a bullet run. It moves to the end only when that terminator
//!   is found (the separator is dropped); otherwise it stays in place.
//! - **Html**: everything else, as raw publisher fragments the renderer
//!   sanitizes. Separator lines at either edge of a block are trimmed and
//!   interior ones become `<hr>`.
//!
//! A block element the segmenter leaves whole keeps its original markup;
//! one that a run splits is rewrapped piece by piece.

use std::{ops::Range, sync::OnceLock};

use regex::Regex;
use scraper::{node::Node, ElementRef, Html, Selector};

use super::{clean_line, strip_invisibles};

/// A chapter run needs at least this many lines.
const MIN_CHAPTERS: usize = 3;

/// The first chapter starts no later than this (10:00).
const MAX_FIRST_CHAPTER_SECS: u32 = 600;

/// A label line consumed before a chapter run has at most this many
/// words.
const MAX_LABEL_WORDS: usize = 4;

/// A sponsor header without a bullet run looks this many lines ahead for
/// its separator; further away, the run stays in place, so a sponsor
/// mention can never carry the episode description off with it.
const MAX_PLAIN_SPONSOR_LINES: usize = 8;

/// One segment of the show notes, in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotesBlock {
    /// Raw publisher HTML; the renderer sanitizes it.
    Html(String),
    /// A chapter list found inline in the notes.
    Chapters(Vec<Chapter>),
    /// The demoted sponsor runs, raw HTML; always the last block.
    Sponsors(String),
}

/// One chapter: where it starts and what it is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chapter {
    pub(crate) start_secs: u32,
    pub(crate) title: String,
}

/// Segmented show notes plus their word count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShowNotes {
    pub(crate) blocks: Vec<NotesBlock>,
    /// Words in the notes, for reporting.
    pub(crate) words: usize,
}

impl ShowNotes {
    /// Every chapter, across chapter blocks.
    pub(crate) fn chapters(&self) -> impl Iterator<Item = &Chapter> {
        self.blocks.iter().flat_map(|block| match block {
            NotesBlock::Chapters(chapters) => chapters.as_slice(),
            _ => &[],
        })
    }

    /// The demoted sponsor HTML, when there is any.
    pub(crate) fn sponsors_html(&self) -> Option<&str> {
        self.blocks.iter().find_map(|block| match block {
            NotesBlock::Sponsors(html) => Some(html.as_str()),
            _ => None,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Segment HTML show notes. `duration_ms` bounds the last chapter when
/// known.
pub(crate) fn segment_notes(
    notes_html: &str,
    duration_ms: Option<u64>,
) -> ShowNotes {
    let fragment = Html::parse_fragment(&strip_invisibles(notes_html));
    let mut layout = Layout::default();
    collect_groups(fragment.root_element(), &mut layout);
    let roles = classify(&layout.lines, duration_ms);
    emit(&layout, &roles)
}

/// Segment plain-text show notes: each nonblank line becomes a
/// paragraph.
pub(crate) fn segment_text(
    notes_text: &str,
    duration_ms: Option<u64>,
) -> ShowNotes {
    let html: String = notes_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| format!("<p>{}</p>", escape_text(line)))
        .collect();
    segment_notes(&html, duration_ms)
}

// ---------------------------------------------------------------------
// Layout: groups of lines.
// ---------------------------------------------------------------------

/// How a group's lines are rewrapped when a run splits it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GroupKind {
    /// A block element whose lines are its `<br>`-separated pieces.
    Block(String),
    /// A `<ul>`/`<ol>` whose lines are its items.
    List(String),
    /// Top-level text and inline elements, split at `<br>`.
    Inline,
    /// A block kept whole (`<pre>`, `<table>`, …): one line.
    Opaque,
    /// An `<hr>`: one separator line.
    Rule,
}

#[derive(Debug)]
struct Group {
    kind: GroupKind,
    /// The original markup, emitted when no run splits the group.
    html: String,
    lines: Range<usize>,
}

#[derive(Debug)]
struct Line {
    /// Visible text, invisibles stripped and whitespace collapsed.
    text: String,
    /// This line's markup (inside its block or item).
    html: String,
    /// An `<li>` or a line starting with a bullet character.
    bullet: bool,
    has_link: bool,
    rule: bool,
}

#[derive(Debug, Default)]
struct Layout {
    groups: Vec<Group>,
    lines: Vec<Line>,
}

impl Layout {
    fn push_group(&mut self, kind: GroupKind, html: String, lines: Vec<Line>) {
        if lines.is_empty() {
            return;
        }
        let start = self.lines.len();
        self.lines.extend(lines);
        self.groups.push(Group {
            kind,
            html,
            lines: start..self.lines.len(),
        });
    }
}

/// A child of a container: a text run or an element.
#[derive(Clone, Copy)]
enum Piece<'a> {
    Text(&'a str),
    Element(ElementRef<'a>),
}

impl Piece<'_> {
    fn html(&self) -> String {
        match self {
            Self::Text(text) => escape_text(text),
            Self::Element(element) => element.html(),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Text(text) => (*text).to_string(),
            Self::Element(element) => element.text().collect(),
        }
    }

    fn is_br(&self) -> bool {
        matches!(self, Self::Element(element) if element.value().name() == "br")
    }

    fn has_link(&self) -> bool {
        match self {
            Self::Text(_) => false,
            Self::Element(element) => {
                element.value().name() == "a"
                    || element.select(link_selector()).next().is_some()
            }
        }
    }

    fn has_element(&self) -> bool {
        matches!(self, Self::Element(element) if element.value().name() != "br")
    }
}

fn pieces(container: ElementRef<'_>) -> Vec<Piece<'_>> {
    container
        .children()
        .filter_map(|child| match child.value() {
            Node::Text(text) => Some(Piece::Text(text)),
            Node::Element(_) => ElementRef::wrap(child).map(Piece::Element),
            _ => None,
        })
        .collect()
}

fn link_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();
    SELECTOR.get_or_init(|| Selector::parse("a").expect("valid selector"))
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "div",
    "dl",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// Containers unwrapped when they hold blocks of their own.
const WRAPPER_TAGS: &[&str] = &[
    "article", "aside", "div", "footer", "header", "main", "nav", "section",
];

/// Blocks kept whole whatever they hold.
const OPAQUE_TAGS: &[&str] = &["details", "dl", "figure", "pre", "table"];

fn is_block(element: ElementRef<'_>) -> bool {
    BLOCK_TAGS.contains(&element.value().name())
}

fn has_block_child(element: ElementRef<'_>) -> bool {
    pieces(element).iter().any(|piece| match piece {
        Piece::Element(child) => is_block(*child),
        Piece::Text(_) => false,
    })
}

fn collect_groups(container: ElementRef<'_>, layout: &mut Layout) {
    let mut inline: Vec<Piece<'_>> = Vec::new();
    for piece in pieces(container) {
        let Piece::Element(element) = piece else {
            inline.push(piece);
            continue;
        };
        if !is_block(element) {
            inline.push(piece);
            continue;
        }
        flush_inline(&mut inline, layout);
        let tag = element.value().name();
        if tag == "hr" {
            layout.push_group(
                GroupKind::Rule,
                element.html(),
                vec![Line {
                    text: String::new(),
                    html: "<hr>".to_string(),
                    bullet: false,
                    has_link: false,
                    rule: true,
                }],
            );
        } else if tag == "ul" || tag == "ol" {
            let items = pieces(element)
                .into_iter()
                .filter_map(|piece| match piece {
                    Piece::Element(item) if item.value().name() == "li" => {
                        line_from(&pieces(item), true)
                    }
                    _ => None,
                })
                .collect();
            layout.push_group(
                GroupKind::List(tag.to_string()),
                element.html(),
                items,
            );
        } else if WRAPPER_TAGS.contains(&tag) && has_block_child(element) {
            collect_groups(element, layout);
        } else if OPAQUE_TAGS.contains(&tag)
            || (tag == "blockquote" && has_block_child(element))
        {
            let line = line_from(&[piece], false);
            layout.push_group(
                GroupKind::Opaque,
                element.html(),
                line.into_iter().collect(),
            );
        } else {
            let lines = split_at_br(&pieces(element));
            layout.push_group(
                GroupKind::Block(tag.to_string()),
                element.html(),
                lines,
            );
        }
    }
    flush_inline(&mut inline, layout);
}

fn flush_inline(inline: &mut Vec<Piece<'_>>, layout: &mut Layout) {
    if inline.is_empty() {
        return;
    }
    let html: String = inline.iter().map(Piece::html).collect();
    let lines = split_at_br(inline);
    layout.push_group(GroupKind::Inline, html, lines);
    inline.clear();
}

fn split_at_br(pieces: &[Piece<'_>]) -> Vec<Line> {
    pieces
        .split(Piece::is_br)
        .filter_map(|run| line_from(run, false))
        .collect()
}

/// A line from its pieces; `None` for a line with no text and no
/// element (whitespace between blocks, an empty paragraph).
fn line_from(pieces: &[Piece<'_>], item: bool) -> Option<Line> {
    let raw_text: String = pieces.iter().map(Piece::text).collect();
    let text = clean_line(&raw_text).unwrap_or_default();
    if text.is_empty() && !pieces.iter().any(Piece::has_element) {
        return None;
    }
    let html = pieces
        .iter()
        .map(Piece::html)
        .collect::<String>()
        .trim()
        .to_string();
    Some(Line {
        bullet: item || starts_with_bullet(&text),
        has_link: pieces.iter().any(Piece::has_link),
        rule: false,
        text,
        html,
    })
}

fn starts_with_bullet(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some('•' | '·' | '▪' | '◦' | '●' | '‣') => true,
        Some('-' | '*' | '+' | '–' | '—') => {
            chars.next().is_some_and(char::is_whitespace)
        }
        _ => false,
    }
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------
// Classification.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Normal,
    Separator,
    /// A line of chapter run `n`.
    Chapter(usize),
    /// A label consumed by a chapter run.
    Label,
    /// A line of a demoted sponsor run.
    Sponsor,
    /// A sponsor run's separator terminator.
    Dropped,
}

struct Classified {
    roles: Vec<Role>,
    runs: Vec<Vec<Chapter>>,
}

fn classify(lines: &[Line], duration_ms: Option<u64>) -> Classified {
    let mut roles: Vec<Role> = lines
        .iter()
        .map(|line| {
            if line.rule || is_separator(&line.text) {
                Role::Separator
            } else {
                Role::Normal
            }
        })
        .collect();
    let runs = mark_chapters(lines, &mut roles, duration_ms);
    mark_sponsors(lines, &mut roles);
    Classified { roles, runs }
}

fn is_separator(text: &str) -> bool {
    static SEPARATOR: OnceLock<Regex> = OnceLock::new();
    SEPARATOR
        .get_or_init(|| {
            Regex::new(r"^(?:[—–]+|[-*_]{3,}|(?:[•·*-]\s*){3,})$")
                .expect("valid separator")
        })
        .is_match(text)
}

/// A chapter line's start and title.
fn parse_chapter(text: &str) -> Option<Chapter> {
    static CHAPTER: OnceLock<Regex> = OnceLock::new();
    let pattern = CHAPTER.get_or_init(|| {
        Regex::new(
            r"^[(\[]?((\d{1,2}:)?\d{1,2}:\d{2})[)\]]?\s*[-–—:|]?\s*(.+)$",
        )
        .expect("valid chapter line")
    });
    let captures = pattern.captures(text)?;
    let parts: Vec<u32> = captures[1]
        .split(':')
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    let start_secs = match parts.as_slice() {
        [minutes, seconds] if *seconds < 60 => minutes * 60 + seconds,
        [hours, minutes, seconds] if *minutes < 60 && *seconds < 60 => {
            hours * 3600 + minutes * 60 + seconds
        }
        _ => return None,
    };
    let title = clean_line(&captures[3])?;
    Some(Chapter { start_secs, title })
}

fn mark_chapters(
    lines: &[Line],
    roles: &mut [Role],
    duration_ms: Option<u64>,
) -> Vec<Vec<Chapter>> {
    let parsed: Vec<Option<Chapter>> = lines
        .iter()
        .zip(roles.iter())
        .map(|(line, role)| {
            (*role == Role::Normal)
                .then(|| parse_chapter(&line.text))
                .flatten()
        })
        .collect();
    let mut runs = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if parsed[index].is_none() {
            index += 1;
            continue;
        }
        let start = index;
        while index < lines.len() && parsed[index].is_some() {
            index += 1;
        }
        let chapters: Vec<Chapter> =
            parsed[start..index].iter().flatten().cloned().collect();
        if !is_chapter_run(&chapters, duration_ms) {
            continue;
        }
        let run = runs.len();
        for role in &mut roles[start..index] {
            *role = Role::Chapter(run);
        }
        if let Some(label) = start
            .checked_sub(1)
            .filter(|label| roles[*label] == Role::Normal)
            .filter(|label| is_label(&lines[*label]))
        {
            roles[label] = Role::Label;
        }
        runs.push(chapters);
    }
    runs
}

fn is_chapter_run(chapters: &[Chapter], duration_ms: Option<u64>) -> bool {
    let (Some(first), Some(last)) = (chapters.first(), chapters.last()) else {
        return false;
    };
    chapters.len() >= MIN_CHAPTERS
        && first.start_secs <= MAX_FIRST_CHAPTER_SECS
        && chapters
            .windows(2)
            .all(|pair| pair[0].start_secs < pair[1].start_secs)
        && duration_ms
            .is_none_or(|duration| u64::from(last.start_secs) * 1000 < duration)
}

fn is_label(line: &Line) -> bool {
    let words = line.text.split_whitespace().count();
    (1..=MAX_LABEL_WORDS).contains(&words)
        && !line.has_link
        && !is_sponsor_header(&line.text)
}

fn is_sponsor_header(text: &str) -> bool {
    static SPONSOR: OnceLock<Regex> = OnceLock::new();
    SPONSOR
        .get_or_init(|| {
            Regex::new(
                r"(?i)^(this episode is )?(brought to you by|sponsored by|thanks to our sponsors|our sponsors|sponsors)\b",
            )
            .expect("valid sponsor header")
        })
        .is_match(text)
}

fn mark_sponsors(lines: &[Line], roles: &mut [Role]) {
    let count = lines.len();
    let open = |roles: &[Role], index: usize| {
        roles[index] == Role::Normal && !is_sponsor_header(&lines[index].text)
    };
    let mut index = 0;
    while index < count {
        if roles[index] != Role::Normal
            || !is_sponsor_header(&lines[index].text)
        {
            index += 1;
            continue;
        }
        let start = index;
        let mut next = start + 1;
        // (end of the run, the separator that ended it)
        let terminated: Option<(usize, Option<usize>)> =
            if next < count && open(roles, next) && lines[next].bullet {
                while next < count && open(roles, next) && lines[next].bullet {
                    next += 1;
                }
                let separator = (next < count
                    && roles[next] == Role::Separator)
                    .then_some(next);
                Some((next, separator))
            } else {
                while next < count
                    && next - start <= MAX_PLAIN_SPONSOR_LINES
                    && open(roles, next)
                {
                    next += 1;
                }
                (next < count
                    && next - start <= MAX_PLAIN_SPONSOR_LINES + 1
                    && roles[next] == Role::Separator)
                    .then_some((next, Some(next)))
            };
        let Some((end, separator)) = terminated else {
            index += 1;
            continue;
        };
        for role in &mut roles[start..end] {
            *role = Role::Sponsor;
        }
        if let Some(separator) = separator {
            roles[separator] = Role::Dropped;
        }
        index = end;
    }
}

// ---------------------------------------------------------------------
// Emission.
// ---------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Html(String),
    Separator,
    Chapters(usize),
}

/// Where a line's markup goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    Body,
    Sponsors,
    Separator,
    Chapters(usize),
    Nowhere,
}

fn destination(role: Role) -> Destination {
    match role {
        Role::Normal => Destination::Body,
        Role::Sponsor => Destination::Sponsors,
        Role::Separator => Destination::Separator,
        Role::Chapter(run) => Destination::Chapters(run),
        Role::Label | Role::Dropped => Destination::Nowhere,
    }
}

fn emit(layout: &Layout, classified: &Classified) -> ShowNotes {
    let roles = &classified.roles;
    let mut tokens: Vec<Token> = Vec::new();
    let mut sponsors: Vec<String> = Vec::new();
    let mut emitted_runs = vec![false; classified.runs.len()];
    for group in &layout.groups {
        let destinations: Vec<Destination> = group
            .lines
            .clone()
            .map(|line| destination(roles[line]))
            .collect();
        let uniform = destinations
            .first()
            .filter(|first| destinations.iter().all(|other| other == *first));
        match uniform {
            Some(Destination::Body) => {
                tokens.push(Token::Html(group.html.clone()));
                continue;
            }
            Some(Destination::Sponsors) => {
                sponsors.push(group.html.clone());
                continue;
            }
            _ => {}
        }
        let mut cursor = group.lines.start;
        while cursor < group.lines.end {
            let target = destination(roles[cursor]);
            let mut end = cursor + 1;
            while end < group.lines.end && destination(roles[end]) == target {
                end += 1;
            }
            let lines = &layout.lines[cursor..end];
            match target {
                Destination::Body => {
                    tokens.push(Token::Html(rewrap(&group.kind, lines)));
                }
                Destination::Sponsors => {
                    sponsors.push(rewrap(&group.kind, lines));
                }
                Destination::Separator => {
                    tokens.push(Token::Separator);
                }
                Destination::Chapters(run) => {
                    if !emitted_runs[run] {
                        emitted_runs[run] = true;
                        tokens.push(Token::Chapters(run));
                    }
                }
                Destination::Nowhere => {}
            }
            cursor = end;
        }
    }

    let mut blocks = Vec::new();
    let mut segment: Vec<Token> = Vec::new();
    for token in tokens {
        if let Token::Chapters(run) = token {
            push_html_segment(&mut blocks, std::mem::take(&mut segment));
            blocks.push(NotesBlock::Chapters(classified.runs[run].clone()));
        } else {
            segment.push(token);
        }
    }
    push_html_segment(&mut blocks, segment);
    if !sponsors.is_empty() {
        blocks.push(NotesBlock::Sponsors(sponsors.concat()));
    }
    let words = layout
        .lines
        .iter()
        .map(|line| {
            line.text
                .split_whitespace()
                .filter(|word| word.chars().any(char::is_alphanumeric))
                .count()
        })
        .sum();
    ShowNotes { blocks, words }
}

/// Rewrap the pieces of a split group in its container.
fn rewrap(kind: &GroupKind, lines: &[Line]) -> String {
    let joined = || {
        lines
            .iter()
            .map(|line| line.html.as_str())
            .collect::<Vec<_>>()
    };
    match kind {
        GroupKind::Block(tag) => {
            format!("<{tag}>{}</{tag}>", joined().join("<br>"))
        }
        GroupKind::List(tag) => {
            let items: String = lines
                .iter()
                .map(|line| format!("<li>{}</li>", line.html))
                .collect();
            format!("<{tag}>{items}</{tag}>")
        }
        GroupKind::Inline => joined().join("<br>"),
        GroupKind::Opaque | GroupKind::Rule => joined().concat(),
    }
}

/// Trim separators at the segment's edges, collapse repeats, turn the
/// rest into `<hr>`, and push the merged HTML (if any) as one block.
fn push_html_segment(blocks: &mut Vec<NotesBlock>, segment: Vec<Token>) {
    let mut html = String::new();
    let mut pending_rule = false;
    for token in segment {
        match token {
            Token::Separator => pending_rule = !html.is_empty(),
            Token::Html(fragment) => {
                if pending_rule {
                    html.push_str("<hr>");
                    pending_rule = false;
                }
                html.push_str(&fragment);
            }
            Token::Chapters(_) => {}
        }
    }
    if !html.trim().is_empty() {
        blocks.push(NotesBlock::Html(html));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRAGMATIC: &str = include_str!(
        "../../../../tests/fixtures/podcast/pragmatic_engineer_notes.html"
    );
    /// The Harjot Gill episode: 1 h 26 min 37 s.
    const PRAGMATIC_DURATION_MS: u64 = 5_197_032;

    fn html_blocks(notes: &ShowNotes) -> Vec<&str> {
        notes
            .blocks
            .iter()
            .filter_map(|block| match block {
                NotesBlock::Html(html) => Some(html.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_real_notes_yield_sixteen_chapters_with_the_label_consumed() {
        let notes = segment_notes(PRAGMATIC, Some(PRAGMATIC_DURATION_MS));
        let chapters: Vec<&Chapter> = notes.chapters().collect();
        assert_eq!(chapters.len(), 16);
        assert_eq!(
            chapters.first().map(|c| (c.start_secs, c.title.as_str())),
            Some((0, "Intro"))
        );
        assert_eq!(
            chapters.get(1).map(|c| (c.start_secs, c.title.as_str())),
            Some((
                197,
                "How Harjot shipped over 5,000 pull requests in a month"
            ))
        );
        assert_eq!(
            chapters.last().map(|c| (c.start_secs, c.title.as_str())),
            Some((5070, "Book recommendation"))
        );
        // Shape: notes, chapters, notes, sponsors.
        assert!(matches!(
            notes.blocks.as_slice(),
            [
                NotesBlock::Html(_),
                NotesBlock::Chapters(_),
                NotesBlock::Html(_),
                NotesBlock::Sponsors(_)
            ]
        ));
        let html = html_blocks(&notes);
        assert!(
            html.iter().all(|block| !block.contains("Timestamps")),
            "the label must be consumed: {html:?}"
        );
        assert!(
            html[0].starts_with("<p>In this conversation"),
            "{}",
            html[0]
        );
        assert!(html[0].ends_with("skills for success.</p>"), "{}", html[0]);
        // The separator after the chapters is trimmed at the block edge;
        // the interior one becomes a rule.
        assert!(
            html[1].starts_with("<p>The Pragmatic Engineer deepdives"),
            "{}",
            html[1]
        );
        assert_eq!(html[1].matches("<hr>").count(), 1, "{}", html[1]);
        assert!(html[1].contains("Get full access to The Pragmatic Engineer"));
        assert!(!html[1].contains('\u{2060}'));
        assert!(notes.words > 400, "{}", notes.words);
    }

    #[test]
    fn the_brought_to_you_by_run_is_demoted_at_its_separator() {
        let notes = segment_notes(PRAGMATIC, Some(PRAGMATIC_DURATION_MS));
        let sponsors = notes.sponsors_html().expect("sponsors");
        assert!(
            sponsors.starts_with("<p>Brought to You By:</p>"),
            "{sponsors}"
        );
        for name in ["turbopuffer", "Entire", "WorkOS"] {
            assert!(sponsors.contains(name), "{name}: {sponsors}");
        }
        assert!(!sponsors.contains('—'), "{sponsors}");
        assert!(!sponsors.contains("In this conversation"));
        assert!(matches!(notes.blocks.last(), Some(NotesBlock::Sponsors(_))));
        assert!(html_blocks(&notes)
            .iter()
            .all(|block| !block.contains("turbopuffer")));
    }

    #[test]
    fn a_sponsor_run_with_no_terminator_stays_in_place() {
        let notes = segment_notes(
            "<p>Sponsored by Acme, the anvil people.</p><p>Body one.</p><p>Body two.</p>",
            None,
        );
        assert_eq!(notes.sponsors_html(), None);
        assert_eq!(
            notes.blocks,
            vec![NotesBlock::Html(
                "<p>Sponsored by Acme, the anvil people.</p><p>Body one.</p><p>Body two.</p>"
                    .to_string()
            )]
        );
    }

    #[test]
    fn a_bullet_run_ends_a_sponsor_run_without_a_separator() {
        let notes = segment_notes(
            "<p>Our sponsors:</p><ul><li>Acme</li><li>Globex</li></ul><p>Main text.</p>",
            None,
        );
        assert_eq!(
            notes.blocks,
            vec![
                NotesBlock::Html("<p>Main text.</p>".to_string()),
                NotesBlock::Sponsors(
                    "<p>Our sponsors:</p><ul><li>Acme</li><li>Globex</li></ul>"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_far_separator_never_swallows_the_description() {
        let mut html =
            String::from("<p>This episode is sponsored by Acme.</p>");
        for n in 0..12 {
            html.push_str(&format!("<p>Paragraph {n}.</p>"));
        }
        html.push_str("<p>---</p><p>Links.</p>");
        let notes = segment_notes(&html, None);
        assert_eq!(notes.sponsors_html(), None);
    }

    #[test]
    fn non_increasing_times_are_not_chapters() {
        let html =
            "<p>00:00 Intro</p><p>05:00 Middle</p><p>03:00 Back again</p>";
        let notes = segment_notes(html, None);
        assert_eq!(notes.chapters().count(), 0);
        assert_eq!(notes.blocks, vec![NotesBlock::Html(html.to_string())]);
    }

    #[test]
    fn chapter_runs_respect_count_start_and_duration() {
        // Two lines are not a run.
        let two = segment_notes("<p>0:00 A</p><p>1:00 B</p>", None);
        assert_eq!(two.chapters().count(), 0);
        // The first chapter must start by 10:00.
        let late =
            segment_notes("<p>10:01 A</p><p>20:00 B</p><p>30:00 C</p>", None);
        assert_eq!(late.chapters().count(), 0);
        // The last chapter must start before the episode ends.
        let html = "<p>0:00 A</p><p>10:00 B</p><p>20:00 C</p>";
        assert_eq!(
            segment_notes(html, Some(20 * 60 * 1000)).chapters().count(),
            0
        );
        assert_eq!(
            segment_notes(html, Some(20 * 60 * 1000 + 1))
                .chapters()
                .count(),
            3
        );
        assert_eq!(segment_notes(html, None).chapters().count(), 3);
    }

    #[test]
    fn br_lines_and_list_items_form_runs_in_place() {
        let notes = segment_notes(
            "<p>Before.</p><p>Chapters:<br>(0:00) Intro<br>[1:30] - Two<br>1:02:03 | Three</p><p>After.</p>",
            None,
        );
        assert_eq!(
            notes.blocks,
            vec![
                NotesBlock::Html("<p>Before.</p>".to_string()),
                NotesBlock::Chapters(vec![
                    Chapter {
                        start_secs: 0,
                        title: "Intro".to_string()
                    },
                    Chapter {
                        start_secs: 90,
                        title: "Two".to_string()
                    },
                    Chapter {
                        start_secs: 3723,
                        title: "Three".to_string()
                    },
                ]),
                NotesBlock::Html("<p>After.</p>".to_string()),
            ]
        );

        let listed = segment_notes(
            "<ul><li>Keep this whole item please</li><li>00:00 One</li><li>02:00 Two</li><li>04:00 Three</li></ul>",
            None,
        );
        assert_eq!(
            listed.blocks,
            vec![
                NotesBlock::Html(
                    "<ul><li>Keep this whole item please</li></ul>".to_string()
                ),
                NotesBlock::Chapters(vec![
                    Chapter {
                        start_secs: 0,
                        title: "One".to_string()
                    },
                    Chapter {
                        start_secs: 120,
                        title: "Two".to_string()
                    },
                    Chapter {
                        start_secs: 240,
                        title: "Three".to_string()
                    },
                ]),
            ]
        );
    }

    #[test]
    fn a_long_line_before_a_run_is_not_a_label() {
        let notes = segment_notes(
            "<p>Here is what we covered in this one</p><p>0:00 A</p><p>1:00 B</p><p>2:00 C</p>",
            None,
        );
        assert_eq!(
            notes.blocks.first(),
            Some(&NotesBlock::Html(
                "<p>Here is what we covered in this one</p>".to_string()
            ))
        );
    }

    #[test]
    fn edge_separators_trim_and_interior_ones_become_rules() {
        let notes = segment_notes(
            "<p>—</p><hr><p>One.</p><p>***</p><p>___</p><p>Two.</p><p>• • •</p>",
            None,
        );
        assert_eq!(
            notes.blocks,
            vec![NotesBlock::Html("<p>One.</p><hr><p>Two.</p>".to_string())]
        );
    }

    #[test]
    fn wrappers_unwrap_and_trailing_inline_text_is_kept() {
        let notes = segment_notes(
            "<div><p>0:00 A</p><p>1:00 B</p><p>2:00 C</p></div> <br /><br />Get full access at <a href=\"https://example.com\">example.com</a>",
            None,
        );
        assert_eq!(notes.chapters().count(), 3);
        assert_eq!(
            html_blocks(&notes),
            vec![" <br><br>Get full access at <a href=\"https://example.com\">example.com</a>"]
        );
    }

    #[test]
    fn plain_text_notes_segment_line_by_line() {
        let notes = segment_text(
            "Intro text & some more\n\n00:00 Start\n05:00 Middle\n09:00 End\n",
            Some(3_600_000),
        );
        assert_eq!(
            notes.blocks,
            vec![
                NotesBlock::Html(
                    "<p>Intro text &amp; some more</p>".to_string()
                ),
                NotesBlock::Chapters(vec![
                    Chapter {
                        start_secs: 0,
                        title: "Start".to_string()
                    },
                    Chapter {
                        start_secs: 300,
                        title: "Middle".to_string()
                    },
                    Chapter {
                        start_secs: 540,
                        title: "End".to_string()
                    },
                ]),
            ]
        );
        assert_eq!(notes.words, 10);
        assert!(segment_notes("", None).is_empty());
    }
}
