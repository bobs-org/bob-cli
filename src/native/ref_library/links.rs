//! Read-only access to the render-time URI link catalog saved with a
//! reference.

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
        print_unavailable, shell_quote, shortest_ref, source_read, usage_error,
    },
    RefRow,
};
use crate::native::{
    highlights_ref::source_store::{
        self, link_site, StoreRead, StoreStatus, StoredLink,
    },
    style::Styler,
};

const NOT_RECORDED_DETAIL: &str =
    "links were not recorded for this reference; run bob ref recatalog";
const NO_LINKS_DETAIL: &str = "no links in this reference";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinksStatus {
    Available,
    NoLinks,
    NotRecorded,
    NoCatalog,
    NoSource,
    SourceMissing,
    SourceInvalid,
}

impl LinksStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NoLinks => "no_links",
            Self::NotRecorded => "not_recorded",
            Self::NoCatalog => "no_catalog",
            Self::NoSource => "no_source",
            Self::SourceMissing => "source_missing",
            Self::SourceInvalid => "source_invalid",
        }
    }

    fn has_recorded_links_array(self) -> bool {
        matches!(self, Self::Available | Self::NoLinks)
    }
}

struct LinksView {
    status: LinksStatus,
    detail: Option<String>,
}

fn links_view(read: &StoreRead) -> LinksView {
    match read.status {
        StoreStatus::NoCatalog => LinksView {
            status: LinksStatus::NoCatalog,
            detail: read.detail.clone(),
        },
        StoreStatus::NoSource => LinksView {
            status: LinksStatus::NoSource,
            detail: read.detail.clone(),
        },
        StoreStatus::SourceMissing => LinksView {
            status: LinksStatus::SourceMissing,
            detail: read.detail.clone(),
        },
        StoreStatus::SourceInvalid => LinksView {
            status: LinksStatus::SourceInvalid,
            detail: read.detail.clone(),
        },
        StoreStatus::Available | StoreStatus::NoCode => {
            match read
                .catalog
                .as_ref()
                .and_then(|catalog| catalog.links.as_ref())
            {
                None => LinksView {
                    status: LinksStatus::NotRecorded,
                    detail: Some(NOT_RECORDED_DETAIL.to_string()),
                },
                Some(links) if links.is_empty() => LinksView {
                    status: LinksStatus::NoLinks,
                    detail: Some(NO_LINKS_DETAIL.to_string()),
                },
                Some(_) => LinksView {
                    status: LinksStatus::Available,
                    detail: None,
                },
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct LinksSectionJson {
    number: Option<String>,
    title: String,
}

#[derive(Debug, Clone, Serialize)]
struct LinkJson {
    id: String,
    n: u32,
    url: String,
    text: String,
    title: Option<String>,
    autolink: bool,
    label: String,
    label_kind: String,
    context: Option<String>,
    site: String,
    markdown: String,
    section: Option<LinksSectionJson>,
    page: Option<u32>,
    page_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LinksSourceJson {
    path: String,
}

#[derive(Debug, Clone, Serialize)]
struct LinksReferenceJson {
    path: String,
    title: String,
    marker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LinksEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    reference: LinksReferenceJson,
    status: &'static str,
    detail: Option<String>,
    source: Option<LinksSourceJson>,
    links: Vec<LinkJson>,
}

fn trim_autolink_context(context: &str) -> String {
    context
        .trim_end_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(ch, ':' | '\u{2014}' | '\u{2013}' | '-' | '(')
        })
        .to_string()
}

fn strip_prefix_ignore_ascii_case<'a>(
    text: &'a str,
    prefix: &str,
) -> Option<&'a str> {
    let len = prefix.len();
    (text.len() >= len && text[..len].eq_ignore_ascii_case(prefix))
        .then(|| &text[len..])
}

fn url_without_scheme(url: &str) -> String {
    if let Some(rest) = strip_prefix_ignore_ascii_case(url, "mailto:") {
        return rest.to_string();
    }
    let Some(colon) = url.find(':') else {
        return url.to_string();
    };
    let rest = &url[colon + 1..];
    rest.strip_prefix("//").unwrap_or(rest).to_string()
}

fn link_label(link: &StoredLink) -> (String, &'static str) {
    if !link.autolink {
        return (link.text.clone(), "text");
    }
    if let Some(context) = link.context.as_deref() {
        let trimmed = trim_autolink_context(context);
        if !trimmed.is_empty() {
            return (trimmed, "context");
        }
    }
    (url_without_scheme(&link.url), "url")
}

fn escape_markdown_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '[' | ']') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn unbalanced_parens(url: &str) -> bool {
    let mut depth = 0i32;
    for ch in url.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    depth != 0
}

