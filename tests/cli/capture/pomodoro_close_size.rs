//! Sized `[N]x` Pomodoro closes parse, then execution refuses until the
//! later timing phase lands. Nothing is written, including on `--dry-run`
//! and in a multi-item batch.

use super::pomodoro_close::{close_worked_vault, run_close_expect_error};
use crate::support::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const NOW: &str = "2026-09-28 09:37:00";
const CODE: &str = "pomodoro_close_size_unavailable";

fn size_unavailable_message(spec_raw: &str) -> String {
    format!(
        "`{spec_raw}` closes at a size, which this bob cannot execute yet; nothing was written"
    )
}

fn vault_files(vault: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, root: &Path, files: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, files);
            } else {
                let rel = path.strip_prefix(root).expect("rel").to_path_buf();
                files.insert(rel, fs::read_to_string(&path).expect("read"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(vault, vault, &mut files);
    files
}

fn parse_error_json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("expected JSON error, got {text:?}: {error}")
    })
}

fn run_sized_error(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .args(flags)
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run sized close");
    assert!(
        !output.status.success(),
        "expected failure:\n{}",
        format_output(&output)
    );
    parse_error_json(stdout(&output).trim())
}

fn assert_size_unavailable(json: &serde_json::Value, spec_raw: &str) {
    assert_eq!(json["ok"], false, "{json}");
    assert_eq!(json["code"], CODE, "{json}");
    let error = json["error"].as_str().unwrap_or("");
    assert!(
        error.contains(&size_unavailable_message(spec_raw)),
        "{json}"
    );
}

fn unchanged_sized_close(
    name: &str,
    flags: &[&str],
    args: &[&str],
    spec_raw: &str,
) {
    let (_temp, vault, day_file) = close_worked_vault(name);
    let before = vault_files(&vault);
    let json = if flags.is_empty() {
        parse_error_json(&run_close_expect_error(&vault, &day_file, NOW, args))
    } else {
        run_sized_error(&vault, &day_file, flags, args)
    };
    assert_size_unavailable(&json, spec_raw);
    assert_eq!(vault_files(&vault), before, "{args:?} {flags:?}");
}

fn sized_close_drafts() -> &'static [(&'static str, &'static str)] {
    &[
        ("2x!2", "2x!2"),
        ("2x0", "2x0"),
        ("@bob:ready=2x", "=2x"),
        ("^bob:ready=2x", "=2x"),
        ("New task @bob:new-size=2x", "=2x"),
    ]
}

#[test]
fn sized_pomodoro_closes_refuse_execution_and_write_nothing() {
    for (index, &(draft, spec_raw)) in sized_close_drafts().iter().enumerate() {
        unchanged_sized_close(
            &format!("bob-cli-sized-close-{index}"),
            &[],
            &[draft],
            spec_raw,
        );
    }
}

#[test]
fn sized_pomodoro_closes_refuse_dry_run_and_write_nothing() {
    for (index, &(draft, spec_raw)) in sized_close_drafts().iter().enumerate() {
        unchanged_sized_close(
            &format!("bob-cli-sized-close-dry-{index}"),
            &["--dry-run"],
            &[draft],
            spec_raw,
        );
    }
}

#[test]
fn sized_close_in_a_batch_rolls_the_whole_batch_back() {
    for (index, draft) in ["-2\n\n2x", "Do this\n\n2x"].into_iter().enumerate()
    {
        unchanged_sized_close(
            &format!("bob-cli-sized-close-batch-{index}"),
            &[],
            &[draft],
            "2x",
        );
    }
}
