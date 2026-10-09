//! Pomodoro-close successor links (`docs/task-dependencies.md` §12):
//! recovery and successor linking inside `=x` embeds, `=x!M`, `=!`, and
//! `^route:id=x…` (vectors SL7, SL8, SL12, SL17, SL20, SL24, SL27).

use super::pomodoro_close::run_close_json;
use crate::support::*;
use std::fs;

const NOW: &str = "2026-10-05 09:30:00";

fn vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261005.md");
    (temp, vault, day_file)
}

fn apollo_pair() -> String {
    concat!(
        "- [ ] #task Fix apollo machine [id:: fix-apollo] ^fix-apollo\n",
        "- [?] #task Re-launch failed agents [dependsOn:: fix-apollo] [id:: relaunch-agents] ^relaunch-failed-agents\n",
    )
    .to_string()
}

// SL7: `=!` closes BOB with nothing carried and FIX following: a `- [ ]
// () — BOB` continuation holds only the successor, with `entry_created`
// and `next_up`, and `next_pomodoro` names it.
#[test]
fn sl7_close_all_creates_continuation_holding_only_successor() {
    let (_temp, vault, day_file) = vault("bob-cli-close-sl7");
    write_file(&vault.join("sase.md"), &apollo_pair());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^fix-apollo]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=!"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["unblocked_check"], "checked");
    assert_eq!(close["still_blocked"], serde_json::json!([]));
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    let row = &unblocked[0];
    assert_eq!(row["note_path"], "sase.md");
    assert_eq!(row["block_id"], "relaunch-failed-agents");
    assert_eq!(row["text"], "Re-launch failed agents");
    assert_eq!(row["previous_status_symbol"], "?");
    assert_eq!(row["status_symbol"], "*");
    assert_eq!(
        row["unblocked_by"],
        serde_json::json!([{
            "note_path": "sase.md",
            "block_id": "fix-apollo",
            "text": "Fix apollo machine",
        }])
    );
    let link = &row["link"];
    assert_eq!(link["day_file"], "20261005.md");
    assert_eq!(link["entry_name"], "BOB");
    assert_eq!(link["entry_created"], true);
    assert_eq!(link["next_up"], true);
    assert_eq!(link["block_link"], "[[sase#^relaunch-failed-agents]]");
    assert_eq!(link["block_id_created"], false);
    let next = &close["next_pomodoro"];
    assert_eq!(next["name"], "BOB");
    assert_eq!(next["created"], true);
    assert_eq!(next["line"], link["entry_line"]);

    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "## Pomodoros\n- [x] (**0920-0930** [t:: 10m]) \u{2014} BOB\n  - ~~[[sase#^fix-apollo]]~~\n- [ ] () \u{2014} BOB\n\t- [[sase#^relaunch-failed-agents]]\n- [ ] () \u{2014} FIX\n",
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read note"),
        "- [x] #task Fix apollo machine [id:: fix-apollo]  [completion:: 2026-10-05] ^fix-apollo\n- [*] #task Re-launch failed agents [dependsOn:: fix-apollo] [id:: relaunch-agents] ^relaunch-failed-agents\n",
    );
    // The added successor bullet carries the unblocked block-line reason.
    let blocks = json["pomodoro_blocks"].as_array().expect("blocks");
    let created = blocks
        .iter()
        .find(|block| block["name"] == "BOB" && block["created"] == true)
        .expect("created BOB block");
    let added: Vec<&serde_json::Value> = created["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter(|line| {
            line["change"] == "added"
                && line["text"].as_str().is_some_and(|text| {
                    text.contains("[[sase#^relaunch-failed-agents]]")
                })
        })
        .collect();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["reason"], "unblocked");
    // The successor keeps the unblocked task-block role.
    let task_blocks = json["task_blocks"].as_array().expect("task_blocks");
    assert!(
        task_blocks
            .iter()
            .any(|block| block["block_id"] == "relaunch-failed-agents"
                && block["roles"] == serde_json::json!(["unblocked"])),
        "{task_blocks:?}"
    );
}

