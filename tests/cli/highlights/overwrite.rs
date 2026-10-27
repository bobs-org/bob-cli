//! Cross-route overwrite acceptance: preservation, archive inertness,
//! sidecar removal, and crash recovery through CLI fixtures.

use super::overwrite_local::{
    commit_if_dirty, commit_vault, filed_vault, note_body, overwrite_command,
    reread_command, snapshot_tree, source_pdf, OverwriteVault, FIXED_NOW, STEM,
    TITLE,
};
use super::overwrite_recovery;
use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

fn state_dir(fixture: &OverwriteVault) -> PathBuf {
    fixture.temp.path().join("state")
}

fn overwrite_subject() -> String {
    format!("bob ref create: overwrite papers/{STEM}")
}

fn add_prose(fixture: &OverwriteVault, extra: &str) {
    let note = fixture.vault.join(format!("ref/papers/{STEM}.md"));
    let mut body = fs::read_to_string(&note).expect("read live note");
    body.push_str(extra);
    fs::write(&note, body).expect("write authored prose");
}

fn frontmatter_line(note: &str, key: &str) -> String {
    note.lines()
        .find(|line| line.starts_with(&format!("{key}:")))
        .unwrap_or_else(|| panic!("missing {key}:\n{note}"))
        .to_string()
}

fn earlier_targets(note: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_earlier = false;
    for line in note.lines() {
        if line == "earlier:" {
            in_earlier = true;
            continue;
        }
        if in_earlier {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("- ") {
                targets.push(rest.trim().trim_matches('"').to_string());
            } else if !line.starts_with(' ') && !line.starts_with('\t') {
                break;
            }
        }
    }
    targets
}

fn parent_task_subtree(vault: &Path) -> String {
    let parent = fs::read_to_string(vault.join("sase.md")).expect("parent");
    let mut subtree = String::new();
    for line in parent.split_inclusive('\n') {
        if line.contains(&format!("^ref-{STEM}"))
            || (!subtree.is_empty() && line.starts_with([' ', '\t']))
        {
            subtree.push_str(line);
        } else if !subtree.is_empty() {
            break;
        }
    }
    assert!(
        !subtree.is_empty(),
        "expected a reading task in sase.md:\n{parent}"
    );
    subtree
}

fn parent_reading_task_count(vault: &Path) -> usize {
    fs::read_to_string(vault.join("sase.md"))
        .expect("parent")
        .lines()
        .filter(|line| {
            line.contains("#task")
                && line.contains("#ref")
                && line.contains("^ref-")
        })
        .count()
}

fn run_ref(fixture: &OverwriteVault, args: &[&str]) -> Output {
    let mut command = bob_command();
    command.arg("ref");
    for arg in args {
        command.arg(arg);
    }
    command
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .env("HOME", fixture.temp.path().join("home"))
        .env("XDG_STATE_HOME", state_dir(fixture))
        .env("XDG_CACHE_HOME", fixture.temp.path().join("cache"))
        .output()
        .unwrap_or_else(|error| panic!("run bob ref {args:?}: {error}"))
}

fn list_paths(fixture: &OverwriteVault) -> Vec<String> {
    let output = run_ref(fixture, &["list", "-R", "all", "-f", "json"]);
    assert_success(&output);
    let document: serde_json::Value = serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|error| {
            panic!("parse list JSON: {error}\n{}", format_output(&output))
        });
    document["refs"]
        .as_array()
        .expect("refs array")
        .iter()
        .map(|row| row["path"].as_str().expect("path").to_string())
        .collect()
}

fn find_json(fixture: &OverwriteVault, query: &str) -> serde_json::Value {
    let output = run_ref(fixture, &["find", query, "-f", "json"]);
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!("parse find JSON: {error}\n{}", format_output(&output))
    })
}

fn plant_filed_intent(
    fixture: &OverwriteVault,
    base: &str,
    expected_head: Option<&str>,
) {
    let canonical = fs::canonicalize(&fixture.vault)
        .expect("canonical vault")
        .to_string_lossy()
        .into_owned();
    let path =
        overwrite_recovery::intent_path(&state_dir(fixture), &fixture.vault);
    fs::create_dir_all(path.parent().expect("intent parent"))
        .expect("intent dirs");
    let document = serde_json::json!({
        "schema_version": 1,
        "vault": canonical,
        "stem": STEM,
        "ref_type": "papers",
        "base_head": base,
        "expected_subject": overwrite_subject(),
        "expected_head": expected_head,
        "touched": [format!("ref/papers/{STEM}.md")],
        "created": [],
    });
    fs::write(
        &path,
        serde_json::to_string(&document).expect("intent json"),
    )
    .expect("write intent");
}

