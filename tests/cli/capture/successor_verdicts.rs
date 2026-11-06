//! Capture-side successor-offer verdicts: `-x/--complete-offer` and
//! `-X/--cancel-offer` end to end (`docs/task-dependencies.md` §12.14,
//! §12.22–§12.23, SV vectors).

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Output;

const NOW: &str = "2026-10-05 09:30:00";

fn vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261005.md");
    (temp, vault, day_file)
}

fn project(contents: &str) -> String {
    format!("---\ntype: [[project]]\nstatus: wip\n---\n{contents}")
}

/// A `!` close offering two blocked dependents: Alpha and Beta.
fn offer_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [?] #task Alpha [dependsOn:: bar] [id:: alpha] ^alpha\n",
            "- [?] #task Beta [dependsOn:: bar] [id:: beta] ^beta\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    (temp, vault, day_file)
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
) -> serde_json::Value {
    let output = capture_output(vault, day_file, flags, text, false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn capture_output(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
    human: bool,
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if !human {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
    }
    command
        .args(flags)
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob capture")
}

fn capture_json_failing(
    vault: &Path,
    day_file: &Path,
    flags: &[&str],
    text: &str,
) -> (Output, serde_json::Value) {
    let output = capture_output(vault, day_file, flags, text, false);
    assert!(!output.status.success(), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], false);
    (output, json)
}

fn vault_bytes(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).expect("read vault dir");
        for entry in read {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir()
                && path.file_name().is_some_and(|name| name == ".obsidian")
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(vault)
                    .expect("relative")
                    .to_string_lossy()
                    .to_string();
                entries.push((
                    relative,
                    fs::read(&path).expect("read vault file"),
                ));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn offer_rows(json: &serde_json::Value) -> Vec<&serde_json::Value> {
    json.pointer("/task_complete/unblocked")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|row| row.get("offer").is_some())
        .collect()
}

// SV1: `-x 1` resolves Alpha Done root-only. Beta recovers but is not
// pulled in. The close counts land on `pull_in.close`.
#[test]
fn done_verdict_closes_row_root_only() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-done");
    let json = capture_json(&vault, &day_file, &["-x", "1"], "!sase:bar");
    assert_eq!(json["pull_in"]["offer_count"], 2);
    assert_eq!(json["pull_in"]["choice"], "none");
    assert_eq!(json["pull_in"]["picked_count"], 0);
    assert_eq!(
        json["pull_in"]["close"],
        serde_json::json!({
            "supported": true,
            "done_count": 1,
            "cancelled_count": 0,
        }),
        "{json}",
    );
    let closes = json["offer_closes"].as_array().expect("offer_closes");
    assert_eq!(closes.len(), 1, "{json}");
    let close = &closes[0];
    assert_eq!(close["n"], 1);
    assert_eq!(close["key"], "deps.md#^alpha");
    assert_eq!(close["outcome"], "done");
    assert!(close.get("reason").is_none(), "{close}");
    assert_eq!(close["note_path"], "deps.md");
    assert_eq!(close["block_id"], "alpha");
    assert_eq!(close["text"], "Alpha");
    assert_eq!(close["previous_status"], " ");
    assert_eq!(close["previous_status_name"], "Ready");
    assert!(close.get("cancel_log").is_none(), "{close}");
    assert_eq!(close["recovered"], serde_json::json!([]));
    assert_eq!(
        close["message"], "✓ completed “Alpha” · Ready → Done",
        "{close}",
    );
    let rows = offer_rows(&json);
    assert_eq!(rows.len(), 2, "{json}");
    let alpha = rows
        .iter()
        .find(|row| row["offer"]["n"] == 1)
        .expect("alpha row");
    assert_eq!(alpha["offer"]["verdict"], "done");
    assert_eq!(alpha["offer"]["selected"], false);
    assert_eq!(alpha["not_linked"], "resolved_in_card");
    assert_eq!(alpha["status_symbol"], "x");
    assert_eq!(alpha["status_name"], "Done");
    let beta = rows
        .iter()
        .find(|row| row["offer"]["n"] == 2)
        .expect("beta row");
    assert!(beta["offer"].get("verdict").is_none(), "{beta}");
    assert_eq!(beta["not_linked"], "not_selected");
    assert_eq!(beta["status_symbol"], " ");

    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    let alpha_line = deps
        .lines()
        .find(|line| line.contains("^alpha"))
        .expect("alpha");
    assert!(alpha_line.starts_with("- [x] #task Alpha"), "{alpha_line}");
    assert!(
        alpha_line.contains("[completion:: 2026-10-05]"),
        "{alpha_line}"
    );
    assert!(!alpha_line.contains("[unblocked::"), "{alpha_line}");
    assert!(!alpha_line.contains("[fresh::"), "{alpha_line}");
    let beta_line = deps
        .lines()
        .find(|line| line.contains("^beta"))
        .expect("beta");
    assert!(beta_line.starts_with("- [ ] #task Beta"), "{beta_line}");
    assert!(!beta_line.contains("[completion::"), "{beta_line}");
    let sase = fs::read_to_string(vault.join("sase.md")).expect("sase");
    assert!(sase.contains("- [x] #task Bar"), "{sase}");
}

