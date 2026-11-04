//! CLI fixtures and write-safety coverage for note-pull capture.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path, path::PathBuf, process::Output};

const NOW: &str = "2026-10-05 09:30:00";
const FIXTURES: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/note_pull");

fn fixture(case: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let root = Path::new(FIXTURES).join(case);
    let temp = TempDir::new(&format!("bob-cli-note-pull-{case}"));
    let vault = temp.path().join("vault");
    copy_tree(&Path::new(FIXTURES).join("shared"), &vault);
    copy_tree(&root.join("vault"), &vault);
    let day = vault.join("20261005.md");
    (temp, vault, day, root)
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture vault copy");
    for entry in fs::read_dir(source).expect("read fixture vault") {
        let entry = entry.expect("fixture entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().expect("fixture entry type").is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path)
                .expect("copy fixture input");
        }
    }
}

fn run_json(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
    dry_run: bool,
) -> Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .args(flags)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW);
    command.output().expect("run bob capture JSON")
}

fn run_human(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
    dry_run: bool,
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .args(flags)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    command.output().expect("run bob capture human")
}

fn normalized_fixture_text(
    root: &Path,
    vault: &Path,
    day: &Path,
    name: &str,
) -> String {
    fs::read_to_string(root.join(name))
        .expect("read note-pull CLI expectation")
        .replace("<VAULT>", &vault.display().to_string())
        .replace("<DAY_FILE>", &day.display().to_string())
}

fn assert_json_fixture(
    root: &Path,
    vault: &Path,
    day: &Path,
    output: &Output,
    name: &str,
    exit_code: i32,
) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "unexpected exit status:\n{}",
        format_output(output)
    );
    let actual: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!("capture stdout is not JSON: {error}\n{}", stdout(output))
        });
    let expected: Value =
        serde_json::from_str(&normalized_fixture_text(root, vault, day, name))
            .expect("fixture expectation is JSON");
    assert_eq!(actual, expected, "CLI output differs from {name}");
    actual
}

fn assert_human_fixture(
    root: &Path,
    vault: &Path,
    day: &Path,
    output: &Output,
    name: &str,
) {
    assert_success(output);
    let actual = stdout(output);
    let expected = normalized_fixture_text(root, vault, day, name);
    assert_eq!(actual, expected, "human output differs from {name}");
}

fn snapshot(
    vault: &Path,
    day: &Path,
    notes: &[&str],
) -> Vec<(PathBuf, Vec<u8>)> {
    notes
        .iter()
        .map(|note| {
            let path = if *note == "<day>" {
                day.to_path_buf()
            } else {
                vault.join(note)
            };
            let bytes = fs::read(&path).unwrap_or_default();
            (path, bytes)
        })
        .collect()
}

fn assert_snapshot_unchanged(before: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in before {
        assert_eq!(
            fs::read(path).unwrap_or_default(),
            *bytes,
            "{} changed",
            path.display()
        );
    }
}

#[test]
fn np1_no_pick_matches_cli_json_and_human_fixtures() {
    let (_temp, vault, day, root) = fixture("np1");
    let before = snapshot(&vault, &day, &["sase.md", "<day>"]);
    let json = run_json(&vault, &day, &[], "@sase", true);
    let report =
        assert_json_fixture(&root, &vault, &day, &json, "stdout.json", 0);
    assert_eq!(report["note_pull"]["route"], "sase");
    assert_snapshot_unchanged(&before);

    let human = run_human(&vault, &day, &[], "@sase", true);
    assert_human_fixture(&root, &vault, &day, &human, "human.txt");
}

#[test]
fn np2_pick_all_matches_cli_json_and_human_fixtures() {
    let (_temp, vault, day, root) = fixture("np2");
    let json = run_json(&vault, &day, &["-p", "all"], "@sase", false);
    let report =
        assert_json_fixture(&root, &vault, &day, &json, "stdout.json", 0);
    assert_eq!(
        report["note_pull"]["successors"].as_array().unwrap().len(),
        3
    );

    let (_human_temp, human_vault, human_day, human_root) = fixture("np2");
    let human =
        run_human(&human_vault, &human_day, &["-p", "all"], "@sase", false);
    assert_human_fixture(
        &human_root,
        &human_vault,
        &human_day,
        &human,
        "human.txt",
    );
}

