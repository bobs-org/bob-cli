//! Annotation-task relation to the reading task (`highlights.annotation_task_relation`).

use crate::support::*;
use std::fs;

const PINNED_NOW: &str = "2026-10-06 09:00:00";

fn relation_config(temp: &TempDir, value: &str) -> std::path::PathBuf {
    let path = temp.path().join("config.yml");
    write_file(
        &path,
        &format!("highlights:\n  annotation_task_relation: {value}\n"),
    );
    path
}

fn sync_with_config(
    vault: &std::path::Path,
    pdf: &std::path::Path,
    config: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("sync")
        .arg(pdf)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", PINNED_NOW)
        .env("BOB_CONFIG_FILE", config);
    for arg in extra {
        command.arg(arg);
    }
    command.output().expect("run bob highlights sync")
}

#[test]
fn v1_local_task_links_the_ref_tracker() {
    let temp = TempDir::new("bob-cli-annotation-relation-v1-local");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/example.pdf");
    let note = vault.join("ref/books/example.md");
    let config = relation_config(&temp, "inspired by");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Example\n",
    );
    seed_v1_ref_note(
        &note,
        "- [/] #task #ref [[lib/books/example.pdf]] #hide ^ref",
    );
    write_file(
        &pdf.with_extension("md"),
        "## Page 4\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Claim.\n\n- #task Compare this claim.\n",
    );

    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    let contents = fs::read_to_string(&note).expect("read ref note");
    let task_line = contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Compare this claim."))
        .unwrap_or_else(|| panic!("missing created task:\n{contents}"))
        .to_string();
    let pos = contents.find(&task_line).expect("task position");
    let after = &contents[pos + task_line.len()..];
    assert!(
        after.starts_with("\n\t- 🪢 **LINKS:** inspired by [[#^ref]]"),
        "v1 local follow-up must carry a same-note LINKS child:\n{contents}"
    );

    // A rerun is a no-op: no duplicate task, no second LINKS line.
    let before = contents.clone();
    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    let rerun = fs::read_to_string(&note).expect("read rerun note");
    assert_eq!(rerun, before);
    assert_eq!(rerun.matches("inspired by [[#^ref]]").count(), 1, "{rerun}");
}

#[test]
fn v1_routed_task_links_the_ref_note() {
    let temp = TempDir::new("bob-cli-annotation-relation-v1-routed");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/task-notes.pdf");
    let note = vault.join("ref/books/task-notes.md");
    let route_note = vault.join("alice.md");
    let config = relation_config(&temp, "inspired by");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Task Notes\n",
    );
    seed_v1_ref_note(
        &note,
        "- [/] #task #ref [[lib/books/task-notes.pdf]] #hide ^ref",
    );
    write_file(&route_note, "---\nparent: \"[[people]]\"\n---\n\n# Alice\n");
    write_file(
        &pdf.with_extension("md"),
        "# Task Notes\n\n## Page 4\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Routed claim.\n\n- #task Follow up with Alice @alice\n",
    );

    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    let route_contents =
        fs::read_to_string(&route_note).expect("read routed note");
    let routed_line = route_contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Follow up with Alice"))
        .unwrap_or_else(|| panic!("missing routed task:\n{route_contents}"))
        .to_string();
    let pos = route_contents.find(&routed_line).expect("routed position");
    let after = &route_contents[pos + routed_line.len()..];
    assert!(
        after.starts_with(
            "\n\t- 🪢 **LINKS:** inspired by [[ref/books/task-notes#^ref]]"
        ),
        "v1 routed follow-up must link the ref note:\n{route_contents}"
    );

    let before = route_contents.clone();
    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    assert_eq!(fs::read_to_string(&route_note).expect("read rerun"), before);
}

#[test]
fn v2_birth_and_follow_up_link_the_final_id() {
    let temp = TempDir::new("bob-cli-annotation-relation-v2-birth");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/collide.pdf");
    let parent_note = vault.join("obsidian.md");
    let config = relation_config(&temp, "inspired by");
    write_file(
        &parent_note,
        "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n\n- [ ] Unrelated old task ^ref-collide\n",
    );
    write_highlights_pdf(&pdf, "- status: wip\n- parent: obsidian\n");
    write_file(
        &pdf.with_extension("md"),
        "## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Colliding quote.\n\n- #task Compare this claim.\n",
    );

    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    let parent_contents =
        fs::read_to_string(&parent_note).expect("read parent");
    assert!(
        parent_contents.contains("^ref-collide-2"),
        "birth must take a suffixed address:\n{parent_contents}"
    );
    let follow_up = parent_contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Compare this claim."))
        .unwrap_or_else(|| panic!("missing follow-up:\n{parent_contents}"))
        .to_string();
    let pos = parent_contents
        .find(&follow_up)
        .expect("follow-up position");
    let after = &parent_contents[pos + follow_up.len()..];
    assert!(
        after
            .starts_with("\n\t- 🪢 **LINKS:** inspired by [[#^ref-collide-2]]"),
        "follow-up must link the final reading-task id:\n{parent_contents}"
    );

    let before = parent_contents.clone();
    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    assert_eq!(
        fs::read_to_string(&parent_note).expect("read rerun"),
        before
    );
}

