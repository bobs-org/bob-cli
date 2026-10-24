//! Fast relation-label catalog scan (`docs/task-relations.md` §16,
//! RV2).
//!
//! One row per key counts every accepted LINKS entry with that label
//! across every scanned note: open, closed, `done/`, daily notes,
//! nested projects, and references. The walk reuses the dependency
//! discover set (every `.md` note including `done/`, minus
//! dot-directories and the always-excluded names). Malformed LINKS
//! lines are skipped with a bounded warning; completion tolerates
//! them, rename must not. This scan never calls
//! `capture_dependency_tasks::discover`.

use std::{collections::BTreeMap, fs, io, path::Path};

use crate::native::{
    capture_dependency_tasks,
    markdown::fenced_lines,
    task_dependencies::blocks::has_task_tag,
    task_relations::{parse::links_children_of, LinksLine},
};

use super::{count_label, sort_spellings, LabelRecord, MAX_LABEL_WARNINGS};

/// Scan the vault into label rows in sorted path order, plus bounded
/// walk warnings.
pub(crate) fn load_scan(
    vault: &Path,
) -> io::Result<(BTreeMap<String, LabelRecord>, Vec<String>)> {
    let (relative_paths, mut warnings) =
        capture_dependency_tasks::vault_note_paths(vault);
    let mut rows: BTreeMap<String, LabelRecord> = BTreeMap::new();
    let mut seen: Vec<String> = Vec::new();
    let mut malformed = 0usize;
    for relative in &relative_paths {
        let absolute = vault.join(relative);
        let contents = match fs::read(&absolute) {
            Ok(bytes) => bytes,
            Err(error) => {
                push_warning(
                    &mut warnings,
                    &mut malformed,
                    format!(
                        "skipping unreadable note {}: {error}",
                        display_relative(relative)
                    ),
                );
                continue;
            }
        };
        // Byte-prefilter before parsing: most notes carry no LINKS.
        if !contents.windows(7).any(|window| window == b"**LINKS") {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&contents) else {
            push_warning(
                &mut warnings,
                &mut malformed,
                format!(
                    "skipping non-UTF8 note {}",
                    display_relative(relative)
                ),
            );
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let fenced = fenced_lines(&lines, 0..lines.len());
        for (task_line, line) in lines.iter().enumerate() {
            if !has_task_tag(line) || fenced.contains(&task_line) {
                continue;
            }
            for child in links_children_of(&lines, &fenced, task_line) {
                match child.parsed {
                    LinksLine::Accepted { entries, .. } => {
                        for entry in &entries {
                            if let Some(label) = entry.label.as_deref() {
                                count_label(&mut rows, &mut seen, label);
                            }
                        }
                    }
                    LinksLine::Malformed => {
                        push_warning(
                            &mut warnings,
                            &mut malformed,
                            format!(
                                "skipping malformed LINKS line {}:{}",
                                display_relative(relative),
                                child.line_index + 1
                            ),
                        );
                    }
                    LinksLine::Empty | LinksLine::NotALine => {}
                }
            }
        }
    }
    if malformed > MAX_LABEL_WARNINGS {
        warnings.push(format!(
            "skipped {malformed} unreadable or malformed relation lines"
        ));
    }
    for record in rows.values_mut() {
        sort_spellings(record);
    }
    Ok((rows, warnings))
}

fn push_warning(
    warnings: &mut Vec<String>,
    malformed: &mut usize,
    warning: String,
) {
    *malformed += 1;
    if *malformed <= MAX_LABEL_WARNINGS {
        warnings.push(warning);
    }
}

fn display_relative(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}
