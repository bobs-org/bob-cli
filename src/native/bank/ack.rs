//! `bob bank ack` — tick shown waiting rows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::model::BankRow;

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct AckSelection {
    pub(crate) ids: Vec<String>,
    pub(crate) day: Option<NaiveDate>,
}

#[derive(Debug, Clone)]
pub(crate) struct AckTarget {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) id: Option<String>,
    pub(crate) payee: String,
}

#[derive(Debug, Clone)]
pub(crate) struct AckPlan {
    pub(crate) acknowledged: Vec<AckTarget>,
    pub(crate) already: Vec<AckTarget>,
    pub(crate) unknown: Vec<String>,
    pub(crate) edits: HashMap<String, String>,
    pub(crate) originals: HashMap<String, String>,
}

pub(crate) fn normalize_id(raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches('^');
    let stripped = trimmed.strip_prefix("bank-").unwrap_or(trimmed);
    stripped.to_ascii_lowercase()
}

pub(crate) fn plan_ack(
    rows: &[BankRow],
    selection: &AckSelection,
    file_contents: &HashMap<String, String>,
) -> AckPlan {
    let wanted: HashSet<String> =
        selection.ids.iter().map(|id| normalize_id(id)).collect();
    let mut acknowledged = Vec::new();
    let mut already = Vec::new();
    let mut unknown = Vec::new();
    let mut to_tick: Vec<&BankRow> = Vec::new();

    if !wanted.is_empty() {
        let mut by_id: HashMap<String, &BankRow> = HashMap::new();
        for row in rows {
            if let Some(id) = &row.id {
                by_id.insert(id.to_ascii_lowercase(), row);
            }
        }
        for raw in &selection.ids {
            let normalized = normalize_id(raw);
            match by_id.get(&normalized) {
                None => unknown.push(raw.clone()),
                Some(row) => {
                    let target = AckTarget {
                        path: row.path.clone(),
                        line: row.line,
                        id: row.id.clone(),
                        payee: row.payee.clone(),
                    };
                    if row.acknowledged {
                        already.push(target);
                    } else {
                        acknowledged.push(target);
                        to_tick.push(row);
                    }
                }
            }
        }
    }

    if let Some(day) = selection.day {
        for row in rows {
            if row.date != day || row.acknowledged {
                continue;
            }
            // When explicit IDs were also given, the day adds the rest.
            let already_listed = acknowledged.iter().any(|target| {
                target.path == row.path && target.line == row.line
            }) || already.iter().any(|target| {
                target.path == row.path && target.line == row.line
            });
            if already_listed {
                continue;
            }
            acknowledged.push(AckTarget {
                path: row.path.clone(),
                line: row.line,
                id: row.id.clone(),
                payee: row.payee.clone(),
            });
            to_tick.push(row);
        }
        // Already-ticked rows of the day are no-ops and stay unlisted; only
        // ID-selected already-ticked rows appear in `already`.
    }

    // Apply ticks in memory per file.
    let mut edits: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for row in to_tick {
        let contents = match file_contents.get(&row.path) {
            Some(contents) => contents.clone(),
            None => continue,
        };
        let lines: Vec<&str> = contents.split('\n').collect();
        let _ = lines;
        edits
            .entry(row.path.clone())
            .or_default()
            .push((row.line, contents));
    }

    let mut final_edits = HashMap::new();
    let mut originals = HashMap::new();
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
    for target in &acknowledged {
        by_file
            .entry(target.path.clone())
            .or_default()
            .push(target.line);
    }
    for (path, mut lines) in by_file {
        lines.sort_unstable();
        lines.dedup();
        let Some(contents) = file_contents.get(&path) else {
            continue;
        };
        originals.insert(path.clone(), contents.clone());
        let mut updated = contents.clone();
        for line in lines.iter().rev() {
            if let Some(edit) = super::month::plan_tick_row(&updated, *line) {
                updated = edit.apply(&updated);
            }
        }
        final_edits.insert(path, updated);
    }

    AckPlan {
        acknowledged,
        already,
        unknown,
        edits: final_edits,
        originals,
    }
}

pub(crate) fn ack_target_json(target: &AckTarget) -> Value {
    json!({
        "path": target.path,
        "line": target.line,
        "id": target.id,
        "payee": target.payee,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn to_json(
    plan: &AckPlan,
    today: NaiveDate,
    dry_run: bool,
    changed: bool,
    diffs: &HashMap<String, String>,
    categories: &[super::config::Category],
    accounts: &[super::config::AccountStatus],
) -> Value {
    let changes: Vec<Value> = {
        let mut paths: Vec<&String> = plan.edits.keys().collect();
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                if dry_run {
                    json!({
                        "path": path,
                        "diff": diffs.get(path).cloned().unwrap_or_default(),
                    })
                } else {
                    json!({"path": path})
                }
            })
            .collect()
    };
    json!({
        "ok": plan.unknown.is_empty(),
        "schema_version": SCHEMA_VERSION,
        "today": today.format("%Y-%m-%d").to_string(),
        "categories": categories.iter().map(|category| json!({
            "key": category.key,
            "emoji": category.emoji,
            "label": category.label,
            "builtin": category.builtin,
        })).collect::<Vec<_>>(),
        "accounts": accounts.iter().map(|account| json!({
            "alias": account.alias,
            "label": account.label,
            "kind": account.kind.as_str(),
            "default": account.default,
            "start": account.start.map(|date| date.format("%Y-%m-%d").to_string()),
            "status": account.status,
            "last_import": Value::Null,
            "balance_cents": Value::Null,
            "balance_asof": Value::Null,
            "delta_cents": Value::Null,
            "hint": account.hint,
        })).collect::<Vec<_>>(),
        "issues": {"errors": 0, "warnings": 0},
        "dry_run": dry_run,
        "changed": changed,
        "changes": changes,
        "findings": [],
        "commit": Value::Null,
        "sync": Value::Null,
        "warnings": [],
        "acknowledged": plan.acknowledged.iter().map(ack_target_json).collect::<Vec<_>>(),
        "already": plan.already.iter().map(ack_target_json).collect::<Vec<_>>(),
        "unknown": plan.unknown,
    })
}

pub(crate) fn staged_files(
    bob_dir: &Path,
    plan: &AckPlan,
) -> Vec<crate::native::capture::StagedTextFile> {
    let mut files = Vec::new();
    let mut paths: Vec<&String> = plan.edits.keys().collect();
    paths.sort();
    for path in paths {
        let updated = plan.edits[path].clone();
        let original = plan.originals.get(path).cloned().unwrap_or_default();
        let target: PathBuf = bob_dir.join(path);
        let existed = target.is_file();
        files.push(crate::native::capture::StagedTextFile {
            target,
            target_existed: existed,
            original_target: original,
            updated_target: updated,
        });
    }
    files
}