// SL8: an `=x` close with carried links and a completing embed appends
// the successor after the carried lines in the continuation.
#[test]
fn sl8_embed_close_appends_successor_after_carried_links() {
    let (_temp, vault, day_file) = vault("bob-cli-close-sl8");
    write_file(
        &vault.join("sase.md"),
        "- [*] #task Keep going [id:: keep] ^keep\n- [ ] #task Fix apollo machine [id:: fix-apollo] ^fix-apollo\n- [?] #task Re-launch failed agents [dependsOn:: fix-apollo] [id:: relaunch-agents] ^relaunch-failed-agents\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^keep]]\n  - ![[sase#^fix-apollo]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=x"]);
    let close = &json["pomodoro_close"];
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    let link = &unblocked[0]["link"];
    assert_eq!(link["entry_name"], "BOB");
    assert_eq!(link["entry_created"], true);
    assert_eq!(link["next_up"], true);

    let day = fs::read_to_string(&day_file).expect("read day");
    let keep = day
        .find("[[sase#^keep]]\n")
        .expect("carried keep in continuation");
    let relaunched = day
        .find("[[sase#^relaunch-failed-agents]]")
        .expect("successor in continuation");
    assert!(relaunched > keep, "{day}");
}

// SL12: one gesture closing two predecessors links their shared
// dependent once, naming both predecessors.
#[test]
fn sl12_two_predecessors_link_shared_dependent_once() {
    let (_temp, vault, day_file) = vault("bob-cli-close-sl12");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task First [id:: p1] ^p1\n- [ ] #task Second [id:: p2] ^p2\n- [?] #task Both [dependsOn:: p1, p2] [id:: both] ^both\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - ![[sase#^p1]]\n  - ![[sase#^p2]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=x"]);
    let unblocked = json["pomodoro_close"]["unblocked"]
        .as_array()
        .expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    assert_eq!(unblocked[0]["block_id"], "both");
    let by: Vec<&str> = unblocked[0]["unblocked_by"]
        .as_array()
        .expect("unblocked_by")
        .iter()
        .map(|cause| cause["block_id"].as_str().expect("cause id"))
        .collect();
    assert_eq!(by, vec!["p1", "p2"]);
    assert_eq!(unblocked[0]["link"]["block_link"], "[[sase#^both]]");
}

// SL17: more than five successors in one gesture links none; each
// recovers to its derived rank with the breaker reason.
#[test]
fn sl17_breaker_links_none_of_six_successors() {
    let (_temp, vault, day_file) = vault("bob-cli-close-sl17");
    let mut note = "- [ ] #task Pred [id:: p] ^p\n".to_string();
    for index in 1..=6 {
        note.push_str(&format!(
            "- [?] #task Dep {index} [dependsOn:: p] [id:: d{index}] ^d{index}\n"
        ));
    }
    write_file(&vault.join("sase.md"), &note);
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - ![[sase#^p]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=x"]);
    let close = &json["pomodoro_close"];
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 6);
    for row in unblocked {
        assert_eq!(row["not_linked"], "breaker");
        assert_eq!(row["previous_status_symbol"], "?");
        assert_eq!(row["status_symbol"], " ");
        assert!(row.get("link").is_none() || row["link"].is_null(), "{row}");
    }
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(!day.contains("[[sase#^d"), "{day}");
}

// SL24: when the close leaves no continuation but a later open entry
// carries the closed name, the successor appends there without creating
// an entry.
#[test]
fn sl24_successor_reuses_later_same_name_entry() {
    let (_temp, vault, day_file) = vault("bob-cli-close-sl24");
    write_file(&vault.join("sase.md"), &apollo_pair());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^fix-apollo]]\n- [ ] () \u{2014} FIX\n- [ ] () \u{2014} BOB\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=!"]);
    let close = &json["pomodoro_close"];
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    let link = &unblocked[0]["link"];
    assert_eq!(link["entry_name"], "BOB");
    assert_eq!(link["entry_created"], false);
    assert_eq!(link["next_up"], false);

    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "## Pomodoros\n- [x] (**0920-0930** [t:: 10m]) \u{2014} BOB\n  - ~~[[sase#^fix-apollo]]~~\n- [ ] () \u{2014} FIX\n- [ ] () \u{2014} BOB\n\t- [[sase#^relaunch-failed-agents]]\n",
    );
}

