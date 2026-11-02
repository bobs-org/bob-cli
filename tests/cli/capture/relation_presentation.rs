//! Capture pull-in copy and relation human output
//! (`docs/task-dependencies.md` §§12.5–12.6, 12.15, 12.18; SU-P vectors).

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-11-02 09:30:00";

const SUCCESSOR_CONFIG: &str = concat!(
    "relations:\n",
    "  successor_labels:\n",
    "    - starts after\n",
);

const TWO_LABEL_CONFIG: &str = concat!(
    "relations:\n",
    "  successor_labels:\n",
    "    - starts after\n",
    "    - follows after\n",
);

fn vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261102.md");
    (temp, vault, day_file)
}

fn write_config(vault: &Path, body: &str) -> PathBuf {
    let config = vault.parent().expect("temp dir").join("config.yml");
    write_file(&config, body);
    config
}

fn agents_body() -> String {
    concat!(
        "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
        "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
        "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
    )
    .to_string()
}

fn sa4_vault(name: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let (temp, vault, day_file) = vault(name);
    write_file(&vault.join("agents.md"), &agents_body());
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    (temp, vault, day_file, config)
}

fn capture_output(
    vault: &Path,
    day_file: &Path,
    config: &Path,
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
        .env("BOB_CONFIG_FILE", config)
        .output()
        .expect("run bob capture")
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    flags: &[&str],
    text: &str,
) -> serde_json::Value {
    let output = capture_output(vault, day_file, config, flags, text, false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn capture_human(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    flags: &[&str],
    text: &str,
) -> String {
    let output = capture_output(vault, day_file, config, flags, text, true);
    assert_success(&output);
    stdout(&output)
}

fn relation_group<'a>(json: &'a serde_json::Value) -> &'a serde_json::Value {
    let groups = json["pull_in"]["groups"]
        .as_array()
        .unwrap_or_else(|| panic!("groups: {json}"));
    groups
        .iter()
        .find(|group| group["group"] == "relation")
        .unwrap_or_else(|| panic!("relation group: {groups:?}"))
}

// SU-P1/P2/P3/P8: relation-only JSON copy matches the shared vectors.
#[test]
fn relation_only_copy_matches_su_p_vectors() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-copy");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert_eq!(json["pull_in"]["offer_count"], 1);
    let copy = &json["pull_in"]["copy"];
    assert_eq!(
        copy["title"],
        "Starts after \u{201c}Send implementation prompt\u{201d}"
    );
    assert_eq!(copy["capsule"], "Starts after");
    assert_eq!(copy["glyph"], "relation");
    let group = relation_group(&json);
    assert_eq!(group["label"], "starts after");
    assert_eq!(group["event"], "done");
    assert_eq!(group["after"]["key"], "agents.md#^implement");
    assert_eq!(group["after"]["note_path"], "agents.md");
    assert_eq!(group["after"]["block_id"], "implement");
    assert_eq!(group["after"]["text"], "Send implementation prompt");
    assert_eq!(
        group["copy"]["title"],
        "Starts after \u{201c}Send implementation prompt\u{201d}"
    );
    assert_eq!(
        group["copy"]["why"],
        "\u{201c}Send implementation prompt\u{201d} is done"
    );
    assert_eq!(group["copy"]["tag"], "your links \u{b7} not dependents");
    assert_eq!(group["copy"]["glyph"], "relation");
    assert!(group.get("folded").is_none(), "{group}");
    assert!(group["copy"].get("fold").is_none(), "{group}");
}

