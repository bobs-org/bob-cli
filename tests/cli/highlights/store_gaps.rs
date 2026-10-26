//! Store-phase integration gaps: routes, reread, rename, dry-run,
//! `-o` outside the library, ignored catalogs, and store-write failures.
//!
//! Markdown creates use the fake-pandoc harness from `ingest.rs`. The fake
//! only copies a static PDF, so no render-time catalog exists: `code.json`
//! is recorded with a `catalog_error` and `bob ref code` reports
//! `no_catalog`. Route, pointer, and history assertions read the store
//! directory on disk (and the create report), which is where the wiring
//! under test lives.

use crate::support::*;
use std::{fs, path::Path};

use super::ingest::{create_command, create_fixture, seed_synced_vault};

/// Extract the `source_markdown: <ptr>` line from a create report.
fn report_pointer(report: &str) -> String {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix("source_markdown:"))
        .unwrap_or_else(|| panic!("no source_markdown in report:\n{report}"))
        .trim()
        .to_string()
}

fn read_code_json(vault: &Path, pointer: &str) -> serde_json::Value {
    let dir = pointer.strip_suffix("/source.md").expect("pointer suffix");
    let raw = fs::read_to_string(vault.join(dir).join("code.json"))
        .expect("read code.json");
    serde_json::from_str(&raw).expect("parse code.json")
}

fn code_json_status(vault: &Path, reference: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("ref")
        .arg("code")
        .arg(reference)
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("run bob ref code");
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!("parse code JSON: {error}\n{}", format_output(&output))
    })
}

fn note_pointer(vault: &Path, relative: &str) -> Option<String> {
    let contents =
        fs::read_to_string(vault.join(relative)).expect("read ref note");
    contents.lines().find_map(|line| {
        line.strip_prefix("source_markdown:")
            .map(|value| value.trim().trim_matches('"').to_string())
    })
}

#[test]
fn store_markdown_create_records_route_and_pointer() {
    let temp = TempDir::new("bob-cli-store-markdown-route");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("report.md");
    write_file(
        &source,
        "# Report\n\nBody with `code`.\n\n```rust\nfn main() {}\n```\n",
    );
    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-N")
        .arg("store_route")
        .output()
        .expect("run create");
    assert_success(&output);
    let report = stdout(&output);
    let pointer = report_pointer(&report);
    assert!(
        pointer.starts_with(".bob/ref-sources/chat/store_route--"),
        "{report}"
    );

    // The store directory holds the verified source plus a catalog record
    // carrying the create route.
    assert!(vault.join(&pointer).is_file(), "{report}");
    let catalog = read_code_json(&vault, &pointer);
    assert_eq!(catalog["version"], serde_json::Value::from(1));
    assert_eq!(catalog["route"], serde_json::Value::from("markdown"));
    assert_eq!(
        catalog["source"]["bytes"],
        serde_json::Value::from(
            "# Report\n\nBody with `code`.\n\n```rust\nfn main() {}\n```\n"
                .len() as u64
        )
    );

    // The fake copies a static PDF whose marker carries no source
    // information, so the intake-filed note itself has no pointer; the
    // report and the on-disk store above are the wiring this harness can
    // observe. (Real renders embed the pointer in the PDF marker, and the
    // hand-built rename test below covers the read surface.)
    let value = code_json_status(&vault, "store_route");
    assert_eq!(value["status"], serde_json::Value::from("no_source"));
}

