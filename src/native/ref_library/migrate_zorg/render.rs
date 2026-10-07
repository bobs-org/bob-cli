//! Rendering for `bob ref migrate-zorg` planned notes.
//!
//! One legacy note per non-chapter record, shaped exactly by the epic plan:
//! frontmatter in a fixed key order (every scalar a YAML double-quoted
//! string via JSON quoting, integers bare, lists in block style), then the
//! sase-4b.1 body with `## Files`, `## Related`, `## Chapters` (books),
//! and the escaped `## Original Record` fence.

/// Words uppercased whole when a record ID is humanized into a title.
const ACRONYMS: &[&str] = &[
    "ai", "api", "cli", "gtd", "html", "http", "json", "llm", "lsp", "mcp",
    "pdf", "prd", "sql", "ui", "url", "ux", "yaml",
];

/// Humanize a record ID: split on `_` and `-`, capitalize each word,
/// uppercase the fixed acronym set (`awesome_mcp_servers` →
/// `"Awesome MCP Servers"`).
pub(crate) fn titleize_id(id: &str) -> String {
    let mut words = Vec::new();
    for chunk in id.split(['_', '-']) {
        if chunk.is_empty() {
            continue;
        }
        let folded = chunk.to_lowercase();
        if ACRONYMS.contains(&folded.as_str()) {
            words.push(folded.to_uppercase());
            continue;
        }
        let mut chars = folded.chars();
        let mut word = String::new();
        if let Some(first) = chars.next() {
            word.extend(first.to_uppercase());
        }
        word.push_str(&chars.collect::<String>());
        words.push(word);
    }
    if words.is_empty() {
        return id.to_string();
    }
    words.join(" ")
}

/// Reduce wikilinks to their display text: `[[target|show]]` → `show`,
/// `[[target]]` → `target`.
pub(crate) fn reduce_wikilinks(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        out.push_str(&rest[..open]);
        let inner_start = open + 2;
        if let Some(close) = rest[inner_start..].find("]]") {
            let inner = &rest[inner_start..inner_start + close];
            out.push_str(inner.split('|').next_back().unwrap_or(""));
            rest = &rest[inner_start + close + 2..];
        } else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Tags from an owner line: `zorg/reference` is added by the caller; this
/// returns each whitespace token matching `^#[A-Za-z][A-Za-z0-9_/-]*$`
/// without the `#`, in order, deduplicated. Zorg ids like `250619#0T`
/// never match: they do not start with `#`.
pub(crate) fn owner_tags(owner_line: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for token in owner_line.split_whitespace() {
        let Some(name) = token.strip_prefix('#') else {
            continue;
        };
        let mut chars = name.chars();
        let valid = chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && chars.all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-')
            });
        if valid && !tags.iter().any(|tag| tag == name) {
            tags.push(name.to_string());
        }
    }
    tags
}

/// Values for an inline field (`url::`, `file::`, `title::`) across a
/// record block: the inline remainder when non-empty, else the first
/// token of each nested list item directly under the field line.
pub(crate) fn field_values(block: &[&str], field: &str) -> Vec<String> {
    let marker = format!("{field}::");
    let mut values = Vec::new();
    let mut index = 0;
    while index < block.len() {
        let Some(position) = find_field(block[index], &marker) else {
            index += 1;
            continue;
        };
        let indent = leading_width(block[index]);
        let inline = block[index][position + marker.len()..].trim();
        if !inline.is_empty() {
            if let Some(first) = inline.split_whitespace().next() {
                values.push(first.to_string());
            }
            index += 1;
            continue;
        }
        index += 1;
        while index < block.len() {
            let line = block[index];
            if line.trim().is_empty() {
                index += 1;
                continue;
            }
            if leading_width(line) <= indent {
                break;
            }
            if let Some(content) = list_content(line)
                && let Some(first) = content.split_whitespace().next()
            {
                values.push(first.to_string());
            }
            index += 1;
        }
    }
    values
}

