//! Capture plan-budget reporting, strict mode, and destination roles.

use crate::support::*;
use std::fs;

fn three_theme_day() -> &'static str {
    concat!(
        "# 2026-07-10\n",
        "## Pomodoros\n",
        "- [ ] () — GOALS\n",
        "  - [[dev#^aaa]]\n",
        "- [ ] () — DECKS\n",
        "  - [[dev#^bbb]]\n",
        "- [ ] () — BOB\n",
        "  - [[dev#^ccc]]\n",
        "## Later\n",
    )
}

fn capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
    config: Option<&std::path::Path>,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    for arg in args {
        command.arg(arg);
    }
    command.env("BOB_DAY_FILE", day_file);
    command.env("BOB_NOW", "2026-07-10 13:40:00");
    if let Some(config) = config {
        command.env("BOB_CONFIG_FILE", config);
    }
    command.output().expect("run bob capture")
}

fn parse_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(output))
    })
}

#[test]
fn capture_plan_budget_warns_when_new_theme_grows_past_cap() {
    let temp = TempDir::new("bob-cli-capture-budget-warn");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());

    let output = capture_json(
        &vault,
        &day_file,
        &["@dev:newid#NEWT", "Ship", "the", "thing."],
        None,
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["kind"], "pomodoro_task");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(json["pomodoro_name"], "NEWT");

    let budget = &json["plan_budget"];
    assert_eq!(budget["status"], "over");
    assert_eq!(budget["themes"]["count"], 4);
    assert_eq!(budget["themes"]["cap"], 3);
    assert_eq!(budget["themes"]["over"], true);
    assert_eq!(budget["themes"]["before"], 3);
    assert_eq!(budget["links"]["count"], 4);
    assert_eq!(budget["links"]["before"], 3);
    assert_eq!(budget["links"]["over"], false);
    assert_eq!(budget["added_themes"], serde_json::json!(["NEWT"]));
    assert_eq!(budget["warnings"][0]["code"], "plan_theme_cap_exceeded");
    assert!(
        budget["warnings"][0]["message"]
            .as_str()
            .expect("warning message")
            .contains("4/3 themes (adds NEWT)"),
        "{budget}"
    );
    assert!(
        json.get("warnings").is_none(),
        "budget warnings stay out of the item warnings: {json}"
    );
}

#[test]
fn capture_plan_budget_human_reports_meter_and_destination() {
    let temp = TempDir::new("bob-cli-capture-budget-human");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@dev:newid#NEWT")
        .arg("Ship")
        .arg("the")
        .arg("thing.")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run human capture");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("plan 4/3 themes · 4/10 links  (+1 theme: NEWT)"),
        "{out}"
    );
    assert!(out.contains("→ new Pomodoro NEWT"), "{out}");
    assert!(
        stderr(&output).contains("bob capture: warning:"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn capture_plan_budget_stays_quiet_when_over_cap_does_not_grow() {
    let temp = TempDir::new("bob-cli-capture-budget-quiet");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing ^quid\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] () — ONE\n",
            "- [ ] () — TWO\n",
            "- [ ] () — THREE\n",
            "- [ ] () — FOUR\n",
            "  - [[dev#^old]]\n",
            "## Later\n",
        ),
    );

    // Linking under an existing entry keeps the theme count at 4/3:
    // the budget is reported but no warning fires.
    let output = capture_json(&vault, &day_file, &["@dev:quid"], None);
    assert_success(&output);
    let json = parse_json(&output);
    let budget = &json["plan_budget"];
    assert_eq!(budget["status"], "over");
    assert_eq!(budget["themes"]["count"], 4);
    assert_eq!(budget["themes"]["before"], 4);
    let warnings = budget
        .get("warnings")
        .and_then(|warnings| warnings.as_array())
        .map_or(0, Vec::len);
    assert_eq!(warnings, 0);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
}

#[test]
fn capture_plan_budget_warns_on_link_growth_past_cap() {
    let temp = TempDir::new("bob-cli-capture-budget-links");
    let vault = temp.path().join("vault");
    let target = vault.join("t.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# T\n## Tasks\n- [ ] #task Existing\n");
    let mut day =
        String::from("# 2026-07-10\n## Pomodoros\n- [ ] () — GOALS\n");
    for index in 0..10 {
        day.push_str(&format!("  - [[t#^a{index:02}]]\n"));
    }
    write_file(&day_file, &day);

    let output = capture_json(
        &vault,
        &day_file,
        &["Eleventh", "link", "@t:link11"],
        None,
    );
    assert_success(&output);
    let json = parse_json(&output);
    let budget = &json["plan_budget"];
    assert_eq!(budget["status"], "over");
    assert_eq!(budget["themes"]["count"], 1);
    assert_eq!(budget["links"]["count"], 11);
    assert_eq!(budget["links"]["before"], 10);
    assert_eq!(budget["warnings"][0]["code"], "plan_link_cap_exceeded");
    assert!(
        budget["warnings"][0]["message"]
            .as_str()
            .expect("warning message")
            .contains("11/10 links"),
        "{budget}"
    );
}

