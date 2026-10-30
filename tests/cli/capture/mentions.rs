//! Capture mention-mirror vectors (`plan:202610/inline_task_mentions.md`
//! cli-capture).

use crate::support::*;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-10-14 10:20:00";
const DAY: &str = "## Pomodoros\n- [ ] Unrelated placeholder\n";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, DAY);
    (temp, vault, day)
}

fn write_mentions_config(path: &Path, enabled: bool) {
    write_file(
        path,
        &format!("relations:\n  linked_from_mentions: {enabled}\n"),
    );
}

fn capture_json_env(
    vault: &Path,
    day: &Path,
    draft: &str,
    extra_env: &[(&str, &str)],
) -> serde_json::Value {
    let run = |dry_run: bool| {
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
        command.env("BOB_DAY_FILE", day).env("BOB_NOW", NOW);
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let output = command.output().expect("run capture");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let before = markdown_snapshot(vault);
    let dry: serde_json::Value = run(true);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(markdown_snapshot(vault), before, "dry run writes no notes");
    let real: serde_json::Value = run(false);
    assert_eq!(real["dry_run"], false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    if let Some(serde_json::Value::Array(items)) = masked.get_mut("captures") {
        for item in items {
            item["dry_run"] = serde_json::Value::Bool(false);
        }
    }
    assert_eq!(
        masked, real,
        "dry-run JSON must equal real-run JSON except for dry_run"
    );
    real
}

fn markdown_snapshot(
    root: &Path,
) -> std::collections::BTreeMap<std::path::PathBuf, String> {
    let mut notes = std::collections::BTreeMap::new();
    for entry in fs::read_dir(root).expect("read fixture directory") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            notes.extend(markdown_snapshot(&path));
        } else if path.extension().is_some_and(|extension| extension == "md") {
            notes.insert(
                path.clone(),
                fs::read_to_string(path).expect("read fixture note"),
            );
        }
    }
    notes
}

fn capture_with_mentions(
    vault: &Path,
    day: &Path,
    draft: &str,
    enabled: bool,
) -> serde_json::Value {
    let config = vault.parent().expect("temp parent").join("config.yml");
    write_mentions_config(&config, enabled);
    capture_json_env(
        vault,
        day,
        draft,
        &[("BOB_CONFIG_FILE", config.to_str().expect("config path"))],
    )
}

fn read(vault: &Path, relative: &str) -> String {
    fs::read_to_string(vault.join(relative)).expect("read note")
}

fn snapshot(vault: &Path, relatives: &[&str]) -> Vec<(String, String)> {
    relatives
        .iter()
        .map(|relative| ((*relative).to_string(), read(vault, relative)))
        .collect()
}

fn assert_unchanged(vault: &Path, before: &[(String, String)]) {
    for (relative, contents) in before {
        assert_eq!(
            read(vault, relative),
            *contents,
            "{relative} must stay byte-identical"
        );
    }
}

fn assert_day_untouched(day: &Path) {
    assert_eq!(
        fs::read_to_string(day).expect("day"),
        DAY,
        "day file must stay untouched"
    );
}

fn has_linked_from(contents: &str, entry: &str) -> bool {
    contents.contains(&format!("  - 🪢 **LINKED FROM:** {entry}\n"))
        || contents.contains(&format!("\t- 🪢 **LINKED FROM:** {entry}\n"))
}

#[test]
fn new_task_mention_mints_source_and_stages_target_mirror() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-new");
    write_file(&vault.join("tgt.md"), "- [ ] #task Keep target ^keep\n");

    let json = capture_with_mentions(&vault, &day, "See [[tgt#^keep]]", true);
    assert_eq!(json["kind"], "task");
    let minted = json["minted_relation_block_ids"][0]["block_id"]
        .as_str()
        .expect("minted source id");
    assert_eq!(json["minted_relation_block_ids"][0]["reason"], "mention");
    assert_eq!(json["minted_relation_block_ids"][0]["role"], "source");
    assert_eq!(json["minted_relation_block_ids"][0]["path"], "mac_inbox.md");
    assert_eq!(json["linked_from_line_updates"][0]["path"], "tgt.md");
    assert_eq!(
        json["linked_from_line_updates"][0]["mentions_added"],
        serde_json::json!([format!("[[mac_inbox#^{minted}]]")])
    );
    assert!(json.get("mirror_skipped").is_none(), "{json}");

    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains("See [[tgt#^keep]]")
            && inbox.contains(&format!("^{minted}")),
        "S minted, text kept:\n{inbox}"
    );
    let tgt = read(&vault, "tgt.md");
    assert!(
        has_linked_from(
            &tgt,
            &format!("[[mac_inbox#^{minted}]] (💬 mentions)")
        ),
        "T mirror staged:\n{tgt}"
    );
    assert_day_untouched(&day);
}

