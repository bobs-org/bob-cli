//! Article notes: bob-owned section numbers and layout.
//!
//! Pure core: text in, [`ArticlePlan`] out. The CLI wires vault I/O around
//! this engine, while the plugin phase consumes the JSON contract.

pub(crate) mod cli;
mod findings;
mod inbound;
mod layout;
mod links;
mod marker;
mod outline;
mod plan;
mod render;
mod scan;
mod sections;
mod vault;
mod verify;

#[cfg(test)]
mod tests;

use findings::Finding;
use plan::{
    line_edits, sha256_hex, ArticlePlan, ArticlePlanResponse, ArticleStatus,
};

pub(crate) use plan::ArticleContext;
#[cfg(test)]
pub(crate) use sections::strip_section_number;
pub(crate) use sections::{heading_identity, heading_plain_identity};

pub(crate) fn run(args: Vec<std::ffi::OsString>) -> i32 {
    cli::run(args)
}

pub(crate) fn completion_candidates(
    bob_dir: &std::path::Path,
) -> Vec<(String, String)> {
    vault::completion_candidates(bob_dir)
}

pub(crate) fn adopt(text: &str, ctx: &ArticleContext<'_>) -> ArticlePlan {
    let sha256 = sha256_hex(text);
    let input_document = scan::classify(text);
    match marker::adopt_decision(text, ctx.path, &input_document.lines) {
        marker::AdoptDecision::Refuse(reason) => {
            ArticlePlan::skipped(ctx.path, sha256, reason)
        }
        marker::AdoptDecision::AlreadyArticle => ArticlePlan {
            path: ctx.path.to_path_buf(),
            status: ArticleStatus::Unchanged,
            reason: None,
            sha256,
            formatted: None,
            edits: Vec::new(),
            changes: plan::ChangeCounts::default(),
            replaced: Vec::new(),
            findings: Vec::new(),
            notice: "already an article".to_string(),
            outline: outline::empty_outline(ctx.path, true),
            inbound: Vec::new(),
            headings: Vec::new(),
            numbering: true,
        },
        marker::AdoptDecision::Insert => {
            let insert = marker::with_article_type(text);
            let mut planned = format(&insert.text, ctx);
            planned.sha256 = sha256;
            if matches!(
                planned.status,
                ArticleStatus::Skipped | ArticleStatus::Error
            ) {
                return planned;
            }
            let formatted = planned
                .formatted
                .clone()
                .unwrap_or_else(|| insert.text.clone());
            let original_lines: Vec<&str> = input_document
                .lines
                .iter()
                .map(|line| line.content)
                .collect();
            let formatted_document = scan::classify(&formatted);
            let formatted_lines: Vec<&str> = formatted_document
                .lines
                .iter()
                .map(|line| line.content)
                .collect();
            planned.edits = line_edits(&original_lines, &formatted_lines);
            planned.status = if formatted == text {
                ArticleStatus::Unchanged
            } else {
                ArticleStatus::Changed
            };
            planned.formatted = Some(formatted);
            planned.notice = adopt_notice(&planned.notice);
            remap_adopt_lines(&mut planned, insert.at, insert.count);
            planned
        }
    }
}