// SV16: the dry run plans the same verdicts as the real run, apart from
// `dry_run` and the `would` preview wording, and writes nothing.
#[test]
fn done_verdict_dry_run_matches_real_run() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-dry");
    let before = vault_bytes(&vault);
    let dry =
        capture_json(&vault, &day_file, &["--dry-run", "-x", "1"], "!sase:bar");
    assert_eq!(dry["dry_run"], true);
    assert_eq!(vault_bytes(&vault), before, "dry run writes nothing");
    let dry_close = &dry["offer_closes"][0];
    assert_eq!(dry_close["outcome"], "done");
    assert_eq!(
        dry_close["message"], "✓ would complete “Alpha” · Ready → Done",
        "{dry_close}",
    );

    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-real");
    let real = capture_json(&vault, &day_file, &["-x", "1"], "!sase:bar");
    assert_eq!(real["dry_run"], false);
    for (key, dry_value, real_value) in [
        (
            "n",
            &dry["offer_closes"][0]["n"],
            &real["offer_closes"][0]["n"],
        ),
        (
            "key",
            &dry["offer_closes"][0]["key"],
            &real["offer_closes"][0]["key"],
        ),
        (
            "outcome",
            &dry["offer_closes"][0]["outcome"],
            &real["offer_closes"][0]["outcome"],
        ),
        (
            "recovered",
            &dry["offer_closes"][0]["recovered"],
            &real["offer_closes"][0]["recovered"],
        ),
    ] {
        assert_eq!(dry_value, real_value, "{key} matches: {dry} vs {real}");
    }
    assert_eq!(
        real["offer_closes"][0]["message"],
        "✓ completed “Alpha” · Ready → Done"
    );
}

// SV2: Cancel with an authored reason prepends to the existing Cancel
// Log and reports `written`.
#[test]
fn cancel_verdict_with_reason_prepends_to_existing_log() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-cancel-log");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [?] #task Gamma [dependsOn:: bar] [id:: gamma] ^gamma\n",
            "  - ❌ **CANCEL LOG**\n",
            "    - *2026-10-01* — earlier reason\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let json =
        capture_json(&vault, &day_file, &["-X", "1=Rerouted"], "!sase:bar");
    let close = &json["offer_closes"][0];
    assert_eq!(close["outcome"], "cancelled");
    assert_eq!(close["reason"], "Rerouted");
    assert_eq!(close["cancel_log"], "written");
    assert_eq!(
        close["message"],
        "⊘ cancelled “Gamma” · Ready → Cancelled · “Rerouted”",
        "{close}",
    );
    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    let line = deps
        .lines()
        .find(|line| line.contains("^gamma"))
        .expect("gamma");
    assert!(line.starts_with("- [-] #task Gamma"), "{line}");
    assert!(line.contains("[cancelled:: 2026-10-05]"), "{line}");
    assert!(
        deps.contains("*2026-10-05* — Rerouted")
            && deps.contains("*2026-10-01* — earlier reason"),
        "{deps}",
    );
    assert!(
        deps.find("*2026-10-05*").unwrap() < deps.find("*2026-10-01*").unwrap(),
        "newest entry first: {deps}",
    );
}

