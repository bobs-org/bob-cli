//! `bob ref` alias equivalence: the permanent `highlights` and
//! `highlights-ref` spellings stay byte-identical to the canonical command.

mod find;
mod list;
mod migrate_zorg;
mod show;

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Output;

const ALIASES: &[&str] = &["highlights", "highlights-ref"];
const VERBS: &[&str] = &["create", "doctor", "jobs", "marker", "scan", "sync"];
const FIXED_NOW: &str = "2026-10-06 12:00:00";

fn run_root(root: &str, args: &[&str], vault: &Path) -> Output {
    let mut argv = vec![root.to_string()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    bob_command()
        .args(&argv)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .unwrap_or_else(|error| panic!("run bob {argv:?}: {error}"))
}

fn assert_identical(left: &Output, right: &Output, label: &str) {
    assert_eq!(
        left.status.code(),
        right.status.code(),
        "{label}: exit code\nleft:\n{}\nright:\n{}",
        format_output(left),
        format_output(right)
    );
    assert_eq!(
        stdout(left),
        stdout(right),
        "{label}: stdout\nleft:\n{}\nright:\n{}",
        format_output(left),
        format_output(right)
    );
    assert_eq!(
        stderr(left),
        stderr(right),
        "{label}: stderr\nleft:\n{}\nright:\n{}",
        format_output(left),
        format_output(right)
    );
    for (side, output) in [("alias", left), ("canonical", right)] {
        let combined = format!("{}\n{}", stdout(output), stderr(output));
        assert!(
            !combined.to_lowercase().contains("deprecat"),
            "{label}: {side} output advertises deprecation:\n{}",
            format_output(output)
        );
    }
}

fn assert_alias_matches_canonical(args: &[&str], vault: &Path, label: &str) {
    let canonical = run_root("ref", args, vault);
    for alias in ALIASES {
        let aliased = run_root(alias, args, vault);
        assert_identical(&aliased, &canonical, &format!("{label} [{alias}]"));
    }
}

fn walk_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    walk_snapshot_inner(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn walk_snapshot_inner(
    root: &Path,
    path: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) {
    let entries = fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child = entry.path();
        if child.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if child.is_dir() {
            walk_snapshot_inner(root, &child, files);
        } else {
            let relative = child
                .strip_prefix(root)
                .expect("child under root")
                .to_string_lossy()
                .into_owned();
            let contents = fs::read(&child).expect("read file");
            files.push((relative, contents));
        }
    }
}

fn write_scan_fixture(vault: &Path) {
    write_highlights_pdf(
        &vault.join("lib/papers/alias-parity.pdf"),
        "- status: ready\n- parent: obsidian\n- title: Alias Parity\n",
    );
}

#[test]
fn ref_help_matches_aliases_for_every_verb() {
    let temp = TempDir::new("bob-cli-ref-alias-help");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let before = walk_snapshot(&vault);

    let mut cases: Vec<Vec<String>> = vec![vec!["--help".to_string()]];
    for verb in VERBS {
        cases.push(vec![verb.to_string(), "--help".to_string()]);
    }
    for args in &cases {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_alias_matches_canonical(&argv, &vault, &format!("{args:?}"));
    }

    assert_eq!(
        walk_snapshot(&vault),
        before,
        "help must not write to the vault"
    );
}

#[test]
fn ref_error_paths_match_aliases_for_every_verb() {
    let temp = TempDir::new("bob-cli-ref-alias-errors");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let before = walk_snapshot(&vault);

    // One deterministic error path per verb: no vault writes involved.
    let cases: &[&[&str]] = &[
        &["create", "--dry-run"],
        &["doctor", "--prefer", "marker"],
        &["jobs", "list", "--format", "bogus"],
        &["marker"],
        &["scan", "--jobs", "0"],
        &["sync"],
    ];
    assert_eq!(cases.len(), VERBS.len());
    for (verb, args) in VERBS.iter().zip(cases.iter()) {
        assert_alias_matches_canonical(args, &vault, verb);
        let canonical = run_root("ref", args, &vault);
        assert_ne!(
            canonical.status.code(),
            Some(0),
            "{verb:?} error path should fail:\n{}",
            format_output(&canonical)
        );
    }

    assert_eq!(
        walk_snapshot(&vault),
        before,
        "error paths must not write to the vault"
    );
}

#[test]
fn ref_doctor_and_scan_dry_run_match_aliases_on_fixture_vault() {
    let temp = TempDir::new("bob-cli-ref-alias-vault");
    let vault = temp.path().join("vault");
    write_scan_fixture(&vault);
    let before = walk_snapshot(&vault);

    for args in [
        &["doctor"][..],
        &["--no-hooks", "doctor"][..],
        &["scan", "--dry-run"][..],
    ] {
        assert_alias_matches_canonical(args, &vault, &format!("{args:?}"));
    }

    assert_eq!(
        walk_snapshot(&vault),
        before,
        "doctor and scan --dry-run must not write to the vault"
    );
}