pub(crate) fn format(text: &str, ctx: &ArticleContext<'_>) -> ArticlePlan {
    let sha256 = sha256_hex(text);
    let input_document = scan::classify(text);
    if ctx.require_marker
        && let Some(reason) =
            marker::skip_reason(text, ctx.path, &input_document.lines)
    {
        return ArticlePlan::skipped(ctx.path, sha256, reason);
    }
    let options = marker::parse_options(text);
    let initial_numbering =
        sections::plan_headings(&input_document.lines, options.numbering);
    let mut link_plan = links::refresh(
        &input_document.lines,
        &initial_numbering.headings,
        ctx.path,
        options.numbering,
    );
    let linked_text = with_line_replacements(text, &input_document, &link_plan);
    let document = scan::classify(&linked_text);
    let mut numbering =
        sections::plan_headings(&document.lines, options.numbering);
    if let Some(line) = options.bad_option_line {
        numbering.findings.push(Finding::warning(
            "bad-option",
            line,
            "section_numbers must be true or false; numbering stays on",
            "Use `section_numbers: true` or `section_numbers: false`",
        ));
    }
    if let Some(open) = document.unclosed_fence {
        numbering.findings.push(Finding::warning(
            "unclosed-fence",
            open + 1,
            "code fence never closes; remaining lines were left untouched",
            "Close the fence",
        ));
    }
    let work = layout::apply(&document, &numbering.headings);
    let output_lines = layout::emit(&document, &work);
    let formatted = layout::join(
        document.bom,
        &output_lines,
        document.unclosed_fence.is_some(),
        document.default_ending,
    );
    let input_contents: Vec<&str> = input_document
        .lines
        .iter()
        .map(|line| line.content)
        .collect();
    let output_contents: Vec<&str> =
        output_lines.iter().map(|(c, _)| c.as_str()).collect();
    let edits = line_edits(&input_contents, &output_contents);
    let mut changes = numbering.changes;
    changes.links = link_plan.refreshed;
    changes.layout = layout::layout_change_count(
        &document.lines,
        &output_lines,
        &numbering.headings,
    );
    let mut all_findings = std::mem::take(&mut numbering.findings);
    all_findings.append(&mut link_plan.findings);
    all_findings.extend(findings::pseudo_footnotes(&input_document.lines));
    if let Some(finding) = findings::title_heading(
        &numbering.headings,
        ctx.path,
        options.title.as_deref(),
    ) {
        all_findings.push(finding);
    }
    all_findings.sort_by_key(|finding| finding.line);
    let outline = outline::build(
        &input_document,
        &numbering.headings,
        &formatted,
        options.title.as_deref(),
        options.numbering,
        ctx.path,
    );

    let input_tokens = verify::word_tokens(
        &input_contents,
        &input_document.lines,
        &numbering.headings,
    );
    let output_classified = scan::classify(&formatted);
    let output_headings =
        sections::plan_headings(&output_classified.lines, options.numbering);
    let output_line_contents: Vec<&str> = output_classified
        .lines
        .iter()
        .map(|line| line.content)
        .collect();
    let output_tokens = verify::word_tokens(
        &output_line_contents,
        &output_classified.lines,
        &output_headings.headings,
    );
    if let Some(reason) =
        verify::words_changed_reason(&input_tokens, &output_tokens)
    {
        return ArticlePlan::error(
            ctx.path,
            sha256,
            reason,
            all_findings,
            outline.clone(),
        );
    }

    let second = format_once(&formatted, ctx.path, options.numbering);
    if second != formatted {
        return ArticlePlan::error(
            ctx.path,
            sha256,
            verify::not_idempotent(),
            all_findings,
            outline.clone(),
        );
    }

    let status = if formatted == text {
        ArticleStatus::Unchanged
    } else {
        ArticleStatus::Changed
    };
    let notice = format_notice(&changes);
    ArticlePlan {
        path: ctx.path.to_path_buf(),
        status,
        reason: None,
        sha256,
        formatted: Some(formatted),
        edits,
        changes,
        replaced: numbering.replaced,
        findings: all_findings,
        notice,
        outline,
        inbound: Vec::new(),
        headings: numbering.headings,
        numbering: options.numbering,
    }
}

pub(crate) fn response(notes: Vec<ArticlePlan>) -> ArticlePlanResponse {
    ArticlePlanResponse::new(notes)
}

/// Report-only `format --all --check` used by `bob nightly`. Exit 1 from
/// "needs formatting" is treated as success; only real failures return 2.
pub(crate) fn nightly_check(root: &std::path::Path) -> i32 {
    let articles = match vault::discover_articles(root) {
        Ok(articles) => articles,
        Err(error) => {
            eprintln!("bob nightly: {error}");
            return 2;
        }
    };
    let sources = articles
        .iter()
        .map(|article| article.text.clone())
        .collect::<Vec<_>>();
    let plans = articles
        .iter()
        .map(|article| {
            format(&article.text, &ArticleContext::new(&article.path))
        })
        .collect::<Vec<_>>();
    let failed = plans.iter().any(|plan| {
        matches!(plan.status, ArticleStatus::Skipped | ArticleStatus::Error)
    });
    render::format_human(
        &plans,
        &sources,
        root,
        &crate::native::vault_links::NoteIndex::default(),
        &crate::native::style::Styler::detect(),
        true,
        false,
    );
    if failed {
        2
    } else {
        0
    }
}