#[test]
fn capture_plan_budget_dry_run_matches_real_run() {
    let temp = TempDir::new("bob-cli-capture-budget-dry-run");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());
    let before_day = fs::read_to_string(&day_file).expect("read day");
    let before_target = fs::read_to_string(&target).expect("read target");

    let dry = capture_json(
        &vault,
        &day_file,
        &["-d", "--", "@dev:newid#NEWT", "Ship", "it."],
        None,
    );
    assert_success(&dry);
    let dry_json = parse_json(&dry);
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), before_day);
    assert_eq!(
        fs::read_to_string(&target).expect("read target"),
        before_target
    );

    let real = capture_json(
        &vault,
        &day_file,
        &["@dev:newid#NEWT", "Ship", "it."],
        None,
    );
    assert_success(&real);
    let real_json = parse_json(&real);
    assert_eq!(real_json["plan_budget"], dry_json["plan_budget"]);
}

#[test]
fn capture_plan_budget_invalid_config_skips_with_warning() {
    let temp = TempDir::new("bob-cli-capture-budget-bad-config");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());
    write_file(&config, "plan:\n  max_themes: 0\n");

    let output = capture_json(
        &vault,
        &day_file,
        &["@dev:newid#NEWT", "Ship", "it."],
        Some(&config),
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert!(json.get("plan_budget").is_none(), "{json}");
    let warnings = json["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0]
            .as_str()
            .expect("warning")
            .contains("plan budget"),
        "{json}"
    );
}

#[test]
fn capture_destination_roles_cover_current_next_up_named_created() {
    let temp = TempDir::new("bob-cli-capture-budget-roles");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
            "- [ ] () — BUGS\n",
            "## Later\n",
        ),
    );

    // Implicit to the running entry: current.
    let output = capture_json(
        &vault,
        &day_file,
        &["@dev:cur1", "Current", "work."],
        None,
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["pomodoro_link_destination"]["role"], "current");
    assert_eq!(json["pomodoro_link_destination"]["name"], "CURRENT");
    assert_eq!(json["pomodoro_name"], "CURRENT");
    assert_eq!(json["creates_pomodoro"], false);

    // Explicit existing name: named.
    let output = capture_json(
        &vault,
        &day_file,
        &["@dev:nam1#bugs", "Named", "work."],
        None,
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["pomodoro_link_destination"]["role"], "named");
    assert_eq!(json["pomodoro_name"], "BUGS");

    // Explicit new name: created.
    let output = capture_json(
        &vault,
        &day_file,
        &["@dev:new1#FRESH", "Fresh", "work."],
        None,
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["pomodoro_link_destination"]["role"], "created");
    assert_eq!(json["creates_pomodoro"], true);
    assert_eq!(json["pomodoro_name"], "FRESH");

    // Implicit with no timed entry: next_up.
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] () — FIRST\n",
            "- [ ] () — SECOND\n",
            "## Later\n",
        ),
    );
    let output =
        capture_json(&vault, &day_file, &["@dev:next1", "Next", "work."], None);
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["pomodoro_link_destination"]["role"], "next_up");
    assert_eq!(json["pomodoro_name"], "FIRST");
}

#[test]
fn capture_destination_human_arrows_name_each_role() {
    let temp = TempDir::new("bob-cli-capture-budget-arrows");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
            "- [ ] () — BUGS\n",
            "## Later\n",
        ),
    );

    for (args, expected) in [
        (
            vec!["@dev:c1", "Work."],
            "→ into running CURRENT (0900-0930)",
        ),
        (vec!["@dev:c2#bugs", "Work."], "→ under BUGS (named)"),
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .args(&args)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00")
            .output()
            .expect("run human capture");
        assert_success(&output);
        let out = stdout(&output);
        assert!(out.contains(expected), "{out}");
    }
}

#[test]
fn capture_strict_mode_refuses_new_theme_past_cap_atomically() {
    let temp = TempDir::new("bob-cli-capture-budget-strict");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());
    write_file(&config, "plan:\n  strict: true\n");
    let before_day = fs::read_to_string(&day_file).expect("read day");
    let before_target = fs::read_to_string(&target).expect("read target");

    let output = capture_json(
        &vault,
        &day_file,
        &["@dev:newid#NEWT", "Ship", "it."],
        Some(&config),
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json = parse_json(&output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "plan_theme_cap_exceeded");
    let error = json["error"].as_str().expect("error message");
    assert!(error.contains("4/3 themes"), "{error}");
    assert!(error.contains("GOALS"), "{error}");
    assert!(error.contains("#now"), "{error}");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        before_day,
        "refused batch writes nothing"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read target"),
        before_target,
        "refused batch writes nothing"
    );
}