#[test]
fn two_same_day_prose_overwrites_keep_distinct_archives_and_family() {
    let fixture =
        filed_vault("bob-cli-overwrite-accept-twice", "ready", false, false);
    let created = frontmatter_line(&note_body(&fixture), "created");
    let reread_source = source_pdf(&fixture, TITLE);
    let reread = reread_command(&fixture, &reread_source)
        .output()
        .expect("reread to v2");
    assert_success(&reread);
    assert!(
        fixture.vault.join("lib/papers/report__v2.pdf").is_file(),
        "family member v2 must exist:\n{}",
        stdout(&reread)
    );
    commit_if_dirty(&fixture.vault, "file v2 reading");

    add_prose(
        &fixture,
        "\nFirst freeze paragraph.\n\n- [x] leftover closed note #task\n",
    );
    commit_vault(&fixture.vault, "first authored prose");
    let first = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .args(["-N", STEM])
        .output()
        .expect("first overwrite");
    assert_success(&first);
    let first_report = stdout(&first);
    assert!(
        first_report
            .contains("earlier: ref_history/papers/report__2026-10-16.md")
            && first_report
                .contains(&format!("task: kept sase.md ^ref-{STEM}")),
        "first overwrite archives prose:\n{first_report}"
    );

    add_prose(&fixture, "\nSecond freeze paragraph.\n");
    commit_vault(&fixture.vault, "second authored prose");
    let second = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .args(["-N", STEM])
        .output()
        .expect("second overwrite");
    assert_success(&second);
    let second_report = stdout(&second);
    assert!(
        second_report
            .contains("earlier: ref_history/papers/report__2026-10-16_2.md"),
        "second same-day overwrite suffixes the archive:\n{second_report}"
    );

    let live = note_body(&fixture);
    assert_eq!(
        frontmatter_line(&live, "created"),
        created,
        "created stays the original value:\n{live}"
    );
    assert!(
        live.contains("recaptured: 2026-10-16"),
        "recaptured refreshes:\n{live}"
    );
    let earlier = earlier_targets(&live);
    let newest = "ref_history/papers/report__2026-10-16_2";
    let older = "ref_history/papers/report__2026-10-16|";
    assert!(
        earlier.iter().any(|entry| entry.contains(newest))
            && earlier.iter().any(|entry| entry.contains(older)),
        "both archives stay on earlier, newest first:\n{live}"
    );
    let first_pos = earlier
        .iter()
        .position(|entry| entry.contains(newest))
        .expect("newest archive");
    let second_pos = earlier
        .iter()
        .position(|entry| entry.contains(older))
        .expect("older archive");
    assert!(
        first_pos < second_pos,
        "newest archive is first:\n{earlier:?}"
    );
    assert_eq!(
        parent_reading_task_count(&fixture.vault),
        2,
        "v1 and v2 each keep one reading task"
    );
    assert!(
        fixture.vault.join("ref/papers/report__v2.md").is_file(),
        "existing family member survives both overwrites"
    );

    let first_archive = fs::read_to_string(
        fixture
            .vault
            .join("ref_history/papers/report__2026-10-16.md"),
    )
    .expect("first archive");
    let second_archive = fs::read_to_string(
        fixture
            .vault
            .join("ref_history/papers/report__2026-10-16_2.md"),
    )
    .expect("second archive");
    assert!(
        first_archive.contains("First freeze paragraph."),
        "first archive keeps its prose:\n{first_archive}"
    );
    assert!(
        second_archive.contains("Second freeze paragraph."),
        "second archive keeps its prose:\n{second_archive}"
    );
    assert!(
        !first_archive.contains("#task #ref")
            && !first_archive.contains("^ref"),
        "frozen tracker is inert:\n{first_archive}"
    );
}