#[test]
fn store_plain_note_create_records_plain_route_with_source_note() {
    let temp = TempDir::new("bob-cli-store-plain-note");
    let (vault, _peer) = seed_synced_vault(&temp);
    write_file(&vault.join("Area.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("zk/plain_note.md"),
        "---\nparent: [[Area]]\n---\n# Plain Note\n\nSource body.\n",
    );
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "seed notes"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    let fixture = temp.path().join("rendered.pdf");
    write_highlights_pdf_pages(&fixture, &[&[]]);
    let pandoc = temp.path().join("fake-pandoc.sh");
    write_executable(
        &pandoc,
        "#!/bin/sh\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp \"$BOB_TEST_FIXTURE_PDF\" \"$out\"\n",
    );
    let output = bob_command()
        .arg("ref")
        .arg("create")
        .arg("zk/plain_note")
        .arg("-M")
        .arg("-b")
        .arg(&vault)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-16 12:00:00")
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env("BOB_TEST_FIXTURE_PDF", &fixture)
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run plain vault-note create");
    assert_success(&output);
    let report = stdout(&output);
    let pointer = report_pointer(&report);
    let catalog = read_code_json(&vault, &pointer);
    assert_eq!(catalog["route"], serde_json::Value::from("plain_note"));
    assert_eq!(
        catalog["source"]["source_note"],
        serde_json::Value::from("zk/plain_note")
    );
    assert!(catalog["source"]["source_note_sha256"].is_string());
}

#[test]
fn store_snapshot_create_records_note_snapshot_route() {
    let pandoc_available = std::process::Command::new("pandoc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    let xelatex_available = std::process::Command::new("xelatex")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !pandoc_available || !xelatex_available {
        eprintln!("skipping snapshot route test: pandoc and xelatex required");
        return;
    }
    let temp = TempDir::new("bob-cli-store-snapshot-route");
    let (vault, _peer) = seed_synced_vault(&temp);
    write_file(&vault.join("Area.md"), "---\ntype: [[area]]\n---\n# Area\n");
    write_file(
        &vault.join("zk/harbor_cranes.md"),
        "---\nparent: [[Area]]\n---\n# Harbor Cranes\n\nA source note.\n",
    );
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "seed notes"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    let output = bob_command()
        .arg("ref")
        .arg("create")
        .arg("harbor_cranes")
        .arg("-b")
        .arg(&vault)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-16 12:00:00")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run snapshot create");
    assert_success(&output);
    let report = stdout(&output);
    let pointer = report_pointer(&report);
    let catalog = read_code_json(&vault, &pointer);
    assert_eq!(catalog["route"], serde_json::Value::from("note_snapshot"));
    assert_eq!(
        catalog["source"]["source_note"],
        serde_json::Value::from("zk/harbor_cranes")
    );
    assert!(catalog["source"]["source_note_sha256"].is_string());
}

#[test]
fn store_reread_gets_new_directory_and_pointer() {
    let temp = TempDir::new("bob-cli-store-reread");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("reread.md");
    write_file(&source, "# Reread\n\nVersion one.\n");
    let first = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-N")
        .arg("reread_note")
        .output()
        .expect("first create");
    assert_success(&first);
    let first_pointer = report_pointer(&stdout(&first));
    assert!(vault.join(&first_pointer).is_file());

    write_file(&source, "# Reread\n\nVersion two.\n");
    let second = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-N")
        .arg("reread_note")
        .arg("-f")
        .output()
        .expect("reread create");
    assert_success(&second);
    let second_report = stdout(&second);
    assert!(second_report.contains("reread:"), "{second_report}");
    let second_pointer = report_pointer(&second_report);
    assert_ne!(
        first_pointer, second_pointer,
        "reread must mint a new store directory"
    );
    assert!(vault.join(&second_pointer).is_file());
    assert!(
        vault.join(&first_pointer).is_file(),
        "old store stays as history"
    );
}

