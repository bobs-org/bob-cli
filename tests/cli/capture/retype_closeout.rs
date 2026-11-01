//! Closeout regressions for the child-capture retype repairs.
//!
//! Pins the three remaining acceptance gaps plus the writer's transactional
//! and exact-byte contract: ID-less `--task-ref` retype, bare-`#` existing
//! subject, and the near-duplicate quoted override.

use crate::support::*;
use serde_json::Value;
use sha2::Digest;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const NOW: &str = "2026-11-01 09:15:00";

fn fixture(
    prefix: &str,
    files: &[(&str, &str)],
    day: &str,
) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261101.md");
    let config = temp.path().join("config.yml");
    write_toggle_task_settings(&vault);
    for (name, contents) in files {
        write_file(&vault.join(name), contents);
    }
    write_file(
        &config,
        concat!(
            "plan:\n  max_themes: 30\n",
            "themes:\n  BUILD: yellow\n  FIX: blue\n  WRITE: magenta\n  PLAN: blue\n",
        ),
    );
    write_file(&day_file, day);
    (temp, vault, day_file, config)
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    args: &[&str],
    dry_run: bool,
) -> (std::process::Output, Value) {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("-d");
    }
    command
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run capture");
    assert_success(&output);
    let json =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!("parse capture JSON: {error}\n{}", format_output(&output))
        });
    (output, json)
}

fn capture_error_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    args: &[&str],
) -> (std::process::Output, Value) {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", NOW);
    let output = command.output().expect("run refused capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    let json =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!(
                "parse capture error JSON: {error}\n{}",
                format_output(&output)
            )
        });
    (output, json)
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeMap::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).expect("read snapshot directory") {
            let path = entry.expect("snapshot entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(
                    path.clone(),
                    fs::read(path).expect("snapshot file"),
                );
            }
        }
    }
    files
}

fn task_ref_for(vault: &Path, name: &str, line: usize) -> String {
    let contents =
        fs::read_to_string(vault.join(name)).expect("read note for ref");
    let physical = contents
        .lines()
        .nth(line - 1)
        .expect("ref line")
        .to_string();
    capture_pomodoro_ref(&physical, line)
}

fn complete_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    draft: &str,
) -> Value {
    let cursor = draft.len();
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!("parse complete JSON: {error}\n{}", format_output(&output))
    })
}

#[test]
fn idless_task_ref_retype_plain_and_drawer() {
    for (name, drawer_args, expect_drawer) in [
        ("plain", Vec::new(), false),
        ("drawer", vec!["--drawer", "REQUIREMENTS"], true),
    ] {
        let (_temp, vault, day_file, config) = fixture(
            &format!("bob-retype-idless-{name}"),
            &[("plain.md", "- [ ] #task Parent without ID\n")],
            "## Pomodoros\n- [ ] () — FIX\n",
        );
        let task_ref = task_ref_for(&vault, "plain.md", 1);
        let mut args: Vec<&str> =
            vec!["--route", "plain", "--task-ref", &task_ref];
        for arg in &drawer_args {
            args.push(arg);
        }
        args.push("--");
        args.push("note #fix");
        // Dry run stages the retype but writes nothing.
        let before =
            fs::read_to_string(vault.join("plain.md")).expect("read before");
        let (_, dry) = capture_json(&vault, &day_file, &config, &args, true);
        assert_eq!(dry["task_theme"]["theme"]["name"], "FIX", "{name}: {dry}");
        assert_eq!(dry["task_theme"]["action"], "set", "{name}: {dry}");
        assert_eq!(
            fs::read_to_string(vault.join("plain.md")).expect("read dry"),
            before,
            "{name}: dry run must not write"
        );
        // Real write retypes without minting an ID, link, or lane change.
        let (_, json) = capture_json(&vault, &day_file, &config, &args, false);
        assert_eq!(
            json["task_theme"]["theme"]["name"], "FIX",
            "{name}: {json}"
        );
        assert_eq!(
            json["parent_status_symbol"], " ",
            "{name}: lane must not change: {json}"
        );
        assert_eq!(
            json["parent_status_name"], "Ready",
            "{name}: lane must not change: {json}"
        );
        let after =
            fs::read_to_string(vault.join("plain.md")).expect("read after");
        assert!(
            after.contains("- [ ] #task Parent without ID [type:: FIX]"),
            "{name}: parent retyped: {after}"
        );
        assert!(
            !after.contains('^'),
            "{name}: ID-less parent must not mint an ID: {after}"
        );
        assert!(
            after.contains("\t- note"),
            "{name}: exact child bytes: {after:?}"
        );
        if expect_drawer {
            assert_eq!(
                json["drawer"]["title"], "REQUIREMENTS",
                "{name}: {json}"
            );
            assert_eq!(json["drawer"]["created"], true, "{name}: {json}");
            assert!(
                after.contains("\t- REQUIREMENTS\n\t\t- note"),
                "{name}: drawer bytes without a blank line: {after:?}"
            );
            assert!(
                !after.contains("REQUIREMENTS\n\n"),
                "{name}: no blank line after header: {after:?}"
            );
        } else {
            assert!(json["drawer"].is_null(), "{name}: {json}");
        }
    }
}

