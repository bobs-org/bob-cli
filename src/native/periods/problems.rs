//! `bob periods list -p` problem report.
//!
//! Adopted ISO years are those with at least one weekly note. The
//! contract lives in `docs/periods.md`. Report-only: exit 0.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use serde_json::json;

use super::super::{env as bob_env, stats, style::Styler};
use super::calendar::PeriodId;
use super::instructions::INSTRUCTIONS_VERSION;
use super::note::{self, ReviewState};

/// One reported problem. `stale_instructions` is informational.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) kind: ProblemKind,
    pub(crate) path: String,
    pub(crate) detail: String,
    pub(crate) fix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProblemKind {
    DanglingDailyParent,
    MissingPeriodNote,
    Unscaffolded,
    WrongParent,
    TaskRows,
    StaleInstructions,
}

impl ProblemKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DanglingDailyParent => "dangling_daily_parent",
            Self::MissingPeriodNote => "missing_period_note",
            Self::Unscaffolded => "unscaffolded",
            Self::WrongParent => "wrong_parent",
            Self::TaskRows => "task_rows",
            Self::StaleInstructions => "stale_instructions",
        }
    }

    fn level(self) -> &'static str {
        match self {
            Self::StaleInstructions => "info",
            _ => "error",
        }
    }
}

impl Problem {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        json!({
            "kind": self.kind.as_str(),
            "path": self.path,
            "detail": self.detail,
            "fix": self.fix,
            "level": self.kind.level(),
        })
    }
}

/// Scan the vault for period-note problems as of `today`.
pub(crate) fn collect_problems(
    vault: &Path,
    today: NaiveDate,
) -> Result<Vec<Problem>, String> {
    let period_notes = scan_period_notes(vault);
    let dailies = scan_canonical_dailies(vault);
    let adopted_years: BTreeSet<i32> = period_notes
        .iter()
        .filter_map(|(id, _, _)| match id {
            PeriodId::Week { year, .. } => Some(*year),
            _ => None,
        })
        .collect();
    let existing: HashSet<PeriodId> =
        period_notes.iter().map(|(id, _, _)| *id).collect();

    let mut problems = Vec::new();

    for (date, path, contents) in &dailies {
        let week = PeriodId::week_for_date(*date);
        let PeriodId::Week { year, .. } = week else {
            continue;
        };
        if !adopted_years.contains(&year) {
            continue;
        }
        let expected = format!("[[{}]]", week.link_target());
        let parent = note::parse(contents)
            .frontmatter
            .get("parent")
            .cloned()
            .unwrap_or_default();
        if parent != expected {
            problems.push(Problem {
                kind: ProblemKind::DanglingDailyParent,
                path: path.clone(),
                detail: if parent.is_empty() {
                    format!("missing parent (expected {expected})")
                } else {
                    format!("parent is {parent} (expected {expected})")
                },
                fix: "run bob periods migrate".to_string(),
            });
        }
        if *date <= today {
            let mut chain = vec![week];
            let mut cursor = week;
            while let Some(parent_id) = cursor.parent() {
                chain.push(parent_id);
                cursor = parent_id;
            }
            for id in chain {
                if !existing.contains(&id) {
                    problems.push(Problem {
                        kind: ProblemKind::MissingPeriodNote,
                        path: id.path().to_string_lossy().replace('\\', "/"),
                        detail: format!(
                            "expected {id} for daily {path} ({date})"
                        ),
                        fix: "run bob periods ensure or bob periods migrate"
                            .to_string(),
                    });
                }
            }
        }
    }

    for (id, path, contents) in &period_notes {
        if contents.trim().is_empty() || !note::structure_ok(*id, contents) {
            problems.push(Problem {
                kind: ProblemKind::Unscaffolded,
                path: path.clone(),
                detail: "empty or missing required headings or fences"
                    .to_string(),
                fix: "fill the note from the scaffold or recreate it"
                    .to_string(),
            });
        }
        let parsed = note::parse(contents);
        let expected_parent = match id.parent() {
            Some(parent) => format!("[[{}]]", parent.link_target()),
            None => "[[org]]".to_string(),
        };
        let parent = parsed
            .frontmatter
            .get("parent")
            .cloned()
            .unwrap_or_default();
        if parent != expected_parent {
            problems.push(Problem {
                kind: ProblemKind::WrongParent,
                path: path.clone(),
                detail: if parent.is_empty() {
                    format!("missing parent (expected {expected_parent})")
                } else {
                    format!("parent is {parent} (expected {expected_parent})")
                },
                fix: "run bob periods migrate".to_string(),
            });
        }
        if has_task_row(contents) {
            problems.push(Problem {
                kind: ProblemKind::TaskRows,
                path: path.clone(),
                detail: "period notes hold no task rows".to_string(),
                fix: "move #task checkbox lines out of the period note"
                    .to_string(),
            });
        }
        if parsed.review_state != ReviewState::Reflected {
            let version = parsed.instructions_version.unwrap_or(0);
            if version < INSTRUCTIONS_VERSION {
                problems.push(Problem {
                    kind: ProblemKind::StaleInstructions,
                    path: path.clone(),
                    detail: format!(
                        "callout v{version} is older than v{INSTRUCTIONS_VERSION}"
                    ),
                    fix: "run bob periods ensure -R".to_string(),
                });
            }
        }
    }

    problems.sort_by(|a, b| {
        (a.kind.as_str(), a.path.as_str())
            .cmp(&(b.kind.as_str(), b.path.as_str()))
    });
    Ok(problems)
}