#[test]
fn no_prose_overwrite_removes_each_sidecar_shape_and_stays_stable() {
    for (prefix, sidecar) in [
        (
            "bob-cli-overwrite-accept-md-sidecar",
            "lib/papers/report.md",
        ),
        (
            "bob-cli-overwrite-accept-textbundle",
            "lib/papers/report.textbundle/text.md",
        ),
    ] {
        let fixture = filed_vault(prefix, "ready", false, false);
        let sidecar_path = fixture.vault.join(sidecar);
        fs::create_dir_all(sidecar_path.parent().expect("sidecar parent"))
            .expect("sidecar dir");
        fs::write(&sidecar_path, "").expect("empty sidecar");
        commit_vault(&fixture.vault, "add empty sidecar");
        let output = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
            .output()
            .expect("overwrite with sidecar");
        assert_success(&output);
        let report = stdout(&output);
        assert!(
            report.contains("earlier: nothing to archive"),
            "empty sidecar is not authored prose:\n{report}"
        );
        assert!(
            !sidecar_path.exists(),
            "stale sidecar {sidecar} must be removed"
        );
        if sidecar.contains("textbundle") {
            assert!(
                !fixture.vault.join("lib/papers/report.textbundle").exists(),
                "emptied textbundle directory is pruned"
            );
        }
        let scan = run_ref(&fixture, &["scan", "--no-hooks"]);
        assert_success(&scan);
        assert!(
            !sidecar_path.exists(),
            "scan must not restore sidecar {sidecar}"
        );
        let sync = bob_command()
            .args(["ref", "sync"])
            .arg(&fixture.pdf)
            .env("BOB_DIR", &fixture.vault)
            .env("BOB_NOW", FIXED_NOW)
            .output()
            .expect("sync after overwrite");
        assert_success(&sync);
        assert!(
            !sidecar_path.exists(),
            "sync must not restore sidecar {sidecar}"
        );
    }
}

#[test]
fn overwrite_preserves_task_children_relations_freshness_and_inbound_links() {
    let fixture =
        filed_vault("bob-cli-overwrite-accept-task", "ready", false, false);
    write_file(
        &fixture.vault.join("other.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# Other\n\n- [ ] #task prerequisite ^other\n",
    );
    write_file(
        &fixture.vault.join("zk/pointer.md"),
        "# Pointer\n\nSee [[ref/papers/report]] and [[ref/papers/report#Highlights]].\n",
    );
    let parent_path = fixture.vault.join("sase.md");
    let parent = fs::read_to_string(&parent_path).expect("parent");
    let mut rewritten = String::new();
    let mut attached = false;
    for line in parent.split_inclusive('\n') {
        if line.contains(&format!("^ref-{STEM}")) && !attached {
            let trimmed = line.trim_end_matches('\n');
            let with_fields = if trimmed.contains("[fresh::") {
                trimmed.to_string()
            } else {
                let suffix = format!("^ref-{STEM}");
                if let Some(head) = trimmed.strip_suffix(&suffix) {
                    format!(
                        "{} [fresh:: 2026-10-16] [keeps:: 2] ^ref-{STEM}",
                        head.trim_end()
                    )
                } else {
                    format!("{trimmed} [fresh:: 2026-10-16] [keeps:: 2]")
                }
            };
            rewritten.push_str(&with_fields);
            if line.ends_with('\n') {
                rewritten.push('\n');
            }
            rewritten.push_str("  child with [priority:: high]\n");
            rewritten.push_str("  - ⛓️ **DEPENDS ON:** [[other#^other]]\n");
            rewritten.push_str("  - 🪢 **LINKS:** related [[other#^other]]\n");
            attached = true;
        } else {
            rewritten.push_str(line);
        }
    }
    assert!(attached, "failed to attach task details:\n{parent}");
    fs::write(&parent_path, rewritten).expect("write parent details");
    commit_vault(&fixture.vault, "task details and inbound links");

    let output = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .arg("-T")
        .arg("A Better Report")
        .output()
        .expect("overwrite with title");
    assert_success(&output);
    let subtree = parent_task_subtree(&fixture.vault);
    assert!(
        subtree.contains("[[ref/papers/report|A Better Report]]")
            && subtree.contains("child with [priority:: high]")
            && subtree.contains("**DEPENDS ON:** [[other#^other]]")
            && subtree.contains("**LINKS:** related [[other#^other]]")
            && subtree.contains("[fresh:: 2026-10-16]")
            && subtree.contains("[keeps:: 2]")
            && subtree.contains(&format!("^ref-{STEM}")),
        "task identity and children stay put:\n{subtree}"
    );
    let pointer = fs::read_to_string(fixture.vault.join("zk/pointer.md"))
        .expect("pointer");
    assert!(
        pointer.contains("[[ref/papers/report]]")
            && pointer.contains("[[ref/papers/report#Highlights]]"),
        "inbound note and heading links stay on the live note:\n{pointer}"
    );
    assert_eq!(
        parent_reading_task_count(&fixture.vault),
        1,
        "exactly one reading task remains"
    );
}

#[test]
fn frozen_archive_is_inert_across_list_find_doctor_and_create() {
    let fixture =
        filed_vault("bob-cli-overwrite-accept-inert", "ready", false, false);
    add_prose(&fixture, "\nAuthored freeze for inertness.\n");
    commit_vault(&fixture.vault, "authored freeze prose");
    let output = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .output()
        .expect("overwrite with archive");
    assert_success(&output);

    let paths = list_paths(&fixture);
    assert_eq!(
        paths
            .iter()
            .filter(|path| path.contains("ref_history/"))
            .count(),
        0,
        "list must not emit archive rows:\n{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|path| path == &format!("ref/papers/{STEM}.md")),
        "live note remains a library row:\n{paths:?}"
    );

    let by_path = find_json(&fixture, &format!("ref/papers/{STEM}.md"));
    assert_eq!(by_path["results"][0]["verdict"], "in_library");
    let archive_query = "ref_history/papers/report__2026-10-16.md".to_string();
    let by_archive = find_json(&fixture, &archive_query);
    assert_eq!(
        by_archive["results"][0]["verdict"], "not_found",
        "find must not treat the archive as a library member:\n{by_archive}"
    );
    let archive_hits = by_archive["results"][0]["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            by_archive["results"][0]["candidates"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .filter(|row| {
            row["ref"]["path"]
                .as_str()
                .is_some_and(|path| path.contains("ref_history/"))
                || row["path"]
                    .as_str()
                    .is_some_and(|path| path.contains("ref_history/"))
        })
        .count();
    assert_eq!(
        archive_hits, 0,
        "find must not emit archive rows:\n{by_archive}"
    );

    let doctor = run_ref(&fixture, &["doctor", "--no-hooks"]);
    assert_success(&doctor);
    let report = stdout(&doctor);
    assert!(
        !report.contains("ref_history/")
            && !report.contains("duplicate_identity"),
        "doctor must not invent archive members:\n{report}"
    );

    let archive = fs::read_to_string(
        fixture
            .vault
            .join("ref_history/papers/report__2026-10-16.md"),
    )
    .expect("archive");
    assert!(
        !archive.contains("#task #ref") && !archive.contains("^ref"),
        "archive is not a reading-task source:\n{archive}"
    );
    assert_eq!(
        parent_reading_task_count(&fixture.vault),
        1,
        "exactly one reading task remains after freeze"
    );
}

