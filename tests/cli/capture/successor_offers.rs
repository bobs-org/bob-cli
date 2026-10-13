//! Capture-side successor-offer vectors (SO1–SO6, SO13–SO19, SO26,
//! SO27, SO29; `docs/task-dependencies.md` §12.12).

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

fn vault_bytes(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).expect("read vault dir");
        for entry in read {
            let entry = entry.expect("dir entry");
            let path = entry.path();
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

fn so1_vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("sase.md"), "- [ ] #task Bar [id:: bar] ^bar\n");
    write_file(
        &vault.join("sase_foo.md"),
        &project(concat!(
            "- [?] #task Foo [dependsOn:: bar] [id:: foo] ^prj\n",
            "- [?] #task First #ref\n",
            "- [ ] #task Second [unblocked:: 2026-10-01] [id:: second] ^second\n",
        )),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n- [ ] () \u{2014} FIX\n  - [[sase#^bar]]\n",
    );
    (temp, vault, day_file)
}

fn fan_note(count: usize) -> String {
    let mut note =
        "- [ ] #task Fix apollo [id:: fix-apollo] ^fix-apollo\n".to_string();
    for n in 1..=count {
        note.push_str(&format!(
            "- [?] #task Fan {n} [dependsOn:: fix-apollo] [id:: fan-{n}] ^fan-{n}\n"
        ));
    }
    note
}

fn fan_vault(
    name: &str,
    count: usize,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("sase.md"), &fan_note(count));
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n- [ ] () \u{2014} FIX\n  - [[sase#^fix-apollo]]\n",
    );
    (temp, vault, day_file)
}

fn offered_rows(json: &serde_json::Value) -> Vec<&serde_json::Value> {
    json.pointer("/task_complete/unblocked")
        .or_else(|| json.pointer("/pomodoro_close/unblocked"))
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|row| row.get("offer").is_some())
        .collect()
}

