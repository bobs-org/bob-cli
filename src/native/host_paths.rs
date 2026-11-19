//! Shared expansion and display helpers for paths supplied by the host.

use std::{
    collections::HashSet,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use glob::{glob_with, MatchOptions, Pattern};

use super::env;

#[derive(Debug)]
pub(crate) enum OperandError {
    NoMatch {
        operand: OsString,
    },
    Unreadable {
        path: PathBuf,
        error: String,
        kind: io::ErrorKind,
    },
}

/// Expand host file operands in argv order, with existing paths taking
/// precedence over glob syntax and duplicate files kept only once.
pub(crate) fn expand_operands(
    operands: &[OsString],
) -> Result<Vec<PathBuf>, OperandError> {
    let options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    let mut expanded = Vec::new();
    let mut seen = HashSet::new();

    for operand in operands {
        let path = env::expand_tilde(Path::new(operand));
        let candidates = if fs::symlink_metadata(&path).is_ok() {
            vec![path]
        } else if let Some(pattern) = path.to_str().filter(|text| {
            text.contains('*') || text.contains('?') || text.contains('[')
        }) {
            let _compiled = match Pattern::new(pattern) {
                Ok(compiled) => compiled,
                Err(_) => {
                    push_unique(path, &mut expanded, &mut seen);
                    continue;
                }
            };
            let paths = glob_with(pattern, options)
                .expect("the pattern was already validated");
            let mut matches = collect_files(paths)?;

            // glob 0.3 filters every dot-prefixed directory entry before it
            // considers the pattern component. Re-run explicit dot globs
            // with that traversal filter disabled, then apply the Pattern
            // with the requested options so only explicitly named hidden
            // components survive.
            if has_explicit_dot_glob_component(pattern) {
                let mut hidden_options = options;
                hidden_options.require_literal_leading_dot = false;
                let hidden_paths = glob_with(pattern, hidden_options)
                    .expect("the pattern was already validated");
                let normalized = strip_leading_dot_prefixes(pattern);
                let hidden_pattern = Pattern::new(normalized)
                    .expect("removing leading ./ cannot invalidate a pattern");
                matches.extend(collect_explicit_hidden(
                    hidden_paths,
                    &hidden_pattern,
                    options,
                )?);
            }
            matches.sort();
            matches.dedup();
            if matches.is_empty() {
                return Err(OperandError::NoMatch {
                    operand: operand.clone(),
                });
            }
            matches
        } else {
            vec![path]
        };

        for candidate in candidates {
            push_unique(candidate, &mut expanded, &mut seen);
        }
    }

    Ok(expanded)
}

fn collect_files(
    paths: impl Iterator<Item = glob::GlobResult>,
) -> Result<Vec<PathBuf>, OperandError> {
    let mut matches = Vec::new();
    for entry in paths {
        match entry {
            Ok(path) if path.is_file() => matches.push(path),
            Ok(_) => {}
            Err(error) => {
                return Err(OperandError::Unreadable {
                    path: error.path().to_path_buf(),
                    error: error.error().to_string(),
                    kind: error.error().kind(),
                });
            }
        }
    }
    Ok(matches)
}

fn collect_explicit_hidden(
    paths: impl Iterator<Item = glob::GlobResult>,
    pattern: &Pattern,
    options: MatchOptions,
) -> Result<Vec<PathBuf>, OperandError> {
    let mut matches = Vec::new();
    for entry in paths {
        match entry {
            Ok(path)
                if path.is_file()
                    && pattern.matches_path_with(&path, options) =>
            {
                matches.push(path)
            }
            Ok(_) => {}
            Err(error) => {
                return Err(OperandError::Unreadable {
                    path: error.path().to_path_buf(),
                    error: error.error().to_string(),
                    kind: error.error().kind(),
                });
            }
        }
    }
    Ok(matches)
}

fn strip_leading_dot_prefixes(mut pattern: &str) -> &str {
    while let Some(rest) = pattern.strip_prefix("./") {
        pattern = rest;
    }
    pattern
}

fn has_explicit_dot_glob_component(pattern: &str) -> bool {
    Path::new(pattern).components().any(|component| {
        let component = component.as_os_str().to_string_lossy();
        component.starts_with('.')
            && (component.contains('*')
                || component.contains('?')
                || component.contains('['))
    })
}

fn push_unique(
    path: PathBuf,
    expanded: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) {
    let identity = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if seen.insert(identity) {
        expanded.push(path);
    }
}

/// Display absolute paths under `$HOME` using a shell-style home prefix.
pub(crate) fn display_path(path: &Path) -> String {
    let home = env::home_dir();
    if path.is_absolute()
        && home.is_absolute()
        && let Ok(suffix) = path.strip_prefix(&home)
    {
        if suffix.as_os_str().is_empty() {
            return "~".to_owned();
        }
        return Path::new("~").join(suffix).to_string_lossy().into_owned();
    }
    path.to_string_lossy().into_owned()
}

pub(crate) fn permission_hint() -> &'static str {
    "bob cannot access this file; on macOS, allow the app running bob (Obsidian or your terminal) to use this folder in System Settings → Privacy & Security → Files and Folders"
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
    };

    use tempfile::TempDir;

    use super::*;
    use crate::native::env::TestEnvGuard;

    fn home(temp: &TempDir) -> TestEnvGuard {
        TestEnvGuard::set(&[("HOME", Some(temp.path().as_os_str()))])
    }

    fn downloads(temp: &TempDir) -> PathBuf {
        let path = temp.path().join("Downloads");
        fs::create_dir_all(&path).expect("Downloads");
        path
    }

    fn expand(values: &[&str]) -> Result<Vec<PathBuf>, OperandError> {
        expand_operands(&values.iter().map(OsString::from).collect::<Vec<_>>())
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect()
    }

    #[test]
    fn expands_tilde_from_home() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let file = downloads(&temp).join("checking.qfx");
        fs::write(&file, b"qfx").expect("file");

        assert_eq!(expand(&["~/Downloads/checking.qfx"]).unwrap(), vec![file]);
    }

    #[test]
    fn an_existing_bracketed_filename_wins_over_pattern_syntax() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let directory = downloads(&temp);
        let literal = directory.join("statement [1].qfx");
        fs::write(&literal, b"literal").expect("literal file");
        fs::write(directory.join("statement 1.qfx"), b"pattern match")
            .expect("pattern match file");

        assert_eq!(
            expand(&["~/Downloads/statement [1].qfx"]).unwrap(),
            vec![literal]
        );
    }

    #[test]
    fn expands_star_question_mark_and_character_classes_in_sorted_order() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let directory = downloads(&temp);
        for name in ["a.qfx", "b.qfx", "txn1.ofx", "txn2.ofx"] {
            fs::write(directory.join(name), b"export").expect("export");
        }

        assert_eq!(
            names(&expand(&["~/Downloads/*.qfx"]).unwrap()),
            ["a.qfx", "b.qfx"]
        );
        assert_eq!(
            names(&expand(&["~/Downloads/txn?.ofx"]).unwrap()),
            ["txn1.ofx", "txn2.ofx"]
        );
        assert_eq!(
            names(&expand(&["~/Downloads/[ab].qfx"]).unwrap()),
            ["a.qfx", "b.qfx"]
        );
    }

    #[test]
    fn expands_names_with_spaces_and_skips_directories() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let directory = downloads(&temp);
        fs::write(directory.join("checking export.qfx"), b"export")
            .expect("export");
        fs::create_dir(directory.join("archived.qfx"))
            .expect("matching directory");

        assert_eq!(
            names(&expand(&["~/Downloads/*"]).unwrap()),
            ["checking export.qfx"]
        );
    }

    #[test]
    fn hidden_entries_require_a_leading_dot_pattern() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let directory = downloads(&temp);
        fs::write(directory.join(".hidden.qfx"), b"hidden")
            .expect("hidden export");
        fs::write(directory.join("visible.qfx"), b"visible")
            .expect("visible export");

        assert_eq!(
            names(&expand(&["~/Downloads/*"]).unwrap()),
            ["visible.qfx"]
        );
        assert_eq!(
            names(&expand(&["~/Downloads/.*"]).unwrap()),
            [".hidden.qfx"]
        );
        assert_eq!(
            strip_leading_dot_prefixes("././Downloads/.*.qfx"),
            "Downloads/.*.qfx"
        );
        assert_eq!(
            strip_leading_dot_prefixes("../Downloads/*.qfx"),
            "../Downloads/*.qfx"
        );
    }

    #[test]
    fn no_match_reports_the_original_operand() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        downloads(&temp);

        assert!(matches!(
            expand(&["~/Downloads/*.qfx"]),
            Err(OperandError::NoMatch { operand })
                if operand == "~/Downloads/*.qfx"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_reported_as_io_error() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let protected = temp.path().join("private");
        fs::create_dir(&protected).expect("private directory");
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o000))
            .expect("make unreadable");
        let result = expand(&["~/private/*.qfx"]);
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o700))
            .expect("restore permissions");

        assert!(matches!(
            result,
            Err(OperandError::Unreadable {
                kind: io::ErrorKind::PermissionDenied,
                ..
            })
        ));
    }

    #[test]
    fn invalid_pattern_falls_back_to_a_literal_path() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let literal = temp.path().join("bad[");

        assert_eq!(expand(&["~/bad["]).unwrap(), vec![literal]);
    }

    #[test]
    fn overlapping_patterns_deduplicate_with_first_appearance_order() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let directory = downloads(&temp);
        for name in ["a.qfx", "b.qfx"] {
            fs::write(directory.join(name), b"export").expect("export");
        }

        assert_eq!(
            names(
                &expand(&["~/Downloads/*.qfx", "~/Downloads/a*.qfx",]).unwrap()
            ),
            ["a.qfx", "b.qfx"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_operands_pass_through_literally() {
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let operand =
            OsString::from_vec(b"~/Downloads/statement\xff.qfx".to_vec());
        let expected = PathBuf::from(&operand);

        assert_eq!(expand_operands(&[operand]).unwrap(), vec![expected]);
    }

    #[test]
    fn display_path_abbreviates_only_absolute_paths_under_home() {
        let temp = TempDir::new().expect("temporary home");
        let _home = home(&temp);
        let inside = temp.path().join("Downloads/statement.qfx");
        let outside = temp.path().parent().unwrap().join("other.qfx");

        assert_eq!(display_path(&inside), "~/Downloads/statement.qfx");
        assert_eq!(display_path(temp.path()), "~");
        assert_eq!(display_path(&outside), outside.to_string_lossy());
        assert_eq!(display_path(Path::new("relative.qfx")), "relative.qfx");
    }

    #[test]
    fn permission_hint_names_the_macos_settings_path() {
        let hint = permission_hint();
        assert!(hint.contains(
            "System Settings → Privacy & Security → Files and Folders"
        ));
    }
}
