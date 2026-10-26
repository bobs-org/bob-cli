//! Read-only access to the exact Markdown and render-time code catalog saved
//! with a reference.

use std::io::{self, Write};

use clap::ArgMatches;
use serde::Serialize;

use super::{
    build_index,
    cli::list_config_from_matches,
    output::{
        generated_at, print_find_error, print_show_error, Format,
        REF_SCHEMA_VERSION,
    },
    show::{resolve_show, ShowFailure},
    store::{
        page_label, print_unavailable, shell_quote, shortest_ref, source_read,
        usage_error,
    },
    RefRow,
};
use crate::native::{
    highlights_ref::source_store::{self, StoreRead, StoreStatus, StoredBlock},
    style::Styler,
};

#[derive(Debug, Clone, Serialize)]
struct CodeSectionJson {
    number: Option<String>,
    title: String,
}

#[derive(Debug, Clone, Serialize)]
struct CodeBlockJson {
    id: String,
    n: u32,
    kind: String,
    lang: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    caption: Option<String>,
    lines: usize,
    bytes: usize,
    text: String,
    fenced: String,
    section: Option<CodeSectionJson>,
    page: Option<u32>,
    page_end: Option<u32>,
    page_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CodeSourceJson {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    captured: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CodeReferenceJson {
    path: String,
    title: String,
    marker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CodeEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    reference: CodeReferenceJson,
    status: &'static str,
    detail: Option<String>,
    source: Option<CodeSourceJson>,
    blocks: Vec<CodeBlockJson>,
}

fn status_name(status: &StoreStatus) -> &'static str {
    match status {
        StoreStatus::Available => "available",
        StoreStatus::NoCode => "no_code",
        StoreStatus::NoCatalog => "no_catalog",
        StoreStatus::NoSource => "no_source",
        StoreStatus::SourceMissing => "source_missing",
        StoreStatus::SourceInvalid => "source_invalid",
    }
}

fn source_json(row: &RefRow, read: &StoreRead) -> Option<CodeSourceJson> {
    let path = row
        .source_markdown
        .as_deref()
        .filter(|pointer| !pointer.trim().is_empty())?
        .to_string();
    let stored = read.catalog.as_ref();
    let bytes = read.bytes.as_deref();
    Some(CodeSourceJson {
        path,
        sha256: bytes.map(source_store::sha256_hex),
        bytes: bytes.map(<[u8]>::len),
        captured: stored.map(|catalog| {
            catalog
                .captured_at
                .split('T')
                .next()
                .unwrap_or(&catalog.captured_at)
                .to_string()
        }),
        route: stored.map(|catalog| catalog.route.clone()),
        text: bytes.map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
    })
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

fn fenced(text: &str, lang: Option<&str>) -> String {
    let fence = "`".repeat(3.max(longest_backtick_run(text) + 1));
    let language = lang.unwrap_or("");
    format!("{fence}{language}\n{text}\n{fence}\n")
}

fn box_drawing_line(line: &str) -> bool {
    !line.is_empty()
        && line
            .chars()
            .all(|ch| matches!(ch as u32, 0x2500..=0x257f | 0x2580..=0x259f))
}

fn meaningful(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rule_line = trimmed.chars().count() >= 3
        && ['-', '=', '*']
            .iter()
            .any(|rule| trimmed.chars().all(|character| character == *rule));
    if trimmed.is_empty()
        || rule_line
        || matches!(
            trimmed,
            "[" | "]" | "[]" | "{" | "}" | "{}" | "(" | ")" | "()"
        )
        || box_drawing_line(trimmed)
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn mermaid_frontmatter(text: &str) -> (Option<String>, &str) {
    let Some(after_open) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let Some(close) = after_open.find("\n---\n") else {
        return (None, text);
    };
    let frontmatter = &after_open[..close];
    let title = frontmatter.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "title")
            .then(|| value.trim().trim_matches(['"', '\'']).trim().to_string())
            .filter(|value| !value.is_empty())
    });
    (title, &after_open[close + "\n---\n".len()..])
}

fn block_json(hash12: &str, block: &StoredBlock) -> CodeBlockJson {
    let title = block
        .attrs
        .get("title")
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty());
    let (mermaid_title, summary_text) = if block.kind == "mermaid" {
        mermaid_frontmatter(&block.text)
    } else {
        (None, block.text.as_str())
    };
    let summary = summary_text.lines().find_map(meaningful);
    let caption = title.clone().or(mermaid_title).or_else(|| summary.clone());
    let lines = if block.text.is_empty() {
        0
    } else {
        block.text.lines().count()
    };
    CodeBlockJson {
        id: format!("{hash12}/{}", block.n),
        n: block.n,
        kind: block.kind.clone(),
        lang: block.lang.clone(),
        title,
        summary,
        caption,
        lines,
        bytes: block.text.len(),
        text: block.text.clone(),
        fenced: fenced(&block.text, block.lang.as_deref()),
        section: block.section.as_ref().map(|section| CodeSectionJson {
            number: section.number.clone(),
            title: section.title.clone(),
        }),
        page: block.page,
        page_end: block.page_end,
        page_source: block.page_source.clone(),
    }
}