fn wrap_markdown_dest(url: &str) -> String {
    if url.chars().any(char::is_whitespace) || unbalanced_parens(url) {
        format!("<{url}>")
    } else {
        url.to_string()
    }
}

fn link_markdown(link: &StoredLink) -> String {
    if link.autolink {
        format!("<{}>", link.text)
    } else {
        format!(
            "[{}]({})",
            escape_markdown_text(&link.text),
            wrap_markdown_dest(&link.url)
        )
    }
}

fn url_path_after_host(url: &str) -> String {
    if strip_prefix_ignore_ascii_case(url, "mailto:").is_some() {
        return String::new();
    }
    let Some(colon) = url.find(':') else {
        return String::new();
    };
    let scheme = url[..colon].to_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "ftp" | "ftps") {
        return String::new();
    }
    let mut rest = &url[colon + 1..];
    if let Some(stripped) = rest.strip_prefix("//") {
        rest = stripped;
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[host_end..].to_string()
}

fn link_json(hash12: &str, link: &StoredLink) -> LinkJson {
    let (label, label_kind) = link_label(link);
    LinkJson {
        id: format!("{hash12}/link-{}", link.n),
        n: link.n,
        url: link.url.clone(),
        text: link.text.clone(),
        title: link.title.clone(),
        autolink: link.autolink,
        label,
        label_kind: label_kind.to_string(),
        context: link.context.clone(),
        site: link_site(&link.url),
        markdown: link_markdown(link),
        section: link.section.as_ref().map(|section| LinksSectionJson {
            number: section.number.clone(),
            title: section.title.clone(),
        }),
        page: link.page,
        page_source: link.page_source.clone(),
    }
}

fn source_json(row: &RefRow) -> Option<LinksSourceJson> {
    row.source_markdown
        .as_deref()
        .filter(|pointer| !pointer.trim().is_empty())
        .map(|path| LinksSourceJson {
            path: path.to_string(),
        })
}

fn catalog_links(read: &StoreRead) -> &[StoredLink] {
    read.catalog
        .as_ref()
        .and_then(|catalog| catalog.links.as_deref())
        .unwrap_or(&[])
}

fn select_links(
    links: Vec<LinkJson>,
    page: Option<u32>,
    number: Option<u32>,
    total: usize,
    format: Format,
) -> Result<Vec<LinkJson>, i32> {
    let mut selected = links;
    if let Some(page) = page {
        selected.retain(|link| link.page == Some(page));
    }
    if let Some(number) = number {
        let Some(link) = selected.iter().find(|link| link.n == number).cloned()
        else {
            let message = format!("no link {number} ({total} links)");
            if format == Format::Json {
                print_find_error(
                    format,
                    "ref links",
                    "link_not_found",
                    &message,
                    None,
                );
            } else {
                eprintln!("bob ref: error: {message}");
            }
            return Err(1);
        };
        selected = vec![link];
    }
    Ok(selected)
}

fn print_json(row: &RefRow, view: &LinksView, links: Vec<LinkJson>) {
    let envelope = LinksEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref links",
        generated_at: generated_at(),
        reference: LinksReferenceJson {
            path: row.path.clone(),
            title: row.title.clone(),
            marker_id: row.marker_id.clone(),
        },
        status: view.status.as_str(),
        detail: view.detail.clone(),
        source: source_json(row),
        links,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("links envelope serializes")
    );
}

fn unique_site_count(links: &[LinkJson]) -> usize {
    let mut sites = Vec::new();
    for link in links {
        if !sites.iter().any(|site| site == &link.site) {
            sites.push(link.site.clone());
        }
    }
    sites.len()
}