// SO1: close `sase.md` `^bar`; `sase_foo.md` `^prj` depended only on
// `^bar` and has two open residents. Both are `project` offers, `^prj`
// is not a numbered offer. `-p 1` mints the missing id.
#[test]
fn so1_project_offers_pick_one() {
    let (_temp, vault, day_file) = so1_vault("bob-cli-so1-dry");
    let dry = capture_json(&vault, &day_file, &["--dry-run"], "!sase:bar");
    assert_eq!(dry["pull_in"]["supported"], true);
    assert_eq!(dry["pull_in"]["offer_count"], 2);
    assert!(dry["pull_in"]["choice"].is_null(), "{dry}");
    assert_eq!(dry["pull_in"]["picked_count"], 0);
    let groups = dry["pull_in"]["groups"].as_array().expect("groups");
    assert!(
        groups.iter().any(|group| group["group"] == "project"
            && group["project"] == "sase_foo.md"),
        "{groups:?}"
    );
    let unblocked = dry["task_complete"]["unblocked"]
        .as_array()
        .expect("unblocked");
    let prj = unblocked
        .iter()
        .find(|row| row["block_id"] == "prj")
        .expect("^prj");
    assert_eq!(prj["not_linked"], "project_task");
    assert!(prj.get("offer").is_none(), "{prj}");
    let offers: Vec<_> = unblocked
        .iter()
        .filter(|row| row.get("offer").is_some())
        .collect();
    assert_eq!(offers.len(), 2, "{unblocked:?}");
    for row in &offers {
        assert_eq!(row["offer"]["group"], "project");
        assert_eq!(row["offer"]["selected"], false);
        assert_eq!(row["offer"]["project"], "sase_foo.md");
    }

    let (_temp, vault, day_file) = so1_vault("bob-cli-so1-pick");
    let json = capture_json(&vault, &day_file, &["-p", "1"], "!sase:bar");
    assert_eq!(json["pull_in"]["choice"], "subset");
    assert_eq!(json["pull_in"]["picked_count"], 1);
    let unblocked = json["task_complete"]["unblocked"]
        .as_array()
        .expect("unblocked");
    let picked = unblocked
        .iter()
        .find(|row| row["offer"]["n"] == 1 && row["offer"]["selected"] == true)
        .expect("picked n=1");
    assert_eq!(picked["status_symbol"], "*");
    assert_eq!(picked["link"]["entry_name"], "FIX");
    assert_eq!(picked["link"]["block_id_created"], true);
    assert!(picked.get("not_linked").is_none(), "{picked}");
    let other = unblocked
        .iter()
        .find(|row| {
            row.get("offer").is_some() && row["offer"]["selected"] == false
        })
        .expect("unpicked resident");
    assert_eq!(other["not_linked"], "not_selected");
    assert!(other.get("link").is_none() || other["link"].is_null());
    let foo = fs::read_to_string(vault.join("sase_foo.md")).expect("foo");
    let first = foo
        .lines()
        .find(|line| line.contains("First"))
        .expect("first line");
    assert!(first.contains("- [*] #task First"), "{first}");
    assert!(first.contains("[fresh:: 2026-10-05]"), "{first}");
    assert!(first.contains(" ^"), "{first}");
    assert!(!first.contains("[unblocked::"), "{first}");
    let second = foo
        .lines()
        .find(|line| line.contains("^second"))
        .expect("second line");
    assert!(second.contains("- [ ] #task Second"), "{second}");
    assert!(!second.contains("[fresh:: 2026-10-05]"), "{second}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(day.contains("[[sase_foo#^"), "{day}");
}

// SO2 / SO32: 11-way and 20-way fan-out. That many hand_off offers,
// selected false, no breaker, no cap.
#[test]
fn so2_so32_fan_out_offers_every_dependent() {
    for (name, count) in [("bob-cli-so2", 11usize), ("bob-cli-so32", 20)] {
        let (_temp, vault, day_file) = fan_vault(name, count);
        let json = capture_json(&vault, &day_file, &[], "!sase:fix-apollo");
        assert_eq!(json["pull_in"]["offer_count"], count);
        assert!(json["pull_in"]["choice"].is_null());
        let offers = offered_rows(&json);
        assert_eq!(offers.len(), count, "{name}");
        for (index, row) in offers.iter().enumerate() {
            assert_eq!(row["offer"]["group"], "hand_off");
            assert_eq!(row["offer"]["selected"], false);
            assert_eq!(row["offer"]["n"], index + 1);
            assert_eq!(row["not_linked"], "not_selected");
            assert!(row.get("link").is_none() || row["link"].is_null());
        }
        assert!(json["pull_in"].get("breaker").is_none(), "{json}");
    }
}

// SO3 / SO4 / SO5: `-p all` links every offer; `-p none` recovers
// without links; a subset by numbers and by keys links only those rows.
#[test]
fn so3_so4_so5_all_none_and_subset() {
    let (_temp, vault, day_file) = fan_vault("bob-cli-so3", 3);
    let all =
        capture_json(&vault, &day_file, &["-p", "all"], "!sase:fix-apollo");
    assert_eq!(all["pull_in"]["choice"], "all");
    assert_eq!(all["pull_in"]["picked_count"], 3);
    for row in offered_rows(&all) {
        assert_eq!(row["offer"]["selected"], true);
        assert_eq!(row["status_symbol"], "*");
        assert!(row.get("not_linked").is_none(), "{row}");
        assert!(row["link"].is_object(), "{row}");
    }
    let note = fs::read_to_string(vault.join("sase.md")).expect("note");
    assert!(note.contains("[fresh:: 2026-10-05]"), "{note}");

    let (_temp, vault, day_file) = fan_vault("bob-cli-so4", 2);
    let none =
        capture_json(&vault, &day_file, &["-p", "none"], "!sase:fix-apollo");
    assert_eq!(none["pull_in"]["choice"], "none");
    assert_eq!(none["pull_in"]["picked_count"], 0);
    for row in offered_rows(&none) {
        assert_eq!(row["not_linked"], "not_selected");
        assert!(row.get("link").is_none() || row["link"].is_null());
    }
    let note = fs::read_to_string(vault.join("sase.md")).expect("note");
    assert!(!note.contains("[fresh:: 2026-10-05]"), "{note}");
    assert!(!note.contains("- [*] #task Fan"), "{note}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(!day.contains("[[sase#^fan-"), "{day}");

    let (_temp, vault, day_file) = fan_vault("bob-cli-so5-n", 3);
    let by_n =
        capture_json(&vault, &day_file, &["-p", "2"], "!sase:fix-apollo");
    assert_eq!(by_n["pull_in"]["choice"], "subset");
    assert_eq!(by_n["pull_in"]["picked_count"], 1);
    let picked: Vec<_> = offered_rows(&by_n)
        .into_iter()
        .filter(|row| row["offer"]["selected"] == true)
        .collect();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0]["offer"]["n"], 2);
    assert_eq!(picked[0]["block_id"], "fan-2");

    let (_temp, vault, day_file) = fan_vault("bob-cli-so5-key", 3);
    let by_key = capture_json(
        &vault,
        &day_file,
        &["-p", "sase.md#^fan-2"],
        "!sase:fix-apollo",
    );
    assert_eq!(by_key["pull_in"]["choice"], "subset");
    assert_eq!(by_key["pull_in"]["picked_count"], 1);
    let picked: Vec<_> = offered_rows(&by_key)
        .into_iter()
        .filter(|row| row["offer"]["selected"] == true)
        .collect();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0]["block_id"], "fan-2");
}

// SO6: a stale key, an unknown key, and an out-of-range number each
// exit 2, leave the vault byte-identical, and list the current offers.
#[test]
fn so6_bad_picks_are_usage_and_write_free() {
    let cases: &[(&str, &str)] = &[
        ("bob-cli-so6-stale", "sase.md#^prj"),
        ("bob-cli-so6-unknown", "sase.md#^missing"),
        ("bob-cli-so6-oor", "99"),
    ];
    for (name, pick) in cases {
        let (_temp, vault, day_file) = fan_vault(name, 2);
        let before = vault_bytes(&vault);
        let output = capture_output(
            &vault,
            &day_file,
            &["-p", pick],
            "!sase:fix-apollo",
            false,
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}:\n{}",
            name,
            format_output(&output)
        );
        let body = stdout(&output);
        assert!(
            body.contains("\"ok\":false") || body.contains("\"ok\": false"),
            "{body}"
        );
        assert!(!body.contains("pull_in_choice_required"), "{body}");
        let err = format!("{}{}", body, stderr(&output));
        assert!(err.contains(pick), "{err}");
        assert!(err.contains("sase.md#^fan-1"), "{err}");
        assert_eq!(vault_bytes(&vault), before, "{name} wrote");
    }
}

// SO13: a later item that completes, links, re-blocks, or adds open
// work drops the earlier offer. `=x~K` is covered with a drop of a
// picked successor link.
#[test]
fn so13_later_item_drops_stale_offers() {
    {
        let (_temp, vault, day_file) = vault("bob-cli-so13-complete");
        write_file(
            &vault.join("sase.md"),
            "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n- [?] #task E [dependsOn:: d] [id:: e] ^e\n",
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n- [ ] () \u{2014} FIX\n",
        );
        let json = capture_json(&vault, &day_file, &[], "!sase:p\n\n!sase:d");
        let first = json["captures"][0]["task_complete"]["unblocked"]
            .as_array()
            .expect("first");
        assert!(
            first.is_empty(),
            "completed D drops the first offer: {first:?}"
        );
        let second = json["captures"][1]["task_complete"]["unblocked"]
            .as_array()
            .expect("second");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0]["block_id"], "e");
    }

    {
        let (_temp, vault, day_file) = vault("bob-cli-so13-link");
        write_file(
            &vault.join("sase.md"),
            "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n",
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n- [ ] () \u{2014} FIX\n",
        );
        let json = capture_json(&vault, &day_file, &[], "!sase:p\n\n@sase:d");
        let first = json["captures"][0]["task_complete"]["unblocked"]
            .as_array()
            .expect("first");
        assert!(
            first.is_empty(),
            "later link drops the first offer: {first:?}"
        );
    }

    {
        let (_temp, vault, day_file) = vault("bob-cli-so13-reblock");
        write_blocked_tasks_settings(&vault);
        write_file(
            &vault.join("sase.md"),
            "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n- [ ] #task Q [id:: q] ^q\n",
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n- [ ] () \u{2014} FIX\n",
        );
        let json =
            capture_json(&vault, &day_file, &[], "!sase:p\n\n&sase:q @sase+d");
        let first = json["captures"][0]["task_complete"]["unblocked"]
            .as_array()
            .expect("first");
        assert!(
            first.is_empty(),
            "later re-block drops the first offer: {first:?}"
        );
        let d = fs::read_to_string(vault.join("sase.md")).expect("note");
        let d_line = d.lines().find(|line| line.contains("^d")).expect("d");
        assert!(d_line.contains("- [?] "), "{d_line}");
    }

    {
        let (_temp, vault, day_file) = vault("bob-cli-so13-finished");
        write_file(
            &vault.join("proj.md"),
            &project(
                "- [ ] #task A [id:: a] ^a\n- [ ] #task Ship #prj #hide ^prj\n",
            ),
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[proj#^a]]\n",
        );
        let json =
            capture_json(&vault, &day_file, &[], "!proj:a\n\nLeftover @proj");
        let first = json["captures"][0]["task_complete"]["unblocked"]
            .as_array()
            .expect("first");
        assert!(
            !first
                .iter()
                .any(|row| { row["offer"]["group"] == "project_finished" }),
            "open work drops project_finished: {first:?}"
        );
    }
}