#[test]
fn np10_empty_and_folded_np12_missing_source_match_write_free_fixtures() {
    for (case, draft, notes) in [
        ("np10-empty", "@sase", vec!["sase.md", "<day>"]),
        ("np10-folded", "@sase", vec!["sase.md", "<day>"]),
        ("np12", "@nosuch", vec!["<day>"]),
    ] {
        let (_temp, vault, day, root) = fixture(case);
        let before = snapshot(&vault, &day, &notes);
        let output = run_json(&vault, &day, &[], draft, false);
        let report =
            assert_json_fixture(&root, &vault, &day, &output, "stdout.json", 2);
        assert!(report.get("code").is_some(), "{report}");
        assert_snapshot_unchanged(&before);
    }
}

#[test]
fn np13_required_choice_matches_refusal_fixture_without_writes() {
    let (_temp, vault, day, root) = fixture("np13");
    let before = snapshot(&vault, &day, &["sase.md", "<day>"]);
    let output =
        run_json(&vault, &day, &["--require-pull-in-choice"], "@sase", false);
    let report =
        assert_json_fixture(&root, &vault, &day, &output, "stdout.json", 1);
    assert_eq!(report["code"], "pull_in_choice_required");
    assert_snapshot_unchanged(&before);
}

#[test]
fn np16_inbox_choice_and_routed_pick_match_cli_fixtures() {
    let (_temp, vault, day, root) = fixture("np16");
    let choice_before =
        snapshot(&vault, &day, &["mac_inbox.md", "health.md", "<day>"]);
    let choice = run_json(
        &vault,
        &day,
        &["-p", "2", "--require-inbox-route-choice"],
        "@mac_inbox",
        false,
    );
    let report =
        assert_json_fixture(&root, &vault, &day, &choice, "choice.json", 1);
    assert_eq!(report["code"], "inbox_route_choice_required");
    assert_snapshot_unchanged(&choice_before);

    let routed = run_json(
        &vault,
        &day,
        &["-p", "2", "-i", "1=health"],
        "@mac_inbox",
        false,
    );
    let report =
        assert_json_fixture(&root, &vault, &day, &routed, "routed.json", 0);
    assert_eq!(report["inbox_route"]["routed_count"], 1);
}

#[test]
fn np28_dry_run_parity_and_refusal_paths_are_write_free() {
    let (_temp, vault, day, _) = fixture("np1");
    let before = snapshot(&vault, &day, &["sase.md", "<day>"]);

    let preview = run_json(&vault, &day, &[], "@sase", true);
    assert_success(&preview);
    assert_snapshot_unchanged(&before);

    let choice =
        run_json(&vault, &day, &["--require-pull-in-choice"], "@sase", false);
    assert_eq!(choice.status.code(), Some(1));
    assert_snapshot_unchanged(&before);

    let invalid = run_json(&vault, &day, &["-p", "9"], "@sase", false);
    assert_eq!(invalid.status.code(), Some(2));
    assert_snapshot_unchanged(&before);

    let failed_batch =
        run_json(&vault, &day, &["-p", "all"], "@sase\n\n@nosuch", false);
    assert_eq!(failed_batch.status.code(), Some(2));
    assert_snapshot_unchanged(&before);

    let (_parity_temp, parity_vault, parity_day, _) = fixture("np2");
    let real = capture_json_dry_run_matches_real_flags(
        &parity_vault,
        &parity_day,
        NOW,
        &["-p", "all"],
        &["@sase"],
    );
    assert_eq!(real["dry_run"], false);
}

