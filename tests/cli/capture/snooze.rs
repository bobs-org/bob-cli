//! Atomic capture snooze operations against fixed-clock fixture vaults.
use crate::support::*;
use serde_json::Value;
use std::{fs, path::PathBuf, process::Output};

const DAY: &str = "## Pomodoros\n\n- [ ] (**1200-1230**) — RUNNING\n  - running context\n- [ ] () — LATER\n\n## Tail\nkeep tail bytes\n";

struct Fixture {
    temp: TempDir,
    vault: PathBuf,
    day: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new(name: &str, tasks: &str) -> Self {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        let day = vault.join("2026/20261125.md");
        let config = temp.path().join("bob.yml");
        write_file(
            &config,
            "timezone: America/New_York\nplan:\n  max_links: 1\n",
        );
        write_blocked_tasks_settings(&vault);
        write_file(&vault.join("jobs.md"), tasks);
        write_file(&day, DAY);
        Self {
            temp,
            vault,
            day,
            config,
        }
    }
    fn run(&self, draft: &str, dry: bool) -> Output {
        self.at(draft, dry, "2026-11-25 12:03:00")
    }
    fn at(&self, draft: &str, dry: bool, now: &str) -> Output {
        let mut cmd = bob_command();
        cmd.args(["capture", "-f", "json", "-b"]).arg(&self.vault);
        if dry {
            cmd.arg("--dry-run");
        }
        cmd.arg("--")
            .arg(draft)
            .env("BOB_DAY_FILE", &self.day)
            .env("BOB_NOW", now)
            .env("BOB_CONFIG_FILE", &self.config)
            .env("XDG_STATE_HOME", self.temp.path().join("state"));
        cmd.output().expect("capture")
    }
    fn ok(&self, draft: &str) -> Value {
        let out = self.run(draft, false);
        assert_success(&out);
        json(&out)
    }
    fn tasks(&self) -> String {
        fs::read_to_string(self.vault.join("jobs.md")).unwrap()
    }
    fn day(&self) -> String {
        fs::read_to_string(&self.day).unwrap()
    }
    fn refuse(&self, draft: &str, message: &str) {
        let before = (self.tasks(), self.day());
        for dry in [true, false] {
            let out = self.run(draft, dry);
            assert!(!out.status.success(), "{}", format_output(&out));
            assert!(
                json(&out)["error"].as_str().unwrap().contains(message),
                "{}",
                format_output(&out)
            );
            assert_eq!((self.tasks(), self.day()), before);
        }
    }
}
fn json(out: &Output) -> Value {
    serde_json::from_str(stdout(out).trim())
        .unwrap_or_else(|_| panic!("{}", format_output(out)))
}

#[test]
fn ready_commits_next_and_dry_run_has_identical_plan() {
    let f = Fixture::new("capture-snooze-ready", "- [ ] #task Work ^work\n");
    let before = (f.tasks(), f.day());
    let out = f.run("@jobs:work z:+45m", true);
    assert_success(&out);
    let preview = json(&out);
    assert_eq!((f.tasks(), f.day()), before);
    let result = f.ok("@jobs:work z:+45m");
    assert_eq!(preview["snooze"], result["snooze"]);
    assert_eq!(preview["task_line"], result["task_line"]);
    assert_eq!(result["snooze"]["time"], "1248");
    assert_eq!(result["snooze"]["wake_at"], "2026-11-25T12:48:00-05:00");
    assert!(f.tasks().contains("- [*] #task Work"));
    assert!(f.tasks().contains("[fresh:: 2026-11-25]"), "{}", f.tasks());
    assert!(result.get("pomodoro_link_destination").is_none());
    let row = result["snooze"]["row"].as_u64().unwrap() as usize;
    assert_eq!(
        f.day().lines().nth(row - 1).unwrap(),
        "- **1248** [[jobs#^work]]"
    );
    assert!(f.day().ends_with("## Tail\nkeep tail bytes\n"));
}