// SO14: unplanned predecessor; one open timed entry. Offered at the
// implicit destination; `-p all` links there.
#[test]
fn so14_unplanned_predecessor_implicit_destination() {
    let (_temp, vault, day_file) = vault("bob-cli-so14");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^other]]\n",
    );
    write_file(&vault.join("other.md"), "- [ ] #task Other ^other\n");
    let json = capture_json(&vault, &day_file, &["-p", "all"], "!sase:p");
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["offer"]["destination"]["entry_role"], "implicit");
    assert_eq!(row["offer"]["destination"]["fallback"], true);
    assert_eq!(row["link"]["entry_name"], "CAPTURE");
    assert_eq!(row["offer"]["selected"], true);
}

// SO15: unplanned predecessor with no open entry recovers, no offer.
#[test]
fn so15_no_open_entry_is_no_destination() {
    let (_temp, vault, day_file) = vault("bob-cli-so15");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [x] (**0800-0820**) \u{2014} DONE\n",
    );
    let json = capture_json(&vault, &day_file, &["-p", "all"], "!sase:p");
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["not_linked"], "no_destination");
    assert!(row.get("offer").is_none(), "{row}");
    assert_eq!(json["pull_in"]["offer_count"], 0);
}

// SO16: two open timed entries: recover, no_destination, no offer.
#[test]
fn so16_two_open_timed_is_no_destination() {
    let (_temp, vault, day_file) = vault("bob-cli-so16");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0900-0920**) \u{2014} ONE\n- [ ] (**0930-0950**) \u{2014} TWO\n",
    );
    let json = capture_json(&vault, &day_file, &["-p", "all"], "!sase:p");
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["not_linked"], "no_destination");
    assert!(row.get("offer").is_none(), "{row}");
}

