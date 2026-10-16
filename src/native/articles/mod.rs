//! Article notes: bob-owned section numbers and layout.
//!
//! Pure core: text in, [`ArticlePlan`] out. The CLI and plugin phases
//! wire I/O around this engine.
#![allow(dead_code)] // wired by the cli phase

mod findings;
mod layout;
mod links;
mod marker;
mod outline;
mod plan;
mod scan;
mod sections;
mod verify;

#[cfg(test)]
mod tests;

use findings::Finding;
use plan::{
    line_edits, sha256_hex, ArticlePlan, ArticlePlanResponse, ArticleStatus,
};

pub(crate) use plan::ArticleContext;
#[allow(unused_imports)] // wired by the cli phase
pub(crate) use plan::ArticlePlanResponse as JsonArticlePlan;
#[allow(unused_imports)] // wired by the cli phase
pub(crate) use sections::{
    heading_identity, heading_plain_identity, strip_section_number,
};

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
    }
}

pub(crate) fn response(notes: Vec<ArticlePlan>) -> ArticlePlanResponse {
    ArticlePlanResponse::new(notes)
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

pub(crate) fn format_as_article(text: &str) -> ArticlePlan {
    format(
        text,
        &ArticleContext::bypass_marker(std::path::Path::new("note.md")),
    )
}