#[test]
fn idless_ref_stale_ambiguous_and_later_item_rollback() {
    // Stale and ambiguous refs refuse without weakening validation.
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-idless-stale",
        &[("plain.md", "- [ ] #task Parent without ID\n")],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let (_, stale) = capture_error_json(
        &vault,
        &day_file,
        &config,
        &[
            "--route",
            "plain",
            "--task-ref",
            "1:deadbeef",
            "--",
            "note #fix",
        ],
    );
    assert!(
        stale["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no longer in plain.md"),
        "{stale}"
    );
    let repeated = "- [ ] #task Same\n- [ ] #task Same\n";
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-idless-ambiguous",
        &[("plain.md", repeated)],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let digest = {
        let line = "- [ ] #task Same";
        let hex = hex::encode(sha2::Sha256::digest(line.as_bytes()));
        format!("99:{}", &hex[..8])
    };
    let (_, ambiguous) = capture_error_json(
        &vault,
        &day_file,
        &config,
        &["--route", "plain", "--task-ref", &digest, "--", "note #fix"],
    );
    assert!(
        ambiguous["error"]
            .as_str()
            .unwrap_or_default()
            .contains("matches more than one line"),
        "{ambiguous}"
    );
    // A later-item failure rolls back the ID-less retype atomically.
    // Two forced items share the same picker ref: the first retypes the
    // parent, so the second resolves against the edited line and refuses as
    // stale without weakening validation. The whole batch leaves no writes.
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-idless-rollback",
        &[("plain.md", "- [ ] #task Parent without ID\n")],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let before = snapshot_files(_temp.path());
    let task_ref = task_ref_for(&vault, "plain.md", 1);
    let batch = "note #fix\n\nother #snack";
    let (_, failed) = capture_error_json(
        &vault,
        &day_file,
        &config,
        &["--route", "plain", "--task-ref", &task_ref, "--", batch],
    );
    let _ = failed;
    assert_eq!(snapshot_files(_temp.path()), before);
}

#[test]
fn existing_id_ref_retype_keeps_identity_and_relocates() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-existing-ref",
        &[(
            "foo.md",
            "- [ ] #task Upgrade Postgres [type:: WRITE] ^bar\n",
        )],
        "## Pomodoros\n- [ ] () — BUILD\n  - [[foo#^bar]]\n- [ ] () — FIX\n",
    );
    let task_ref = task_ref_for(&vault, "foo.md", 1);
    let (_, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["--route", "foo", "--task-ref", &task_ref, "--", "note #fix"],
        false,
    );
    assert_eq!(json["task_theme"]["theme"]["name"], "FIX", "{json}");
    assert_eq!(json["task_theme"]["action"], "set", "{json}");
    let after = fs::read_to_string(vault.join("foo.md")).expect("read note");
    assert!(
        after.contains("- [ ] #task Upgrade Postgres [type:: FIX] ^bar"),
        "existing ID preserved with new type: {after}"
    );
    assert!(
        after.contains("\t- note") || after.contains("  - note"),
        "child inserted: {after:?}"
    );
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("[[foo#^bar]]"),
        "live link survives the retype: {day}"
    );
    // Inherited override clearing on the ID path.
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-clearing",
        &[(
            "sase.md",
            "---\ntype: [[project]]\ntheme: WRITE\n---\n# Sase\n\n## Tasks\n- [ ] #task Write docs [type:: WRITE] ^write-y\n",
        )],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let task_ref = task_ref_for(&vault, "sase.md", 8);
    let (_, cleared) = capture_json(
        &vault,
        &day_file,
        &config,
        &[
            "--route",
            "sase",
            "--task-ref",
            &task_ref,
            "--",
            "note #write",
        ],
        true,
    );
    assert_eq!(cleared["task_theme"]["action"], "cleared", "{cleared}");
}