#[test]
fn refusal_leaves_vault_and_intent_unchanged() {
    let fixture =
        filed_vault("bob-cli-overwrite-accept-refuse", "ready", false, false);
    let before = snapshot_tree(&fixture.vault);
    let output = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .arg("-o")
        .arg(fixture.temp.path().join("out.pdf"))
        .output()
        .expect("overwrite with -o");
    assert_eq!(
        output.status.code(),
        Some(2),
        "-o must refuse overwrite:\n{}",
        format_output(&output)
    );
    assert_eq!(
        snapshot_tree(&fixture.vault),
        before,
        "a refused overwrite must not write"
    );
    assert!(
        !overwrite_recovery::intent_path(&state_dir(&fixture), &fixture.vault)
            .exists(),
        "a refused overwrite must not persist an intent"
    );
}

#[test]
fn unrelated_dirty_and_staged_files_survive_overwrite() {
    let fixture =
        filed_vault("bob-cli-overwrite-accept-dirty", "ready", false, false);
    write_file(&fixture.vault.join("scratch.md"), "unrelated dirty\n");
    write_file(&fixture.vault.join("staged.md"), "unrelated staged\n");
    git_in(&fixture.vault, ["add", "staged.md"]);
    let output = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .output()
        .expect("overwrite with unrelated dirty files");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(fixture.vault.join("scratch.md")).expect("scratch"),
        "unrelated dirty\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.vault.join("staged.md")).expect("staged"),
        "unrelated staged\n"
    );
    let cached = git_in(&fixture.vault, ["diff", "--cached", "--name-only"]);
    let tracked = git_in(&fixture.vault, ["ls-files", "staged.md"]);
    assert!(
        stdout(&cached).lines().any(|line| line == "staged.md")
            || stdout(&tracked).lines().any(|line| line == "staged.md"),
        "unrelated staged file stays staged or committed:\n cached={} tracked={}",
        stdout(&cached),
        stdout(&tracked)
    );
}

