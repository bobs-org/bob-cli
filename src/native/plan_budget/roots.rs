//! Today roots: the root parents of Today's tasks. `docs/plan.md`
//! §Today roots is the authoritative definition; this module is its
//! Rust implementation, and the `bob-ledger-tools` JavaScript mirror
//! shares the `today_roots` conformance vectors in
//! `tests/fixtures/pomodoro_homes/vectors.json`.
//!
//! The roots are a view of [`TodayTask`] rows, never a budget: no cap,
//! no lint, no status effect, and nothing is written to the vault.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{PlanReport, TodayTask};
use crate::native::{
    pomodoro_homes::{self, DiskHomeSource, HomeResolver, Hue},
    style::Styler,
    vault_links::LinkResolution,
};

/// One root parent holding Today tasks. `key` is the root note's vault
/// path with `.md` (or `@unfiled`), `label` the lowercase stem (or
/// `unfiled`), `hue` the root's area hue (`null` for the bucket), and
/// `tasks` the distinct Today tasks under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TodayRoot {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) hue: Option<Hue>,
    pub(crate) tasks: usize,
}

/// Fill [`PlanReport::today_roots`] from the report's Today rows.
/// `daily_path` is the daily note's vault-relative path with `.md`.
pub(crate) fn with_today_roots(
    mut report: PlanReport,
    bob_dir: &Path,
    daily_path: &Path,
) -> PlanReport {
    report.today_roots = today_roots(bob_dir, daily_path, &report.today_tasks);
    report
}

/// Group `tasks` by root parent, resolved against the current note
/// tree through the Pomodoro home resolver (R1, R2, R11). A valid
/// `[root:: X]` on the task line wins; archive and ref notes are
/// walked through; a task with no area or project lands in the
/// `@unfiled` bucket. Order is `tasks` descending, ties by the Today
/// order of each root's first task, and buckets always last. The
/// counts sum to `tasks.len()`.
pub(crate) fn today_roots(
    bob_dir: &Path,
    daily_path: &Path,
    tasks: &[TodayTask],
) -> Vec<TodayRoot> {
    if tasks.is_empty() {
        return Vec::new();
    }
    let mut resolver = HomeResolver::new(DiskHomeSource::new(bob_dir));
    let mut roots: Vec<TodayRoot> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for task in tasks {
        let home = resolver.home_for_link(
            daily_path,
            LinkResolution::Found(PathBuf::from(&task.path)),
            &task.block_id,
        );
        let key = home.root_parent;
        if let Some(position) = index.get(&key) {
            roots[*position].tasks += 1;
            continue;
        }
        let hue = if key.starts_with('@') {
            None
        } else {
            resolver.resolve_note(Path::new(&key)).hue
        };
        index.insert(key.clone(), roots.len());
        roots.push(TodayRoot {
            label: pomodoro_homes::root_label(&key),
            key,
            hue,
            tasks: 1,
        });
    }
    // The sort is stable, so equal counts keep first-task order.
    roots.sort_by_key(|root| {
        (root.key.starts_with('@'), std::cmp::Reverse(root.tasks))
    });
    roots
}

/// The human `ROOTS` line body (without the leading label): a dot
/// painted in the root's hue, the label, and a dim count, joined by the
/// styler's separator. Empty when there are no roots.
pub(crate) fn human_roots(roots: &[TodayRoot], styler: &Styler) -> String {
    roots
        .iter()
        .map(|root| {
            let dot = match root.hue {
                Some(hue) => styler.xterm256(hue.xterm(), "○"),
                None => styler.dim("○"),
            };
            format!(
                "{dot} {} {}",
                root.label,
                styler.dim(&root.tasks.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(&format!(" {} ", styler.separator()))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use serde::Deserialize;
    use serde_json::Value;

    use super::*;
    use crate::native::plan_budget::today::today_tasks;

    #[derive(Debug, Deserialize)]
    struct Vectors {
        notes: BTreeMap<PathBuf, String>,
        today_roots: Vec<RootsCase>,
    }

    #[derive(Debug, Deserialize)]
    struct RootsCase {
        name: String,
        daily_path: PathBuf,
        daily: String,
        #[serde(default)]
        tasks_notes: BTreeMap<PathBuf, String>,
        expected: Vec<Value>,
    }

    fn write(root: &Path, relative: &Path, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("note parent"))
            .expect("create vault directory");
        fs::write(path, contents).expect("write vault note");
    }

    /// The shared `today_roots` vectors: every case runs Today and then
    /// the roots over a fresh mini-vault, and the roots must partition
    /// TODAY.
    #[test]
    fn shared_today_roots_vectors_hold() {
        let vectors: Vectors = serde_json::from_str(include_str!(
            "../../../tests/fixtures/pomodoro_homes/vectors.json"
        ))
        .expect("homes vectors parse");
        assert!(!vectors.today_roots.is_empty());
        for case in &vectors.today_roots {
            let temp = tempfile::tempdir().expect("temporary mini-vault");
            for (path, contents) in &vectors.notes {
                write(temp.path(), path, contents);
            }
            for (path, contents) in &case.tasks_notes {
                write(temp.path(), path, contents);
            }
            write(temp.path(), &case.daily_path, &case.daily);
            let daily = case.daily_path.to_string_lossy().into_owned();
            let result = today_tasks(temp.path(), &daily, &case.daily);
            let actual =
                today_roots(temp.path(), &case.daily_path, &result.tasks);
            assert_eq!(
                serde_json::to_value(&actual).expect("roots JSON"),
                Value::Array(case.expected.clone()),
                "{}",
                case.name,
            );
            let total: usize = actual.iter().map(|root| root.tasks).sum();
            assert_eq!(total, result.tasks.len(), "partition: {}", case.name);
        }
    }

    fn root(label: &str, hue: Option<Hue>, tasks: usize) -> TodayRoot {
        TodayRoot {
            key: if hue.is_some() {
                format!("{label}.md")
            } else {
                format!("@{label}")
            },
            label: label.to_string(),
            hue,
            tasks,
        }
    }

    #[test]
    fn human_roots_paint_dots_by_hue_and_dim_counts() {
        let roots = vec![
            root("bob", Some(Hue::Orange), 5),
            root("sase", Some(Hue::Blue), 2),
            root("unfiled", None, 1),
        ];
        assert_eq!(
            human_roots(&roots, &Styler::plain()),
            "○ bob 5 - ○ sase 2 - ○ unfiled 1"
        );
        assert_eq!(
            human_roots(&roots, &Styler::colored()),
            "\u{1b}[38;5;208m○\u{1b}[0m bob \u{1b}[2m5\u{1b}[0m \
             · \u{1b}[38;5;33m○\u{1b}[0m sase \u{1b}[2m2\u{1b}[0m \
             · \u{1b}[2m○\u{1b}[0m unfiled \u{1b}[2m1\u{1b}[0m"
        );
        assert_eq!(human_roots(&[], &Styler::plain()), "");
    }

    #[test]
    fn no_tasks_means_no_roots_and_no_vault_reads() {
        let missing = Path::new("/nonexistent/bob-vault");
        assert!(
            today_roots(missing, Path::new("2026/20261101.md"), &[]).is_empty()
        );
    }
}
