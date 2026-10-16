//! Article notes: bob-owned section numbers and layout.
//!
//! Pure core: text in, [`ArticlePlan`] out. The CLI and plugin phases
//! wire I/O around this engine.
#![allow(dead_code)] // wired by the cli phase

mod findings;
mod layout;
mod marker;
mod plan;
mod scan;
mod sections;
mod verify;

#[cfg(test)]
mod tests;

use findings::Finding;
use plan::{line_edits, sha256_hex, ArticlePlan, ArticleStatus};

pub(crate) use plan::ArticleContext;
#[allow(unused_imports)] // wired by the cli phase
pub(crate) use sections::{
    heading_identity, heading_plain_identity, strip_section_number,
};

pub(crate) fn format(text: &str, ctx: &ArticleContext<'_>) -> ArticlePlan {
    let sha256 = sha256_hex(text);
    let document = scan::classify(text);
    if ctx.require_marker
        && let Some(reason) =
            marker::skip_reason(text, ctx.path, &document.lines)
    {
        return ArticlePlan::skipped(ctx.path, sha256, reason);
    }
    let options = marker::parse_options(text);
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
    let input_contents: Vec<&str> =
        document.lines.iter().map(|line| line.content).collect();
    let output_contents: Vec<&str> =
        output_lines.iter().map(|(c, _)| c.as_str()).collect();
    let edits = line_edits(&input_contents, &output_contents);
    let mut changes = numbering.changes;
    changes.layout = layout::layout_change_count(
        &document.lines,
        &output_lines,
        &numbering.headings,
    );
    let sections =
        sections::article_sections(&numbering.headings, document.lines.len());

    let input_tokens = verify::word_tokens(
        &input_contents,
        &document.lines,
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
            numbering.findings,
            sections,
        );
    }

    let second = format_once(&formatted, options.numbering);
    if second != formatted {
        return ArticlePlan::error(
            ctx.path,
            sha256,
            verify::not_idempotent(),
            numbering.findings,
            sections,
        );
    }

    let status = if formatted == text {
        ArticleStatus::Unchanged
    } else {
        ArticleStatus::Changed
    };
    ArticlePlan {
        path: ctx.path.to_path_buf(),
        status,
        reason: None,
        sha256,
        formatted: Some(formatted),
        edits,
        changes,
        replaced: numbering.replaced,
        findings: numbering.findings,
        sections,
    }
}

fn format_once(text: &str, numbering: bool) -> String {
    let document = scan::classify(text);
    let numbering = sections::plan_headings(&document.lines, numbering);
    let work = layout::apply(&document, &numbering.headings);
    let output_lines = layout::emit(&document, &work);
    layout::join(
        document.bom,
        &output_lines,
        document.unclosed_fence.is_some(),
        document.default_ending,
    )
}

pub(crate) fn format_as_article(text: &str) -> ArticlePlan {
    format(
        text,
        &ArticleContext::bypass_marker(std::path::Path::new("note.md")),
    )
}