// SO17: emptied-project pull-in, unpicked. `#hide` removed; no link;
// `not_selected`; `project_finished`.
#[test]
fn so17_emptied_project_unpicked() {
    let (_temp, vault, day_file) = vault("bob-cli-so17");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task A [id:: a] ^a\n- [ ] #task Ship #prj #hide ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[proj#^a]]\n",
    );
    let json = capture_json(&vault, &day_file, &[], "!proj:a");
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["not_linked"], "not_selected");
    assert_eq!(row["offer"]["group"], "project_finished");
    assert_eq!(row["offer"]["selected"], false);
    assert_eq!(row["hide_removed"], true);
    assert!(row.get("link").is_none() || row["link"].is_null());
    let note = fs::read_to_string(vault.join("proj.md")).expect("proj");
    assert!(!note.contains("#hide"), "{note}");
}

// SO18: parent-project pull-in unpicked; picked links after K's `^prj`.
#[test]
fn so18_parent_project_unpicked_and_picked() {
    let setup = |name: &str| {
        let (temp, vault, day_file) = vault(name);
        write_file(
            &vault.join("k.md"),
            "---\ntype: [[project]]\nstatus: wip\nparent: \"[[n-proj]]\"\n---\n- [ ] #task Kid work ^prj\n",
        );
        write_file(
            &vault.join("n-proj.md"),
            &project(
                "- [ ] #task Parent work #prj #hide ^prj\n  - 🧩 **Sub-projects:** [[k]]\n",
            ),
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[k#^prj]]\n",
        );
        (temp, vault, day_file)
    };

    let (_temp, vault, day_file) = setup("bob-cli-so18-none");
    let json = capture_json(&vault, &day_file, &[], "!k:prj");
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["not_linked"], "not_selected");
    assert_eq!(row["offer"]["group"], "project_finished");
    assert_eq!(row["hide_removed"], true);
    assert!(row.get("link").is_none() || row["link"].is_null());

    let (_temp, vault, day_file) = setup("bob-cli-so18-all");
    let json = capture_json(&vault, &day_file, &["-p", "all"], "!k:prj");
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["offer"]["selected"], true);
    assert!(row["link"].is_object(), "{row}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(
        day.contains("[[n-proj#^prj]]"),
        "picked parent pull-in links after K's ^prj bullet:\n{day}"
    );
}

// SO19: `plan.link_unblocked: false` yields disabled, offer_count 0;
// emptied `^prj` still loses `#hide`. `-p all` does not override.
#[test]
fn so19_disabled_config_has_no_offers() {
    let (_temp, vault, day_file) = vault("bob-cli-so19");
    write_file(
        &vault.join("proj.md"),
        &project(
            "- [ ] #task A [id:: a] ^a\n- [ ] #task Ship #prj #hide ^prj\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[proj#^a]]\n",
    );
    let config = vault.join("bob-config.yml");
    write_file(&config, "plan:\n  link_unblocked: false\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-p")
        .arg("all")
        .arg("--")
        .arg("!proj:a")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run disabled capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["pull_in"]["offer_count"], 0);
    let row = &json["task_complete"]["unblocked"][0];
    assert_eq!(row["not_linked"], "disabled");
    assert!(row.get("offer").is_none(), "{row}");
    let note = fs::read_to_string(vault.join("proj.md")).expect("proj");
    assert!(!note.contains("#hide"), "{note}");
}

