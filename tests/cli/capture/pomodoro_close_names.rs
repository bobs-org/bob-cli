//! `x` names the closed session after the work done and the carried
//! session after the work still planned (R6).

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};

const NOW: &str = "2026-09-28 09:37:00";

fn config() -> &'static str {
    "types:\n  BUILD: yellow\n  FIX: blue\n  READ: green\n"
}

fn vault_with(
    name: &str,
    entry: &str,
    links: &[&str],
) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    let config_file = temp.path().join("config.yml");
    write_toggle_task_settings(&vault);
    write_file(&config_file, config());
    let mut day =
        format!("## Pomodoros\n\n- [ ] (**0920-0950** [t:: 30m]) — {entry}\n");
    for link in links {
        day.push_str(&format!("\t- {link}\n"));
    }
    write_file(&day_file, &day);
    write_file(
        &vault.join("tasks.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task Build it [type:: BUILD] [created::2026-09-20] ^build1\n",
            "- [*] #task Fix it [type:: FIX] [created::2026-09-20] ^fix1\n",
            "- [*] #task Read it [type:: READ] [created::2026-09-20] ^read1\n",
            "- [*] #task Plain [created::2026-09-20] ^plain1\n",
        ),
    );
    (temp, vault, day_file, config_file)
}

fn close(
    vault: &Path,
    day_file: &Path,
    config_file: &Path,
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", config_file)
        .output()
        .expect("run close capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("close json")
}

#[test]
fn only_fix_worked_narrows_closed_and_sheds_fix_from_the_continuation() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-fix-only",
        "BUILD + FIX",
        &["[[tasks#^build1]]#", "[[tasks#^fix1]]"],
    );
    let json = close(&vault, &day_file, &config_file, &["x!2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["pomodoro_name"], "BUILD + FIX", "{json}");
    assert_eq!(close["closed_name"], "FIX", "{json}");
    assert_eq!(
        close["entry_line"], "- [x] (**0920-0940** [t:: 20m]) — FIX",
        "{json}"
    );
    let next = &close["next_pomodoro"];
    assert_eq!(next["name"], "BUILD", "{json}");
    assert_eq!(next["created"], true, "{json}");
    assert_eq!(next["shed"][0]["name"], "FIX", "{json}");
    assert_eq!(next["shed"][0]["key"], "fix", "{json}");
    assert_eq!(next["shed"].as_array().map(Vec::len), Some(1), "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("- [x] (**0920-0940** [t:: 20m]) — FIX\n"),
        "{day}"
    );
    assert!(day.contains("- [ ] () — BUILD\n"), "{day}");
    assert!(!day.contains("— BUILD + FIX"), "{day}");
}

#[test]
fn carried_types_keep_the_name_and_report_nothing_shed() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-all-carried",
        "BUILD + FIX",
        &["[[tasks#^build1]]", "[[tasks#^fix1]]"],
    );
    let json = close(&vault, &day_file, &config_file, &["x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed_name"], "BUILD + FIX", "{json}");
    let next = &close["next_pomodoro"];
    assert_eq!(next["name"], "BUILD + FIX", "{json}");
    assert!(next.get("shed").is_none(), "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("— BUILD + FIX\n"), "{day}");
    assert!(day.contains("- [ ] () — BUILD + FIX\n"), "{day}");
}

#[test]
fn labels_stay_and_a_parked_type_is_not_carried() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-labels",
        "BOB + BUILD + FIX",
        &["[[tasks#^build1]]", "[[tasks#^fix1]]"],
    );
    // 2 is parked (worked, not carried); 1 stays in progress.
    let json = close(&vault, &day_file, &config_file, &["x1*2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed_name"], "BOB + BUILD + FIX", "{json}");
    let next = &close["next_pomodoro"];
    assert_eq!(next["name"], "BOB + BUILD", "{json}");
    assert_eq!(next["shed"][0]["name"], "FIX", "{json}");
}

