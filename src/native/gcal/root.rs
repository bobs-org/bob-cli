//! Default-root validation shared by doctor now and list/sync later.

use std::path::{Path, PathBuf};

use crate::native::{
    pomodoro_homes::{path_string, DiskHomeSource, Home, HomeResolver},
    task_root::is_invalid_root_value,
    vault_links::LinkResolution,
};

/// A resolved, valid root candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootTarget {
    pub name: String,
    pub path: PathBuf,
    pub home: Home,
}

/// Why a root candidate was rejected, with R11 reason and fix text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootRejection {
    pub reason: &'static str,
    pub message: String,
    pub fix: String,
}

/// Validate `name` as a `[root::]` value relative to today's daily note.
pub(crate) fn validate_root_candidate(
    vault: &Path,
    daily_rel: &Path,
    name: &str,
) -> Result<RootTarget, RootRejection> {
    let trimmed = name.trim();
    if trimmed.is_empty() || is_invalid_root_value(trimmed) {
        return Err(RootRejection {
            reason: "invalid",
            message: format!(
                "{trimmed:?} is not a valid root name (blank, or contains \
                 [ ] | # ^)"
            ),
            fix: "write the bare note name, for example meet".to_string(),
        });
    }

    let source = DiskHomeSource::new(vault);
    let mut resolver = HomeResolver::new(source);
    match resolver.resolve_target(daily_rel, trimmed) {
        LinkResolution::Missing => Err(RootRejection {
            reason: "missing",
            message: format!("no note matches {trimmed:?}"),
            fix: format!("create {trimmed}.md or pick an existing area"),
        }),
        LinkResolution::Ambiguous => Err(RootRejection {
            reason: "ambiguous",
            message: format!("two or more notes match {trimmed:?}"),
            fix: format!("use a vault-relative path such as {trimmed}.md"),
        }),
        LinkResolution::Found(path) => {
            let home = resolver.resolve_note(&path);
            if home.key == "@unfiled" {
                return Err(RootRejection {
                    reason: "no_home",
                    message: format!(
                        "{} is not an area or project",
                        path.display()
                    ),
                    fix: format!(
                        "set type: area or type: project on {}",
                        path.display()
                    ),
                });
            }
            let target_key = path_string(&path);
            if home.root_parent != target_key {
                return Err(RootRejection {
                    reason: "root_field_not_root",
                    message: format!(
                        "{trimmed} groups under {}; write [root:: {}] or \
                         set root: true on {trimmed}",
                        home.root_parent, home.root_parent
                    ),
                    fix: format!(
                        "{} groups under {}; write [root:: {}] or set \
                         root: true on {trimmed}",
                        trimmed, home.root_parent, home.root_parent
                    ),
                });
            }
            Ok(RootTarget {
                name: trimmed.to_string(),
                path,
                home,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_note(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, body).expect("write note");
    }

    #[test]
    fn rejects_invalid_and_missing() {
        let dir = tempfile::tempdir().expect("temp");
        write_note(dir.path(), "2026/20261030.md", "# day\n");
        let daily = Path::new("2026/20261030.md");
        let invalid = validate_root_candidate(dir.path(), daily, "[[meet]]")
            .expect_err("wikilink");
        assert_eq!(invalid.reason, "invalid");
        let missing = validate_root_candidate(dir.path(), daily, "meet")
            .expect_err("missing");
        assert_eq!(missing.reason, "missing");
    }

    #[test]
    fn accepts_an_area_that_is_its_own_root() {
        let dir = tempfile::tempdir().expect("temp");
        write_note(
            dir.path(),
            "meet.md",
            "---\ntype: area\ncolor: violet\n---\n# Meet\n",
        );
        write_note(dir.path(), "2026/20261030.md", "# day\n");
        let target = validate_root_candidate(
            dir.path(),
            Path::new("2026/20261030.md"),
            "meet",
        )
        .expect("area");
        assert_eq!(target.name, "meet");
        assert_eq!(
            target.home.kind,
            crate::native::pomodoro_homes::HomeKind::Area
        );
    }
}