// Labels off still author copy for the remaining groups.
#[test]
fn labels_disabled_still_copies_hand_off_group() {
    let (_temp, vault, day_file) = vault("bob-cli-su-p-labels-off");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [?] #task Prepare evaluation prompt [dependsOn:: implement] [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let config = write_config(&vault, "relations:\n  successor_labels: []\n");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(
        json["task_complete"].get("relation_successors").is_none(),
        "{json}"
    );
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["group"], "hand_off");
    assert_eq!(groups[0]["copy"]["title"], "Hand-offs");
    assert_eq!(groups[0]["copy"]["glyph"], "hand_off");
    assert!(groups[0].get("folded").is_none(), "{groups:?}");
    assert_eq!(
        json["pull_in"]["copy"]["title"],
        "Pull in successors of Send implementation prompt"
    );
    assert_eq!(json["pull_in"]["copy"]["capsule"], "Pull in");
    assert_eq!(json["pull_in"]["copy"]["glyph"], "hand_off");
}

// SA18: mixed groups stay in engine order and keep the mixed title.
#[test]
fn mixed_groups_preserve_order_and_title() {
    let (_temp, vault, day_file) = vault("bob-cli-su-p-mixed");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
            "- [ ] #task Ship schema change [id:: schema] ^schema\n",
            "- [?] #task Announce schema change [dependsOn:: schema] [id:: schema-change] ^schema-change\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n  - [[agents#^implement]]\n  - [[agents#^schema]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let json = capture_json(&vault, &day_file, &config, &["--dry-run"], "x*!2");
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    let kinds: Vec<&str> = groups
        .iter()
        .map(|group| group["group"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds, vec!["hand_off", "relation"], "{groups:?}");
    assert_eq!(groups[0]["copy"]["title"], "Hand-offs");
    assert_eq!(groups[0]["copy"]["glyph"], "hand_off");
    assert_eq!(groups[1]["copy"]["glyph"], "relation");
    let title = json["pull_in"]["copy"]["title"]
        .as_str()
        .expect("mixed title");
    assert!(title.starts_with("Pull in successors"), "{title}");
    assert_eq!(json["pull_in"]["copy"]["capsule"], "Pull in");
    assert_eq!(json["pull_in"]["copy"]["glyph"], "hand_off");

    let out = capture_human(&vault, &day_file, &config, &["--dry-run"], "x*!2");
    let hand = out.find("  Hand-offs\n").expect("hand-off header");
    let relation = out
        .find("Starts after \u{201c}Send implementation prompt\u{201d} \u{b7} it is parked (your \u{1faa2} links, not dependents)")
        .expect("relation header");
    assert!(hand < relation, "{out}");
    let relation_section = &out[relation..];
    for line in relation_section
        .lines()
        .filter(|line| line.contains("offer"))
    {
        assert!(!line.contains("unblocked"), "{line}");
    }
}

// SA12: several triggering targets print the shared suffix.
#[test]
fn multi_target_row_prints_suffix() {
    let (_temp, vault, day_file) = vault("bob-cli-su-p-suffix");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]] \u{2022} starts after [[agents#^deploy]]\n",
            "- [*] #task Send implementation prompt [id:: implement] ^implement\n",
            "- [*] #task Deploy release [id:: deploy] ^deploy\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BUILD\n  - [[agents#^implement]]\n  - [[agents#^deploy]]\n",
    );
    let config = write_config(&vault, SUCCESSOR_CONFIG);
    let out = capture_human(&vault, &day_file, &config, &["--dry-run"], "x*");
    assert!(
        out.contains(
            "after \u{201c}Send implementation prompt\u{201d} and \u{201c}Deploy release\u{201d}"
        ),
        "{out}"
    );
}

// Distinct labels to the same target stay two groups.
#[test]
fn two_labels_same_target_emit_two_groups() {
    let (_temp, vault, day_file) = vault("bob-cli-su-p-two-label");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [ ] #task Review the rollout [id:: review] ^review\n",
            "  - 🪢 **LINKS:** follows after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n",
    );
    let config = write_config(&vault, TWO_LABEL_CONFIG);
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    let relation: Vec<_> = groups
        .iter()
        .filter(|group| group["group"] == "relation")
        .collect();
    assert_eq!(relation.len(), 2, "{groups:?}");
    let labels: Vec<&str> = relation
        .iter()
        .map(|group| group["label"].as_str().unwrap_or_default())
        .collect();
    assert!(labels.contains(&"starts after"), "{labels:?}");
    assert!(labels.contains(&"follows after"), "{labels:?}");
    assert!(
        relation
            .iter()
            .all(|group| group["after"]["key"] == "agents.md#^implement"),
        "{relation:?}"
    );
    let title = json["pull_in"]["copy"]["title"]
        .as_str()
        .expect("relation-only title");
    assert!(title.ends_with("2 tasks"), "{title}");
}