#[test]
fn drifted_work_settles_the_closed_session() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-drift",
        "BUILD + FIX",
        &["[[tasks#^read1]]", "[[tasks#^plain1]]"],
    );
    let json = close(&vault, &day_file, &config_file, &["x!1,2"]);
    let close = &json["pomodoro_close"];
    // Only READ was worked (the plain link is untyped): the closed
    // session records exactly the worked type.
    assert_eq!(close["closed_name"], "READ", "{json}");
    assert_eq!(
        close["entry_line"], "- [x] (**0920-0940** [t:: 20m]) — READ",
        "{json}"
    );
    // Nothing carries, so the continuation keeps the running name.
    let next = &close["next_pomodoro"];
    assert_eq!(next["name"], "BUILD + FIX", "{json}");
    assert!(next.get("shed").is_none(), "{json}");
}

#[test]
fn untyped_work_never_renames() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-untyped",
        "BUILD + FIX",
        &["[[tasks#^plain1]]"],
    );
    let json = close(&vault, &day_file, &config_file, &["x!1"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed_name"], "BUILD + FIX", "{json}");
    let next = &close["next_pomodoro"];
    assert_eq!(next["name"], "BUILD + FIX", "{json}");
    assert!(next.get("shed").is_none(), "{json}");
}

#[test]
fn single_type_entries_keep_matching_work() {
    let (_temp, vault, day_file, config_file) =
        vault_with("bob-cli-close-names-single", "FIX", &["[[tasks#^fix1]]"]);
    let json = close(&vault, &day_file, &config_file, &["x!1"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed_name"], "FIX", "{json}");
    assert_eq!(close["next_pomodoro"]["name"], "FIX", "{json}");
    assert!(close["next_pomodoro"].get("shed").is_none(), "{json}");
}

#[test]
fn single_type_close_settles_to_the_worked_type() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-single-settle",
        "FIX",
        &["[[tasks#^build1]]"],
    );
    let json = close(&vault, &day_file, &config_file, &["x!1"]);
    let close = &json["pomodoro_close"];
    // A `— FIX` session that worked only BUILD closes as `— BUILD`.
    assert_eq!(close["closed_name"], "BUILD", "{json}");
    assert_eq!(
        close["entry_line"], "- [x] (**0920-0940** [t:: 20m]) — BUILD",
        "{json}"
    );
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("- [x] (**0920-0940** [t:: 20m]) — BUILD\n"),
        "{day}"
    );
}

#[test]
fn link_close_forms_apply_the_same_names() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-link",
        "BUILD + FIX",
        &["[[tasks#^build1]]#"],
    );
    // The solo link joins the running entry as link 2, then completes.
    let json = close(&vault, &day_file, &config_file, &["^tasks:fix1=x!2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed_name"], "FIX", "{json}");
    assert_eq!(close["next_pomodoro"]["name"], "BUILD", "{json}");
    assert_eq!(close["next_pomodoro"]["shed"][0]["name"], "FIX", "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("— FIX\n"), "{day}");
    assert!(day.contains("- [ ] () — BUILD\n"), "{day}");
}

#[test]
fn human_output_names_the_narrowed_and_shed_types() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-human",
        "BUILD + FIX",
        &["[[tasks#^build1]]#", "[[tasks#^fix1]]"],
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("x!2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config_file)
        .output()
        .expect("human close");
    assert_success(&output);
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "closed FIX 0920-0950",
            "· from BUILD + FIX",
            "2026/20260928.md line 3",
            "next: BUILD (created)",
            "carries 1 link",
            "shed FIX",
        ],
    );
}

#[test]
fn new_task_close_forms_apply_the_same_names() {
    let (_temp, vault, day_file, config_file) = vault_with(
        "bob-cli-close-names-new-task",
        "BUILD + FIX",
        &["[[tasks#^build1]]#"],
    );
    // The default type gives the new task FIX; it joins as link 2 and
    // completes while the deferred BUILD link carries.
    write_file(
        &config_file,
        "plan:\n  default_type: FIX\ntypes:\n  BUILD: yellow\n  FIX: blue\n",
    );
    let json = close(
        &vault,
        &day_file,
        &config_file,
        &["Ship the patch @tasks:new1=x!2"],
    );
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed_name"], "FIX", "{json}");
    assert_eq!(close["next_pomodoro"]["name"], "BUILD", "{json}");
    assert_eq!(close["next_pomodoro"]["shed"][0]["name"], "FIX", "{json}");
}