/// The record's `title::` field when present (wikilinks reduced), with
/// indented continuation lines folded in; otherwise the humanized ID.
pub(crate) fn record_title(block: &[&str], id: &str) -> String {
    let marker = "title::";
    for (index, line) in block.iter().enumerate() {
        let Some(position) = find_field(line, marker) else {
            continue;
        };
        let indent = leading_width(line);
        let mut title = line[position + marker.len()..].trim().to_string();
        for next in block.iter().skip(index + 1) {
            if next.trim().is_empty() {
                break;
            }
            if leading_width(next) <= indent {
                break;
            }
            let trimmed = next.trim();
            if list_content(next).is_some() || trimmed.contains("::") {
                break;
            }
            if !title.is_empty() {
                title.push(' ');
            }
            title.push_str(trimmed);
        }
        let title = reduce_wikilinks(&title).trim().to_string();
        if !title.is_empty() {
            return title;
        }
        break;
    }
    titleize_id(id)
}

/// Each `| KEY: value` line of a record block as `(KEY, value)`, in
/// order: only lines whose first non-whitespace character is `|`, so
/// owner-line pipes (`| pg::29`) never leak in.
pub(crate) fn related_lines(block: &[&str]) -> Vec<(String, String)> {
    let mut related = Vec::new();
    for line in block {
        let trimmed = line.trim_start();
        let Some(body) = trimmed.strip_prefix('|') else {
            continue;
        };
        let body = body.trim_start();
        let length = body
            .chars()
            .take_while(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-')
            })
            .collect::<String>()
            .len();
        if length == 0 {
            continue;
        }
        let (key, rest) = body.split_at(length);
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        let value = value.trim().to_string();
        if !value.is_empty() {
            related.push((key.to_string(), value));
        }
    }
    related
}

/// The chapter title: the owner-line text after the `LID::`/`ID::`
/// token, with the block id removed.
pub(crate) fn chapter_title(owner_line: &str, key: &str) -> String {
    let marker = format!("{key}::");
    let Some(position) = owner_line.find(&marker) else {
        return String::new();
    };
    let after = &owner_line[position + marker.len()..];
    let mut kept = Vec::new();
    let mut skip_first = true;
    for token in after.split_whitespace() {
        if skip_first {
            // The key's own value token (`part_1` in `LID::part_1`).
            skip_first = false;
            if !token.starts_with('^') {
                continue;
            }
        }
        if token.starts_with("^z-") || token == "^" {
            continue;
        }
        kept.push(token);
    }
    kept.join(" ")
}

/// Escape Dataview inline field markers outside the code fence.
pub(crate) fn escape_body(text: &str) -> String {
    text.replace("::", "\\:\\:")
}

/// Case-insensitive `field::` marker byte position in one line.
/// `match_indices` only yields char boundaries, so slicing is safe.
fn find_field(line: &str, marker: &str) -> Option<usize> {
    line.to_lowercase()
        .match_indices(&marker.to_lowercase())
        .next()
        .map(|(position, _)| position)
}

/// Leading whitespace width in characters.
fn leading_width(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

/// The content of a `-`/`*`/`+`/ordered list item, if the line is one.
fn list_content(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if let Some(content) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return Some(content);
    }
    let digits = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if !digits.is_empty()
        && let Some(after) = trimmed.get(digits.len()..)
        && let Some(content) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
    {
        return Some(content);
    }
    None
}