// SV3: blank Cancel with no Cancel Log writes no log entry.
#[test]
fn cancel_verdict_blank_reason_without_log_writes_none() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-cancel-blank");
    let json = capture_json(&vault, &day_file, &["-X", "2"], "!sase:bar");
    let close = &json["offer_closes"][0];
    assert_eq!(close["outcome"], "cancelled");
    assert!(close.get("reason").is_none(), "{close}");
    assert_eq!(close["cancel_log"], "none");
    assert_eq!(
        close["message"], "⊘ cancelled “Beta” · Ready → Cancelled",
        "{close}",
    );
    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    let line = deps
        .lines()
        .find(|line| line.contains("^beta"))
        .expect("beta");
    assert!(line.starts_with("- [-] #task Beta"), "{line}");
    assert!(!deps.contains("CANCEL LOG"), "{deps}");
}

// SV4: blank Cancel with an existing Cancel Log writes the `🤷` fallback.
#[test]
fn cancel_verdict_blank_reason_with_log_writes_fallback() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-cancel-fallback");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [?] #task Gamma [dependsOn:: bar] [id:: gamma] ^gamma\n",
            "  - ❌ **CANCEL LOG**\n",
            "    - *2026-10-01* — earlier reason\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let json = capture_json(&vault, &day_file, &["-X", "1"], "!sase:bar");
    let close = &json["offer_closes"][0];
    assert_eq!(close["cancel_log"], "fallback");
    assert!(close.get("reason").is_none(), "{close}");
    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    assert!(deps.contains("*2026-10-05* — 🤷 no reason given"), "{deps}");
}

// SV6: an ID-less row resolves through its digest key without minting.
#[test]
fn done_verdict_on_id_less_row_keeps_no_id() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-idless");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("deps.md"),
        "- [?] #task Idless waits [dependsOn:: bar]\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let dry = capture_json(&vault, &day_file, &["--dry-run"], "!sase:bar");
    assert_eq!(dry["pull_in"]["offer_count"], 1);
    let key = dry["task_complete"]["unblocked"][0]["offer"]["key"]
        .as_str()
        .expect("key")
        .to_string();
    assert!(key.starts_with("deps.md@"), "{key}");
    let json = capture_json(&vault, &day_file, &["-x", &key], "!sase:bar");
    let close = &json["offer_closes"][0];
    assert_eq!(close["key"], key);
    assert_eq!(close["block_id"], "");
    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    assert!(
        deps.contains("- [x] #task Idless waits [dependsOn:: bar]  [completion:: 2026-10-05]"),
        "{deps}",
    );
    assert!(!deps.contains('^'), "{deps}");
}

// SV5: only the root closes; open subtasks stay byte-identical and the
// descriptor reports the scope note.
#[test]
fn done_verdict_resolves_root_only_with_scope_note() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-scope");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [?] #task Parent [dependsOn:: bar] [id:: parent] ^parent\n",
            "  - [ ] #task Child one\n",
            "  - [ ] #task Child two\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let dry = capture_json(&vault, &day_file, &["--dry-run"], "!sase:bar");
    let scope = dry["task_complete"]["unblocked"][0]["offer"]["close"]
        ["scope_note"]
        .as_str()
        .expect("scope note")
        .to_string();
    assert_eq!(scope, "· its 2 open subtasks stay open", "{scope}");
    let json = capture_json(&vault, &day_file, &["-x", "1"], "!sase:bar");
    assert_eq!(json["offer_closes"][0]["outcome"], "done");
    let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
    assert!(
        deps.contains("- [x] #task Parent [dependsOn:: bar] [id:: parent]"),
        "{deps}",
    );
    assert!(deps.contains("  - [ ] #task Child one\n"), "{deps}");
    assert!(deps.contains("  - [ ] #task Child two\n"), "{deps}");
}