#[test]
fn capture_strict_mode_never_refuses_session_starts() {
    let temp = TempDir::new("bob-cli-capture-budget-strict-start");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());
    write_file(&config, "plan:\n  strict: true\n");

    // A start that creates a fourth theme past the cap still succeeds.
    let output = capture_json(
        &vault,
        &day_file,
        &["@dev:startid#FRESH=", "Start", "work."],
        Some(&config),
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["creates_pomodoro"], true);
    assert!(json.get("plan_budget").is_some(), "{json}");
}

#[test]
fn capture_complete_create_row_reports_plan_themes() {
    let temp = TempDir::new("bob-cli-complete-plan-themes");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Dev\n## Tasks\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] () — GOALS\n",
            "- [ ] () — DECKS\n",
            "## Later\n",
        ),
    );

    let draft = "Do work @dev:abc#NEWTHEME";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "pomodoro_name");
    let candidates = json["candidates"].as_array().expect("candidates array");
    let created = candidates
        .iter()
        .find(|candidate| candidate["creates_pomodoro"] == true)
        .expect("a creates_pomodoro row");
    assert_eq!(created["plan_themes_after"], 3);
    assert_eq!(created["plan_themes_cap"], 3);
    for candidate in candidates {
        if candidate["creates_pomodoro"] != true {
            assert!(candidate.get("plan_themes_after").is_none());
        }
    }
}

#[test]
fn capture_complete_start_name_again_row_reports_plan_themes() {
    let temp = TempDir::new("bob-cli-complete-start-name-again-themes");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Dev\n## Tasks\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "  - [[dev#^old]]\n",
            "- [ ] () — GOALS\n",
            "- [ ] () — DECKS\n",
            "- [ ] () — BOB\n",
            "## Later\n",
        ),
    );

    let draft = "=#";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "pomodoro_start_name");
    let candidates = json["candidates"].as_array().expect("candidates array");
    let again = candidates
        .iter()
        .find(|candidate| candidate["replacement"] == "plan")
        .expect("an again PLAN row");
    assert_eq!(again["creates_pomodoro"], true);
    assert_eq!(again["plan_themes_after"], 4);
    assert_eq!(again["plan_themes_cap"], 3);
    for candidate in candidates {
        if candidate["creates_pomodoro"] != true {
            assert!(
                candidate.get("plan_themes_after").is_none(),
                "{candidate}"
            );
            assert!(candidate.get("plan_themes_cap").is_none(), "{candidate}");
        }
    }
}

#[test]
fn capture_complete_start_name_new_row_reports_plan_themes() {
    let temp = TempDir::new("bob-cli-complete-start-name-new-themes");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Dev\n## Tasks\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "  - [[dev#^old]]\n",
            "- [ ] () — GOALS\n",
            "- [ ] () — DECKS\n",
            "- [ ] () — BOB\n",
            "## Later\n",
        ),
    );

    let draft = "=#fr";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "pomodoro_start_name");
    let candidates = json["candidates"].as_array().expect("candidates array");
    let created = candidates
        .iter()
        .find(|candidate| candidate["replacement"] == "fr")
        .expect("a new FR row");
    assert_eq!(created["creates_pomodoro"], true);
    assert_eq!(created["plan_themes_after"], 4);
    assert_eq!(created["plan_themes_cap"], 3);
}

#[test]
fn capture_strict_mode_never_refuses_named_starts() {
    let temp = TempDir::new("bob-cli-capture-budget-strict-named");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());
    write_file(&config, "plan:\n  strict: true\n");

    let output =
        capture_json(&vault, &day_file, &["--", "=#fresh"], Some(&config));
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_start");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], true);
    assert_eq!(
        json["plan_budget"]["added_themes"],
        serde_json::json!(["FRESH"])
    );
    let warnings = json["plan_budget"]["warnings"]
        .as_array()
        .expect("budget warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "plan_theme_cap_exceeded"),
        "{json}"
    );
}