fn render_human(
    title: &str,
    path: &str,
    links: &[LinkJson],
    styler: Styler,
    hint_ref: &str,
) -> String {
    if links.is_empty() {
        return "no links\n".to_string();
    }
    let link_word = if links.len() == 1 { "link" } else { "links" };
    let site_count = unique_site_count(links);
    let site_word = if site_count == 1 { "site" } else { "sites" };
    let mut output = format!("{}\n", styler.bold(title));
    output.push_str(&format!(
        "{} · {} {link_word} · {} {site_word}\n",
        path,
        links.len(),
        site_count
    ));
    let mut last_section: Option<(Option<String>, String)> = None;
    for link in links {
        if let Some(section) = link.section.as_ref() {
            let key = (section.number.clone(), section.title.clone());
            if last_section.as_ref() != Some(&key) {
                let heading = match section.number.as_deref() {
                    Some(number) => format!("{number}  {}", section.title),
                    None => section.title.clone(),
                };
                output.push_str(&format!("\n  {}\n", styler.dim(&heading)));
                last_section = Some(key);
            }
        } else {
            last_section = None;
        }
        let page = link
            .page
            .map(|page| format!("  {}", styler.dim(&format!("p. {page}"))))
            .unwrap_or_default();
        output.push_str(&format!(
            "  {}  {}{page}\n",
            styler.bold(&format!("{:>3}", link.n)),
            link.label
        ));
        let rest = url_path_after_host(&link.url);
        let path_part = if rest.is_empty() {
            String::new()
        } else {
            format!("  {}", styler.dim(&rest))
        };
        output.push_str(&format!(
            "      {}{path_part}\n",
            styler.cyan(&link.site)
        ));
    }
    let hint_n = links.first().map(|link| link.n).unwrap_or(1);
    output.push_str(&format!(
        "\nhint: bob ref links {} {hint_n} | pbcopy\n",
        shell_quote(hint_ref)
    ));
    output
}