#[test]
fn completion_candidate_is_literal_and_both_parse_and_capture_accept_it() {
    let f = Fixture::new("capture-snooze-completion-accept", "");
    write_file(
        &f.day,
        concat!(
            "## Events\n",
            "- **1230-1300** Yogi interview\n",
            "\n",
            "## Pomodoros\n",
            "- [ ] (**1200-1230**) — RUNNING\n",
            "- [ ] () — LATER\n",
            "\n## Tail\nkeep tail bytes\n",
        ),
    );

    let draft = "Fix sink @home z:";
    let completion = bob_command()
        .args(["capture-complete", "-b"])
        .arg(&f.vault)
        .args(["-c", &draft.len().to_string(), "-f", "json", "--"])
        .arg(draft)
        .env("BOB_DAY_FILE", &f.day)
        .env("BOB_NOW", "2026-11-25 12:03:00")
        .env("BOB_CONFIG_FILE", &f.config)
        .output()
        .expect("capture-complete");
    assert_success(&completion);
    let menu: Value = serde_json::from_str(stdout(&completion).trim())
        .expect("capture-complete JSON");
    assert_eq!(menu["context"], "snooze");
    assert_eq!(menu["replacement"]["start"], draft.len());
    assert_eq!(menu["replacement"]["end"], draft.len());
    let candidate = menu["candidates"][0]["replacement"]
        .as_str()
        .expect("literal candidate");
    assert_eq!(candidate, "1300");
    assert_eq!(
        menu["candidates"][0]["label"],
        "13:00 · after Yogi interview"
    );
    let mut accepted = draft.to_string();
    let start = menu["replacement"]["start"].as_u64().unwrap() as usize;
    let end = menu["replacement"]["end"].as_u64().unwrap() as usize;
    accepted.replace_range(start..end, candidate);

    let parsed = bob_command()
        .args(["capture-parse", "-f", "json", "--"])
        .arg(&accepted)
        .env("BOB_NOW", "2026-11-25 12:03:00")
        .env("BOB_CONFIG_FILE", &f.config)
        .output()
        .expect("capture-parse");
    assert_success(&parsed);
    let parsed: Value = serde_json::from_str(stdout(&parsed).trim())
        .expect("capture-parse JSON");
    assert!(parsed["spans"].as_array().unwrap().iter().any(|span| {
        span["kind"] == "snooze"
            && span["start"] == accepted.find("z:").unwrap() as u64
            && span["end"] == accepted.len() as u64
    }));

    let dry_run = f.at(&accepted, true, "2026-11-25 12:03:00");
    assert_success(&dry_run);
    assert_eq!(json(&dry_run)["snooze"]["time"], "1300");
}

#[test]
fn next_and_in_progress_task_bytes_do_not_change() {
    for (name, status) in [("next", '*'), ("progress", '/')] {
        let line = format!(
            "- [{status}] #task Work [fresh::2026-11-01] ^work\n  - authored\n"
        );
        let f = Fixture::new(&format!("capture-snooze-{name}"), &line);
        f.ok("@jobs:work z:1530");
        assert_eq!(f.tasks(), line);
    }
}

#[test]
fn new_tasks_keep_children_and_mint_against_staged_and_non_task_ids() {
    let f = Fixture::new("capture-snooze-mint", "- prose ^fix-sink\n");
    let result = f.ok("Fix sink @jobs z:19\n- bring tools\n\nFix sink @jobs z:20\n\nAuthored @jobs:chosen z:21");
    let ids: Vec<_> = result["captures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["block_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids[2], "chosen");
    assert_ne!(ids[0], "fix-sink");
    assert_ne!(ids[0], ids[1]);
    assert!(f.tasks().contains("bring tools"));
    assert_eq!(f.tasks().matches("- [*] #task").count(), 3);
    assert_eq!(
        f.day().matches("- [ ] (").count(),
        2,
        "no extra session: {}",
        f.day()
    );
    assert_eq!(f.day().matches("[[jobs#^").count(), 3);
    let ledger = f.day().split("## Snoozed").next().unwrap().to_string();
    assert!(!ledger.contains("[[jobs#^"));
}

#[test]
fn same_note_target_and_daily_edits_coalesce() {
    let f = Fixture::new("capture-snooze-same-note", "# Jobs\n");
    write_file(
        &f.day,
        &format!("## Tasks\n- [ ] #task Here ^here\n\n{DAY}"),
    );
    let result = f.ok("@!20261125:here z:19");
    assert!(f.day().contains("- [*] #task Here"));
    assert!(f.day().contains("- **1900** [[2026/20261125#^here]]"));
    let row = result["snooze"]["row"].as_u64().unwrap() as usize;
    assert!(f
        .day()
        .lines()
        .nth(row - 1)
        .unwrap()
        .starts_with("- **1900**"));
}

#[test]
fn add_consumes_live_link_notes_and_keeps_recorded_time() {
    let f = Fixture::new("capture-snooze-consume", "- [/] #task Work ^work\n");
    write_file(&f.day, "## Pomodoros\n- [ ] (**1200-1230**) — RUNNING\n  - [[jobs#^work]]\n    - keep nested\n      - deepest\n- [x] (**1000-1030**) — OLD [t::30m]\n  - [[jobs#^work]]\n\n## Tail\nkeep\n");
    f.ok("@jobs:work z:1530");
    let day = f.day();
    assert!(day.contains("(**1200-1230**)"));
    assert!(day.contains("[t::30m]"));
    assert!(
        day.contains(
            "- **1530** [[jobs#^work]]\n  - keep nested\n    - deepest"
        ),
        "{day}"
    );
}

