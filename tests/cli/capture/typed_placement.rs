//! Bare captures place Task Links by the task's effective type (D7).

use crate::support::*;
use std::fs;

const NOW: &str = "2026-07-10 13:40:00";

fn typed_config() -> String {
    concat!(
        "plan:\n  max_themes: 30\n  default_type: CHORE\n",
        "types:\n  BUILD: yellow\n  FIX: blue\n  READ: green\n",
        "  WRITE: magenta\n  PLAN: blue\n  CHORE: yellow\n",
    )
    .to_string()
}

fn fixture(
    prefix: &str,
    config: &str,
    notes: &[(&str, &str)],
    day: &str,
) -> (
    TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let config_file = temp.path().join("config.yml");
    write_toggle_task_settings(&vault);
    write_file(&config_file, config);
    for (name, contents) in notes {
        write_file(&vault.join(name), contents);
    }
    write_file(&day_file, day);
    (temp, vault, day_file, config_file)
}

fn capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    config: &std::path::Path,
    args: &[&str],
    dry_run: bool,
) -> (std::process::Output, serde_json::Value) {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("-d");
    }
    command
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", config);
    let output = command.output().expect("run capture");
    let json =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    (output, json)
}

fn running_build_day() -> &'static str {
    concat!(
        "# 2026-07-10\n",
        "## Pomodoros\n",
        "- [ ] (**1330-1400** [t:: 30m]) — BUILD\n",
        "  - existing context\n",
        "- [ ] () — MEET\n",
        "## Later\n",
    )
}

#[test]
fn bare_new_task_creates_chore_placeholder_when_running_type_differs() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-inbox",
        &typed_config(),
        &[("inbox.md", "# Inbox\n## Tasks\n")],
        running_build_day(),
    );
    let (output, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Buy oat milk @inbox:oat1"],
        false,
    );
    assert_success(&output);
    assert_eq!(json["kind"], "pomodoro_task", "{json}");
    let destination = &json["pomodoro_link_destination"];
    assert_eq!(destination["name"], "CHORE", "{json}");
    assert_eq!(destination["role"], "created", "{json}");
    assert_eq!(destination["type"]["theme"]["name"], "CHORE", "{json}");
    assert_eq!(destination["type"]["source"], "default", "{json}");
    assert_eq!(destination["type"]["label"], "default type", "{json}");
    assert_eq!(json["creates_pomodoro"], true, "{json}");
    // BUILD's timer is untouched: the running entry keeps its time range
    // and gains no link.
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("- [ ] (**1330-1400** [t:: 30m]) — BUILD\n"),
        "{day}"
    );
    assert!(
        day.contains("- [ ] () — CHORE\n  - [[inbox#^oat1]]\n"),
        "{day}"
    );

    // The dry-run preview names the typed destination the same way.
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-inbox-dry",
        &typed_config(),
        &[("inbox.md", "# Inbox\n## Tasks\n")],
        running_build_day(),
    );
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-d")
        .arg("Buy oat milk @inbox:oat1")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("run dry-run capture");
    assert_success(&output);
    let human = stdout(&output);
    // The established created-row order (`→ new Pomodoro X`, pinned by
    // the plan-budget suite and docs/capture.md) gains the appended
    // typed label; only the suffix is new.
    assert!(
        human.contains("→ new Pomodoro CHORE · default type"),
        "{human}"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("dry run writes nothing"),
        running_build_day(),
        "dry run writes nothing"
    );
}

#[test]
fn bare_existing_link_creates_chore_placeholder_when_running_type_differs() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-existing",
        &typed_config(),
        &[(
            "inbox.md",
            "# Inbox\n## Tasks\n- [ ] #task Existing inbox task ^aaa\n",
        )],
        running_build_day(),
    );
    let (output, json) =
        capture_json(&vault, &day_file, &config, &["@inbox:aaa"], false);
    assert_success(&output);
    assert_eq!(json["pomodoro_link_action"], "linked", "{json}");
    let destination = &json["pomodoro_link_destination"];
    assert_eq!(destination["name"], "CHORE", "{json}");
    assert_eq!(destination["role"], "created", "{json}");
    assert_eq!(destination["type"]["label"], "default type", "{json}");
    assert_eq!(json["creates_pomodoro"], true, "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("- [ ] (**1330-1400** [t:: 30m]) — BUILD\n"),
        "{day}"
    );
    assert!(
        day.contains("- [ ] () — CHORE\n  - [[inbox#^aaa]]\n"),
        "{day}"
    );
}

