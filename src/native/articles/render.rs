//! Human and JSON output for `bob articles`.

use similar::{ChangeTag, TextDiff};

use super::plan::{ArticlePlan, ArticleStatus};
use crate::native::style::{pad_right, Styler};

pub(crate) fn format_json(plans: Vec<ArticlePlan>) {
    println!("{}", json_line(&super::response(plans)));
}

pub(crate) fn list_json(plans: Vec<ArticlePlan>) {
    let mut value = serde_json::to_value(super::response(plans))
        .expect("article response serializes");
    if let Some(notes) = value["notes"].as_array_mut() {
        for note in notes {
            if let Some(fields) = note.as_object_mut() {
                fields.remove("edits");
            }
        }
    }
    println!("{value}");
}

pub(crate) fn outline_json(plan: &ArticlePlan) {
    println!(
        "{}",
        serde_json::to_string(&plan.outline).unwrap_or_default()
    );
}

pub(crate) fn format_human(
    plans: &[ArticlePlan],
    sources: &[String],
    styler: &Styler,
    check: bool,
    dry_run: bool,
) {
    for (index, plan) in plans.iter().enumerate() {
        match plan.status {
            ArticleStatus::Changed => {
                if dry_run {
                    print_diff(
                        plan,
                        sources
                            .get(index)
                            .map(String::as_str)
                            .unwrap_or_default(),
                        styler,
                    );
                } else if check {
                    println!(
                        "{} {} · needs formatting",
                        styler.yellow("●"),
                        display_path(&plan.path)
                    );
                } else {
                    let detail = plan
                        .notice
                        .strip_prefix("Article formatted")
                        .unwrap_or(&plan.notice)
                        .trim_start_matches([' ', '·']);
                    let line = if detail.is_empty() {
                        format!("{}", display_path(&plan.path))
                    } else {
                        format!("{} · {detail}", display_path(&plan.path))
                    };
                    println!("{} {}", styler.green("✓"), line);
                }
                print_findings(plan, styler);
            }
            ArticleStatus::Unchanged => {}
            ArticleStatus::Skipped | ArticleStatus::Error => {
                let message = plan
                    .reason
                    .as_ref()
                    .map(|reason| reason.message.as_str())
                    .unwrap_or("article could not be formatted");
                eprintln!(
                    "bob articles: {}: {message}",
                    display_path(&plan.path)
                );
            }
        }
    }
}

pub(crate) fn list_human(
    plans: &[ArticlePlan],
    root: &std::path::Path,
    styler: &Styler,
) {
    let total = plans.len();
    println!(
        "{} {} {}",
        styler.cyan("Articles"),
        styler.separator(),
        format!("{total} · {}", root.display())
    );

    let rows = plans
        .iter()
        .map(|plan| {
            let state = list_state(plan);
            (
                display_path(&plan.path),
                plan.outline.sections.len().to_string(),
                plan.outline.words.to_string(),
                plan.outline
                    .sections
                    .iter()
                    .filter(|section| section.stub)
                    .count()
                    .to_string(),
                plan.findings.len().to_string(),
                state,
            )
        })
        .collect::<Vec<_>>();
    let article_width = rows
        .iter()
        .map(|row| row.0.chars().count())
        .max()
        .unwrap_or(7)
        .max(7);
    let state_width = rows
        .iter()
        .map(|row| row.5.chars().count())
        .max()
        .unwrap_or(5)
        .max(5);
    println!(
        "{}  {}  {}  {}  {}  {}",
        pad_right("ARTICLE", article_width),
        pad_right("SECTIONS", 8),
        pad_right("WORDS", 5),
        pad_right("TODO", 4),
        pad_right("FINDINGS", 8),
        "STATE"
    );
    for row in &rows {
        println!(
            "{}  {}  {}  {}  {}  {}",
            pad_right(&row.0, article_width),
            pad_right(&row.1, 8),
            pad_right(&row.2, 5),
            pad_right(&row.3, 4),
            pad_right(&row.4, 8),
            pad_right(&row.5, state_width)
        );
    }
    let needs_format = plans
        .iter()
        .filter(|plan| matches!(plan.status, ArticleStatus::Changed))
        .count();
    println!(
        "{}",
        styler.dim(&format!(
            "{total} articles · {needs_format} needs formatting"
        ))
    );
}

