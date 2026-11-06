//! Plain-text reply splitting for Gmail bodies.
//!
//! Plain email is untrusted text, never raw Markdown passed through. This
//! module folds quoted history and signatures into separate inert prose and
//! preserves useful paragraph and list shape through the shared structured
//! serializer (`inert::render_blocks`), so sender lines can never introduce
//! vault structure of their own.
//!
//! Segmentation rules, applied top to bottom:
//!
//! * The exact `-- ` signature delimiter (two dashes plus one trailing
//!   space, nothing else) switches the rest of the pre-history text into the
//!   signature segment. A bare `--` or an unrelated dash rule never delimits,
//!   so ambiguous prose stays in its segment. A second delimiter inside
//!   quoted history does not start a new signature: history stays history.
//! * A `>`-prefixed line is a quoted run line: one leading `>` (plus one
//!   optional space) is stripped and the remainder joins the quoted segment.
//!   Inline replies outside quoted runs stay in main text.
//! * An `On ... wrote:` line never switches segments on its own: it drops its
//!   own attribution line when a later `>` run proves history, and lets those
//!   `>` runs fold while following unquoted inline replies stay in main.
//!   Without quoted evidence the line is ordinary prose, so a sentence like
//!   `On second thought, I wrote:` is never treated as history on its own.
//!   The divider line itself is dropped.
//! * A strong Outlook divider (a `--`/`__` rule naming the original or
//!   forwarded message) starts a quote tail unconditionally: forwards carry
//!   no `>` prefixes. The divider line itself is dropped; the `From:`/`Sent:`
//!   header block that follows stays in quoted text as readable attribution.
//! * Inside a quote tail every line joins the quoted segment, quoted or not.
//!   Ambiguous content is never deleted: when no rule fires, text stays in
//!   the segment it already belongs to.
//!
//! Main, quoted, and signature lines each become paragraph blocks, with
//! `-/+/*` and `N./N)` line markers interpreted as renderer-owned lists. The
//! markers live in [`inert::Block::List`]; item text is raw sender text and
//! is escaped at render time, so a hostile `- [ ] task` line renders as an
//! inert list item, never a vault checkbox.

use super::inert::{render_blocks, Block, Inline};

/// Segmented rendering of one plain-text candidate. All three prose fields
/// are already inert; they contain body content, not mail-note wrappers.
pub(crate) struct PlainRender {
    pub(crate) markdown: String,
    pub(crate) quoted: Option<String>,
    pub(crate) signature: Option<String>,
}

/// Split plain text into main, quoted-history, and signature prose.
///
/// Pure: no I/O, no sender bytes in errors (there are no errors). Empty
/// segments render to `None`; a message with no readable lines at all yields
/// empty `markdown` and no segments.
pub(crate) fn render_plain(text: &str) -> PlainRender {
    let normalized = normalize(text);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut main: Vec<String> = Vec::new();
    let mut quoted: Vec<String> = Vec::new();
    let mut signature: Vec<String> = Vec::new();

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Segment {
        Main,
        Quoted,
        Signature,
    }
    let mut segment = Segment::Main;
    // Suffix evidence: a quote-tail `On ... wrote:` line only takes effect
    // when a `>` line follows it.
    let mut later_quote = vec![false; lines.len() + 1];
    for (index, line) in lines.iter().enumerate().rev() {
        later_quote[index] = later_quote[index + 1] || is_quote_line(line);
    }

    for (index, line) in lines.iter().enumerate() {
        if segment != Segment::Quoted {
            if is_signature_delimiter(line) {
                segment = Segment::Signature;
                continue;
            }
            // Quoted runs fold from Main and from Signature (a signature
            // before history): `>` lines always join the quoted segment
            // without switching, so new unquoted inline replies stay in main.
            if (segment == Segment::Main || segment == Segment::Signature)
                && is_quote_line(line)
            {
                quoted.push(strip_quote_prefix(line));
                continue;
            }
            // Unprefixed forwarded-message tails (strong Outlook dividers)
            // switch permanently: forwards carry no `>` prefixes, so every
            // following line is history. `On ... wrote:` never switches: it
            // only drops its own attribution line and lets following `>`
            // runs fold via the quoted-run path above, so new unquoted
            // inline replies stay in main text. A quote tail can follow the
            // signature: the replier signs above the history their client
            // appended.
            if is_outlook_divider(line) {
                segment = Segment::Quoted;
                continue;
            }
            if is_on_wrote(line) && later_quote[index + 1] {
                continue;
            }
        }
        match segment {
            Segment::Main => main.push((*line).to_string()),
            Segment::Quoted => quoted.push(quote_tail_line(line)),
            Segment::Signature => signature.push((*line).to_string()),
        }
    }

    PlainRender {
        markdown: render_optional(&shape_blocks(&main)),
        quoted: render_nonempty(&shape_blocks(&quoted)),
        signature: render_nonempty(&shape_blocks(&signature)),
    }
}