/// One planned note's exact file contents.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_note(
    hub: &str,
    title: &str,
    tags: &[String],
    ref_type: &str,
    legacy_status: &str,
    urls: &[String],
    source_block: &str,
    source_id: &str,
    source_path: &str,
    line_start: usize,
    line_end: usize,
    files: &[String],
    related: &[(String, String)],
    chapters: &[(String, String, String, String)],
    blocks: &[String],
) -> String {
    let mut out = String::from("---\n");
    push_quoted(&mut out, "parent", &format!("[[{hub}]]"));
    push_quoted(&mut out, "type", "[[ref]]");
    out.push_str("tags:\n");
    for tag in tags {
        push_list_item(&mut out, tag);
    }
    push_quoted(&mut out, "ref_type", ref_type);
    push_quoted(&mut out, "status", "legacy");
    push_quoted(&mut out, "legacy_status", legacy_status);
    push_quoted(&mut out, "title", title);
    if urls.len() == 1 {
        push_quoted(&mut out, "url", &urls[0]);
    } else if urls.len() > 1 {
        out.push_str("url:\n");
        for url in urls {
            push_list_item(&mut out, url);
        }
    }
    push_quoted(&mut out, "source_note", &format!("[[{hub}]]"));
    push_quoted(&mut out, "source_block", source_block);
    push_quoted(&mut out, "source_id", source_id);
    push_quoted(&mut out, "source_path", source_path);
    out.push_str(&format!("source_line_start: {line_start}\n"));
    out.push_str(&format!("source_line_end: {line_end}\n"));
    if !chapters.is_empty() {
        out.push_str("source_blocks:\n");
        for (block, _, _, _) in chapters {
            push_list_item(&mut out, block);
        }
        out.push_str("legacy_chapter_statuses:\n");
        for (_, _, _, status) in chapters {
            push_list_item(&mut out, status);
        }
    }
    out.push_str("---\n");
    out.push_str(&format!("# {title}\n\n"));
    out.push_str(&format!(
        "Original block: [[{hub}#{source_block}|{hub} {source_block}]]\n"
    ));
    if !files.is_empty() {
        out.push_str("\n## Files\n\n");
        for file in files {
            out.push_str(&format!("- {file}\n"));
        }
    }
    if !related.is_empty() {
        out.push_str("\n## Related\n\n");
        for (key, value) in related {
            out.push_str(&format!("- {key}: {}\n", escape_body(value)));
        }
    }
    if !chapters.is_empty() {
        out.push_str("\n## Chapters\n\n");
        for (block, key, chapter, status) in chapters {
            out.push_str(&format!(
                "- [[{hub}#{block}|{key}]] {} · {status}\n",
                escape_body(chapter)
            ));
        }
    }
    out.push_str("\n## Original Record\n\n");
    out.push_str(
        "Dataview inline field markers are escaped in this copy so the page-level fields stay\ncanonical.\n\n",
    );
    let copied = blocks
        .iter()
        .map(|block| escape_body(block))
        .collect::<Vec<_>>()
        .join("\n\n");
    let fence = fence_for(&copied);
    out.push_str(&format!("{fence}text\n{copied}\n{fence}\n"));
    out
}

/// A YAML double-quoted scalar line via JSON quoting.
fn push_quoted(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!(
        "{key}: {}\n",
        serde_json::to_string(value).expect("string serializes")
    ));
}

/// A block-style list item holding one double-quoted string.
fn push_list_item(out: &mut String, value: &str) {
    out.push_str(&format!(
        "  - {}\n",
        serde_json::to_string(value).expect("string serializes")
    ));
}