#[test]
fn bare_link_joins_the_running_entry_when_the_type_agrees() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-agree",
        &typed_config(),
        &[(
            "dev.md",
            "---\ntype: [[project]]\ntask_type: BUILD\n---\n# Dev\n## Tasks\n- [ ] #task Dev task ^dev1\n",
        )],
        running_build_day(),
    );
    let (output, json) =
        capture_json(&vault, &day_file, &config, &["@dev:dev1"], false);
    assert_success(&output);
    let destination = &json["pomodoro_link_destination"];
    assert_eq!(destination["name"], "BUILD", "{json}");
    assert_eq!(destination["role"], "current", "{json}");
    assert_eq!(destination["type"]["theme"]["name"], "BUILD", "{json}");
    assert_eq!(destination["type"]["source"], "note", "{json}");
    assert_eq!(destination["type"]["label"], "from dev", "{json}");
    assert_eq!(json["creates_pomodoro"], false, "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("  - [[dev#^dev1]]\n"), "{day}");
    assert!(!day.contains("— CHORE"), "{day}");
}

#[test]
fn bare_new_ref_task_places_into_read() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-ref",
        &typed_config(),
        &[("inbox.md", "# Inbox\n## Tasks\n")],
        running_build_day(),
    );
    let (output, json) = capture_json(
        &vault,
        &day_file,
        &config,
        &["Read the manual \\#ref @inbox:ref1"],
        false,
    );
    assert_success(&output);
    assert_eq!(json["kind"], "pomodoro_task", "{json}");
    let destination = &json["pomodoro_link_destination"];
    assert_eq!(destination["name"], "READ", "{json}");
    assert_eq!(destination["role"], "created", "{json}");
    assert_eq!(destination["type"]["theme"]["name"], "READ", "{json}");
    assert_eq!(destination["type"]["source"], "reading", "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("- [ ] () — READ\n  - [[inbox#^ref1]]\n"),
        "{day}"
    );
}

#[test]
fn bare_link_without_a_default_keeps_legacy_selection() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-legacy",
        "plan:\n  max_themes: 30\ntypes:\n  BUILD: yellow\n  MEET: blue\n",
        &[(
            "inbox.md",
            "# Inbox\n## Tasks\n- [ ] #task Existing inbox task ^aaa\n",
        )],
        running_build_day(),
    );
    let (output, json) =
        capture_json(&vault, &day_file, &config, &["@inbox:aaa"], false);
    assert_success(&output);
    let destination = &json["pomodoro_link_destination"];
    assert_eq!(destination["name"], "BUILD", "{json}");
    assert_eq!(destination["role"], "current", "{json}");
    assert!(destination.get("type").is_none(), "{json}");
    assert_eq!(json["creates_pomodoro"], false, "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains(
            "- [ ] (**1330-1400** [t:: 30m]) — BUILD\n  - existing context\n  - [[inbox#^aaa]]\n"
        ),
        "{day}"
    );
}

#[test]
fn bare_batch_links_share_one_typed_placeholder() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-batch",
        &typed_config(),
        &[(
            "inbox.md",
            "# Inbox\n## Tasks\n- [ ] #task First chore ^one\n- [ ] #task Second chore ^two\n",
        )],
        running_build_day(),
    );
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = run_with_stdin(&mut command, "@inbox:one\n\n@inbox:two\n");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2, "{json}");
    for item in captures {
        assert_eq!(
            item["pomodoro_link_destination"]["name"], "CHORE",
            "{item}"
        );
    }
    let day = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(day.matches("— CHORE").count(), 1, "{day}");
    assert!(
        day.contains("  - [[inbox#^one]]\n  - [[inbox#^two]]\n"),
        "{day}"
    );
}