/// Fold CRLF/CR to LF so line rules see uniform endings. Stray controls are
/// left for the inert renderer, which drops them without hiding delimiters.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The exact `-- ` signature delimiter (two dashes plus one trailing
/// space). A bare `--` or an unrelated dash rule (`---`, `-- foo`) never
/// delimits: ambiguous content stays in its segment rather than silently
/// moving ordinary prose into a signature.
fn is_signature_delimiter(line: &str) -> bool {
    line == "-- "
}

/// A `>`-quoted run line: the first non-blank character is `>`.
fn is_quote_line(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

/// Strip one reply level: leading whitespace, one `>`, one optional space.
fn strip_quote_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    let rest = trimmed.get(1..).unwrap_or("");
    rest.strip_prefix(' ').unwrap_or(rest).to_string()
}

/// Inside a quote tail, `>` lines lose one reply level; every other line is
/// history verbatim (forwarded bodies carry no `>` prefixes).
fn quote_tail_line(line: &str) -> String {
    if is_quote_line(line) {
        strip_quote_prefix(line)
    } else {
        line.to_string()
    }
}

/// Gmail's `On <date>, <author> wrote:` reply boundary. The `later_quote`
/// evidence check at the call site keeps ordinary sentences out.
fn is_on_wrote(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() > 512 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    lower.starts_with("on ") && lower.ends_with("wrote:")
}

/// A strong Outlook divider: a `--`/`__` rule naming the original or
/// forwarded message. Ordinary dash rules and prose never match.
fn is_outlook_divider(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() > 256 {
        return false;
    }
    let ruled = trimmed.starts_with("--") || trimmed.starts_with("__");
    if !ruled {
        return false;
    }
    let lower = trimmed.to_lowercase();
    lower.contains("original message") || lower.contains("forwarded message")
}

/// Group lines into paragraph and renderer-owned list blocks.
fn shape_blocks(lines: &[String]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut list: Vec<ListItem> = Vec::new();
    let mut list_ordered = false;
    let mut list_start = 1u32;

    for line in lines {
        if line.trim().is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, list_ordered, list_start, &mut blocks);
            continue;
        }
        if let Some(item) = parse_list_item(line) {
            flush_paragraph(&mut paragraph, &mut blocks);
            if list.is_empty()
                || list_ordered != item.ordered
                || (item.ordered
                    && item.number.is_some_and(|n| {
                        n != list_last_number(&list, list_start)
                    }))
            {
                flush_list(&mut list, list_ordered, list_start, &mut blocks);
                list_ordered = item.ordered;
                list_start = item.number.unwrap_or(1);
            }
            list.push(item);
        } else {
            flush_list(&mut list, list_ordered, list_start, &mut blocks);
            paragraph.push(line.trim().to_string());
        }
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    flush_list(&mut list, list_ordered, list_start, &mut blocks);
    blocks
}

fn flush_paragraph(paragraph: &mut Vec<String>, blocks: &mut Vec<Block>) {
    if paragraph.is_empty() {
        return;
    }
    blocks.push(Block::Paragraph(vec![Inline::Text(
        std::mem::take(paragraph).join("\n"),
    )]));
}

fn flush_list(
    list: &mut Vec<ListItem>,
    ordered: bool,
    start: u32,
    blocks: &mut Vec<Block>,
) {
    if list.is_empty() {
        return;
    }
    let items = std::mem::take(list)
        .into_iter()
        .map(|item| vec![Block::Paragraph(vec![Inline::Text(item.text)])])
        .collect();
    blocks.push(Block::List {
        ordered,
        start,
        items,
    });
}