#[test]
fn sub_bullet_under_existing_task_stages_parent_mention() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-sub");
    write_file(&vault.join("src.md"), "- [ ] #task Source task ^src\n");
    write_file(&vault.join("tgt.md"), "- [ ] #task Keep target ^keep\n");

    let json = capture_with_mentions(
        &vault,
        &day,
        "@src+src Merged into [[tgt#^keep]]",
        true,
    );
    assert_eq!(json["kind"], "sub_bullet");
    assert!(json.get("minted_relation_block_ids").is_none(), "{json}");
    assert_eq!(json["linked_from_line_updates"][0]["path"], "tgt.md");
    assert_eq!(
        json["linked_from_line_updates"][0]["mentions_added"],
        serde_json::json!(["[[src#^src]]"])
    );

    let src = read(&vault, "src.md");
    assert!(
        src.contains("Merged into [[tgt#^keep]]"),
        "sub-bullet captured:\n{src}"
    );
    let tgt = read(&vault, "tgt.md");
    assert!(
        has_linked_from(&tgt, "[[src#^src]] (💬 mentions)"),
        "T mirror staged:\n{tgt}"
    );
    assert_day_untouched(&day);
}

#[test]
fn tilde_in_same_capture_relation_wins() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-tilde");
    write_file(&vault.join("tgt.md"), "- [ ] #task Keep target ^keep\n");

    let json = capture_with_mentions(
        &vault,
        &day,
        r#"See [[tgt#^keep]] ~tgt:keep|see"#,
        true,
    );
    assert_eq!(json["relation_updates"][0]["action"], "added");
    assert_eq!(json["relation_updates"][0]["label"], "see");
    let minted = json["minted_relation_block_ids"][0]["block_id"]
        .as_str()
        .expect("relation mint");
    assert!(
        json["minted_relation_block_ids"][0].get("reason").is_none(),
        "relation mint omits reason:\n{json}"
    );
    let mentions_added = json["linked_from_line_updates"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .flat_map(|row| {
            row["mentions_added"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert!(
        mentions_added.is_empty(),
        "relation wins, no mention add:\n{json}"
    );

    let tgt = read(&vault, "tgt.md");
    assert!(
        has_linked_from(&tgt, &format!("[[mac_inbox#^{minted}]] (see)")),
        "relation label on T:\n{tgt}"
    );
    assert!(
        !tgt.contains("💬 mentions"),
        "mention must not share the identity:\n{tgt}"
    );
    assert_day_untouched(&day);
}

#[test]
fn ampersand_in_same_capture_suppresses_mention() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-amp");
    write_file(&vault.join("job.md"), "- [ ] #task Order parts ^parts\n");

    let json = capture_with_mentions(
        &vault,
        &day,
        "See [[job#^parts]] &job:parts",
        true,
    );
    assert_eq!(json["dependency_update"]["role"], "dependent");
    assert!(
        json.get("linked_from_line_updates")
            .and_then(|value| value.as_array())
            .is_none_or(|rows| {
                rows.iter().all(|row| {
                    row["mentions_added"]
                        .as_array()
                        .is_none_or(|added| added.is_empty())
                })
            }),
        "dependency suppresses mention:\n{json}"
    );

    let job = read(&vault, "job.md");
    assert!(
        !job.contains("💬 mentions"),
        "no mention entry on T:\n{job}"
    );
    assert!(
        job.contains("🧱 **BLOCKS:**"),
        "& still writes BLOCKS:\n{job}"
    );
    assert_day_untouched(&day);
}

#[test]
fn archived_target_is_skipped() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-arch");
    write_file(
        &vault.join("done/old.md"),
        "- [x] #task Archived target ^old\n",
    );

    let json =
        capture_with_mentions(&vault, &day, "See [[done/old#^old]]", true);
    assert_eq!(json["mirror_skipped"][0]["reason"], "archived_target");
    assert_eq!(json["mirror_skipped"][0]["block_id"], "old");
    assert!(json.get("linked_from_line_updates").is_none(), "{json}");
    assert!(json.get("minted_relation_block_ids").is_none(), "{json}");
    assert_eq!(
        read(&vault, "done/old.md"),
        "- [x] #task Archived target ^old\n",
        "archived T is not written"
    );
    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        !inbox.contains(" ^"),
        "archived target must not mint S:\n{inbox}"
    );
    assert_day_untouched(&day);
}