// SL27: `=x~K` drops the dependent's link in the same close that
// completes its prerequisite: the drop wins, so the dependent is never
// re-linked and recovers to Ready.
#[test]
fn sl27_dropped_link_is_not_relinked() {
    let (_temp, vault, day_file) = vault("bob-cli-close-sl27");
    write_file(&vault.join("sase.md"), &apollo_pair());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - ![[sase#^fix-apollo]]\n  - [[sase#^relaunch-failed-agents]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=x~2"]);
    let close = &json["pomodoro_close"];
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    assert_eq!(unblocked[0]["block_id"], "relaunch-failed-agents");
    assert_eq!(unblocked[0]["previous_status_symbol"], "?");
    assert_eq!(unblocked[0]["status_symbol"], " ");
    assert_eq!(unblocked[0]["not_linked"], "already_planned");
    assert!(
        unblocked[0].get("link").is_none() || unblocked[0]["link"].is_null(),
        "{unblocked:?}"
    );
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(!day.contains("relaunch-failed-agents"), "{day}");
}

// Plan example (c): `^sase:fix-apollo=!` links the predecessor into BOB,
// closes it, and creates the continuation holding only the successor.
#[test]
fn route_close_links_predecessor_then_closes_with_successor() {
    let (_temp, vault, day_file) = vault("bob-cli-close-slroute");
    write_file(&vault.join("sase.md"), &apollo_pair());
    write_file(&vault.join("bob.md"), "- [ ] #task Better ^better\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[bob#^better]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["^sase:fix-apollo=!"]);
    let close = &json["pomodoro_close"];
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    let link = &unblocked[0]["link"];
    assert_eq!(link["entry_name"], "BOB");
    assert_eq!(link["entry_created"], true);
    assert_eq!(link["next_up"], true);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "## Pomodoros\n- [x] (**0920-0930** [t:: 10m]) \u{2014} BOB\n  - ~~[[bob#^better]]~~\n  - ~~[[sase#^fix-apollo]]~~\n- [ ] () \u{2014} BOB\n\t- [[sase#^relaunch-failed-agents]]\n- [ ] () \u{2014} FIX\n",
    );
}

// A two-item `=x!1 =` draft closes with the successor, then starts the
// continuation that holds it.
#[test]
fn close_then_start_opens_successor_continuation() {
    let (_temp, vault, day_file) = vault("bob-cli-close-slchain");
    write_file(&vault.join("sase.md"), &apollo_pair());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^fix-apollo]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=x!1 ="]);
    let empty = Vec::new();
    let captures = json["captures"].as_array().unwrap_or(&empty);
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "pomodoro_close");
    assert_eq!(captures[1]["kind"], "pomodoro_start");
    assert_eq!(
        captures[0]["pomodoro_close"]["unblocked"][0]["link"]["entry_name"],
        "BOB"
    );
    assert_eq!(captures[1]["pomodoro_start"]["pomodoro_name"], "BOB");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains(
            "- [ ] (**0930-0955** [t:: 25m]) \u{2014} BOB\n\t- [[sase#^relaunch-failed-agents]]\n"
        ),
        "{day}"
    );
}

// SL20: a draft that completes the successor right after linking it
// reports neither row: the first item's `unblocked` omits the taken-back
// link (net), while the second item links what its own close unblocked.
#[test]
fn sl20_second_completion_drops_first_link_net() {
    let (_temp, vault, day_file) = vault("bob-cli-close-sl20");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n- [?] #task E [dependsOn:: d] [id:: e] ^e\n",
    );
    write_file(&vault.join("other.md"), "- [ ] #task Other ^other\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAP\n  - [[sase#^other]]\n- [ ] () \u{2014} FIX\n  - [[sase#^p]]\n",
    );

    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW),
        "!sase:p\n\n!sase:d\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert!(
        captures[0]["task_complete"]["unblocked"]
            .as_array()
            .expect("first unblocked")
            .is_empty(),
        "{}",
        captures[0]["task_complete"]
    );
    let second = captures[1]["task_complete"]["unblocked"]
        .as_array()
        .expect("second unblocked");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0]["block_id"], "e");
    assert_eq!(second[0]["link"]["block_link"], "[[sase#^e]]");
}

// Dry-run JSON equals real-run JSON (minus `dry_run`) for a close that
// links a successor.
#[test]
fn close_successor_dry_run_matches_real() {
    let setup = |name: &str| {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        write_toggle_task_settings(&vault);
        write_file(&vault.join("sase.md"), &apollo_pair());
        let day_file = vault.join("20261005.md");
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^fix-apollo]]\n- [ ] () \u{2014} FIX\n",
        );
        (temp, vault, day_file)
    };
    let (_temp, vault, day_file) = setup("bob-cli-close-sldry");
    capture_json_dry_run_matches_real(&vault, &day_file, NOW, &["=!"]);
}