// SV11: one verdict per row. `-p` with `-x`/`-X`, `-x` with `-X`,
// and a second `-X` for the same row all conflict write-free.
#[test]
fn verdict_conflicts_are_write_free() {
    for (name, flags) in [
        ("pull-in-and-done", vec!["-p", "1", "-x", "1"]),
        ("done-and-cancel", vec!["-x", "1", "-X", "1=r"]),
        ("cancel-twice", vec!["-X", "1=a", "-X", "1=b"]),
        ("pull-in-and-cancel", vec!["-p", "2", "-X", "2=r"]),
    ] {
        let (_temp, vault, day_file) =
            offer_vault(&format!("bob-cli-verdict-conflict-{name}"));
        let before = vault_bytes(&vault);
        let (output, json) =
            capture_json_failing(&vault, &day_file, &flags, "!sase:bar");
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        assert_eq!(json["code"], "offer_verdict_conflict");
        assert_eq!(vault_bytes(&vault), before, "conflict writes nothing");
        assert_eq!(json["pull_in"]["offer_count"], 2, "{json}");
        assert_eq!(json["offers"].as_array().expect("offers").len(), 2);
    }
    // A repeated `-x` for one row folds into one Done.
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-done-twice");
    let json =
        capture_json(&vault, &day_file, &["-x", "1", "-x", "1"], "!sase:bar");
    assert_eq!(json["offer_closes"].as_array().expect("closes").len(), 1);
}

// SV11: an `-i` answer on a card-resolved task conflicts write-free.
#[test]
fn inbox_route_answer_on_resolved_task_conflicts() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-inbox-conflict");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("inbox.md"),
        "# Inbox\n- [?] #task InboxAlpha [dependsOn:: bar] [id:: ialpha] ^ialpha\n",
    );
    write_file(
        &vault.join("deps.md"),
        "- [?] #task Beta [dependsOn:: bar] [id:: beta] ^beta\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let before = vault_bytes(&vault);
    let (output, json) = capture_json_failing(
        &vault,
        &day_file,
        &["-X", "2", "-i", "1=notes"],
        "!sase:bar",
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json["code"], "offer_verdict_conflict");
    assert_eq!(vault_bytes(&vault), before, "conflict writes nothing");
}

// SV12: `-p all` selects every offer without a verdict.
#[test]
fn pull_in_all_skips_verdict_rows() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-all");
    let json =
        capture_json(&vault, &day_file, &["-p", "all", "-x", "1"], "!sase:bar");
    assert_eq!(json["pull_in"]["choice"], "all");
    assert_eq!(json["pull_in"]["picked_count"], 1);
    assert_eq!(json["pull_in"]["close"]["done_count"], 1);
    let rows = offer_rows(&json);
    let alpha = rows
        .iter()
        .find(|row| row["offer"]["n"] == 1)
        .expect("alpha");
    assert_eq!(alpha["offer"]["verdict"], "done");
    assert_eq!(alpha["offer"]["selected"], false);
    let beta = rows
        .iter()
        .find(|row| row["offer"]["n"] == 2)
        .expect("beta");
    assert_eq!(beta["offer"]["selected"], true);
    assert!(beta.get("link").is_some(), "{beta}");
    assert!(beta.get("not_linked").is_none(), "{beta}");
}