#[test]
fn writing_create_and_intake_scan_recover_crashed_overwrite() {
    let (_temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-overwrite-accept-create");
    let base = overwrite_recovery::head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    write_file(&vault.join("unrelated.md"), "keep me\n");
    git_in(&vault, ["add", "unrelated.md"]);
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );

    let create = bob_command()
        .args(["ref", "create", "anything"])
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("writing create recovers");
    assert!(
        stderr(&create).contains(
            "recovered: rolled back an interrupted overwrite of half-written"
        ),
        "writing create rolls back:\n{}",
        format_output(&create)
    );
    assert!(
        fs::read_to_string(vault.join("ref/blogs/half-written.md"))
            .expect("note")
            .contains("body"),
        "create recovery restores the base"
    );
    assert!(
        !overwrite_recovery::intent_path(&state, &vault).exists(),
        "create recovery clears the intent"
    );
    assert_eq!(
        fs::read_to_string(vault.join("unrelated.md")).expect("unrelated"),
        "keep me\n"
    );

    let (_temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-overwrite-accept-intake");
    let base = overwrite_recovery::head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    let intake = bob_command()
        .args(["ref", "scan", "-i"])
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("intake-only scan recovers");
    assert_eq!(
        intake.status.code(),
        Some(0),
        "intake-only recovery exits 0:\n{}",
        format_output(&intake)
    );
    assert!(
        format!("{}{}", stdout(&intake), stderr(&intake)).contains(
            "recovered: rolled back an interrupted overwrite of half-written"
        ),
        "intake-only scan rolls back:\n{}",
        format_output(&intake)
    );
    assert!(
        fs::read_to_string(vault.join("ref/blogs/half-written.md"))
            .expect("note")
            .contains("body"),
        "intake-only recovery restores the base"
    );

    let (_temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-overwrite-accept-json");
    let base = overwrite_recovery::head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    let json = bob_command()
        .args(["ref", "scan", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("json writing scan recovers");
    assert_eq!(
        json.status.code(),
        Some(0),
        "json writing scan recovers:\n{}",
        format_output(&json)
    );
    let document: serde_json::Value = serde_json::from_str(&stdout(&json))
        .unwrap_or_else(|error| {
            panic!(
                "json scan stdout stays parseable: {error}\n{}",
                format_output(&json)
            )
        });
    assert_ne!(
        document["error"]["code"], "recovery_required",
        "a writing json scan must not leave recovery_required:\n{document}"
    );
    assert!(
        stderr(&json).contains(
            "recovered: rolled back an interrupted overwrite of half-written"
        ),
        "json writing scan reports recovery on stderr:\n{}",
        format_output(&json)
    );
    assert!(
        !format!("{}{}", stdout(&json), stderr(&json))
            .to_lowercase()
            .contains("repeat"),
        "recovery must not advise repeating the overwrite:\n{}",
        format_output(&json)
    );
}

#[test]
fn leftover_committed_intent_then_second_overwrite_of_same_stem() {
    let fixture =
        filed_vault("bob-cli-overwrite-accept-second", "ready", false, false);
    let base = {
        let output = git_in(&fixture.vault, ["rev-parse", "HEAD"]);
        stdout(&output).trim().to_string()
    };
    let first = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .output()
        .expect("first overwrite");
    assert_success(&first);
    assert!(
        !stdout(&first).to_lowercase().contains("repeat"),
        "success must not advise repeating:\n{}",
        stdout(&first)
    );
    let committed = {
        let output = git_in(&fixture.vault, ["rev-parse", "HEAD"]);
        stdout(&output).trim().to_string()
    };
    plant_filed_intent(&fixture, &base, Some(&committed));
    let scan = run_ref(&fixture, &["scan", "--no-hooks"]);
    assert_success(&scan);
    assert!(
        stderr(&scan).contains(&format!(
            "recovered: kept the completed overwrite of {STEM}"
        )),
        "leftover committed intent is retained:\n{}",
        format_output(&scan)
    );
    assert!(
        !overwrite_recovery::intent_path(&state_dir(&fixture), &fixture.vault)
            .exists(),
        "committed leftover intent is cleared"
    );

    add_prose(&fixture, "\nSecond-pass freeze.\n");
    commit_vault(&fixture.vault, "prose for second overwrite");
    let second = overwrite_command(&fixture, &source_pdf(&fixture, TITLE))
        .output()
        .expect("second overwrite of same stem");
    assert_success(&second);
    assert!(
        stdout(&second)
            .contains("earlier: ref_history/papers/report__2026-10-16.md"),
        "a later overwrite of the same stem still archives:\n{}",
        stdout(&second)
    );
}