#[test]
fn store_rename_keeps_pointer_and_store_directory() {
    let temp = TempDir::new("bob-cli-store-rename-handbuilt");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    for directory in ["lib/papers", "ref/papers", "xlib"] {
        fs::create_dir_all(vault.join(directory)).expect("create dir");
    }
    write_area_note(&vault.join("sase.md"), "SASE");
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "seed vault"]);
    let pdf_rel = "lib/papers/rename_me.pdf";
    write_highlights_pdf(
        &vault.join(pdf_rel),
        "- status: ready\n- parent: sase\n- title: Rename Me\n- id: rename_me\n",
    );

    let sync = bob_command()
        .arg("ref")
        .arg("sync")
        .arg(vault.join(pdf_rel))
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("run initial ref sync");
    assert_success(&sync);
    assert!(
        note_pointer(&vault, "ref/papers/rename_me.md").is_none(),
        "marker sync alone files no pointer"
    );

    // Hand-build a verified store for the synced PDF, attach the pointer
    // to the note, and re-sync: repeat plans preserve the authored line,
    // so the note lands back in sync with the pointer intact.
    let source = "# Rename Me\n\nBody.\n";
    let digest = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(source.as_bytes()))
    };
    let hash12 = &digest[..12];
    let dir =
        vault.join(format!(".bob/ref-sources/papers/rename_me--{hash12}"));
    fs::create_dir_all(&dir).expect("store dir");
    write_file(&dir.join("source.md"), source);
    let pdf_sha = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(
            fs::read(vault.join(pdf_rel)).expect("read pdf"),
        ))
    };
    write_file(
        &dir.join("code.json"),
        &format!(
            r#"{{"version":1,"captured_at":"2026-10-20T00:00:00Z","route":"markdown","source":{{"sha256":"{digest}","bytes":{},"origin":"/tmp/rename_me.md"}},"pdf":{{"path":"{pdf_rel}","sha256":"{pdf_sha}"}},"renderer":{{"bob":"0","pandoc":"test"}},"blocks":[]}}"#,
            source.len(),
        ),
    );
    let pointer =
        format!(".bob/ref-sources/papers/rename_me--{hash12}/source.md");
    let note_path = vault.join("ref/papers/rename_me.md");
    let before = fs::read_to_string(&note_path).expect("read synced note");
    let with_pointer = before.replacen(
        "---\n",
        &format!("---\nsource_markdown: {pointer}\n"),
        1,
    );
    assert_ne!(before, with_pointer, "note has frontmatter to extend");
    write_file(&note_path, &with_pointer);
    // `bob ref sync` refuses on a dirty worktree, so commit the
    // pointer attachment before re-syncing.
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "attach pointer"]);

    let resync = bob_command()
        .arg("ref")
        .arg("sync")
        .arg(vault.join(pdf_rel))
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("re-sync with pointer");
    assert_success(&resync);
    let kept = note_pointer(&vault, "ref/papers/rename_me.md")
        .expect("repeat sync preserves the authored pointer");
    assert_eq!(kept, pointer);
    // Commit anything the re-sync wrote so the rename lands clean.
    git_in(&vault, ["add", "-A"]);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&vault)
        .arg("status")
        .arg("--porcelain")
        .output()
        .expect("git status");
    if !status.stdout.is_empty() {
        git_in(&vault, ["commit", "-q", "-m", "resync"]);
    }

    let renamed = bob_command()
        .arg("ref")
        .arg("rename")
        .arg("rename_me")
        .arg("renamed_note")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("run rename");
    assert_success(&renamed);
    let after = note_pointer(&vault, "ref/papers/renamed_note.md")
        .expect("renamed note keeps a pointer");
    assert_eq!(after, pointer, "rename keeps source_markdown verbatim");
    assert!(
        dir.is_dir(),
        "rename must not move or copy the store directory"
    );
    assert!(
        vault.join(&pointer).is_file(),
        "the pointed-to source.md stays in place"
    );
    // No second store directory may appear for the new stem.
    let chat_stores = fs::read_dir(vault.join(".bob/ref-sources/papers"))
        .expect("list paper stores")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("renamed_note--")
        })
        .count();
    assert_eq!(chat_stores, 0, "rename must not copy the store");

    // The preserved pointer still resolves through the read surface.
    let value = code_json_status(&vault, "renamed_note");
    assert_eq!(value["status"], serde_json::Value::from("no_code"));
    assert!(value["source"]["text"].is_string(), "{value}");
}