// A verdict combined with explicit picks, with `-p none`, and as the
// only choice under `--require-pull-in-choice`.
#[test]
fn verdicts_combine_with_picks_and_count_as_choice() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-picks");
    let json =
        capture_json(&vault, &day_file, &["-p", "2", "-x", "1"], "!sase:bar");
    assert_eq!(json["pull_in"]["choice"], "subset");
    let rows = offer_rows(&json);
    assert_eq!(
        rows.iter()
            .filter(|row| row["offer"]["selected"] == true)
            .count(),
        1
    );

    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-none");
    let json = capture_json(
        &vault,
        &day_file,
        &["-p", "none", "-x", "1"],
        "!sase:bar",
    );
    assert_eq!(json["offer_closes"].as_array().expect("closes").len(), 1);
    let rows = offer_rows(&json);
    assert!(rows.iter().all(|row| row["offer"]["selected"] == false));

    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-required");
    let json = capture_json(
        &vault,
        &day_file,
        &["--require-pull-in-choice", "-x", "1"],
        "!sase:bar",
    );
    assert_eq!(json["pull_in"]["choice"], "none");
    assert_eq!(json["offer_closes"].as_array().expect("closes").len(), 1);
}

// SV13: unknown selectors refuse write-free, and a replay after success
// is stale: the resolved row is no longer offered.
#[test]
fn stale_verdicts_are_write_free() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-stale");
    let before = vault_bytes(&vault);
    for flags in [vec!["-x", "99"], vec!["-X", "deps.md#^nope"]] {
        let (output, json) =
            capture_json_failing(&vault, &day_file, &flags, "!sase:bar");
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        assert_eq!(json["code"], "offer_close_stale");
        assert_eq!(vault_bytes(&vault), before, "stale writes nothing");
    }

    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-replay");
    let before = vault_bytes(&vault);
    assert_ne!(before, Vec::new());
    capture_json(&vault, &day_file, &["-x", "1"], "!sase:bar");
    let (output, json) =
        capture_json_failing(&vault, &day_file, &["-x", "1"], "!sase:bar");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json["code"], "offer_close_stale");
}

// SV8: ineligible rows refuse with the engine's `why`: recurring rows
// and rows this capture creates. Refusals write nothing.
#[test]
fn ineligible_verdicts_refuse_with_why() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-recurring");
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("deps.md"),
        concat!(
            "- [?] #task Alpha [dependsOn:: bar] [id:: alpha] ^alpha\n",
            "- [?] #task Weekly [dependsOn:: bar] [id:: weekly] [repeat:: weekly] ^weekly\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
    );
    let before = vault_bytes(&vault);
    let (output, json) =
        capture_json_failing(&vault, &day_file, &["-x", "2"], "!sase:bar");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(json["code"], "offer_close_refused");
    assert_eq!(
        json["refused"],
        serde_json::json!({
            "key": "deps.md#^weekly",
            "n": 2,
            "outcome": "done",
            "why": "“Weekly” repeats — close it in Obsidian so Tasks writes the next one",
        }),
        "{json}",
    );
    assert_eq!(vault_bytes(&vault), before, "refusal writes nothing");
}

// SV9/SV10: a card close recovers its dependent without offering it
// again. The dependent is reported under `recovered`, never re-offered.
#[test]
fn verdict_close_recovers_dependent_without_reoffer() {
    for (flag, outcome) in
        [(vec!["-x", "1"], "done"), (vec!["-X", "1"], "cancelled")]
    {
        let name = format!("bob-cli-verdict-cascade-{outcome}");
        let (_temp, vault, day_file) = vault(&name);
        write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
        write_file(
            &vault.join("deps.md"),
            concat!(
                "- [?] #task Alpha [dependsOn:: bar] [id:: alpha] ^alpha\n",
                "- [?] #task Gamma [dependsOn:: alpha] [id:: gamma] ^gamma\n",
            ),
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n  - [[sase#^bar]]\n",
        );
        let json = capture_json(&vault, &day_file, &flag, "!sase:bar");
        let close = &json["offer_closes"][0];
        assert_eq!(close["outcome"], outcome, "{close}");
        let recovered = close["recovered"].as_array().expect("recovered");
        assert_eq!(recovered.len(), 1, "{close}");
        assert_eq!(recovered[0]["block_id"], "gamma");
        assert_eq!(recovered[0]["text"], "Gamma");
        assert_eq!(recovered[0]["previous_status_name"], "Blocked");
        assert_eq!(recovered[0]["status_name"], "Ready");
        assert_eq!(recovered[0]["unblocked_stamped"], false);
        let rows = offer_rows(&json);
        assert_eq!(rows.len(), 1, "Gamma is never re-offered: {json}");
        assert_eq!(rows[0]["text"], "Alpha");
        let deps = fs::read_to_string(vault.join("deps.md")).expect("deps");
        let gamma = deps
            .lines()
            .find(|line| line.contains("^gamma"))
            .expect("gamma");
        assert!(gamma.starts_with("- [ ] #task Gamma"), "{gamma}");
        assert!(!gamma.contains("[fresh::"), "{gamma}");
    }
}

// SV8: rows this capture creates refuse with `new_task` write-free.
#[test]
fn new_task_verdicts_refuse_write_free() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-new-task");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n# Cash\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n- [ ] () — FIX\n",
    );
    let draft = "Finish packet @cash:pkt@\n- Draft memo\n- Collect docs\n";
    let before = vault_bytes(&vault);
    let (output, json) =
        capture_json_failing(&vault, &day_file, &["-x", "1"], draft);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(json["code"], "offer_close_refused");
    assert_eq!(
        json["refused"]["why"],
        "“Draft memo” is new in this capture — drop it from the draft instead",
    );
    assert_eq!(json["refused"]["outcome"], "done");
    assert_eq!(vault_bytes(&vault), before, "refusal writes nothing");
}