fn format_source_bytes(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}

fn render_human(
    title: &str,
    path: &str,
    total_bytes: usize,
    captured: &str,
    blocks: &[CodeBlockJson],
    styler: Styler,
    hint_ref: &str,
    hint_number: Option<u32>,
) -> String {
    if blocks.is_empty() {
        return "no code blocks\n".to_string();
    }
    let mut output = format!("{}\n", styler.bold(title));
    let block_word = if blocks.len() == 1 { "block" } else { "blocks" };
    output.push_str(&format!(
        "{} · {} {block_word} · {} · captured {}\n",
        path,
        blocks.len(),
        format_source_bytes(total_bytes),
        captured
    ));
    let mut last_section: Option<(Option<String>, String)> = None;
    for block in blocks {
        if let Some(section) = block.section.as_ref() {
            let key = (section.number.clone(), section.title.clone());
            if last_section.as_ref() != Some(&key) {
                let label = section
                    .number
                    .as_deref()
                    .map(|number| format!("{}  ", styler.dim(number)))
                    .unwrap_or_default();
                output.push_str(&format!(
                    "\n  {label}{}\n",
                    styler.bold(&section.title)
                ));
                last_section = Some(key);
            }
        } else {
            last_section = None;
        }
        let lang = block.lang.as_deref().unwrap_or("text");
        let summary = block.summary.as_deref().unwrap_or("(empty block)");
        output.push_str(&format!(
            "  {}  {}  {} · {} · {}\n",
            styler.bold(&format!("{:>3}", block.n)),
            styler.cyan(lang),
            styler.dim(&page_label(block.page, block.page_end)),
            if block.lines == 1 {
                "1 line".to_string()
            } else {
                format!("{} lines", block.lines)
            },
            summary
        ));
    }
    output.push_str(&format!(
        "\nhint: bob ref code {} {} | pbcopy\n",
        shell_quote(hint_ref),
        hint_number.unwrap_or(2)
    ));
    output
}

fn render_markdown(title: &str, blocks: &[CodeBlockJson]) -> String {
    let mut output = format!("# {title}\n");
    let mut last_section: Option<(Option<String>, String)> = None;
    for block in blocks {
        if let Some(section) = block.section.as_ref() {
            let key = (section.number.clone(), section.title.clone());
            if last_section.as_ref() != Some(&key) {
                let number = section
                    .number
                    .as_deref()
                    .map(|number| format!("{number} "))
                    .unwrap_or_default();
                output.push_str(&format!("\n## {number}{}\n", section.title));
                last_section = Some(key);
            }
        } else {
            last_section = None;
        }
        let lang = block.lang.as_deref().unwrap_or("text");
        output.push_str(&format!(
            "\nBlock {} · {} · {}\n\n{}",
            block.n,
            lang,
            page_label(block.page, block.page_end),
            block.fenced
        ));
    }
    output
}

fn print_json(row: &RefRow, read: &StoreRead, blocks: Vec<CodeBlockJson>) {
    let envelope = CodeEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref code",
        generated_at: generated_at(),
        reference: CodeReferenceJson {
            path: row.path.clone(),
            title: row.title.clone(),
            marker_id: row.marker_id.clone(),
        },
        status: status_name(&read.status),
        detail: read.detail.clone(),
        source: source_json(row, read),
        blocks,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("code envelope serializes")
    );
}

fn unavailable(
    read: &StoreRead,
    format: Format,
    has_number: bool,
    query: &str,
) -> Option<i32> {
    if read.status == StoreStatus::NoCode
        && format == Format::Human
        && !has_number
    {
        println!("no code blocks");
        return Some(0);
    }
    print_unavailable(
        read.detail
            .as_deref()
            .unwrap_or("saved code is unavailable"),
        query,
        "code",
    );
    Some(1)
}