#[test]
fn mentions_off_is_noop() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-off");
    write_file(&vault.join("tgt.md"), "- [ ] #task Keep target ^keep\n");
    let before = snapshot(&vault, &["tgt.md"]);

    let json = capture_with_mentions(&vault, &day, "See [[tgt#^keep]]", false);
    assert!(json.get("linked_from_line_updates").is_none(), "{json}");
    assert!(json.get("minted_relation_block_ids").is_none(), "{json}");
    assert!(json.get("mirror_skipped").is_none(), "{json}");
    assert_unchanged(&vault, &before);
    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains("See [[tgt#^keep]]"),
        "task still captured:\n{inbox}"
    );
    assert!(
        !inbox.contains(" ^"),
        "mentions off must not mint S:\n{inbox}"
    );
    assert_day_untouched(&day);
}

#[test]
fn no_block_links_json_is_byte_identical() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-nlinks");
    let config = vault.parent().unwrap().join("config.yml");
    let preview = |enabled| {
        write_mentions_config(&config, enabled);
        let output = bob_command()
            .args(["capture", "-b"])
            .arg(&vault)
            .args(["-f", "json", "--dry-run", "--", "Buy milk"])
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", NOW)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .expect("preview capture");
        assert_success(&output);
        output.stdout
    };
    assert_eq!(preview(false), preview(true));
    assert!(!vault.join("mac_inbox.md").exists());
}

#[test]
fn identical_new_tasks_each_get_a_mention_and_their_own_result() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-duplicates");
    write_file(&vault.join("tgt.md"), "- [ ] #task Keep target ^keep\n");
    let json = capture_with_mentions(
        &vault,
        &day,
        "Same [[tgt#^keep]]\n\nSame [[tgt#^keep]]",
        true,
    );
    let captures = json["captures"].as_array().expect("batch items");
    assert_eq!(captures.len(), 2);
    let inbox = read(&vault, "mac_inbox.md");
    let target = read(&vault, "tgt.md");
    for (index, item) in captures.iter().enumerate() {
        let id = item["block_id"].as_str().expect("each source gets an id");
        assert_eq!(item["minted_relation_block_ids"][0]["block_id"], id);
        assert_eq!(inbox.lines().nth(index).unwrap(), item["task_line"]);
        assert!(target.contains(&format!("[[mac_inbox#^{id}]] (💬 mentions)")));
    }
    assert_ne!(captures[0]["block_id"], captures[1]["block_id"]);
}

#[test]
fn same_note_mirrors_do_not_shift_later_source_mints() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-same-note");
    write_file(&vault.join("job.md"), "- [ ] #task Keep target ^keep\n");
    let json = capture_with_mentions(
        &vault,
        &day,
        "First [[#^keep]] @job\n\nSecond [[#^keep]] @job",
        true,
    );
    let captures = json["captures"].as_array().expect("batch items");
    let note = read(&vault, "job.md");
    for item in captures {
        let id = item["block_id"].as_str().expect("each source gets an id");
        assert!(note.lines().any(|line| line == item["task_line"]));
        assert!(note.contains(&format!("[[#^{id}]] (💬 mentions)")));
        assert_eq!(item["minted_relation_block_ids"][0]["block_id"], id);
    }
    assert_ne!(captures[0]["block_id"], captures[1]["block_id"]);
    assert_day_untouched(&day);
}