#[test]
fn bare_link_strict_refusal_names_the_type_cap_and_escape() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-strict",
        "plan:\n  max_themes: 3\n  strict: true\n  default_type: CHORE\ntypes:\n  BUILD: yellow\n  MEET: blue\n  READ: green\n  CHORE: yellow\n",
        &[("inbox.md", "# Inbox\n## Tasks\n- [ ] #task Existing inbox task ^aaa\n")],
        "# 2026-07-10\n## Pomodoros\n- [ ] (**1330-1400** [t:: 30m]) — BUILD\n- [ ] () — MEET\n- [ ] () — READ\n## Later\n",
    );
    let before = fs::read_to_string(&day_file).expect("read day");
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("@inbox:aaa")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("run strict capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert_eq!(json["ok"], false, "{json}");
    assert_eq!(json["code"], "strict_cap", "{json}");
    let error = json["error"].as_str().expect("error message");
    assert!(error.contains("CHORE"), "{error}");
    assert!(error.contains("4 of 3"), "{error}");
    assert!(error.contains("#CHORE"), "{error}");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        before,
        "refused batch writes nothing"
    );
}

#[test]
fn bare_link_timed_ambiguity_still_fails() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-ambiguous",
        &typed_config(),
        &[("inbox.md", "# Inbox\n## Tasks\n- [ ] #task Existing inbox task ^aaa\n")],
        "# 2026-07-10\n## Pomodoros\n- [ ] (**1330-1400** [t:: 30m]) — BUILD\n- [ ] (**1400-1430** [t:: 30m]) — MEET\n## Later\n",
    );
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("@inbox:aaa")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("run ambiguous capture");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
    assert_eq!(json["ok"], false, "{json}");
    assert!(
        json["error"].as_str().is_some_and(
            |error| error.contains("multiple open timed Pomodoros")
        ),
        "{json}"
    );
}

#[test]
fn bare_toggle_places_by_type_and_reports_it() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-toggle",
        &typed_config(),
        &[(
            "inbox.md",
            "# Inbox\n## Tasks\n- [ ] #task Toggle me ^tog\n",
        )],
        running_build_day(),
    );
    let (output, json) =
        capture_json(&vault, &day_file, &config, &["@inbox+tog!"], false);
    assert_success(&output);
    assert_eq!(json["toggle_direction"], "link", "{json}");
    let destination = &json["pomodoro_link_destination"];
    assert_eq!(destination["name"], "CHORE", "{json}");
    assert_eq!(destination["type"]["source"], "default", "{json}");
    assert_eq!(destination["type"]["label"], "default type", "{json}");
    assert_eq!(json["creates_pomodoro"], true, "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains("- [ ] () — CHORE\n  - [[inbox#^tog]]\n"),
        "{day}"
    );
}

#[test]
fn bare_project_note_tasks_place_by_their_own_types() {
    let (_temp, vault, day_file, config) = fixture(
        "bob-cli-typed-placement-project-note",
        &typed_config(),
        &[(
            "cash.md",
            "---\ntype: \"[[area]]\"\ntask_type: BUILD\n---\n# Cash\n",
        )],
        running_build_day(),
    );
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = run_with_stdin(
        &mut command,
        "Finish packet! @cash^proj3@\n- Draft memo :draft-memo\n- Fix bug [type:: FIX] :fix-bug\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "project_note", "{json}");
    assert_eq!(json["creates_pomodoro"], true, "{json}");
    let destination = &json["pomodoro_link_destination"];
    assert_eq!(destination["name"], "BUILD", "{json}");
    assert_eq!(destination["type"]["source"], "note", "{json}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("  - [[cash_proj3#^draft-memo]]\n"), "{day}");
    assert!(
        day.contains("- [ ] () — FIX\n  - [[cash_proj3#^fix-bug]]\n"),
        "{day}"
    );
}
