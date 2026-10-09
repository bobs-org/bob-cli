//! v1 tracker line migration to a v2 reading-task line.
//!
//! [`migrate_v1_tracker_line`] starts from the v1 tracker line, keeps the
//! mark and every field except `#hide` and the PDF wikilink, ensures the
//! adjacent `#task #ref` pair, inserts the path-qualified ref link with a
//! sanitized title alias where the PDF link sat, ensures a
//! `[created::YYYY-MM-DD]`, and ends with the preview `^ref-<slug>` id.

use crate::native::ref_tasks::sanitize_title_alias;

/// Migrate one v1 tracker line to its v2 reading-task shape.
///
/// `v1_line` is the raw tracker line (leading indentation preserved).
/// `ref_target` is the vault-relative ref path without `.md`
/// (`ref/papers/stem`). `title_raw` is already resolved (H1, frontmatter
/// title, or stem) and is sanitized here. `created` is `YYYY-MM-DD`.
/// `preview_block_id` is the allocated `^ref-<slug>` without the caret.
pub(crate) fn migrate_v1_tracker_line(
    v1_line: &str,
    ref_target: &str,
    title_raw: &str,
    created: &str,
    preview_block_id: &str,
) -> String {
    let indent_len = v1_line
        .find(|c: char| !c.is_whitespace() || c == '\n' || c == '\r')
        .unwrap_or(0);
    // Preserve only spaces/tabs as indentation.
    let mut indent_end = 0usize;
    for b in v1_line.bytes() {
        if b == b' ' || b == b'\t' {
            indent_end += 1;
        } else {
            break;
        }
    }
    let indent = &v1_line[..indent_end];
    let rest = &v1_line[indent_end..];

    // Checkbox prefix: `- [m] ` (or `*`/`+`), preserved verbatim.
    let (checkbox, after_checkbox) = split_checkbox(rest);
    let mut body = after_checkbox.to_string();

    // Record PDF link position before removal so the new link lands there.
    let pdf_span = find_pdf_wikilink_span(&body);
    if let Some((open, end)) = pdf_span {
        body = format!("{} {}", &body[..open].trim_end(), &body[end..]);
        // Normalize: collapse leading double spaces from removal.
        body = collapse_spaces(&body);
    }

    // Drop whole-token `#hide` (ASCII case-sensitive, matching birth).
    body = drop_hide_token(&body);

    // Remove trailing `^ref` token (the v1 block id).
    body = remove_trailing_caret_ref(&body);
    body = collapse_spaces(&body);

    // Ensure adjacent `#task #ref`.
    body = ensure_task_ref_pair(&body);

    // Insert the path-qualified ref link where the PDF link sat, else
    // right after `#ref`.
    let alias = sanitize_title_alias(title_raw);
    let new_link = format!("[[{ref_target}|{alias}]]");
    body = insert_ref_link(&body, &new_link, pdf_span.is_some());

    // Ensure `[created::DATE]`.
    if !body.contains("[created::") {
        body = format!("{body} [created::{created}]");
    }

    body = collapse_spaces(&body);
    let _ = indent_len;
    format!("{indent}{checkbox}{body} ^{preview_block_id}")
}

/// Split `- [m] ` (or `*`/`+`) from the rest; falls back to empty prefix.
fn split_checkbox(line: &str) -> (String, String) {
    for marker in ["- ", "* ", "+ "] {
        if let Some(after) = line.strip_prefix(marker) {
            if after.starts_with('[') {
                if let Some(close) = after.find(']') {
                    let mark_part = &after[..close + 1];
                    let rest = after[close + 1..].trim_start().to_string();
                    return (format!("{marker}{mark_part} "), rest);
                }
            }
        }
    }
    (String::new(), line.to_string())
}

/// Byte span of the first PDF wikilink `[[...]]`, if any.
///
/// A PDF link targets `lib/` or `xlib/`, or contains `.pdf`
/// (case-insensitive) before any `|` or `#`.
fn find_pdf_wikilink_span(body: &str) -> Option<(usize, usize)> {
    let mut search_from = 0usize;
    while let Some(open_rel) = body[search_from..].find("[[") {
        let open = search_from + open_rel;
        let after = &body[open + 2..];
        let Some(close_rel) = after.find("]]") else {
            break;
        };
        let end = open + 2 + close_rel + 2;
        let inside = &body[open + 2..open + 2 + close_rel];
        let target = inside
            .split('|')
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        let lower = target.to_ascii_lowercase();
        if lower.starts_with("lib/")
            || lower.starts_with("xlib/")
            || lower.contains(".pdf")
        {
            return Some((open, end));
        }
        search_from = end;
    }
    None
}