struct ListItem {
    ordered: bool,
    number: Option<u32>,
    text: String,
}

/// Expected number for the next ordered item: start, then one past the last
/// parsed number, else sequential from the previous item.
fn list_last_number(list: &[ListItem], start: u32) -> u32 {
    list.iter()
        .rev()
        .find_map(|item| item.number)
        .map(|n| n.saturating_add(1))
        .unwrap_or_else(|| start.saturating_add(list.len() as u32))
}

/// Split a `-`/`+`/`*` or `N.`/`N)` line marker. Returns `None` for prose,
/// including `-` runs without trailing content.
fn parse_list_item(line: &str) -> Option<ListItem> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if matches!(first, '-' | '+' | '*') {
        let rest = chars.as_str();
        if rest.is_empty() || !rest.starts_with([' ', '\t']) {
            return None;
        }
        let text = rest.trim().to_string();
        if text.is_empty() {
            return None;
        }
        return Some(ListItem {
            ordered: false,
            number: None,
            text,
        });
    }
    if first.is_ascii_digit() {
        let digits: String =
            trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = &trimmed[digits.len()..];
        let mut rest_chars = rest.chars();
        let marker = rest_chars.next()?;
        if !matches!(marker, '.' | ')') {
            return None;
        }
        let after = rest_chars.as_str();
        if !after.is_empty() && !after.starts_with([' ', '\t']) {
            return None;
        }
        let text = after.trim().to_string();
        if text.is_empty() {
            return None;
        }
        return Some(ListItem {
            ordered: true,
            number: digits.parse::<u32>().ok(),
            text,
        });
    }
    None
}

fn render_optional(blocks: &[Block]) -> String {
    render_blocks(blocks).trim().to_string()
}