#[test]
fn retime_dedupes_resolved_spellings_and_preserves_notes_and_history() {
    let f = Fixture::new("capture-snooze-retime", "- [*] #task Work ^work\n");
    write_file(&f.day, &format!("{DAY}\n## Snoozed\n- **1300** ![[jobs#^work|alias]]\n  - first\n- **1400** [[jobs.md#^work]]\n  - second\n- ~~**1000** [[jobs#^other]]~~ · woke 10:01\n"));
    let result = f.ok("@jobs:work z:+1h");
    assert_eq!(result["snooze"]["action"], "retime");
    assert_eq!(f.day().matches("- **").count(), 1);
    assert!(
        f.day()
            .contains("- **1303** [[jobs#^work]]\n  - first\n  - second"),
        "{}",
        f.day()
    );
    assert!(f
        .day()
        .contains("- ~~**1000** [[jobs#^other]]~~ · woke 10:01"));
}

#[test]
fn link_now_uses_running_destination_and_transfers_nested_notes_once() {
    let f = Fixture::new("capture-snooze-link-now", "- [*] #task Work ^work\n");
    write_file(&f.day, &format!("{DAY}\n## Snoozed\n- **1530** ![[jobs.md#^work|alias]]\n  - note\n    - nested\n"));
    let result = f.ok("@jobs:work");
    assert_eq!(result["snooze"]["action"], "link_now");
    assert_eq!(result["pomodoro_link_destination"]["role"], "current");
    let day = f.day();
    assert!(!day.contains("## Snoozed"), "{day}");
    assert_eq!(day.matches("#^work").count(), 1);
    assert_eq!(day.matches("- nested").count(), 1);
    assert!(
        day.find("#^work").unwrap() < day.find("LATER").unwrap(),
        "{day}"
    );
    assert!(!day.contains("woke"));
    assert!(!f
        .temp
        .path()
        .join("state/bob-cli/snooze/last-wake.json")
        .exists());
}

#[test]
fn drop_is_row_only_and_struck_history_is_not_active() {
    let line = "- [*] #task Work [fresh::2026-11-01] ^work\n";
    let f = Fixture::new("capture-snooze-drop", line);
    f.ok("@jobs:work z:1530");
    let result = f.ok("@jobs:work!");
    assert_eq!(result["snooze"]["action"], "drop");
    assert_eq!(f.tasks(), line);
    assert!(!f.day().contains("## Snoozed"));
    assert!(!f.day().contains("#^work"));
    f.refuse("@jobs:work!", "not snoozed");
    write_file(
        &f.day,
        &format!(
            "{DAY}\n## Snoozed\n- ~~**1100** [[jobs#^work]]~~ · woke 11:00\n"
        ),
    );
    f.refuse("@jobs:work!", "not snoozed");
    f.ok("@jobs:work z:19");
    assert!(!f.day().contains("woke"));
    assert!(f.day().contains("- **1900** [[jobs#^work]]"));
}

#[test]
fn blocked_and_closed_targets_refuse_without_dependency_or_schedule_changes() {
    for task in [
        "- [?] #task Work ^work\n",
        "- [ ] #task Work [scheduled::2026-11-26] ^work\n",
        "- [ ] #task Work ^work\n  - ⛓️ **DEPENDS ON:** [[jobs#^prereq]]\n- [ ] #task Prerequisite ^prereq\n",
    ] {
        let f = Fixture::new("capture-snooze-blocked", task);
        f.refuse("@jobs:work z:19", "Blocked");
    }
    for (status, message) in [('x', "completed"), ('-', "cancelled")] {
        let f = Fixture::new(
            "capture-snooze-closed",
            &format!("- [{status}] #task Work ^work\n"),
        );
        f.refuse("@jobs:work z:19", message);
    }
    let f = Fixture::new("capture-snooze-nontask", "- prose ^work\n");
    f.refuse("@jobs:work z:19", "not a task");
}

#[test]
fn inherited_project_block_and_event_clock_refuse() {
    let f = Fixture::new("capture-snooze-project", "---\ntype: \"[[project]]\"\n---\n\n## Project\n- [?] #task Project [scheduled::2026-11-26] ^prj\n\n## Tasks\n- [ ] #task Work ^work\n");
    f.refuse("@jobs:work z:19", "Blocked");
    f.refuse("New work @jobs:new z:19", "Blocked");
    write_file(
        &f.day,
        &format!(
            "## Events\n- [ ] #task **1500-1530** Meeting ^meeting\n\n{DAY}"
        ),
    );
    f.refuse(
        "@!20261125:meeting z:19",
        "Event Tasks wait on their own clock",
    );
}