// Human rows name the linked successor, its destination, and recovered
// and still-blocked rows, and `next:` counts the unblocked successor.
#[test]
fn close_successor_human_rows_and_next_line() {
    let (_temp, vault, day_file) = vault("bob-cli-close-slhuman");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Fix apollo machine [id:: fix-apollo] ^fix-apollo\n- [?] #task Re-launch failed agents [dependsOn:: fix-apollo] [id:: relaunch-agents] ^relaunch-failed-agents\n- [?] #task Wait on two [dependsOn:: fix-apollo, live-q] [id:: two] ^two\n",
    );
    write_file(
        &vault.join("travel.md"),
        "- [ ] #task Live queue [id:: live-q] ^live-q\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^fix-apollo]]\n- [ ] () \u{2014} FIX\n",
    );

    let output = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--")
            .args(["=!"])
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW)
            .env("NO_COLOR", "1");
        command.output().expect("run bob capture human")
    };
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "linked [?] \u{2192} [*] Re-launch failed agents  sase.md ^relaunch-failed-agents \u{2192} new BOB session (next up)"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "still blocked  Wait on two  sase.md ^two \u{00b7} waits on 1 more"
        ),
        "{out}"
    );
    assert!(out.contains("next: BOB (created)"), "{out}");
    assert!(out.contains("1 unblocked"), "{out}");
}

// An `=!` close whose successor lives in the day file: the continuation
// carries the link while the day task flips in the same write.
#[test]
fn close_all_day_file_successor_composes_edits() {
    let (_temp, vault, day_file) = vault("bob-cli-close-slday");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Predecessor [id:: p] ^p\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n## Tasks\n- [?] #task Day job [dependsOn:: p] ^job\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=!"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["unblocked_check"], "checked");
    let unblocked = close["unblocked"].as_array().expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    let row = &unblocked[0];
    assert_eq!(row["note_path"], "20261005.md");
    assert_eq!(row["block_id"], "job");
    assert_eq!(row["previous_status_symbol"], "?");
    assert_eq!(row["status_symbol"], "*");
    assert_eq!(row["link"]["block_link"], "[[20261005#^job]]");
    assert_eq!(row["link"]["entry_name"], "BOB");
    assert_eq!(row["link"]["entry_created"], true);
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("- [*] #task Day job [dependsOn:: p] ^job"),
        "{day}"
    );
    assert!(
        day.contains("  - [[sase#^p]]") || day.contains("~~[[sase#^p]]~~"),
        "{day}"
    );
    assert!(day.contains("[[20261005#^job]]"), "{day}");
}

// An unreadable dependency note makes the close snapshot incomplete:
// the `=!` close still succeeds with no successor work (`unavailable`).
#[test]
fn close_with_invalid_dependency_note_reports_unavailable() {
    let (_temp, vault, day_file) = vault("bob-cli-close-slunavail");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Predecessor [id:: p] ^p\n",
    );
    write_file(
        &vault.join("d.md"),
        "- [?] #task Dependent [dependsOn:: p, q] [id:: d] ^d\n",
    );
    let mut bad = b"- [ ] #task Other [id:: q] ^q\n".to_vec();
    bad.push(0xff);
    fs::write(vault.join("bad.md"), bad).expect("write bad note");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=!"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["unblocked_check"], "unavailable");
    assert_eq!(close["unblocked"], serde_json::json!([]));
    assert_eq!(close["still_blocked"], serde_json::json!([]));
    assert_eq!(
        fs::read_to_string(vault.join("d.md")).expect("read dependent"),
        "- [?] #task Dependent [dependsOn:: p, q] [id:: d] ^d\n"
    );
}

// An `=x` close whose embeds carry no `[id::]` identity links nothing
// and leaves no successor rows behind.
#[test]
fn close_without_identities_reports_no_successors() {
    let (_temp, vault, day_file) = vault("bob-cli-close-slnoid");
    write_file(&vault.join("sase.md"), "- [ ] #task Plain subtask ^plain\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - ![[sase#^plain]]\n- [ ] () \u{2014} FIX\n",
    );

    let json = run_close_json(&vault, &day_file, NOW, &["=x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["unblocked"], serde_json::json!([]));
    assert_eq!(close["still_blocked"], serde_json::json!([]));
    assert_eq!(close["unblocked_check"], "checked");
}