#[test]
fn bare_marker_reports_existing_with_row_effects() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-bare-subject",
        &[(
            "foo.md",
            "- [ ] #task Upgrade Postgres [type:: WRITE] ^bar\n",
        )],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    for draft in [
        "note @foo+bar #",
        "note @foo+bar #fix",
        "note @foo+bar#requirements #",
        "note @foo+bar#requirements #fix",
    ] {
        let json = complete_json(&vault, &day_file, &config, draft);
        assert_eq!(json["context"], "task_type", "{draft}: {json}");
        assert_eq!(json["subject"], "existing", "{draft}: {json}");
        let suggestion = &json["theme_suggestion"];
        assert_eq!(
            suggestion["theme"]["key"], "write",
            "{draft}: current type stays WRITE: {json}"
        );
        if draft.ends_with(" #") {
            assert_eq!(
                json["initial_selection"], "none",
                "{draft}: empty query keeps none: {json}"
            );
            // Row effects must be visible, not header-only.
            let candidates = json["candidates"].as_array().expect("candidates");
            let write = candidates
                .iter()
                .find(|row| row["key"] == "write")
                .expect("write row");
            assert_eq!(write["current"], true, "{draft}: {json}");
            assert_eq!(write["effect"], "keep", "{draft}: {json}");
        }
    }
    // New-task negative control: a separate `#type` on a `^` task stays new.
    let json =
        complete_json(&vault, &day_file, &config, "Fix parser @sase^parser #");
    assert_eq!(json["subject"], "new", "{json}");
}

#[test]
fn near_duplicate_teaches_requested_override_and_creates_it() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-near-duplicate",
        &[(
            "foo.md",
            "- [ ] #task Upgrade Postgres ^bar\n  - REQUIREMENTS\n    - existing\n",
        )],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let before = snapshot_files(_temp.path());
    let (_, refused) = capture_error_json(
        &vault,
        &day_file,
        &config,
        &["note @foo+bar#req #fix"],
    );
    assert_eq!(refused["code"], "drawer_near_duplicate", "{refused}");
    let message = refused["error"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains("did you mean `#requirements`?"),
        "suggests the existing drawer: {refused}"
    );
    assert!(
        message.contains("write `#\"REQ\"` to create REQ"),
        "override teaches the requested title: {refused}"
    );
    assert_eq!(
        snapshot_files(_temp.path()),
        before,
        "refused request leaves the vault unchanged"
    );
    // The menu refusal comes from the same resolver.
    let menu = complete_json(&vault, &day_file, &config, "note @foo+bar#req");
    let refused_row = menu["candidates"]
        .as_array()
        .expect("drawer candidates")
        .iter()
        .find(|row| row["group"] == "new")
        .expect("new row")
        .clone();
    assert_eq!(
        refused_row["refused"],
        "did you mean `#requirements`? write `#\"REQ\"` to create REQ",
        "{menu}"
    );
    // Submitting the quoted override creates the requested drawer.
    let (_, created) = capture_json(
        &vault,
        &day_file,
        &config,
        &["note @foo+bar#\"REQ\" #fix"],
        false,
    );
    assert_eq!(created["drawer"]["title"], "REQ", "{created}");
    assert_eq!(created["drawer"]["created"], true, "{created}");
    let after = fs::read_to_string(vault.join("foo.md")).expect("read note");
    assert!(after.contains("  - REQ\n"), "distinct drawer: {after:?}");
    assert!(
        after.contains("  - REQUIREMENTS\n"),
        "existing drawer kept: {after:?}"
    );
}