fn render_nonempty(blocks: &[Block]) -> Option<String> {
    let rendered = render_optional(blocks);
    (!rendered.is_empty()).then_some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::collect_done::block_ids_in_markdown;
    use crate::native::task_dependencies::raw_wikilink_spans;
    use crate::native::task_fields::all_inline_fields;
    use crate::native::task_status_hooks::{
        parse_tasks, TaskStatusType, TasksSettings,
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn scanner_settings() -> TasksSettings {
        TasksSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
                ('X', TaskStatusType::Done),
                ('/', TaskStatusType::InProgress),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        }
    }

    fn assert_inert(doc: &str) {
        let settings = scanner_settings();
        assert!(
            parse_tasks(doc, &settings).is_empty(),
            "tasks in plain output:\n{doc}"
        );
        for line in doc.split('\n') {
            assert!(
                all_inline_fields(line).is_empty(),
                "inline field in {line:?} of:\n{doc}"
            );
            assert!(
                raw_wikilink_spans(line).is_empty(),
                "wikilink in {line:?} of:\n{doc}"
            );
        }
        assert!(
            block_ids_in_markdown(doc).is_empty(),
            "block ids in plain output:\n{doc}"
        );
        for line in doc.split('\n') {
            for forbidden in ["<%", "<!--", "![", "[["] {
                assert!(
                    !line.contains(forbidden),
                    "forbidden {forbidden:?} in {line:?} of:\n{doc}"
                );
            }
        }
    }

    #[test]
    fn splits_quote_runs_signature_and_on_wrote_tails() {
        let result = render_plain(
            "New reply\nwith two lines\n-- \nSent from my phone\nOn Tue, Pat wrote:\n> Old line one\n> Old line two\n",
        );
        assert_eq!(result.markdown, "New reply\nwith two lines");
        assert_eq!(
            result.quoted.as_deref(),
            Some("Old line one\nOld line two")
        );
        assert_eq!(result.signature.as_deref(), Some("Sent from my phone"));
    }

    #[test]
    fn keeps_inline_replies_in_main_and_folds_outlook_forward() {
        let result = render_plain(
            "Yes, agreed.\n> Did you ship it?\nNot yet, Friday.\n> Old context\n",
        );
        assert!(result.markdown.contains("Yes, agreed."));
        assert!(result.markdown.contains("Not yet, Friday."));
        assert!(!result.markdown.contains("Did you ship"));
        assert!(result.quoted.as_deref().unwrap().contains("Did you ship"));

        let outlook = render_plain(
            "See below.\n---------- Forwarded message ---------\nFrom: Pat <pat@example.test>\nSent: Tuesday\n\nForwarded body here.\n",
        );
        assert_eq!(outlook.markdown, "See below.");
        let quoted = outlook.quoted.as_deref().unwrap();
        assert!(quoted.contains("Forwarded body here."));
        assert!(quoted.contains("From: Pat"));
        assert!(!quoted.contains("Forwarded message ---"));
    }

    #[test]
    fn ordinary_sentences_are_never_history_or_signatures() {
        // No quoted evidence: the `wrote:` sentence stays prose.
        let result =
            render_plain("On second thought, I wrote:\nA follow-up idea.\n");
        assert!(result.quoted.is_none());
        assert!(result.markdown.contains("On second thought"));

        // Only the exact `-- ` (two dashes plus one trailing space)
        // delimits. A bare `--` stays in main text so stripped transports
        // never silently move ordinary prose into a signature.
        let exact = render_plain("Section one\n-- \nSection two\n");
        assert!(exact.signature.as_deref() == Some("Section two"));
        assert!(exact.markdown.contains("Section one"));

        let bare = render_plain("Section one\n--\nSection two\n");
        assert!(bare.signature.is_none());
        assert!(bare.markdown.contains("Section one"));
        assert!(bare.markdown.contains("Section two"));

        // Dashes that are not the exact delimiter stay in main text.
        let rule = render_plain("Part one\n---\nPart two\n");
        assert!(rule.signature.is_none());
        assert!(rule.markdown.contains("Part one"));
        assert!(rule.markdown.contains("Part two"));
    }

    #[test]
    fn preserves_paragraph_and_list_shape_without_vault_syntax() {
        let result = render_plain(
            "Groceries\n\n- milk [[link]]\n- eggs #tag\n\n1. first [key:: v]\n2. second - [ ] task\n",
        );
        assert!(result.markdown.contains("Groceries"));
        assert!(result.markdown.contains("- milk"));
        assert!(result.markdown.contains("1. first"));
        assert_inert(&result.markdown);
    }

    #[test]
    fn inline_answer_after_on_wrote_stays_main() {
        let result = render_plain(
            "On Tue, Pat wrote:\n> old question\nNew inline answer\n",
        );
        assert!(result.markdown.contains("New inline answer"));
        assert!(!result
            .quoted
            .as_deref()
            .unwrap_or("")
            .contains("New inline"));
        assert!(result.quoted.as_deref().unwrap().contains("old question"));
    }

    #[test]
    fn signature_before_history_and_strong_outlook_headers_fold() {
        let sig_first = render_plain(
            "Thanks!\n-- \nPat\nOn Tue, Sam wrote:\n> Prior work\n",
        );
        assert!(sig_first.markdown.contains("Thanks!"));
        assert_eq!(sig_first.signature.as_deref(), Some("Pat"));
        assert!(sig_first.quoted.as_deref().unwrap().contains("Prior"));

        let outlook = render_plain(
            "See below.\n---------- Forwarded message ---------\nFrom: Dana <d@example.test>\nSent: Monday\nTo: You\nSubject: Hi\n\nForwarded text\n",
        );
        assert_eq!(outlook.markdown, "See below.");
        let quoted = outlook.quoted.as_deref().unwrap();
        assert!(quoted.contains("Forwarded text"));
        assert!(quoted.contains("From: Dana"));
    }

    #[test]
    fn crlf_and_nested_quotes_fold_deterministically() {
        let input = "Reply\r\n> Level one\r\n>> Level two\r\n-- \r\nSig\n";
        let first = render_plain(input);
        let second = render_plain(input);
        assert_eq!(first.markdown, second.markdown);
        assert_eq!(first.quoted, second.quoted);
        assert_eq!(first.signature, second.signature);
        assert!(first.markdown.contains("Reply"));
        assert!(!first.markdown.contains('\r'));
        let quoted = first.quoted.as_deref().unwrap();
        assert!(quoted.contains("Level one"));
        // One reply level is stripped; the inner marker stays escaped text.
        assert!(!quoted.lines().any(|line| line.starts_with("> ")));
        assert_eq!(first.signature.as_deref(), Some("Sig"));
    }
}