// Held-only already-started prints the SU-P13 notice; already-planned is silent.
#[test]
fn held_only_notice_skips_already_planned() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-held");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [/] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
    assert!(json["pull_in"].get("copy").is_none(), "{json}");
    let out = capture_human(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(
        out.contains(
            "\u{1faa2} \u{201c}Prepare evaluation prompt\u{201d} starts after \u{201c}Send implementation prompt\u{201d} \u{b7} not offered: already started"
        ),
        "{out}"
    );
    assert!(!out.contains("unblocked"), "{out}");

    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-planned");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} BUILD\n  - [[agents#^implement]]\n  - [[agents#^evaluate]]\n",
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    let held = json["task_complete"]["relation_held"]
        .as_array()
        .expect("held");
    assert_eq!(held[0]["reason"], "already_planned", "{held:?}");
    let out = capture_human(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(!out.contains("not offered"), "{out}");
    assert!(!out.contains('\u{1faa2}'), "{out}");
}

// Hidden sibling folds; all-zero fold is omitted.
#[test]
fn fold_counts_hidden_and_omits_all_zero() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-fold");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [ ] #task Write rollout notes #hide [id:: rollout] ^rollout\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    let group = relation_group(&json);
    assert_eq!(group["folded"]["hidden"], 1, "{group}");
    assert_eq!(group["copy"]["fold"], "not offered \u{b7} 1 hidden");
    let out = capture_human(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(out.contains("not offered  1 hidden"), "{out}");
}

// All sources filtered: no groups, silence besides the close line.
#[test]
fn all_filtered_omits_groups_and_stays_silent() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-filtered");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [ ] #task Prepare evaluation prompt #hide [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert_eq!(json["pull_in"]["offer_count"], 0);
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
    let out = capture_human(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(out.contains("not offered: hidden"), "{out}");
    assert!(!out.contains("unblocked"), "{out}");
}

// Unavailable snapshot: one muted uncertainty line, no groups.
#[test]
fn unavailable_prints_uncertainty_and_omits_groups() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-unavail");
    fs::write(
        vault.join("poison.md"),
        [b"LINKS".as_slice(), &[0xff]].concat(),
    )
    .expect("poison relation snapshot");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert_eq!(json["task_complete"]["relation_check"], "unavailable");
    assert!(
        json["task_complete"].get("relation_successors").is_none(),
        "{json}"
    );
    assert!(json["pull_in"].get("groups").is_none(), "{json}");
    let out = capture_human(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(
        out.contains("\u{1faa2} Couldn't check starts after links \u{b7} nothing offered")
            || out.contains("\u{1faa2} Couldn't check relation links \u{b7} nothing offered"),
        "{out}"
    );
}

// Refusal reuses the same groups and copy, including human headers.
#[test]
fn refusal_includes_relation_copy() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-refuse");
    let output = capture_output(
        &vault,
        &day_file,
        &config,
        &["--require-pull-in-choice"],
        "!agents:implement",
        false,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["code"], "pull_in_choice_required");
    assert_eq!(
        json["pull_in"]["copy"]["title"],
        "Starts after \u{201c}Send implementation prompt\u{201d}"
    );
    assert_eq!(json["pull_in"]["groups"][0]["copy"]["glyph"], "relation");

    let human = capture_output(
        &vault,
        &day_file,
        &config,
        &["--require-pull-in-choice"],
        "!agents:implement",
        true,
    );
    assert_eq!(human.status.code(), Some(1), "{}", format_output(&human));
    let out = stdout(&human);
    assert!(
        out.contains(
            "Starts after \u{201c}Send implementation prompt\u{201d} \u{b7} it is done (your \u{1faa2} links, not dependents)"
        ),
        "{out}"
    );
    assert!(out.contains("offer 1"), "{out}");
    assert!(!out.contains("unblocked"), "{out}");
}