// SV7: Done on a `project_finished` `^prj` row finishes the project in
// the same gesture as its last task.
#[test]
fn done_verdict_on_project_finished_finishes_project() {
    let (_temp, vault, day_file) = vault("bob-cli-verdict-prj");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task A [id:: a] ^a\n- [ ] #task Ship #prj #hide ^prj\n",
        ),
    );
    write_file(&day_file, "## Pomodoros\n- [ ] () — FIX\n  - [[proj#^a]]\n");
    let json = capture_json(&vault, &day_file, &["-x", "1"], "!proj:a");
    assert_eq!(json["pull_in"]["offer_count"], 1);
    let close = &json["offer_closes"][0];
    assert_eq!(close["key"], "proj.md#^prj");
    assert_eq!(close["outcome"], "done");
    assert_eq!(
        close["message"], "✓ completed “Ship” · Ready → Done",
        "{close}",
    );
    let note = fs::read_to_string(vault.join("proj.md")).expect("proj");
    assert!(note.contains("- [x] #task A [id:: a]"), "{note}");
    let prj = note
        .lines()
        .find(|line| line.contains("^prj"))
        .expect("prj");
    assert!(prj.starts_with("- [x] #task Ship #prj"), "{prj}");
    assert!(prj.contains("[completion:: 2026-10-05]"), "{prj}");
}

// Human output prints the receipt in place of the linked line, and the
// dry-run hint teaches the verdict flags.
#[test]
fn verdict_human_output_prints_receipt_and_hint() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-human");
    let output = capture_output(
        &vault,
        &day_file,
        &["-X", "2=Rerouted"],
        "!sase:bar",
        true,
    );
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains(
            "offer 2  ⊘ cancelled “Beta” · Ready → Cancelled · “Rerouted”"
        ),
        "{text}",
    );

    // Plain dry runs keep the old hint.
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-hint");
    let output =
        capture_output(&vault, &day_file, &["--dry-run"], "!sase:bar", true);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("-p 1,2 · -p all"), "{text}");
    assert!(!text.contains("-x"), "{text}");
}

// The dry-run verdict receipt uses the `would` forms and the hint names
// the verdict flags.
#[test]
fn verdict_dry_run_human_output_uses_would_forms() {
    let (_temp, vault, day_file) = offer_vault("bob-cli-verdict-dry-human");
    let output = capture_output(
        &vault,
        &day_file,
        &["--dry-run", "-x", "1"],
        "!sase:bar",
        true,
    );
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("offer 1  ✓ would complete “Alpha” · Ready → Done"),
        "{text}",
    );
    assert!(text.contains("-x 1 done · -X N[=why] cancel"), "{text}");
}