fn format_once(text: &str, path: &std::path::Path, numbered: bool) -> String {
    let input_document = scan::classify(text);
    let initial = sections::plan_headings(&input_document.lines, numbered);
    let links = links::refresh(
        &input_document.lines,
        &initial.headings,
        path,
        numbered,
    );
    let linked_text = with_line_replacements(text, &input_document, &links);
    let document = scan::classify(&linked_text);
    let numbering = sections::plan_headings(&document.lines, numbered);
    let work = layout::apply(&document, &numbering.headings);
    let output_lines = layout::emit(&document, &work);
    layout::join(
        document.bom,
        &output_lines,
        document.unclosed_fence.is_some(),
        document.default_ending,
    )
}

fn with_line_replacements(
    text: &str,
    document: &scan::Document<'_>,
    links: &links::LinkPlan,
) -> String {
    let mut out = String::with_capacity(text.len());
    if document.bom {
        out.push('\u{feff}');
    }
    for line in &document.lines {
        out.push_str(
            links.line_replacements[line.index]
                .as_deref()
                .unwrap_or(line.content),
        );
        out.push_str(line.ending);
    }
    out
}

fn adopt_notice(format_notice: &str) -> String {
    if let Some(rest) = format_notice.strip_prefix("Article formatted") {
        let rest = rest.trim_start_matches([' ', '·']).trim();
        if rest.is_empty() {
            "Adopted as an article".to_owned()
        } else {
            format!("Adopted as an article · {rest}")
        }
    } else {
        format!("Adopted as an article · {format_notice}")
    }
}

fn remap_adopt_lines(plan: &mut ArticlePlan, at: usize, count: usize) {
    if count == 0 {
        return;
    }
    for finding in &mut plan.findings {
        finding.line = remap_one_based(finding.line, at, count);
    }
    for replaced in &mut plan.replaced {
        replaced.line = remap_one_based(replaced.line, at, count);
    }
    plan.outline.preamble.line =
        remap_one_based(plan.outline.preamble.line, at, count);
    for section in &mut plan.outline.sections {
        section.line = remap_one_based(section.line, at, count);
        section.end_line = remap_zero_based(section.end_line, at, count);
    }
}

fn remap_one_based(line: usize, at: usize, count: usize) -> usize {
    remap_zero_based(line.saturating_sub(1), at, count) + 1
}

fn remap_zero_based(index: usize, at: usize, count: usize) -> usize {
    if index < at {
        index
    } else if index < at + count {
        at
    } else {
        index - count
    }
}

fn format_notice(changes: &plan::ChangeCounts) -> String {
    let mut clauses = Vec::new();
    if changes.numbered > 0 {
        clauses.push(format!("{} sections numbered", changes.numbered));
    }
    if changes.renumbered > 0 {
        clauses.push(format!("{} renumbered", changes.renumbered));
    }
    if changes.promoted > 0 {
        clauses.push(format!(
            "{} headings promoted (#### → ###)",
            changes.promoted
        ));
    }
    if changes.setext > 0 {
        clauses.push(format!("{} Setext headings converted", changes.setext));
    }
    if changes.links > 0 {
        clauses.push(format!("{} links refreshed", changes.links));
    }
    if clauses.is_empty() {
        "Article formatted".to_owned()
    } else {
        format!("Article formatted · {}", clauses.join(" · "))
    }
}

#[cfg(test)]
pub(crate) fn format_as_article(text: &str) -> ArticlePlan {
    format(
        text,
        &ArticleContext::bypass_marker(std::path::Path::new("note.md")),
    )
}