#[test]
fn target_runs_and_one_note_pull_keep_capture_order_and_offer_ownership() {
    for (name, draft, run_first) in [
        (
            "run-before-pull",
            "@cash+a @cash+b Shared note\n\n@sase",
            true,
        ),
        (
            "run-after-pull",
            "@sase\n\n@cash+a @cash+b Shared note",
            false,
        ),
    ] {
        let (_temp, vault, day, _) = fixture("np1");
        let target_note = vault.join("cash.md");
        let before_target = "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n";
        fs::write(&target_note, before_target).expect("write target tasks");
        let before = snapshot(&vault, &day, &["sase.md", "cash.md", "<day>"]);

        let output = run_json(&vault, &day, &["-p", "2"], draft, true);
        assert_success(&output);
        let json: Value =
            serde_json::from_slice(&output.stdout).expect("mixed capture JSON");
        let captures = json["captures"].as_array().expect("ordered captures");
        assert_eq!(captures.len(), 3, "{name}: {json}");

        let target_results = if run_first {
            &captures[0..2]
        } else {
            &captures[1..3]
        };
        assert_eq!(target_results[0]["kind"], "sub_bullet", "{name}");
        assert_eq!(target_results[1]["kind"], "sub_bullet", "{name}");
        assert_eq!(target_results[0]["fanout"]["position"], 1, "{name}");
        assert_eq!(target_results[1]["fanout"]["position"], 2, "{name}");

        let pull = captures
            .iter()
            .find(|capture| capture["kind"] == "note_pull")
            .expect("one note-pull result");
        let offers = pull["note_pull"]["successors"]
            .as_array()
            .expect("advertised note-pull tasks");
        assert_eq!(offers.len(), 3, "{name}: {json}");
        assert_eq!(json["pull_in"]["offer_count"], 3, "{name}: {json}");
        assert_eq!(json["pull_in"]["picked_count"], 1, "{name}: {json}");
        assert_eq!(
            offers
                .iter()
                .map(|row| row["offer"]["n"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3],
            "{name}: {json}"
        );
        let selected = offers
            .iter()
            .filter(|row| row["offer"]["selected"] == true)
            .collect::<Vec<_>>();
        assert_eq!(selected.len(), 1, "{name}: {json}");
        assert_eq!(selected[0]["block_id"], "next", "{name}: {json}");
        assert_eq!(selected[0]["offer"]["n"], 2, "{name}: {json}");
        let keys = offers
            .iter()
            .map(|row| row["offer"]["key"].as_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(keys.len(), offers.len(), "{name}: {json}");
        assert_snapshot_unchanged(&before);
    }
}

#[test]
fn note_pull_human_subset_two_notes_and_mixed_groups() {
    // Subset pick links only the picked row and reports partial selection
    // without the no-pick hint.
    let (_temp, vault, day, _) = fixture("np1");
    let subset = run_human(&vault, &day, &["-p", "2"], "@sase", true);
    assert_success(&subset);
    let text = stdout(&subset);
    assert!(
        text.contains("Pull from sase · 1 of 3 pulled in"),
        "subset header: {text}"
    );
    assert!(
        !text.contains("nothing pulled in"),
        "subset must not hint: {text}"
    );
    assert!(
        text.contains("not pulled in  offer 1"),
        "subset reports unpicked: {text}"
    );

    // Two notes continue numbers across the draft and the no-pick hint
    // uses the first and last numbers.
    let (_two_temp, two_vault, two_day, _) = fixture("np1");
    fs::write(two_vault.join("dev.md"), "- [ ] #task Dev row ^dev-row\n")
        .expect("write second pull source");
    let two = run_human(&two_vault, &two_day, &[], "@sase\n\n@dev", true);
    assert_success(&two);
    let two_text = stdout(&two);
    assert!(
        two_text.contains("Pull from sase · 3 ready tasks"),
        "two-note sase header: {two_text}"
    );
    assert!(
        two_text.contains("Pull from dev · 1 ready task"),
        "two-note dev header: {two_text}"
    );
    assert!(
        two_text.contains("nothing pulled in · choose with -p 1,4 or -p all"),
        "two-note hint uses first/last: {two_text}"
    );

    // A mixed pull plus hand-off draft keeps the mixed-group copy and does
    // not print the pull-only hint.
    let (_mixed_temp, mixed_vault, mixed_day, _) = fixture("np1");
    fs::write(
        mixed_vault.join("other.md"),
        concat!(
            "- [ ] #task Dependent [dependsOn:: pre] [id:: dep] ^dep\n",
            "- [ ] #task Prereq [id:: pre] ^pre\n",
        ),
    )
    .expect("write hand-off source");
    let mixed =
        run_human(&mixed_vault, &mixed_day, &[], "@sase\n\n!other:pre", true);
    assert_success(&mixed);
    let mixed_text = stdout(&mixed);
    assert!(
        !mixed_text.contains("nothing pulled in · choose with -p"),
        "mixed groups keep their own copy: {mixed_text}"
    );
}

fn move_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    crate::support::write_capture_task_settings(&vault);
    let area = |tasks: &str| {
        format!("---\ntype: \"[[area]]\"\n---\n## Tasks\n{tasks}")
    };
    fs::write(
        vault.join("work.md"),
        area(concat!(
            "- [/] #task Alpha ^alpha\n",
            "- [*] #task Beta ^beta\n",
            "- [ ] #task Gamma ^gamma\n",
        )),
    )
    .expect("work note");
    fs::write(vault.join("bob.md"), area("")).expect("transfer dest");
    let day = vault.join("20261005.md");
    fs::write(
        &day,
        "## Pomodoros\n- [ ] (**0800-0825** [t:: 25m]) — FIX\n",
    )
    .expect("day note");
    (temp, vault, day)
}

fn pull_successors(json: &Value) -> Vec<Value> {
    json["captures"]
        .as_array()
        .expect("captures")
        .iter()
        .find(|capture| capture["kind"] == "note_pull")
        .expect("note-pull capture")["note_pull"]["successors"]
        .as_array()
        .expect("successors")
        .clone()
}

#[test]
fn note_pull_move_project_and_transfer_follow_picked_row() {
    for (name, draft, dest, link) in [
        (
            "project",
            "@work\n\nDocs push @work^docs-push@\n- ^gamma",
            "work_docs_push.md",
            "[[work_docs_push#^gamma]]",
        ),
        (
            "transfer",
            "@work\n\n@work+gamma > @bob",
            "bob.md",
            "[[bob#^gamma]]",
        ),
    ] {
        let (_temp, vault, day) =
            move_vault(&format!("bob-cli-np-move-{name}"));
        // `-p all` links all three; the moved row follows to the
        // destination with a stable key and a remapped day link.
        let output = run_json(&vault, &day, &["-p", "all"], draft, true);
        assert_success(&output);
        let json: Value =
            serde_json::from_slice(&output.stdout).expect("move capture JSON");
        assert_eq!(json["pull_in"]["offer_count"], 3, "{name}: {json}");
        assert_eq!(json["pull_in"]["picked_count"], 3, "{name}: {json}");
        let successors = pull_successors(&json);
        assert_eq!(successors.len(), 3, "{name}: {json}");
        let gamma = successors
            .iter()
            .find(|row| row["block_id"] == "gamma")
            .expect("gamma survives");
        assert_eq!(gamma["note_path"], dest, "{name}: {json}");
        assert_eq!(gamma["offer"]["key"], "work.md#^gamma", "{name}: {json}");
        assert_eq!(gamma["status_symbol"], "*", "{name}: {json}");
        assert_eq!(gamma["link"]["block_link"], link, "{name}: {json}");
        // One remapped link keeps the note-pull reason in the day block.
        let reasons: Vec<_> = json["pomodoro_blocks"]
            .as_array()
            .expect("blocks")
            .iter()
            .flat_map(|block| {
                block["lines"].as_array().cloned().unwrap_or_default()
            })
            .filter(|line| {
                line["text"].as_str().unwrap_or_default().contains(link)
            })
            .collect();
        assert_eq!(reasons.len(), 1, "{name}: {json}");
        assert_eq!(reasons[0]["reason"], "note_pull", "{name}: {json}");

        // No picks keeps the moved row offered with a stable number; both
        // `-p <n>` and `-p <key>` select it.
        let probe = run_json(&vault, &day, &[], draft, true);
        assert_success(&probe);
        let probe_json: Value =
            serde_json::from_slice(&probe.stdout).expect("probe JSON");
        let probe_rows = pull_successors(&probe_json);
        let probe_gamma = probe_rows
            .iter()
            .find(|row| row["block_id"] == "gamma")
            .expect("probe gamma");
        assert_eq!(probe_gamma["note_path"], dest, "{name}");
        assert_eq!(probe_gamma["not_linked"], "not_selected", "{name}");
        let n = probe_gamma["offer"]["n"].as_u64().expect("n");
        assert_eq!(n, 3, "{name}");
        for flags in [
            vec!["-p".to_string(), n.to_string()],
            vec!["-p".to_string(), "work.md#^gamma".to_string()],
        ] {
            let refs: Vec<&str> = flags.iter().map(String::as_str).collect();
            let picked = run_json(&vault, &day, &refs, draft, true);
            assert_success(&picked);
            let picked_json: Value =
                serde_json::from_slice(&picked.stdout).expect("picked JSON");
            let picked_rows = pull_successors(&picked_json)
                .into_iter()
                .filter(|row| row["offer"]["selected"] == true)
                .collect::<Vec<_>>();
            assert_eq!(picked_rows.len(), 1, "{name}: {flags:?}");
            assert_eq!(picked_rows[0]["block_id"], "gamma");
            assert_eq!(picked_rows[0]["note_path"], dest);
        }

        // Dry-run and real-run agree on the moved identity.
        let real = run_json(&vault, &day, &["-p", "all"], draft, false);
        assert_success(&real);
        let real_json: Value =
            serde_json::from_slice(&real.stdout).expect("real JSON");
        assert_eq!(
            pull_successors(&real_json).len(),
            3,
            "{name}: dry/real parity"
        );
    }
}

#[test]
fn note_pull_move_destination_dedup_and_rollback() {
    let (_temp, vault, day) = move_vault("bob-cli-np-move-dedup");
    // Pull, move, then pull the destination: the destination pull omits
    // the row the first pull owns; one owner and three links total.
    let draft =
        "@work\n\nDocs push @work^docs-push@\n- ^gamma\n\n@work_docs_push";
    let output = run_json(&vault, &day, &["-p", "all"], draft, true);
    assert_success(&output);
    let json: Value =
        serde_json::from_slice(&output.stdout).expect("dedup JSON");
    let gamma_rows: Vec<_> = json["captures"]
        .as_array()
        .expect("captures")
        .iter()
        .filter(|capture| capture["kind"] == "note_pull")
        .flat_map(|capture| {
            capture["note_pull"]["successors"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|row| row["block_id"] == "gamma")
        .collect();
    assert_eq!(gamma_rows.len(), 1, "one offer owner: {json}");
    assert_eq!(gamma_rows[0]["note_path"], "work_docs_push.md");

    // Move first, then pull the source: the moved task is not offered.
    let moved_first = run_json(
        &vault,
        &day,
        &[],
        "Docs push @work^docs-push@\n- ^gamma\n\n@work",
        true,
    );
    assert_success(&moved_first);
    let moved_json: Value =
        serde_json::from_slice(&moved_first.stdout).expect("source JSON");
    let source_gamma: Vec<_> = moved_json["captures"]
        .as_array()
        .expect("captures")
        .iter()
        .filter(|capture| capture["kind"] == "note_pull")
        .flat_map(|capture| {
            capture["note_pull"]["successors"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|row| row["block_id"] == "gamma")
        .collect();
    assert!(
        source_gamma.is_empty(),
        "moved task not offered: {moved_json}"
    );

    // A later item failure rolls back the moved pick; bytes stay unchanged.
    let before_work =
        fs::read_to_string(vault.join("work.md")).expect("work before");
    let before_day = fs::read_to_string(&day).expect("day before");
    let failing = run_json(
        &vault,
        &day,
        &["-p", "all"],
        "@work\n\n@work+gamma > @bob\n\n@nosuch",
        false,
    );
    assert!(!failing.status.success(), "later failure must refuse");
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("work after"),
        before_work,
        "failed batch writes nothing"
    );
    assert_eq!(
        fs::read_to_string(&day).expect("day after"),
        before_day,
        "failed batch leaves the day alone"
    );
}