#[test]
fn writer_acceptance_exact_bytes_and_transaction() {
    // New drawer plus separate type and plain child plus separate type.
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-writer-types",
        &[(
            "foo.md",
            "- [ ] #task Upgrade Postgres ^bar\n- [ ] #task Plain ^plain\n",
        )],
        "## Pomodoros\n- [ ] () — BUILD\n  - [[foo#^bar]]\n- [ ] () — FIX\n",
    );
    let (_, drawer) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Postgres 17 minimum @foo+bar#requirements #fix"],
        false,
    );
    assert_eq!(drawer["drawer"]["title"], "REQUIREMENTS", "{drawer}");
    assert_eq!(drawer["drawer"]["created"], true, "{drawer}");
    assert_eq!(drawer["task_theme"]["theme"]["name"], "FIX", "{drawer}");
    let (_, plain) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Some note. @foo+plain #fix"],
        false,
    );
    assert!(plain["drawer"].is_null(), "{plain}");
    assert_eq!(plain["task_theme"]["theme"]["name"], "FIX", "{plain}");
    // Closed, unconfigured, and later-item failures preserve the snapshot.
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-writer-rollback",
        &[("foo.md", "- [x] #task Done ^done\n- [ ] #task Open ^open\n")],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let before = snapshot_files(_temp.path());
    for args in [
        vec!["note @foo+done #fix"],
        vec!["note @foo+open #snack"],
        vec!["note @foo+open #fix", "@missing+nope"],
    ] {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--no-clip")
            .arg("-f")
            .arg("json")
            .args(&args)
            .env("BOB_DIR", &vault)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_NOW", NOW);
        let output = command.output().expect("run failing capture");
        assert!(!output.status.success(), "{args:?}");
        let json: Value =
            serde_json::from_str(stdout(&output).trim()).expect("error JSON");
        assert!(
            ["closed", "type_not_configured"]
                .iter()
                .any(|code| json.to_string().contains(code))
                || !json["ok"].as_bool().unwrap_or(true),
            "{args:?}: {json}"
        );
    }
    assert_eq!(snapshot_files(_temp.path()), before);
}

#[test]
fn writer_exact_bytes_batch_header_and_spans() {
    for (name, original) in [
        ("tab", "- [ ] #task Parent ^parent\n\t- existing\n"),
        ("spaces", "- [ ] #task Parent ^parent\n  - existing\n"),
    ] {
        let (_temp, vault, day_file, config) = fixture(
            &format!("bob-retype-bytes-{name}"),
            &[("cash.md", original)],
            "## Pomodoros\n- [ ] () — FIX\n",
        );
        let (_, json) = capture_json(
            &vault,
            &day_file,
            &config,
            &["fresh @cash+parent#requirements #fix"],
            false,
        );
        assert_eq!(json["drawer"]["created"], true, "{name}: {json}");
        let after =
            fs::read_to_string(vault.join("cash.md")).expect("read note");
        assert!(
            after.contains("- REQUIREMENTS\n"),
            "{name}: exact drawer bytes: {after:?}"
        );
        assert!(
            !after.contains("REQUIREMENTS\n\n"),
            "{name}: no blank line after header: {after:?}"
        );
    }
    let (_temp, vault, day_file, config) = fixture(
        "bob-retype-batch-header",
        &[("cash.md", "- [ ] #task Parent ^parent\n")],
        "## Pomodoros\n- [ ] () — FIX\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("first @cash+parent#requirements\n\nsecond @cash+parent#requirements")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run batch drawer capture");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2, "{json}");
    assert_eq!(captures[0]["drawer"]["created"], true, "{json}");
    assert_eq!(captures[1]["drawer"]["created"], false, "{json}");
    let after = fs::read_to_string(vault.join("cash.md")).expect("read note");
    assert_eq!(
        after.matches("- REQUIREMENTS").count(),
        1,
        "one header for two batch rows: {after:?}"
    );
    assert!(
        !after.contains("REQUIREMENTS\n\n"),
        "no blank line between header and first entry: {after:?}"
    );
    let parse = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @cash+parent#requirements")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture-parse");
    assert_success(&parse);
    let parsed: Value =
        serde_json::from_str(stdout(&parse).trim()).expect("parse JSON");
    let text = parsed.to_string();
    assert!(
        text.contains("REQUIREMENTS") && text.contains("requirements"),
        "drawer span title/key survive: {parsed}"
    );
}