// SA14: a hand-off row keeps dependency wording and appends the relation reason.
#[test]
fn mixed_hand_off_row_appends_secondary_reason() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-sa14");
    write_file(
        &vault.join("agents.md"),
        concat!(
            "- [?] #task Prepare evaluation prompt [dependsOn:: implement] [id:: evaluate] ^evaluate\n",
            "  - 🪢 **LINKS:** starts after [[agents#^implement]]\n",
            "- [/] #task Send implementation prompt [id:: implement] ^implement\n",
        ),
    );
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert_eq!(
        json["task_complete"]["unblocked"][0]["offer"]["group"],
        "hand_off"
    );
    assert_eq!(json["pull_in"]["groups"][0]["group"], "hand_off");
    let out = capture_human(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(out.contains("Hand-offs"), "{out}");
    assert!(out.contains("unblocked"), "{out}");
    assert!(
        out.contains(
            "\u{b7} starts after \u{201c}Send implementation prompt\u{201d}"
        ),
        "{out}"
    );
}

// Keep-moving-only groups pin the §12.15 picker strings.
#[test]
fn keep_moving_only_copy_uses_picker_strings() {
    let (_temp, vault, day_file) = vault("bob-cli-su-p-km");
    write_file(&vault.join("dev.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[project]]\nparent: [[dev]]\nstatus: wip\n---\n- [ ] #task #prj Sase ^prj\n",
    );
    write_file(
        &vault.join("sase_badges.md"),
        concat!(
            "---\ntype: [[project]]\nparent: [[sase]]\nstatus: wip\n---\n",
            "- [ ] #task Draft the badge spec [id:: t] ^t\n",
            "- [ ] #task #prj Badges ^prj\n",
            "- [ ] #task Write badge docs [id:: a] ^a\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () \u{2014} FIX\n  - [[sase_badges#^t]]\n",
    );
    let config = write_config(&vault, "relations:\n  successor_labels: []\n");
    let json = capture_json(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!sase_badges:t",
    );
    let groups = json["pull_in"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["group"], "keep_moving");
    assert_eq!(
        groups[0]["copy"]["title"],
        "Keep \u{2039}sase_badges\u{203a} moving"
    );
    assert_eq!(groups[0]["copy"]["glyph"], "keep_moving");
    assert_eq!(
        groups[0]["copy"]["tag"],
        "ready in sase_badges \u{b7} not dependents"
    );
    assert_eq!(
        json["pull_in"]["copy"]["title"],
        "Keep \u{2039}sase_badges\u{203a} moving"
    );
    assert_eq!(json["pull_in"]["copy"]["capsule"], "Keep moving");
    assert_eq!(json["pull_in"]["copy"]["glyph"], "keep_moving");
}

// Relation-only human rows use lane names, never unblocked/unlock.
#[test]
fn relation_human_rows_use_lane_names() {
    let (_temp, vault, day_file, config) = sa4_vault("bob-cli-su-p-lanes");
    let out = capture_human(
        &vault,
        &day_file,
        &config,
        &["--dry-run"],
        "!agents:implement",
    );
    assert!(out.contains("offer 1  Ready"), "{out}");
    assert!(!out.contains("unblocked"), "{out}");
    assert!(!out.contains('\u{1f513}'), "{out}");
    let picked = capture_human(
        &vault,
        &day_file,
        &config,
        &["-p", "all"],
        "!agents:implement",
    );
    assert!(picked.contains("linked Ready \u{2192} Next"), "{picked}");
    assert!(
        picked.contains(
            "\u{b7} starts after \u{201c}Send implementation prompt\u{201d}"
        ),
        "{picked}"
    );
}