#[test]
fn capture_complete_create_row_omits_plan_themes_without_config() {
    let temp = TempDir::new("bob-cli-complete-plan-themes-bad-config");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("dev.md"), "# Dev\n## Tasks\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] () — GOALS\n",
            "## Later\n",
        ),
    );
    write_file(&config, "plan:\n  max_themes: 0\n");

    let draft = "Do work @dev:abc#NEWTHEME";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    let candidates = json["candidates"].as_array().expect("candidates array");
    let created = candidates
        .iter()
        .find(|candidate| candidate["creates_pomodoro"] == true)
        .expect("a creates_pomodoro row");
    assert!(created.get("plan_themes_after").is_none());
    assert!(created.get("plan_themes_cap").is_none());
}

#[test]
fn capture_without_ledger_change_has_no_plan_budget() {
    let temp = TempDir::new("bob-cli-capture-budget-quiet");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());

    let output = capture_json(
        &vault,
        &day_file,
        &["Plain", "work", "without", "pomodoro.", "@dev"],
        None,
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert!(json.get("plan_budget").is_none(), "{json}");
}

#[test]
fn capture_invalid_config_without_ledger_change_has_no_warning() {
    let temp = TempDir::new("bob-cli-capture-budget-quiet-bad-config");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing\n");
    write_file(&day_file, three_theme_day());
    write_file(&config, "plan:\n  max_themes: 0\n");

    let output = capture_json(
        &vault,
        &day_file,
        &["Plain", "work", "without", "pomodoro.", "@dev"],
        Some(&config),
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert!(json.get("plan_budget").is_none(), "{json}");
    let warnings = json["warnings"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    assert!(
        !warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("plan budget")),
        "non-ledger capture must not warn about the plan config: {json}"
    );
}

#[test]
fn capture_strict_mode_refuses_toggle_path() {
    let temp = TempDir::new("bob-cli-capture-strict-toggle");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_file(&target, "# Dev\n## Tasks\n- [ ] #task Existing ^newid\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] () — GOALS\n",
            "  - [[dev#^aaa]]\n",
            "  - [[dev#^newid]]\n",
            "- [ ] () — DECKS\n",
            "  - [[dev#^bbb]]\n",
            "- [ ] () — BOB\n",
            "  - [[dev#^ccc]]\n",
            "## Later\n",
        ),
    );
    write_file(&config, "plan:\n  strict: true\n");
    write_toggle_task_settings(&vault);

    let output =
        capture_json(&vault, &day_file, &["@dev+newid#NEWT"], Some(&config));
    assert_eq!(
        output.status.code(),
        Some(1),
        "toggle should be refused: {}",
        format_output(&output)
    );
    let json = parse_json(&output);
    assert_eq!(json["code"], "plan_theme_cap_exceeded", "{json}");
}

#[test]
fn capture_strict_mode_refuses_project_note_path() {
    let temp = TempDir::new("bob-cli-capture-strict-project-note");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day_file, three_theme_day());
    write_file(&config, "plan:\n  strict: true\n");

    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00")
            .env("BOB_CONFIG_FILE", &config),
        "Body @cash^human1+#NEWT\n- Draft the memo :draft-memo\n",
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "project-note should be refused: {}",
        format_output(&output)
    );
    let json = parse_json(&output);
    assert_eq!(json["code"], "plan_theme_cap_exceeded", "{json}");
}

#[test]
fn capture_parse_link_close_with_drop_reports_close() {
    for raw in ["^r:id=x~1", "Text @r:id=x~1"] {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(raw)
            .output()
            .expect("run bob capture-parse");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
        let close = &json["pomodoro_close"];
        assert!(close.is_object(), "{raw} should report a close: {json}");
        assert_eq!(close["drop"], serde_json::json!([1]), "{raw}: {json}");
    }
}

#[test]
fn capture_now_tag_writes_task_line_with_created_and_id() {
    let temp = TempDir::new("bob-cli-capture-now-e2e");
    let vault = temp.path().join("vault");
    let target = vault.join("dev.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Dev\n## Tasks\n");
    write_file(&day_file, "# 2026-07-10\n## Pomodoros\n");
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{"globalFilter":"#task","statusSettings":{"coreStatuses":[{"symbol":" ","name":"Todo","type":"TODO"},{"symbol":"x","name":"Done","type":"DONE"}],"customStatuses":[]}}"##,
    );

    let output = capture_json(
        &vault,
        &day_file,
        &["Ship", "the", "thing", "@dev", "#now"],
        None,
    );
    assert_success(&output);
    let json = parse_json(&output);
    let task_line = json["task_line"].as_str().expect("task line");
    assert!(
        task_line.starts_with("- [ ] #task Ship the thing #now [created::"),
        "task line should carry #task, #now and created stamp:\n{task_line}"
    );
    let body = fs::read_to_string(&target).expect("read target");
    assert!(
        body.contains("- [ ] #task Ship the thing #now [created::"),
        "written file should carry #task, #now and created stamp:\n{body}"
    );
}