#[test]
fn same_title_as_an_existing_idless_source_is_a_distinct_pair() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-existing-title");
    write_file(&vault.join("tgt.md"), "- [ ] #task Keep target ^keep\n");
    capture_with_mentions(&vault, &day, "Same [[tgt#^keep]]", false);
    let before = read(&vault, "mac_inbox.md");
    let json = capture_with_mentions(&vault, &day, "Same [[tgt#^keep]]", true);
    let id = json["block_id"].as_str().expect("new source gets an id");
    let inbox = read(&vault, "mac_inbox.md");
    assert!(inbox.starts_with(&before));
    assert!(read(&vault, "tgt.md")
        .contains(&format!("[[mac_inbox#^{id}]] (💬 mentions)")));
}

#[test]
fn unwritable_mirrors_never_mint_a_source() {
    for (name, target, contents, config, reason) in [
        ("disabled", "tgt", "- [ ] #task Target ^keep\n", "relations:\n  linked_from_mentions: true\n  linked_from_lines: false\n", "disabled"),
        ("previous-day", "2026/20261013", "- [ ] #task Target ^keep\n", "relations:\n  linked_from_mentions: true\n", "previous_daily_target"),
        ("malformed", "tgt", "- [ ] #task Target ^keep\n  - 🪢 **LINKED FROM:** broken\n", "relations:\n  linked_from_mentions: true\n", "malformed_line"),
    ] {
        let (_temp, vault, day) = setup(&format!("bob-cli-cap-mn-{name}"));
        let target_path = format!("{target}.md");
        write_file(&vault.join(&target_path), contents);
        let config_path = vault.parent().unwrap().join("config.yml");
        write_file(&config_path, config);
        let json = capture_json_env(&vault, &day, &format!("See [[{target}#^keep]]"), &[("BOB_CONFIG_FILE", config_path.to_str().unwrap())]);
        assert_eq!(json["mirror_skipped"][0]["reason"], reason, "{name}: {json}");
        assert!(json.get("minted_relation_block_ids").is_none(), "{json}");
        assert!(!read(&vault, "mac_inbox.md").contains(" ^"));
        assert_eq!(read(&vault, &target_path), contents);
    }
}

#[test]
fn upgrading_a_mention_to_a_relation_reports_the_removed_mention() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-upgrade");
    write_file(
        &vault.join("src.md"),
        "- [ ] #task See [[tgt#^keep]] ^src\n",
    );
    write_file(&vault.join("tgt.md"), "- [ ] #task Target ^keep\n  - 🪢 **LINKED FROM:** [[src#^src]] (💬 mentions)\n");
    let json =
        capture_with_mentions(&vault, &day, "@src+src ~tgt:keep|see", true);
    assert_eq!(
        json["linked_from_line_updates"][0]["mentions_removed"],
        serde_json::json!(["[[src#^src]]"])
    );
    assert!(json["linked_from_line_updates"][0]
        .get("mentions_added")
        .is_none());
    assert!(read(&vault, "tgt.md").contains("[[src#^src]] (see)"));
    assert!(!read(&vault, "tgt.md").contains("💬 mentions"));
}

#[test]
fn rollback_leaves_every_note_untouched() {
    let (_temp, vault, day) = setup("bob-cli-cap-mn-rollback");
    write_file(&vault.join("tgt.md"), "- [ ] #task Keep target ^keep\n");
    let before = snapshot(&vault, &["tgt.md"]);
    let config = vault.parent().expect("temp parent").join("config.yml");
    write_mentions_config(&config, true);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .arg("--")
        .arg("See [[tgt#^keep]]\n\n:dee\n")
        .output()
        .expect("run capture rollback");
    assert_eq!(
        output.status.code(),
        Some(2),
        "a batch with a query rolls back:\n{}",
        format_output(&output)
    );
    assert_unchanged(&vault, &before);
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "rollback must not write the inbox"
    );
    assert_day_untouched(&day);
}