fn render_markdown(title: &str, links: &[LinkJson]) -> String {
    let mut output = format!("# {title}\n");
    let mut last_section: Option<(Option<String>, String)> = None;
    for link in links {
        if let Some(section) = link.section.as_ref() {
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
        let page = link
            .page
            .map(|page| format!(" · p. {page}"))
            .unwrap_or_default();
        output.push_str(&format!("- {}{page}\n", link.markdown));
    }
    output
}

fn write_bytes(bytes: &[u8], what: &str) -> i32 {
    match io::stdout().lock().write_all(bytes) {
        Ok(()) => 0,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(error) => {
            eprintln!("bob ref: error: write {what}: {error}");
            1
        }
    }
}

/// Run `bob ref links`: resolve one note, verify its saved source and catalog,
/// then select or render recorded URI links without invoking pandoc.
pub(crate) fn run_links(matches: &ArgMatches) -> i32 {
    let format_value = matches.get_one::<String>("format").map(String::as_str);
    let number = matches.get_one::<u32>("number").copied();
    let page = matches.get_one::<u32>("page").copied();
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
    if raw && number.is_none() {
        return usage_error("--format raw requires a link number N");
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
                "ref links",
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
                "ref links",
                "ambiguous_reference",
                &format!("ambiguous reference: {query}"),
                &candidates,
            );
            return 1;
        }
        Err(ShowFailure::NotFound { query, candidates }) => {
            print_show_error(
                format,
                "ref links",
                "unknown_reference",
                &format!("no reference note matches {query}"),
                &candidates,
            );
            return 1;
        }
    };
    let row = &index.rows[resolved.row];
    let read = source_read(&index, row);
    let view = links_view(&read);
    let hash12 = read
        .bytes
        .as_deref()
        .map(source_store::hash12_for)
        .unwrap_or_default();
    if format_value == Some("json") {
        let mut links = Vec::new();
        if view.status.has_recorded_links_array() {
            links = catalog_links(&read)
                .iter()
                .map(|link| link_json(&hash12, link))
                .collect();
            links = match select_links(
                links,
                page,
                number,
                catalog_links(&read).len(),
                format,
            ) {
                Ok(links) => links,
                Err(code) => return code,
            };
        }
        print_json(row, &view, links);
        return 0;
    }

    if view.status == LinksStatus::NoLinks
        && format == Format::Human
        && number.is_none()
    {
        if let Some(page) = page {
            println!("no links on page {page}");
            return 0;
        }
        println!("no links");
        return 0;
    }
    if view.status != LinksStatus::Available {
        if view.status == LinksStatus::NoLinks {
            if let Some(number) = number {
                eprintln!("bob ref: error: no link {number} (0 links)");
                return 1;
            }
        }
        print_unavailable(
            view.detail
                .as_deref()
                .unwrap_or("saved links are unavailable"),
            query,
            "links",
        );
        return 1;
    }

    let total = catalog_links(&read).len();
    let links = catalog_links(&read)
        .iter()
        .map(|link| link_json(&hash12, link))
        .collect();
    let links = match select_links(links, page, number, total, format) {
        Ok(links) => links,
        Err(code) => return code,
    };

    if raw {
        let link = links.first().expect("number selected one link");
        return write_bytes(link.url.as_bytes(), "link URL");
    }
    if format == Format::Markdown && number.is_some() {
        return write_bytes(links[0].markdown.as_bytes(), "Markdown link");
    }
    if format == Format::Markdown {
        print!("{}", render_markdown(&row.title, &links));
        return 0;
    }

    if links.is_empty() {
        if let Some(page) = page {
            println!("no links on page {page}");
            return 0;
        }
    }
    let hint_ref = shortest_ref(&index, resolved.row);
    print!(
        "{}",
        render_human(
            &row.title,
            &row.path,
            &links,
            Styler::detect(),
            &hint_ref,
        )
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::highlights_ref::code_catalog::CodeSection;

    fn stored(
        url: &str,
        text: &str,
        autolink: bool,
        context: Option<&str>,
    ) -> StoredLink {
        StoredLink {
            n: 1,
            url: url.to_string(),
            text: text.to_string(),
            title: None,
            autolink,
            context: context.map(str::to_string),
            section: Some(CodeSection {
                number: Some("1".into()),
                title: "Intro".into(),
            }),
            page: Some(1),
            page_source: Some("anchor".into()),
        }
    }

    #[test]
    fn written_link_uses_text_label_and_bracket_markdown() {
        let link = stored(
            "https://ai.google.dev/gemini-api/docs/imagen",
            "Imagen 4 model card",
            false,
            Some("Google's current image model is documented in the"),
        );
        assert_eq!(link_label(&link), ("Imagen 4 model card".into(), "text"));
        assert_eq!(link_site(&link.url), "ai.google.dev");
        assert_eq!(
            link_markdown(&link),
            "[Imagen 4 model card](https://ai.google.dev/gemini-api/docs/imagen)"
        );
    }

    #[test]
    fn autolink_label_trims_context_punctuation() {
        let link = stored(
            "https://github.com/black-forest-labs/flux",
            "https://github.com/black-forest-labs/flux",
            true,
            Some("See the model card: — – - ("),
        );
        assert_eq!(link_label(&link), ("See the model card".into(), "context"));
        assert_eq!(
            link_markdown(&link),
            "<https://github.com/black-forest-labs/flux>"
        );
    }

    #[test]
    fn autolink_falls_back_to_url_label_without_context() {
        let link = stored(
            "https://example.com/a",
            "https://example.com/a",
            true,
            None,
        );
        assert_eq!(link_label(&link), ("example.com/a".into(), "url"));
        let empty = stored(
            "https://example.com/a",
            "https://example.com/a",
            true,
            Some("   :("),
        );
        assert_eq!(link_label(&empty), ("example.com/a".into(), "url"));
    }

    #[test]
    fn site_covers_www_mailto_and_obsidian() {
        assert_eq!(link_site("https://WWW.Example.COM/x"), "example.com");
        assert_eq!(link_site("mailto:me@example.com"), "email");
        assert_eq!(link_site("obsidian://open?vault=x"), "obsidian");
        let mail =
            stored("mailto:me@example.com", "me@example.com", true, None);
        assert_eq!(link_label(&mail), ("me@example.com".into(), "url"));
        assert_eq!(link_markdown(&mail), "<me@example.com>");
    }

    #[test]
    fn markdown_escapes_text_and_wraps_awkward_destinations() {
        let brackets =
            stored("https://example.com/a", r"see [this] \path", false, None);
        assert_eq!(
            link_markdown(&brackets),
            r"[see \[this\] \\path](https://example.com/a)"
        );
        let spaces = stored("https://example.com/a b", "spaced", false, None);
        assert_eq!(
            link_markdown(&spaces),
            "[spaced](<https://example.com/a b>)"
        );
        let unbalanced =
            stored("https://example.com/foo(bar", "paren", false, None);
        assert_eq!(
            link_markdown(&unbalanced),
            "[paren](<https://example.com/foo(bar>)"
        );
        let balanced = stored(
            "https://en.wikipedia.org/wiki/Foo_(bar)",
            "wiki",
            false,
            None,
        );
        assert_eq!(
            link_markdown(&balanced),
            "[wiki](https://en.wikipedia.org/wiki/Foo_(bar))"
        );
    }
}