pub(crate) fn outline_human(plan: &ArticlePlan, styler: &Styler) {
    let sections = &plan.outline.sections;
    let stubs = sections.iter().filter(|section| section.stub).count();
    println!(
        "{} {} {} words · {} sections · {} TODO",
        styler.cyan(&plan.outline.title),
        styler.dim("· article ·"),
        plan.outline.words,
        sections.len(),
        stubs
    );

    let number_width = sections
        .iter()
        .map(|section| section.number.as_deref().unwrap_or("¶").chars().count())
        .max()
        .unwrap_or(1)
        .max(4);
    let word_width = sections
        .iter()
        .map(|section| section.words.to_string().len())
        .max()
        .unwrap_or(1)
        .max(3);
    println!(
        "  {}  {}  {} w",
        styler.dim("¶"),
        "Preamble",
        pad_left(&plan.outline.preamble.words.to_string(), word_width)
    );

    for section in sections {
        let indentation = "  ".repeat(section.depth.saturating_sub(1));
        let mut number = section.number.as_deref().unwrap_or("·").to_string();
        if section.stale {
            number.push('•');
        }
        let number = pad_right(&number, number_width);
        let title = if section.stub {
            styler.yellow(&section.title)
        } else {
            section.title.clone()
        };
        let words = if section.stub {
            styler.yellow("TODO")
        } else {
            format!("{} w", pad_left(&section.words.to_string(), word_width))
        };
        println!("  {indentation}{}  {title}  {words}", styler.dim(&number));
    }
    if sections.iter().any(|section| section.stale) {
        println!("{}", styler.dim("• section numbers need formatting"));
    }
}

fn json_line(response: &super::plan::ArticlePlanResponse) -> String {
    serde_json::to_string(response).unwrap_or_else(|_| {
        "{\"schema_version\":1,\"notes\":[],\"summary\":{\"articles\":0,\"changed\":0,\"unchanged\":0,\"skipped\":0,\"errors\":1}}".to_string()
    })
}

fn list_state(plan: &ArticlePlan) -> String {
    match plan.status {
        ArticleStatus::Changed => "● needs format".to_string(),
        ArticleStatus::Unchanged => "✓ formatted".to_string(),
        ArticleStatus::Skipped | ArticleStatus::Error => {
            let message = plan
                .reason
                .as_ref()
                .map(|reason| reason.message.as_str())
                .unwrap_or("cannot format");
            format!("skipped: {message}")
        }
    }
}

fn print_findings(plan: &ArticlePlan, styler: &Styler) {
    for finding in &plan.findings {
        println!(
            "  {} {}:{} · {} ({})",
            styler.warning_prefix(),
            display_path(&plan.path),
            finding.line,
            finding.message,
            finding.hint
        );
    }
}

fn print_diff(plan: &ArticlePlan, source: &str, styler: &Styler) {
    let Some(formatted) = plan.formatted.as_deref() else {
        return;
    };
    println!(
        "{}",
        styler.dim(&format!("--- a/{}", display_file_path(&plan.path)))
    );
    println!(
        "{}",
        styler.dim(&format!("+++ b/{}", display_file_path(&plan.path)))
    );
    let diff = TextDiff::from_lines(source, formatted);
    let mut unified = diff.unified_diff();
    unified.context_radius(3);
    for hunk in unified.iter_hunks() {
        println!("{}", styler.dim(&hunk.header().to_string()));
        for change in hunk.iter_changes() {
            let tag = match change.tag() {
                ChangeTag::Insert => "+",
                ChangeTag::Delete => "-",
                ChangeTag::Equal => " ",
            };
            let line = format!(
                "{tag}{}",
                change.value().trim_end_matches(['\r', '\n'])
            );
            let line = match change.tag() {
                ChangeTag::Insert => styler.green(&line),
                ChangeTag::Delete => styler.red(&line),
                ChangeTag::Equal => line,
            };
            println!("{line}");
        }
    }
}

fn display_path(path: &std::path::Path) -> String {
    path.with_extension("").to_string_lossy().replace('\\', "/")
}

fn display_file_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn pad_left(value: &str, width: usize) -> String {
    format!(
        "{}{}",
        " ".repeat(width.saturating_sub(value.chars().count())),
        value
    )
}
