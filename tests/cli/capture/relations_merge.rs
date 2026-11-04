//! Capture `~` / `~~` merge-label commits (`docs/task-relations.md` §21).

use crate::support::*;
use std::fs;

const NOW: &str = "2026-10-14 10:20:00";
const DAY: &str = "## Pomodoros\n- [ ] Unrelated placeholder\n";
const CONFIG: &str = "relations:\n  merge_labels: [merged into]\n";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, DAY);
    let config = vault.join("config.yml");
    write_file(&config, CONFIG);
    (temp, vault, day)
}

fn capture(
    vault: &std::path::Path,
    day: &std::path::Path,
    draft: &str,
    dry_run: bool,
) -> serde_json::Value {
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
    command.arg("--").arg(draft);
    let output = command
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", vault.join("config.yml"))
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn item(result: &serde_json::Value) -> &serde_json::Value {
    result
        .get("captures")
        .and_then(serde_json::Value::as_array)
        .and_then(|captures| captures.first())
        .unwrap_or(result)
}

#[test]
fn to_relation_merges_existing_owner_into_the_link_target() {
    let (_temp, vault, day) = setup("bob-cli-relation-merge-to");
    write_file(
        &vault.join("source.md"),
        "- [ ] #task Source ^src\n\t- source note\n",
    );
    write_file(
        &vault.join("target.md"),
        "- [ ] #task Target ^target\n\t- target note\n",
    );
    let before_source = fs::read_to_string(vault.join("source.md"))
        .expect("source before dry run");
    let before_target = fs::read_to_string(vault.join("target.md"))
        .expect("target before dry run");
    let draft = "@source+src ~target:target|\"merged into\"";

    let dry = capture(&vault, &day, draft, true);
    let dry_item = item(&dry);
    assert_eq!(dry_item["merges"][0]["outcome"], "merged", "{dry}");
    assert_eq!(dry_item["relation_updates"][0]["merge_effect"], "merged");
    assert_eq!(
        fs::read_to_string(vault.join("source.md")).expect("source after dry"),
        before_source,
        "dry run leaves the source untouched"
    );
    assert_eq!(
        fs::read_to_string(vault.join("target.md")).expect("target after dry"),
        before_target,
        "dry run leaves the target untouched"
    );

    let real = capture(&vault, &day, draft, false);
    let real_item = item(&real);
    assert_eq!(real_item["merges"][0]["outcome"], "merged");
    assert_eq!(real_item["relation_updates"][0]["merge_effect"], "merged");
    let source = fs::read_to_string(vault.join("source.md"))
        .expect("read merged source");
    let target =
        fs::read_to_string(vault.join("target.md")).expect("read merge target");
    assert!(source.contains("- [-] #task Source"), "{source}");
    assert!(
        source.contains("merged into [[target#^target]]"),
        "{source}"
    );
    assert!(target.contains("source note"), "{target}");
    assert!(target.contains("[[source#^src]] (merged into)"), "{target}");
}

#[test]
fn from_relation_merges_a_picked_task_into_a_new_capture_task() {
    let (_temp, vault, day) = setup("bob-cli-relation-merge-from");
    write_file(
        &vault.join("source.md"),
        "- [ ] #task Source ^src\n\t- source note\n",
    );

    let result = capture(
        &vault,
        &day,
        "New target ~~source:src|\"merged into\"",
        false,
    );
    let result_item = item(&result);
    assert_eq!(result_item["merges"][0]["outcome"], "merged");
    assert_eq!(result_item["relation_updates"][0]["direction"], "from");
    assert_eq!(result_item["relation_updates"][0]["merge_effect"], "merged");
    let source = fs::read_to_string(vault.join("source.md"))
        .expect("read merged source");
    assert!(source.contains("- [-] #task Source"), "{source}");
}

#[test]
fn a_closed_merge_source_is_link_only() {
    let (_temp, vault, day) = setup("bob-cli-relation-merge-closed");
    write_file(
        &vault.join("source.md"),
        "- [x] #task Source [created:: 2026-10-01] ^src\n",
    );
    write_file(&vault.join("target.md"), "- [ ] #task Target ^target\n");

    let result = capture(
        &vault,
        &day,
        "@source+src ~target:target|\"merged into\"",
        false,
    );
    let result_item = item(&result);
    assert_eq!(result_item["merges"][0]["outcome"], "already_closed");
    assert_eq!(
        result_item["relation_updates"][0]["merge_effect"],
        "already_closed"
    );
    let source = fs::read_to_string(vault.join("source.md"))
        .expect("read closed source");
    assert!(source.starts_with("- [x] #task Source"), "{source}");
}