// SO26: human output of a close with offers and no flag contains
// `offer 1` and a group header, exits 0 without reading stdin, and
// does not link.
#[test]
fn so26_human_lists_offers_without_linking() {
    let (_temp, vault, day_file) = fan_vault("bob-cli-so26", 1);
    let output =
        capture_output(&vault, &day_file, &[], "!sase:fix-apollo", true);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Hand-offs"), "{out}");
    assert!(out.contains("offer 1"), "{out}");
    assert!(out.contains("not pulled in"), "{out}");
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(!day.contains("[[sase#^fan-1]]"), "{day}");
}

// SO27: `--require-pull-in-choice` with offers and no `--pull-in` is
// the write-free exit-1 refusal, and the body includes `pull_in` and
// `offers`.
#[test]
fn so27_require_choice_refuses_when_offers_exist() {
    let (_temp, vault, day_file) = so1_vault("bob-cli-so27");
    let before = vault_bytes(&vault);
    let output = capture_output(
        &vault,
        &day_file,
        &["--require-pull-in-choice"],
        "!sase:bar",
        false,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "pull_in_choice_required");
    assert_eq!(
        json["error"],
        "this capture has successor offers; pass --pull-in"
    );
    assert_eq!(json["pull_in"]["offer_count"], 2);
    assert!(json["pull_in"]["choice"].is_null());
    let offers = json["offers"].as_array().expect("offers");
    assert_eq!(offers.len(), 2);
    assert!(offers.iter().all(|row| row.get("offer").is_some()));
    assert_eq!(vault_bytes(&vault), before);
}

// SO29: with zero offers the hidden flag commits; `-p all` / `-p none`
// with zero offers succeed.
#[test]
fn so29_zero_offers_are_a_noop() {
    let setup = |name: &str| {
        let (temp, vault, day_file) = vault(name);
        write_file(&vault.join("sase.md"), "- [*] #task Fix me ^fix\n");
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^fix]]\n",
        );
        (temp, vault, day_file)
    };
    let (_temp, vault, day_file) = setup("bob-cli-so29-require");
    let json = capture_json(
        &vault,
        &day_file,
        &["--require-pull-in-choice"],
        "!sase:fix",
    );
    assert_eq!(json["ok"], true);
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"]["choice"].is_null());

    let (_temp, vault, day_file) = setup("bob-cli-so29-all");
    let json = capture_json(&vault, &day_file, &["-p", "all"], "!sase:fix");
    assert_eq!(json["ok"], true);
    assert_eq!(json["pull_in"]["choice"], "all");
    assert_eq!(json["pull_in"]["offer_count"], 0);

    let (_temp, vault, day_file) = setup("bob-cli-so29-none");
    let json = capture_json(&vault, &day_file, &["-p", "none"], "!sase:fix");
    assert_eq!(json["ok"], true);
    assert_eq!(json["pull_in"]["choice"], "none");
}

// Dry-run JSON and real-run JSON with the same `--pull-in` flags match
// on `pull_in`, `unblocked`, and `still_blocked`.
#[test]
fn dry_run_parity_matches_pull_in_rows() {
    let (_temp, vault, day_file) = fan_vault("bob-cli-so-parity", 2);
    let json = capture_json_dry_run_matches_real_flags(
        &vault,
        &day_file,
        NOW,
        &["-p", "1"],
        &["!sase:fix-apollo"],
    );
    assert_eq!(json["pull_in"]["choice"], "subset");
    assert_eq!(json["pull_in"]["picked_count"], 1);
}

// A capture that offers nothing still has `pull_in.supported: true`.
#[test]
fn no_successors_still_reports_pull_in() {
    let (_temp, vault, day_file) = vault("bob-cli-so-empty");
    write_file(&vault.join("sase.md"), "- [*] #task Fix me ^fix\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^fix]]\n",
    );
    let json = capture_json(&vault, &day_file, &[], "!sase:fix");
    assert_eq!(json["pull_in"]["supported"], true);
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"]["choice"].is_null());
    assert_eq!(json["pull_in"]["picked_count"], 0);
    assert!(json["pull_in"].get("groups").is_none());
}