#[test]
fn store_dry_run_prints_planned_pointer_and_writes_nothing() {
    let temp = TempDir::new("bob-cli-store-dry-run");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("dry.md");
    write_file(&source, "# Dry Run\n");
    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-N")
        .arg("dry_store")
        .arg("-d")
        .output()
        .expect("dry run");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("source_markdown: .bob/ref-sources/chat/dry_store--"),
        "{report}"
    );
    assert!(report.contains("writes: none"), "{report}");
    let store_root = vault.join(".bob/ref-sources");
    let mut leftover = Vec::new();
    if store_root.is_dir() {
        for type_entry in fs::read_dir(&store_root)
            .expect("list store root")
            .flatten()
        {
            for entry in fs::read_dir(type_entry.path())
                .expect("list store type")
                .flatten()
            {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("dry_store--")
                {
                    leftover.push(entry.path());
                }
            }
        }
    }
    assert!(leftover.is_empty(), "dry run created stores: {leftover:?}");
}

#[test]
fn store_output_outside_library_writes_no_store() {
    let temp = TempDir::new("bob-cli-store-outside");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("outside.md");
    write_file(&source, "# Outside\n\nBody.\n");
    let out_dir = temp.path().join("out");
    fs::create_dir_all(&out_dir).expect("out dir");
    let outside_pdf = out_dir.join("outside.pdf");
    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-o")
        .arg(&outside_pdf)
        .output()
        .expect("outside create");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("source_markdown: none (output outside the library)"),
        "{report}"
    );
    assert!(outside_pdf.is_file());
    let store_root = vault.join(".bob/ref-sources");
    let count = if store_root.is_dir() {
        fs::read_dir(&store_root)
            .expect("list store root")
            .flatten()
            .map(|entry| {
                fs::read_dir(entry.path())
                    .map(|inner| inner.count())
                    .unwrap_or(0)
            })
            .sum()
    } else {
        0
    };
    assert_eq!(count, 0, "outside output must write no store");
}

#[test]
fn store_ignored_catalog_warns_but_files_reference() {
    let temp = TempDir::new("bob-cli-store-ignored");
    let (vault, _peer) = seed_synced_vault(&temp);
    write_file(&vault.join(".gitignore"), ".bob/ref-sources/**/*.json\n");
    git_in(&vault, ["add", ".gitignore"]);
    git_in(&vault, ["commit", "-q", "-m", "ignore store catalogs"]);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("ignored.md");
    write_file(&source, "# Ignored\n\nBody.\n");
    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-N")
        .arg("ignored_note")
        .output()
        .expect("ignored create");
    assert_success(&output);
    let combined = format!("{}\n{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("code catalogs are ignored by the vault .gitignore"),
        "{combined}"
    );
    assert!(vault.join("ref/chat/ignored_note.md").is_file());
}

#[test]
fn store_write_failure_warns_and_files_without_pointer() {
    let temp = TempDir::new("bob-cli-store-failure");
    let (vault, _peer) = seed_synced_vault(&temp);
    // Make the store root a regular file so every store write fails.
    let root = vault.join(".bob/ref-sources");
    fs::create_dir_all(root.parent().expect("bob dir")).expect("bob dir");
    write_file(&root, "blocking file");
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("failure.md");
    write_file(&source, "# Failure\n\nBody.\n");
    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-N")
        .arg("failure_note")
        .output()
        .expect("failing create");
    assert_success(&output);
    let combined = format!("{}\n{}", stdout(&output), stderr(&output));
    assert!(combined.contains("warning:"), "{combined}");
    let note = fs::read_to_string(vault.join("ref/chat/failure_note.md"))
        .expect("read filed note");
    assert!(!note.contains("source_markdown:"), "{note}");
}