/// Run `bob ref code`: resolve one note, verify its saved source and catalog,
/// then select or render records without invoking pandoc.
pub(crate) fn run_code(matches: &ArgMatches) -> i32 {
    let format_value = matches.get_one::<String>("format").map(String::as_str);
    let number = matches.get_one::<u32>("number").copied();
    let page = matches.get_one::<u32>("page").copied();
    let source_only = matches.get_flag("source");
    let format = match format_value.unwrap_or(if number.is_some() {
        "raw"
    } else {
        "human"
    }) {
        "human" => Format::Human,
        "json" => Format::Json,
        "markdown" => Format::Markdown,
        "raw" => Format::Human,
        _ => unreachable!("clap validates formats"),
    };
    let raw = format_value == Some("raw")
        || (format_value.is_none() && number.is_some());

    if source_only
        && (number.is_some() || page.is_some() || format_value.is_some())
    {
        return usage_error("--source conflicts with N, --page, and --format");
    }
    if raw && number.is_none() {
        return usage_error("--format raw requires a block number N");
    }
    let query = matches
        .get_one::<String>("ref")
        .map(String::as_str)
        .unwrap_or_default();
    let config = list_config_from_matches(matches);
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                format,
                "ref code",
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    let resolved = match resolve_show(&index, query) {
        Ok(resolved) => resolved,
        Err(ShowFailure::Ambiguous { query, candidates }) => {
            print_show_error(
                format,
                "ref code",
                "ambiguous_reference",
                &format!("ambiguous reference: {query}"),
                &candidates,
            );
            return 1;
        }
        Err(ShowFailure::NotFound { query, candidates }) => {
            print_show_error(
                format,
                "ref code",
                "unknown_reference",
                &format!("no reference note matches {query}"),
                &candidates,
            );
            return 1;
        }
    };
    let row = &index.rows[resolved.row];
    let read = source_read(&index, row);
    if format_value == Some("json") {
        let catalog = read.catalog.as_ref();
        let hash12 = read
            .bytes
            .as_deref()
            .map(source_store::hash12_for)
            .unwrap_or_default();
        let mut blocks = catalog
            .filter(|_| {
                matches!(
                    read.status,
                    StoreStatus::Available | StoreStatus::NoCode
                )
            })
            .map(|catalog| {
                catalog
                    .blocks
                    .iter()
                    .map(|block| block_json(&hash12, block))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(page) = page {
            blocks.retain(|block| {
                block.page.is_some_and(|start| {
                    start <= page && page <= block.page_end.unwrap_or(start)
                })
            });
        }
        if let Some(number) = number
            && matches!(
                read.status,
                StoreStatus::Available | StoreStatus::NoCode
            )
        {
            let Some(block) =
                blocks.iter().find(|block| block.n == number).cloned()
            else {
                let count = catalog.map_or(0, |catalog| catalog.blocks.len());
                print_find_error(
                    format,
                    "ref code",
                    "block_not_found",
                    &format!("no block {number} ({count} blocks)"),
                    None,
                );
                return 1;
            };
            blocks = vec![block];
        }
        print_json(row, &read, blocks);
        return 0;
    }

    if source_only {
        if let Some(bytes) = read.bytes.as_deref() {
            return match io::stdout().lock().write_all(bytes) {
                Ok(()) => 0,
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
                Err(error) => {
                    eprintln!("bob ref: error: write saved Markdown: {error}");
                    1
                }
            };
        }
        eprintln!(
            "bob ref: error: {}",
            read.detail
                .as_deref()
                .unwrap_or("saved Markdown is unavailable")
        );
        eprintln!("hint: bob ref code {} -f json", shell_quote(query));
        return 1;
    }

    let Some(catalog) = read.catalog.as_ref() else {
        return unavailable(&read, format, number.is_some(), query)
            .unwrap_or(1);
    };
    if read.status != StoreStatus::Available {
        if read.status == StoreStatus::NoCode
            && let Some(number) = number
        {
            eprintln!("bob ref: error: no block {number} (0 blocks)");
            return 1;
        }
        return unavailable(&read, format, number.is_some(), query)
            .unwrap_or(1);
    }
    let hash12 = read
        .bytes
        .as_deref()
        .map(source_store::hash12_for)
        .unwrap_or_default();
    let mut blocks = catalog
        .blocks
        .iter()
        .map(|block| block_json(&hash12, block))
        .collect::<Vec<_>>();
    if let Some(page) = page {
        blocks.retain(|block| {
            block.page.is_some_and(|start| {
                start <= page && page <= block.page_end.unwrap_or(start)
            })
        });
    }
    if let Some(number) = number {
        let Some(block) =
            blocks.iter().find(|block| block.n == number).cloned()
        else {
            eprintln!(
                "bob ref: error: no block {number} ({} blocks)",
                catalog.blocks.len()
            );
            return 1;
        };
        blocks = vec![block];
    }

    if raw {
        let block = blocks.first().expect("number selected one block");
        return match io::stdout().lock().write_all(block.text.as_bytes()) {
            Ok(()) => 0,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
            Err(error) => {
                eprintln!("bob ref: error: write code block: {error}");
                1
            }
        };
    }
    if format == Format::Markdown && number.is_some() {
        print!("{}", blocks[0].fenced);
        return 0;
    }
    if format == Format::Markdown {
        print!("{}", render_markdown(&row.title, &blocks));
        return 0;
    }

    if blocks.is_empty() && page.is_some() {
        println!("no code blocks on page {}", page.unwrap());
        return 0;
    }
    let hint_ref = shortest_ref(&index, resolved.row);
    let total_bytes = read.bytes.as_deref().map(<[u8]>::len).unwrap_or(0);
    let captured = read
        .catalog
        .as_ref()
        .map(|catalog| {
            catalog
                .captured_at
                .split('T')
                .next()
                .unwrap_or(&catalog.captured_at)
                .to_string()
        })
        .unwrap_or_default();
    print!(
        "{}",
        render_human(
            &row.title,
            &row.path,
            total_bytes,
            &captured,
            &blocks,
            Styler::detect(),
            &hint_ref,
            number,
        )
    );
    0
}