#[test]
fn v2_explicit_route_links_cross_note_to_the_residence() {
    let temp = TempDir::new("bob-cli-annotation-relation-v2-routed");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/alpha.pdf");
    let parent_note = vault.join("obsidian.md");
    let route_note = vault.join("alice.md");
    let config = relation_config(&temp, "inspired by");
    write_file(&parent_note, "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n");
    write_file(&route_note, "# Alice\n");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Alpha\n",
    );
    write_file(
        &pdf.with_extension("md"),
        "# Alpha\n\n## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Alpha quote.\n\n- #task Tell alice @alice\n",
    );

    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    let route_contents = fs::read_to_string(&route_note).expect("read route");
    let routed = route_contents
        .lines()
        .find(|line| line.starts_with("- [ ] #task Tell alice"))
        .unwrap_or_else(|| panic!("missing routed task:\n{route_contents}"))
        .to_string();
    let pos = route_contents.find(&routed).expect("routed position");
    let after = &route_contents[pos + routed.len()..];
    assert!(
        after.contains("inspired by [[obsidian#^ref-alpha]]"),
        "explicit route must link cross-note to the residence:\n{route_contents}"
    );
}

#[test]
fn trackerless_routed_task_has_no_links_line() {
    let temp = TempDir::new("bob-cli-annotation-relation-trackerless");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/lonely.pdf");
    let note = vault.join("ref/books/lonely.md");
    let route_note = vault.join("alice.md");
    let config = relation_config(&temp, "inspired by");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Lonely\n",
    );
    // Trackerless legacy note: no `^ref` line.
    write_file(
        &note,
        "# Lonely\n\n## Highlights\n\n<!-- highlights:begin -->\n\n<!-- highlights:end -->\n",
    );
    write_file(&route_note, "# Alice\n");
    write_file(
        &pdf.with_extension("md"),
        "## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Lonely quote.\n\n- #task Follow up @alice\n",
    );

    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    let route_contents = fs::read_to_string(&route_note).expect("read route");
    assert!(
        route_contents.contains("- [ ] #task Follow up"),
        "trackerless routed task must still be created:\n{route_contents}"
    );
    assert!(
        !route_contents.contains("🪢 **LINKS:**"),
        "with no ref task there must be no LINKS line:\n{route_contents}"
    );
}

#[test]
fn invalid_label_fails_before_any_write() {
    let temp = TempDir::new("bob-cli-annotation-relation-invalid");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/bad.pdf");
    let note = vault.join("ref/books/bad.md");
    let config = relation_config(&temp, "'bad | label'");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Bad\n",
    );
    seed_v1_ref_note(
        &note,
        "- [/] #task #ref [[lib/books/bad.pdf]] #hide ^ref",
    );
    write_file(
        &pdf.with_extension("md"),
        "## Page 1\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Bad quote.\n\n- #task Should not be created.\n",
    );
    let note_before = fs::read_to_string(&note).expect("read note before");

    let output = sync_with_config(&vault, &pdf, &config, &[]);
    assert!(
        !output.status.success(),
        "invalid label must fail:\n{}",
        format_output(&output)
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("invalid highlights.annotation_task_relation"),
        "error must name the key:\n{stderr}"
    );
    assert!(
        stderr.contains("; fix or remove it"),
        "error must carry the fix hint:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note after"),
        note_before,
        "vault must stay unchanged"
    );
    assert!(
        !fs::read_to_string(&note)
            .expect("read note")
            .contains("Should not be created"),
        "no task may be created"
    );
}

#[test]
fn resync_after_reconcile_stays_a_noop() {
    let temp = TempDir::new("bob-cli-annotation-relation-reconcile");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/books/example.pdf");
    let note = vault.join("ref/books/example.md");
    let config = relation_config(&temp, "inspired by");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Example\n",
    );
    seed_v1_ref_note(
        &note,
        "- [/] #task #ref [[lib/books/example.pdf]] #hide ^ref",
    );
    write_file(
        &pdf.with_extension("md"),
        "## Page 4\n\n- status: wip\n- parent: obsidian\n\n---\n\n> Claim.\n\n- #task Compare this claim.\n",
    );

    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    // Reconcile mints the follow-up id and mirrors LINKED FROM.
    let daily = vault.join("2026/20261006.md");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    assert_success(
        &bob_command()
            .arg("task")
            .arg("reconcile")
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", PINNED_NOW)
            .env("BOB_DAY_FILE", &daily)
            .output()
            .expect("reconcile relations"),
    );
    let after_reconcile = fs::read_to_string(&note).expect("read reconciled");
    assert!(
        after_reconcile.contains("🪢 **LINKS:** inspired by [[#^ref]]"),
        "LINKS must survive reconcile:\n{after_reconcile}"
    );
    assert!(
        after_reconcile.contains("🪢 **LINKED FROM:**"),
        "reconcile must mirror LINKED FROM:\n{after_reconcile}"
    );

    assert_success(&sync_with_config(&vault, &pdf, &config, &[]));
    let resynced = fs::read_to_string(&note).expect("read resynced");
    assert_eq!(resynced, after_reconcile);
    assert_eq!(
        resynced
            .lines()
            .filter(|line| line.starts_with("- [")
                && line.contains("#task Compare this claim."))
            .count(),
        1,
        "{resynced}"
    );
}
