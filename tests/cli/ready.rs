//! `bob ready`: the per-note Ready cap overview, worklist, JSON,
//! --check, --cap preview, and resolution errors.

use crate::support::*;
use serde_json::Value;

const NOW: &str = "2026-10-08";

const ALPHA: &str = "\
---\n\
type: \"[[project]]\"\n\
status: wip\n\
parent: \"[[beta]]\"\n\
---\n\
- [ ] #task Fresh one [fresh:: 2026-10-05]\n\
- [ ] #task New one\n\
- [ ] #task New two\n\
- [ ] #task New three\n\
- [ ] #task New four\n\
- [ ] #task Stale one [fresh:: 2026-09-20]\n\
- [ ] #task Parent\n\
  - [ ] #task Child one\n\
  - [ ] #task Child two\n\
- [ ] #task Lifecycle ^prj\n\
- [ ] #task Repeater [repeat:: every day]\n\
- [*] #task Next thing [id:: next-one]\n\
- [/] #task Pending thing\n\
- [ ] #task Waiting [dependsOn:: next-one]\n\
- [x] #task Done deed\n";

const BETA: &str = "\
---\n\
type: \"[[area]]\"\n\
---\n\
- [ ] #task B one\n\
- [ ] #task B two\n\
- [ ] #task B three\n\
- [ ] #task B four\n\
- [ ] #task B five\n\
- [?] #task Blocked symbol\n\
- [ ] #task Hidden #hide\n\
- [ ] #task Future [scheduled:: 2026-12-01]\n";

const GAMMA: &str = "\
---\n\
type: \"[[project]]\"\n\
status: wip\n\
ready_cap: 8\n\
---\n\
- [ ] #task G one\n\
- [ ] #task G two\n\
- [ ] #task G three\n";

fn ready_vault(prefix: &str) -> TempDir {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("alpha.md"), ALPHA);
    write_file(&vault.join("beta.md"), BETA);
    write_file(&vault.join("gamma.md"), GAMMA);
    // Room with a note-level cap, an empty note, an exempt note, a
    // terminal project, and an invalid cap (one lint, not per row).
    write_file(
        &vault.join("delta.md"),
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n## Tasks\n",
    );
    write_file(
        &vault.join("eps.md"),
        "---\ntype: \"[[area]]\"\nready_cap: off\n---\n- [ ] #task E one\n- [ ] #task E two\n",
    );
    write_file(
        &vault.join("zeta.md"),
        "---\ntype: \"[[project]]\"\nstatus: done\n---\n- [ ] #task Z one\n- [ ] #task Z two\n",
    );
    write_file(
        &vault.join("eta.md"),
        "---\ntype: \"[[project]]\"\nstatus: wip\nready_cap: lots\n---\n- [ ] #task H one\n",
    );
    // Notes that never enter the per-note list.
    write_file(
        &vault.join("2026/20261008.md"),
        "# 2026-10-08\n\n- [ ] #task Daily one\n- [ ] #task Daily two\n",
    );
    write_file(&vault.join("loose.md"), "- [ ] #task Loose one\n");
    write_file(&vault.join("_templates/tpl.md"), "- [ ] #task Templated\n");
    write_file(
        &vault.join("done/old.md"),
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Old\n",
    );
    // Nested folder and the ambiguous stem pair.
    write_file(
        &vault.join("x/theta.md"),
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task T one\n",
    );
    write_file(
        &vault.join("x/dup.md"),
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Dup x\n",
    );
    write_file(
        &vault.join("y/dup.md"),
        "---\ntype: \"[[project]]\"\nstatus: wip\n---\n- [ ] #task Dup y\n",
    );
    temp
}

fn vault_dir(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("vault")
}

fn ready_command(temp: &TempDir, args: &[&str]) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("ready")
        .env("BOB_DIR", vault_dir(temp))
        .env("BOB_NOW", NOW);
    for arg in args {
        command.arg(arg);
    }
    command
}