#[test]
fn later_failure_rolls_back_earlier_snooze_and_new_task() {
    let f = Fixture::new(
        "capture-snooze-rollback",
        "- [ ] #task Work ^work\n- [x] #task Done ^done\n",
    );
    f.refuse(
        "@jobs:work z:1530\n\nNew task @jobs:new z:19\n\n@jobs:done z:20",
        "completed",
    );
    f.refuse("@jobs:work z:1530\n\n@jobs:done z:not-a-time", "use 1530");
}

#[test]
fn sequential_same_task_operations_see_staged_rows_and_tasks() {
    let f = Fixture::new("capture-snooze-sequential", "# Jobs\n");
    let result = f.ok("Work @jobs:work z:15\n\n@jobs:work z:16\n\n@jobs:work!\n\n@jobs:work z:17");
    assert_eq!(f.tasks().matches("#task Work").count(), 1);
    assert_eq!(f.day().matches("#^work").count(), 1);
    assert!(f.day().contains("- **1700** [[jobs#^work]]"));
    assert_eq!(result["captures"][1]["snooze"]["action"], "retime");
    assert_eq!(result["captures"][2]["snooze"]["action"], "drop");
}

#[test]
fn full_link_budget_does_not_block_snooze_or_count_its_row() {
    let f = Fixture::new(
        "capture-snooze-budget",
        "- [*] #task Existing ^other\n- [ ] #task Work ^work\n",
    );
    write_file(
        &f.day,
        "## Pomodoros\n- [ ] (**1200-1230**) — RUNNING\n  - [[jobs#^other]]\n",
    );
    let result = f.ok("@jobs:work z:19");
    assert!(result.get("pomodoro_link_destination").is_none());
    if !result["plan_budget"].is_null() {
        assert_eq!(result["plan_budget"]["links"]["count"], 1);
    }
    assert_eq!(
        f.day()
            .split("## Snoozed")
            .next()
            .unwrap()
            .matches("#^")
            .count(),
        1
    );
}

#[test]
fn time_refusals_use_same_day_and_dst_rules() {
    let mut f =
        Fixture::new("capture-snooze-clock", "- [ ] #task Work ^work\n");
    f.refuse("@jobs:work z:11", "link it now, or use `s:1` for tomorrow");
    f.refuse("@jobs:work z:+24h", "same-day only");
    // BOB_DAY_FILE must name the same day as the injected clock.
    f.day = f.vault.join("2026/20260308.md");
    write_file(&f.day, DAY);
    let out = f.at("@jobs:work z:0230", false, "2026-03-08 01:00:00");
    assert!(!out.status.success());
    assert_eq!(json(&out)["code"], "dst_gap", "{}", format_output(&out));
    assert_eq!(f.day(), DAY);
}

#[test]
fn local_daily_link_aliases_retime_and_drop() {
    let f = Fixture::new("capture-snooze-local", "# Jobs\n");
    write_file(&f.day, &format!("## Tasks\n- [*] #task Here ^here\n\n{DAY}\n## Snoozed\n- **1300** [[#^here|local]]\n  - keep\n"));
    f.ok("@!20261125:here z:19");
    assert_eq!(f.day().matches("#^here").count(), 1);
    assert!(f.day().contains("  - keep"));
    f.ok("@20261125:here!");
    assert!(!f.day().contains("## Snoozed"));
}

#[test]
fn crlf_link_now_remaps_destination_after_an_earlier_snoozed_section() {
    let f = Fixture::new("capture-snooze-crlf", "- [*] #task Work ^work\n");
    let day = "## Snoozed\n- **1530** [[jobs#^work]]\n  - remember\n    - nested\n\n## Pomodoros\n- [ ] (**1200-1230**) — RUNNING\n  - context\n\n## Tail\nunchanged\n".replace('\n', "\r\n");
    write_file(&f.day, &day);
    let result = f.ok("@jobs:work");
    let after = f.day();
    assert!(!after.contains("## Snoozed"));
    assert!(!after.replace("\r\n", "").contains('\n'));
    assert!(after.ends_with("## Tail\r\nunchanged\r\n"));
    assert_eq!(after.matches("- remember").count(), 1);
    let line = result["pomodoro_link_destination"]["line"]
        .as_u64()
        .unwrap() as usize;
    assert!(
        after.lines().nth(line - 1).unwrap().contains("1200-1230"),
        "{result}"
    );
}

#[test]
fn earlier_status_edit_is_visible_to_snooze_eligibility() {
    let f = Fixture::new(
        "capture-snooze-staged-status",
        "- [ ] #task Work ^work\n",
    );
    f.refuse("!jobs:work\n\n@jobs:work z:19", "completed");
}