/// Nightly report-only step. Athena never creates notes.
pub(crate) fn nightly_check(vault: &Path) -> i32 {
    let today = bob_env::current_datetime().date();
    match collect_problems(vault, today) {
        Ok(problems) => {
            print_human(&problems);
            0
        }
        Err(message) => {
            eprintln!("bob nightly: {message}");
            1
        }
    }
}

pub(crate) fn print_human(problems: &[Problem]) {
    let styler = Styler::detect();
    if problems.is_empty() {
        println!("no period-note problems");
        return;
    }
    for problem in problems {
        let kind = if problem.kind == ProblemKind::StaleInstructions {
            styler.cyan(problem.kind.as_str())
        } else {
            styler.yellow(problem.kind.as_str())
        };
        println!("{kind}  {}", problem.path);
        println!("  {}", problem.detail);
        println!("  fix: {}", problem.fix);
    }
}

pub(crate) fn scan_period_notes(
    vault: &Path,
) -> Vec<(PeriodId, String, String)> {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(vault) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == "all_time.md" {
                push_period(vault, &path, PeriodId::AllTime, &mut rows);
            } else if name.len() == 7
                && name.ends_with(".md")
                && let Ok(id) = PeriodId::parse(&name[..4])
                && matches!(id, PeriodId::Year(_))
            {
                push_period(vault, &path, id, &mut rows);
            }
        }
    }
    if let Ok(entries) = fs::read_dir(vault) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(dir_name) = dir.file_name().and_then(|n| n.to_str())
            else {
                continue;
            };
            if dir_name.len() != 4
                || !dir_name.bytes().all(|byte| byte.is_ascii_digit())
            {
                continue;
            }
            let Ok(inner) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in inner.flatten() {
                let note_path = entry.path();
                if !note_path.is_file() {
                    continue;
                }
                let Some(name) = note_path.file_name().and_then(|n| n.to_str())
                else {
                    continue;
                };
                if !name.ends_with(".md") || !name.contains('-') {
                    continue;
                }
                let stem = name.trim_end_matches(".md");
                if let Ok(id) = PeriodId::parse(stem)
                    && !matches!(id, PeriodId::Year(_) | PeriodId::AllTime)
                {
                    push_period(vault, &note_path, id, &mut rows);
                }
            }
        }
    }
    rows
}

pub(crate) fn scan_canonical_dailies(
    vault: &Path,
) -> Vec<(NaiveDate, String, String)> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(vault) else {
        return rows;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dir_name.len() != 4
            || !dir_name.bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let Ok(inner) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in inner.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(vault)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
            let Some(date) = stats::index::canonical_daily_date(&relative)
            else {
                continue;
            };
            let contents = fs::read_to_string(&path).unwrap_or_default();
            rows.push((date, relative, contents));
        }
    }
    rows.sort_by_key(|(date, path, _)| (*date, path.clone()));
    rows
}

fn push_period(
    vault: &Path,
    path: &Path,
    id: PeriodId,
    rows: &mut Vec<(PeriodId, String, String)>,
) {
    let contents = fs::read_to_string(path).unwrap_or_default();
    let display = path
        .strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
    rows.push((id, display, contents));
}

fn has_task_row(contents: &str) -> bool {
    contents.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("- [") && trimmed.contains("#task")
    })
}

#[allow(dead_code)]
pub(crate) fn display_path(vault: &Path, target: &Path) -> String {
    target
        .strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| target.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_vault(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bob-periods-problems-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp vault");
        dir
    }

    #[test]
    fn each_problem_kind_is_reported() {
        let vault = temp_vault("kinds");
        // A weekly note makes 2026 an adopted year.
        let week = vault.join("2026/2026-W45.md");
        fs::create_dir_all(week.parent().unwrap()).unwrap();
        fs::write(
            &week,
            "---\nparent: \"[[journal]]\"\n---\n\n# 2026-W45\n\n- [ ] do it #task\n",
        )
        .unwrap();
        let daily = vault.join("2026/20261103.md");
        fs::write(&daily, "---\nparent: '[[2026/202611]]'\n---\n\n# day\n")
            .unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let problems = collect_problems(&vault, today).expect("scan");
        let kinds: BTreeSet<_> =
            problems.iter().map(|p| p.kind.as_str()).collect();
        assert!(kinds.contains("dangling_daily_parent"), "{problems:?}");
        assert!(kinds.contains("missing_period_note"), "{problems:?}");
        assert!(kinds.contains("unscaffolded"), "{problems:?}");
        assert!(kinds.contains("wrong_parent"), "{problems:?}");
        assert!(kinds.contains("task_rows"), "{problems:?}");
        assert!(kinds.contains("stale_instructions"), "{problems:?}");
        let _ = fs::remove_dir_all(&vault);
    }
}