fn drop_hide_token(body: &str) -> String {
    let tokens: Vec<&str> = body.split_whitespace().collect();
    let kept: Vec<&str> =
        tokens.into_iter().filter(|t| *t != "#hide").collect();
    kept.join(" ")
}

fn remove_trailing_caret_ref(body: &str) -> String {
    let mut tokens: Vec<&str> = body.split_whitespace().collect();
    // V1 ends with exactly `^ref`; also drop a stray `^ref-*` preview.
    if let Some(last) = tokens.last() {
        if *last == "^ref" || last.starts_with("^ref-") {
            tokens.pop();
        }
    }
    tokens.join(" ")
}

fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ensure_task_ref_pair(body: &str) -> String {
    let mut tokens: Vec<String> =
        body.split_whitespace().map(str::to_string).collect();
    let has_task = tokens.iter().any(|t| t.eq_ignore_ascii_case("#task"));
    let has_ref = tokens.iter().any(|t| t.eq_ignore_ascii_case("#ref"));
    if has_task && has_ref {
        // Make them adjacent: move `#ref` right after the first `#task`.
        let task_idx = tokens
            .iter()
            .position(|t| t.eq_ignore_ascii_case("#task"))
            .unwrap_or(0);
        let ref_idx = tokens
            .iter()
            .position(|t| t.eq_ignore_ascii_case("#ref"))
            .unwrap_or(0);
        if ref_idx != task_idx + 1 {
            let r = tokens.remove(ref_idx);
            let insert_at = if ref_idx < task_idx {
                task_idx
            } else {
                task_idx + 1
            };
            // Normalize to lowercase pair.
            tokens[task_idx] = "#task".to_string();
            tokens.insert(insert_at, "#ref".to_string());
            let _ = r;
        } else {
            tokens[task_idx] = "#task".to_string();
            tokens[task_idx + 1] = "#ref".to_string();
        }
        return tokens.join(" ");
    }
    if !has_task {
        tokens.insert(0, "#task".to_string());
    }
    if !has_ref {
        let task_idx = tokens
            .iter()
            .position(|t| t.eq_ignore_ascii_case("#task"))
            .unwrap_or(0);
        tokens.insert(task_idx + 1, "#ref".to_string());
    }
    tokens.join(" ")
}

fn insert_ref_link(body: &str, new_link: &str, had_pdf: bool) -> String {
    // If the body still holds a non-PDF wikilink (unexpected for v1),
    // leave it and append the ref link after `#ref`.
    let _ = had_pdf;
    let mut tokens: Vec<String> =
        body.split_whitespace().map(str::to_string).collect();
    if tokens
        .iter()
        .any(|t| t.contains("[[") && t.contains("]]") && t.contains("ref/"))
    {
        return tokens.join(" ");
    }
    // Insert right after `#ref`.
    if let Some(idx) =
        tokens.iter().position(|t| t.eq_ignore_ascii_case("#ref"))
    {
        tokens.insert(idx + 1, new_link.to_string());
    } else {
        tokens.push(new_link.to_string());
    }
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_basic_v1_line() {
        let out = migrate_v1_tracker_line(
            "- [ ] #task #ref [[lib/papers/x.pdf]] #hide ^ref",
            "ref/papers/x",
            "X Title",
            "2026-10-09",
            "ref-x",
        );
        assert_eq!(
            out,
            "- [ ] #task #ref [[ref/papers/x|X Title]] [created::2026-10-09] ^ref-x"
        );
    }

    #[test]
    fn keeps_fields_and_mark() {
        let out = migrate_v1_tracker_line(
            "- [*] #task #ref [[lib/papers/x.pdf]] #hide [fresh:: 2026-10-01] [id:: foo] ^ref",
            "ref/papers/x",
            "T",
            "2026-10-09",
            "ref-x",
        );
        assert!(out.starts_with("- [*] "), "{out}");
        assert!(out.contains("[fresh:: 2026-10-01]"), "{out}");
        assert!(out.contains("[id:: foo]"), "{out}");
        assert!(!out.contains("#hide"), "{out}");
        assert!(!out.contains("lib/papers"), "{out}");
        assert!(out.ends_with("^ref-x"), "{out}");
    }
}