/// The fence for the Original Record copy: three backticks, or one more
/// than the longest backtick run inside the copied text.
fn fence_for(copied: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in copied.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acronym_words_uppercase_when_humanized() {
        assert_eq!(titleize_id("awesome_mcp_servers"), "Awesome MCP Servers");
        assert_eq!(titleize_id("clean_arch"), "Clean Arch");
        assert_eq!(titleize_id("nvim-treesitter_lsp"), "Nvim Treesitter LSP");
        assert_eq!(titleize_id("25_goog_evals"), "25 Goog Evals");
        assert_eq!(titleize_id("http-go-link"), "HTTP Go Link");
    }

    #[test]
    fn wikilinks_reduce_to_display_text() {
        assert_eq!(
            reduce_wikilinks("See [[target|Shown]] and [[bare]] done"),
            "See Shown and bare done"
        );
    }

    #[test]
    fn owner_line_tags_skip_zorg_ids() {
        assert_eq!(
            owner_tags(
                "- 250416 250415#0t [[read]] #dev #tools #dev ID::x ^z-250415-0t"
            ),
            vec!["dev".to_string(), "tools".to_string()]
        );
        assert!(owner_tags("- 250619#0T ID::x ^z-1").is_empty());
    }

    #[test]
    fn url_field_reads_inline_first_token_and_nested_items() {
        let block = [
            "- ID::x ^z-1",
            "  * status:: READ",
            "  * url:: http://go/dart-devtools (and others)",
        ];
        assert_eq!(
            field_values(&block, "url"),
            vec!["http://go/dart-devtools".to_string()]
        );
        let nested = [
            "- ID::x ^z-1",
            "  * url::",
            "    - https://a.example/one",
            "    - https://b.example/two",
            "  * status:: READ",
        ];
        assert_eq!(
            field_values(&nested, "url"),
            vec![
                "https://a.example/one".to_string(),
                "https://b.example/two".to_string()
            ]
        );
        let none = ["- ID::x ^z-1", "  * url:: NONE"];
        assert_eq!(field_values(&none, "url"), vec!["NONE".to_string()]);
    }

    #[test]
    fn title_folds_continuation_lines() {
        let block = [
            "- ID::x ^z-1",
            "  * title:: The Main Technology Platforms and Intermediaries",
            "    Ecosystem",
            "  * status:: READ",
        ];
        assert_eq!(
            record_title(&block, "x"),
            "The Main Technology Platforms and Intermediaries Ecosystem"
        );
        let plain = ["- ID::x ^z-1", "  * status:: READ"];
        assert_eq!(
            record_title(&plain, "awesome_mcp_servers"),
            "Awesome MCP Servers"
        );
    }

    #[test]
    fn plain_record_renders_byte_for_byte() {
        let block = [
            "- 250411 250411#0p [[read]] #mcp ID::awesome_mcp_servers ^z-250411-0p",
            "  | LINKS: [[mcp_ref#^z-250411-0p|model_context_protocol]]",
            "  * file:: [[lib/docs/awesome_mcp_servers.pdf]]",
            "  * status:: READ",
            "  * url:: http://go/awesome-mcp",
        ];
        let contents = render_note(
            "work_ref",
            "Awesome MCP Servers",
            &["zorg/reference".to_string(), "mcp".to_string()],
            "docs",
            "read",
            &["http://go/awesome-mcp".to_string()],
            "^z-250411-0p",
            "awesome_mcp_servers",
            "work_ref.md",
            3,
            7,
            &["[[lib/docs/awesome_mcp_servers.pdf]]".to_string()],
            &[(
                "LINKS".to_string(),
                "[[mcp_ref#^z-250411-0p|model_context_protocol]]".to_string(),
            )],
            &[],
            &[block.join("\n")],
        );
        let expected = [
            "---",
            "parent: \"[[work_ref]]\"",
            "type: \"[[ref]]\"",
            "tags:",
            "  - \"zorg/reference\"",
            "  - \"mcp\"",
            "ref_type: \"docs\"",
            "status: \"legacy\"",
            "legacy_status: \"read\"",
            "title: \"Awesome MCP Servers\"",
            "url: \"http://go/awesome-mcp\"",
            "source_note: \"[[work_ref]]\"",
            "source_block: \"^z-250411-0p\"",
            "source_id: \"awesome_mcp_servers\"",
            "source_path: \"work_ref.md\"",
            "source_line_start: 3",
            "source_line_end: 7",
            "---",
            "# Awesome MCP Servers",
            "",
            "Original block: [[work_ref#^z-250411-0p|work_ref ^z-250411-0p]]",
            "",
            "## Files",
            "",
            "- [[lib/docs/awesome_mcp_servers.pdf]]",
            "",
            "## Related",
            "",
            "- LINKS: [[mcp_ref#^z-250411-0p|model_context_protocol]]",
            "",
            "## Original Record",
            "",
            "Dataview inline field markers are escaped in this copy so the page-level fields stay",
            "canonical.",
            "",
            "```text",
            "- 250411 250411#0p [[read]] #mcp ID\\:\\:awesome_mcp_servers ^z-250411-0p",
            "  | LINKS: [[mcp_ref#^z-250411-0p|model_context_protocol]]",
            "  * file\\:\\: [[lib/docs/awesome_mcp_servers.pdf]]",
            "  * status\\:\\: READ",
            "  * url\\:\\: http://go/awesome-mcp",
            "```",
            "",
        ]
        .join("\n");
        assert_eq!(contents, expected);
    }

    #[test]
    fn book_note_folds_chapters_byte_for_byte() {
        let book = "- 250921 250920#01 [[read]] ID::ad_tech_book ^z-250920-01\n  * status:: BOOK";
        let chapters = vec![
            (
                "^z-250920-10".to_string(),
                "chapter_1".to_string(),
                "Introduction".to_string(),
                "collect_fleeting_notes".to_string(),
            ),
            (
                "^z-250921-08".to_string(),
                "chapter_2".to_string(),
                "(b)".to_string(),
                "unread".to_string(),
            ),
        ];
        let chapter_blocks = vec![
            "- 250921 250920#10 [[read]] LID::chapter_1 (a) ^z-250920-10\n  * status:: COLLECT_FLEETING_NOTES\n  * title:: Introduction".to_string(),
            "- 250921#08 [[read]] LID::chapter_2 (b) ^z-250921-08\n  * status:: UNREAD".to_string(),
        ];
        let mut blocks = vec![book.to_string()];
        blocks.extend(chapter_blocks);
        let contents = render_note(
            "ad_tech_book",
            "Ad Tech Book",
            &["zorg/reference".to_string()],
            "books",
            "book",
            &[],
            "^z-250920-01",
            "ad_tech_book",
            "ad_tech_book.md",
            14,
            15,
            &["[[lib/books/ad_tech_book.epub]]".to_string()],
            &[],
            &chapters,
            &blocks,
        );
        assert!(contents.contains(
            "source_blocks:\n  - \"^z-250920-10\"\n  - \"^z-250921-08\"\n"
        ));
        assert!(contents.contains(
            "legacy_chapter_statuses:\n  - \"collect_fleeting_notes\"\n  - \"unread\"\n"
        ));
        assert!(contents.contains(
            "## Chapters\n\n- [[ad_tech_book#^z-250920-10|chapter_1]] Introduction · collect_fleeting_notes\n- [[ad_tech_book#^z-250921-08|chapter_2]] (b) · unread\n"
        ));
        assert!(contents
            .contains("## Files\n\n- [[lib/books/ad_tech_book.epub]]\n"));
        assert!(!contents.contains("url:"));
        // No chapters leak their own note shape: one fence holding the
        // book block plus both chapter blocks, every `::` escaped.
        let fence = contents.match_indices("```text").count();
        assert_eq!(fence, 1);
        assert!(contents.contains("status\\:\\: BOOK"));
        assert!(contents.contains("title\\:\\: Introduction"));
    }

    #[test]
    fn related_keeps_only_pipe_lines() {
        let block = [
            "- 250416 ID::x | pg::29 ^z-1",
            "  | LINKS: [[critique]] [[nvim_plugins]]",
            "  * status:: READ",
        ];
        assert_eq!(
            related_lines(&block),
            vec![(
                "LINKS".to_string(),
                "[[critique]] [[nvim_plugins]]".to_string()
            )]
        );
    }
}