fn ready_json(temp: &TempDir, extra: &[&str]) -> (std::process::Output, Value) {
    let mut args = vec!["-f", "json"];
    args.extend(extra);
    let output = ready_command(temp, &args)
        .output()
        .expect("run bob ready -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("ready JSON");
    (output, value)
}

#[test]
fn overview_json_reports_totals_and_order() {
    let temp = ready_vault("bob-cli-ready-json");
    let (_, value) = ready_json(&temp, &[]);

    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["date"], NOW);
    assert_eq!(value["definition"], "ready_lane");
    assert_eq!(value["cap"]["default"], 5);
    assert_eq!(value["cap"]["source"], "default");

    let totals = &value["totals"];
    assert_eq!(totals["notes"], 8);
    assert_eq!(totals["areas"], 1);
    assert_eq!(totals["projects"], 7);
    assert_eq!(totals["crowded"], 1);
    assert_eq!(totals["full"], 1);
    assert_eq!(totals["room"], 5);
    assert_eq!(totals["empty"], 1);
    assert_eq!(totals["exempt"], 1);
    // 9 + 5 + 3 + 0 + 1 + 1 + 1 + 1.
    assert_eq!(totals["counted"], 21);
    assert_eq!(totals["excess"], 4);
    assert_eq!(totals["recurring"], 1);

    // State rank, then count descending, then name: crowded, full,
    // room by count, empty, exempt.
    let names: Vec<&str> = value["notes"]
        .as_array()
        .expect("notes array")
        .iter()
        .map(|note| note["name"].as_str().expect("note name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "alpha", "beta", "gamma", "dup", "dup", "eta", "theta", "delta",
            "eps"
        ],
    );

    let alpha = &value["notes"][0];
    assert_eq!(alpha["path"], "alpha.md");
    assert_eq!(alpha["kind"], "project");
    assert_eq!(alpha["status"], "wip");
    assert_eq!(alpha["parent"], "beta");
    assert_eq!(alpha["count"], 9);
    assert_eq!(alpha["cap"], 5);
    assert_eq!(alpha["cap_source"], "default");
    assert_eq!(alpha["state"], "crowded");
    assert_eq!(alpha["over_by"], 4);
    assert_eq!(alpha["make_up"]["ready"], 1);
    assert_eq!(alpha["make_up"]["new"], 7);
    assert_eq!(alpha["make_up"]["rotten"], 1);
    assert_eq!(alpha["recurring"], 1);

    // The exempt note carries a null cap; the terminal project is
    // not capped at all.
    let eps = &value["notes"][8];
    assert_eq!(eps["state"], "exempt");
    assert!(eps["cap"].is_null());
    assert!(value["notes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|note| note["path"] != "zeta.md"));

    // Excluded paths never enter the per-note list.
    for path in [
        "2026/20261008.md",
        "loose.md",
        "_templates/tpl.md",
        "done/old.md",
    ] {
        assert!(
            value["notes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|note| note["path"] != path),
            "excluded path {path} must be absent",
        );
    }

    // Invalid and terminal lints are emitted once per note.
    let codes: Vec<(&str, &str)> = value["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .map(|warning| {
            (
                warning["code"].as_str().expect("lint code"),
                warning["path"].as_str().expect("lint path"),
            )
        })
        .collect();
    assert_eq!(
        codes,
        vec![
            ("note_ready_cap_invalid", "eta.md"),
            ("note_ready_in_terminal_project", "zeta.md"),
        ],
    );
}

#[test]
fn overview_human_has_sections_order_and_no_ansi() {
    let temp = ready_vault("bob-cli-ready-human");
    let output = ready_command(&temp, &[]).output().expect("run bob ready");
    assert_success(&output);
    let human = stdout(&output);

    assert!(
        human.contains("bob ready") && human.contains("cap 5 per note"),
        "expected header:\n{human}"
    );
    assert!(
        human.contains("CROWDED 1")
            && human.contains("4 over")
            && human.contains("1 full"),
        "expected summary:\n{human}"
    );
    assert_text_order(
        &human,
        &["CROWDED", "alpha", "FULL", "beta  5/5", "ROOM", "Make room"],
    );
    assert!(
        human.contains("9/5") && human.contains("+4"),
        "expected the crowded meter:\n{human}"
    );
    assert!(
        human.contains("Make room") && human.contains("bob ready alpha"),
        "expected the quickest win:\n{human}"
    );
    assert!(
        human.contains("defer Ctrl+Shift+P 1\u{2013}4")
            && human.contains("drop Ctrl+Shift+P x")
            && human.contains("sequence Ctrl+Shift+P b"),
        "expected the Task Card ready hint:\n{human}"
    );
    assert!(
        !human.contains("sequence / defer / drop Ctrl+Shift+P")
            && !human.contains("type to search"),
        "the classic ready hint must not appear:\n{human}"
    );
    assert!(
        human.contains("not capped: eps 2 (ready_cap: off)"),
        "expected the exempt footer:\n{human}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn overview_all_clear_reports_room() {
    let temp = TempDir::new("bob-cli-ready-clear");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("solo.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Only\n",
    );

    let output = ready_command(&temp, &[])
        .output()
        .expect("run bob ready clear");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("CROWDED 0") && human.contains("every note has room"),
        "expected the all-clear line:\n{human}"
    );
    assert!(
        !human.contains("Make room"),
        "no quickest win when nothing is crowded:\n{human}"
    );
}

#[test]
fn overview_all_expands_room_empty_and_lints() {
    let temp = ready_vault("bob-cli-ready-all");
    let output = ready_command(&temp, &["-a"])
        .output()
        .expect("run bob ready -a");
    assert_success(&output);
    let human = stdout(&output);

    assert!(
        human.contains("gamma") && human.contains("3/8"),
        "expected expanded room rows:\n{human}"
    );
    assert!(
        human.contains("EMPTY") && human.contains("delta"),
        "expected the empty list:\n{human}"
    );
    assert!(
        human.contains("LINTS")
            && human.contains("note_ready_cap_invalid")
            && human.contains("note_ready_in_terminal_project"),
        "expected the lint block:\n{human}"
    );
    assert!(
        human.contains("zeta.md"),
        "expected the terminal project under not capped:\n{human}"
    );
}

#[test]
fn cap_preview_replaces_only_the_default() {
    let temp = ready_vault("bob-cli-ready-preview");
    let (_, value) = ready_json(&temp, &["-n", "3"]);

    assert_eq!(value["cap"]["default"], 3);
    assert_eq!(value["cap"]["source"], "preview");
    // Beta was full at 5/5; under a 3-cap it is crowded.
    let beta = value["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .find(|note| note["name"] == "beta")
        .expect("beta note");
    assert_eq!(beta["state"], "crowded");
    assert_eq!(beta["cap"], 3);
    assert_eq!(beta["cap_source"], "preview");
    // The note override still wins.
    let gamma = value["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .find(|note| note["name"] == "gamma")
        .expect("gamma note");
    assert_eq!(gamma["cap"], 8);
    assert_eq!(gamma["cap_source"], "note");
}

#[test]
fn cap_preview_rejects_out_of_range_values() {
    let temp = ready_vault("bob-cli-ready-bad-cap");
    for raw in ["0", "1000", "lots"] {
        let output = ready_command(&temp, &["-n", raw])
            .output()
            .expect("run bob ready with a bad --cap");
        assert_eq!(output.status.code(), Some(2), "bad --cap exits 2: {raw}");
        assert!(
            stderr(&output).contains("--cap"),
            "expected the --cap message:\n{}",
            stderr(&output)
        );
    }
}

#[test]
fn check_exits_3_when_crowded_and_0_when_clear() {
    let crowded = ready_vault("bob-cli-ready-check-crowded");
    let output = ready_command(&crowded, &["-c"])
        .output()
        .expect("run bob ready --check on a crowded vault");
    assert_eq!(output.status.code(), Some(3));
    assert!(stdout(&output).contains("CROWDED 1"));

    // JSON still prints under --check.
    let output = ready_command(&crowded, &["-c", "-f", "json"])
        .output()
        .expect("run bob ready --check -f json");
    assert_eq!(output.status.code(), Some(3));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("check JSON");
    assert_eq!(value["ok"], true);

    let temp = TempDir::new("bob-cli-ready-check-clear");
    let vault = vault_dir(&temp);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("solo.md"),
        "---\ntype: \"[[area]]\"\n---\n- [ ] #task Only\n",
    );
    let output = ready_command(&temp, &["-c"])
        .output()
        .expect("run clear --check");
    assert_success(&output);
}

#[test]
fn worklist_lists_file_order_with_also_here() {
    let temp = ready_vault("bob-cli-ready-worklist");
    let output = ready_command(&temp, &["alpha"])
        .output()
        .expect("run bob ready alpha");
    assert_success(&output);
    let human = stdout(&output);

    assert!(
        human.contains("bob ready")
            && human.contains("alpha")
            && human.contains("9/5")
            && human.contains("CROWDED +4"),
        "expected the worklist title:\n{human}"
    );
    assert!(
        human.contains("parent beta")
            && human.contains("1 ready + 7 new + 1 rotten"),
        "expected kind, parent, and make-up:\n{human}"
    );
    let fresh = human.find("Fresh one").expect("fresh row");
    let stale = human.find("Stale one").expect("stale row");
    assert!(fresh < stale, "rows stay in file order:\n{human}");
    assert!(
        human.contains("alpha.md:6") && human.contains("fresh 3d"),
        "expected the jumpable reference and fresh label:\n{human}"
    );
    assert!(
        human.contains("rotten 18d"),
        "expected the rotten age:\n{human}"
    );
    assert!(
        human.contains("also here: 1 next")
            && human.contains("1 pending")
            && human.contains("1 blocked"),
        "expected the also-here line:\n{human}"
    );
    assert!(
        human.contains("Make room for 4:")
            && human.contains("sequence Ctrl+Shift+P b"),
        "expected the make-room footer:\n{human}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn overview_human_always_advertises_task_card_keys() {
    let temp = ready_vault("bob-cli-ready-task-card-hint");
    for day in ["2026-10-01", "2026-10-18", "2026-10-19"] {
        let output = ready_command(&temp, &[])
            .env("BOB_NOW", day)
            .output()
            .expect("run bob ready on a Task Card day");
        assert_success(&output);
        let human = stdout(&output);
        assert!(
            human.contains("defer Ctrl+Shift+P 1\u{2013}4")
                && human.contains("drop Ctrl+Shift+P x")
                && human.contains("sequence Ctrl+Shift+P b"),
            "{day}: expected Task Card ready hints:\n{human}"
        );
        assert!(
            !human.contains("sequence / defer / drop Ctrl+Shift+P"),
            "{day}: the classic wording must not appear:\n{human}"
        );
        assert!(
            !human.contains("type to search"),
            "{day}: the card hint never mentions search:\n{human}"
        );
    }
}

#[test]
fn worklist_json_carries_tasks_and_also() {
    let temp = ready_vault("bob-cli-ready-worklist-json");
    let (_, value) = ready_json(&temp, &["alpha"]);

    assert_eq!(value["note"]["name"], "alpha");
    assert_eq!(value["note"]["state"], "crowded");
    let tasks = value["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 9);
    assert_eq!(tasks[0]["path"], "alpha.md");
    assert_eq!(tasks[0]["line"], 6);
    assert_eq!(tasks[0]["text"], "Fresh one");
    assert_eq!(tasks[0]["bucket"], Value::Null);
    assert_eq!(tasks[0]["fresh_on"], "2026-10-05");
    assert_eq!(tasks[1]["bucket"], "new");
    assert!(
        tasks.iter().all(|task| task["text"] != "Lifecycle"),
        "the ^prj row is not listed"
    );
    assert!(
        tasks.iter().all(|task| task["text"] != "Repeater"),
        "recurring rows are not listed"
    );
    assert_eq!(value["also"]["next"], 1);
    assert_eq!(value["also"]["pending"], 1);
    assert_eq!(value["also"]["blocked"], 1);
    assert_eq!(value["also"]["recurring"], 1);
}

#[test]
fn worklist_resolves_paths_stems_and_case() {
    let temp = ready_vault("bob-cli-ready-resolve");
    for input in ["alpha", "alpha.md", "ALPHA"] {
        let output = ready_command(&temp, &[input])
            .output()
            .unwrap_or_else(|error| panic!("run bob ready {input}: {error}"));
        assert_success(&output);
        assert!(
            stdout(&output).contains("CROWDED +4"),
            "expected alpha for {input}:\n{}",
            stdout(&output)
        );
    }
    // Nested vault paths resolve too.
    let output = ready_command(&temp, &["x/theta"])
        .output()
        .expect("run bob ready x/theta");
    assert_success(&output);
    assert!(stdout(&output).contains("theta"));
}

#[test]
fn worklist_rejects_ambiguous_unknown_and_untyped_notes() {
    let temp = ready_vault("bob-cli-ready-resolve-errors");

    let output = ready_command(&temp, &["dup"])
        .output()
        .expect("ambiguous dup");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("ambiguous")
            && stderr(&output).contains("x/dup.md")
            && stderr(&output).contains("y/dup.md"),
        "expected every candidate:\n{}",
        stderr(&output)
    );

    let output = ready_command(&temp, &["alpah"])
        .output()
        .expect("unknown alpah");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("unknown note")
            && stderr(&output).contains("alpha"),
        "expected a closest-name suggestion:\n{}",
        stderr(&output)
    );

    let output = ready_command(&temp, &["loose"])
        .output()
        .expect("untyped loose");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("not an area or project note"),
        "expected the untyped-note error:\n{}",
        stderr(&output)
    );

    // Errors keep the JSON envelope under -f json.
    let output = ready_command(&temp, &["alpah", "-f", "json"])
        .output()
        .expect("unknown note as JSON");
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["error"]["code"], "unknown_note");
}

#[test]
fn ready_rejects_invalid_config_with_exit_2() {
    let temp = ready_vault("bob-cli-ready-bad-config");
    let config = temp.path().join("config.yml");
    write_file(&config, "plan:\n  max_ready_per_note: 0\n");

    let output = bob_command()
        .arg("ready")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob ready with invalid config");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("max_ready_per_note"),
        "expected the config error naming the key:\n{}",
        stderr(&output)
    );

    let output = bob_command()
        .arg("ready")
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", vault_dir(&temp))
        .env("BOB_NOW", NOW)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob ready -f json with invalid config");
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["schema_version"], 1);
}

#[test]
fn ready_help_documents_usage_options_and_environment() {
    let output = bob_command()
        .arg("ready")
        .arg("--help")
        .output()
        .expect("run bob ready --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Usage: bob ready")
            && help.contains("[NOTE]")
            && help.contains("per-note cap"),
        "expected usage:\n{help}"
    );
    assert!(
        help.contains("bob ready sase_remote") && help.contains("bob ready -a"),
        "expected examples:\n{help}"
    );
    assert!(
        help.contains("BOB_DIR")
            && help.contains("BOB_NOW")
            && help.contains("BOB_CONFIG_FILE")
            && help.contains("NO_COLOR")
            && help.contains("ready_cap:"),
        "expected environment and override docs:\n{help}"
    );
    assert_stdout_has_no_ansi(&output);
}
