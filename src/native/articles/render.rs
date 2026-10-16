//! Human and JSON output for `bob articles`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use similar::{ChangeTag, TextDiff};

use super::findings::Severity;
use super::inbound;
use super::plan::{ArticlePlan, ArticleStatus};
use crate::native::style::{pad_right, Styler};
use crate::native::vault_links::NoteIndex;

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

pub(crate) fn adopt_human(
    plan: &ArticlePlan,
    source: &str,
    styler: &Styler,
    write: bool,
) {
    match plan.status {
        ArticleStatus::Skipped | ArticleStatus::Error => {
            let message = plan
                .reason
                .as_ref()
                .map(|reason| reason.message.as_str())
                .unwrap_or("note could not be adopted");
            eprintln!("bob articles: {}: {message}", display_path(&plan.path));
        }
        ArticleStatus::Unchanged => {
            println!(
                "{} {} · {}",
                styler.green("✓"),
                display_path(&plan.path),
                plan.notice
            );
        }
        ArticleStatus::Changed if write => {
            let detail = plan
                .notice
                .strip_prefix("Adopted as an article")
                .unwrap_or(&plan.notice)
                .trim_start_matches([' ', '·']);
            let line = if detail.is_empty() {
                display_path(&plan.path)
            } else {
                format!("{} · {detail}", display_path(&plan.path))
            };
            println!("{} {}", styler.green("✓"), line);
            print_findings(plan, styler);
        }
        ArticleStatus::Changed => {
            println!("{}", styler.cyan(&plan.notice));
            print_diff(plan, source, styler);
            println!("{}", styler.cyan("Numbers replaced"));
            if plan.replaced.is_empty() {
                println!("  {}", styler.dim("none"));
            } else {
                for item in &plan.replaced {
                    let original = source_line(source, item.line);
                    println!(
                        "  {} {} {}",
                        original,
                        styler.dim("→"),
                        item.number
                    );
                }
            }
            print_findings(plan, styler);
            println!("{}", styler.dim("Preview only · rerun with -w to write"));
        }
    }
}

fn source_line(source: &str, line: usize) -> &str {
    source.lines().nth(line.saturating_sub(1)).unwrap_or("")
}

pub(crate) fn format_human(
    plans: &[ArticlePlan],
    sources: &[String],
    root: &Path,
    index: &NoteIndex,
    styler: &Styler,
    check: bool,
    dry_run: bool,
) {
    for (plan_index, plan) in plans.iter().enumerate() {
        match plan.status {
            ArticleStatus::Changed => {
                if dry_run {
                    print_diff(
                        plan,
                        sources
                            .get(plan_index)
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
    if dry_run {
        print_inbound_diffs(plans, root, index, styler);
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
        format!("{total} · {}", display_root(root))
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
            "{} · {needs_format} {} formatting",
            plural_count(total, "article", "articles"),
            if needs_format == 1 { "needs" } else { "need" }
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
        let prefix = match finding.severity {
            Severity::Warning => styler.warning_prefix(),
            Severity::Info => styler.dim("i"),
        };
        println!(
            "  {} {}:{} · {} ({})",
            prefix,
            display_path(&plan.path),
            finding.line,
            finding.message,
            finding.hint
        );
    }
}

fn plural_count(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

fn print_inbound_diffs(
    plans: &[ArticlePlan],
    root: &Path,
    index: &NoteIndex,
    styler: &Styler,
) {
    let mut seen = BTreeSet::new();
    for plan in plans {
        for inbound in &plan.inbound {
            if !seen.insert(inbound.path.clone()) {
                continue;
            }
            let absolute = root.join(&inbound.path);
            let Ok(source) = fs::read_to_string(&absolute) else {
                continue;
            };
            let rewritten =
                inbound::rewritten_file(&source, &inbound.path, plans, index);
            if rewritten == source {
                continue;
            }
            print_text_diff(&inbound.path, &source, &rewritten, styler);
        }
    }
}

fn print_diff(plan: &ArticlePlan, source: &str, styler: &Styler) {
    let Some(formatted) = plan.formatted.as_deref() else {
        return;
    };
    print_text_diff(&plan.path, source, formatted, styler);
}

fn print_text_diff(
    path: &Path,
    source: &str,
    formatted: &str,
    styler: &Styler,
) {
    println!(
        "{}",
        styler.dim(&format!("--- a/{}", display_file_path(path)))
    );
    println!(
        "{}",
        styler.dim(&format!("+++ b/{}", display_file_path(path)))
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

fn display_root(root: &Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
    else {
        return root.display().to_string();
    };
    if root == home {
        "~".to_owned()
    } else if let Ok(relative) = root.strip_prefix(&home) {
        format!("~/{}", relative.display())
    } else {
        root.display().to_string()
    }
}

fn pad_left(value: &str, width: usize) -> String {
    format!(
        "{}{}",
        " ".repeat(width.saturating_sub(value.chars().count())),
        value
    )
}
