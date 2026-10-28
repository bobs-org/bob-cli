//! Structured `theme_not_configured` warnings on `bob capture`.

use crate::support::*;
use serde_json::Value;
use std::path::Path;

fn write_vocab(config: &Path) {
    write_file(
        config,
        concat!(
            "plan:\n  max_themes: 30\n",
            "themes:\n  BUILD: yellow\n  PLAN: blue\n",
            "properties:\n",
            "  - name: scheduled\n",
            "    values: date\n",
            "  - name: dependsOn\n",
            "    values: local_task_id\n",
            "  - name: priority\n",
            "    values: priority\n",
            "    schedules: scheduled\n",
            "    levels:\n",
            "      - label: P1\n",
            "        value: high\n",
            "        min_days: 2\n",
            "        max_days: 7\n",
        ),
    );
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    config: &Path,
    args: &[&str],
) -> Value {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault).args([
        "--dry-run",
        "--no-clip",
        "-f",
        "json",
    ]);
    for arg in args {
        command.arg("--").arg(arg);
    }
    command
        .env("BOB_DIR", vault)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", "2026-10-24 08:00:00")
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .env("XDG_CACHE_HOME", vault.join("cache"));
    let output = command.output().expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn warning_codes(json: &Value) -> Vec<String> {
    json.get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|warning| {
            warning
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

#[test]
fn capture_warns_on_unconfigured_start_again_and_name_field() {
    let temp = TempDir::new("bob-cli-capture-theme-warn-start");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    let day_file = vault.join("2026/20261024.md");
    write_capture_task_settings(&vault);
    write_vocab(&config);
    write_file(&vault.join("sase.md"), "- [ ] #task Work ^foo1\n");
    write_file(&day_file, "## Pomodoros\n- [x] () — SNACK\n");

    let start = capture_json(&vault, &day_file, &config, &["=SNACK"]);
    assert_eq!(warning_codes(&start), ["theme_not_configured"]);
    assert_eq!(start["warnings"][0]["label"], "SNACK");
    assert!(
        start["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("SNACK isn't one of your configured themes"),
        "{start}"
    );

    let again = capture_json(&vault, &day_file, &config, &["=#snack"]);
    assert_eq!(warning_codes(&again), ["theme_not_configured"]);

    let named = capture_json(&vault, &day_file, &config, &["@sase:foo1#SNACK"]);
    assert_eq!(warning_codes(&named), ["theme_not_configured"]);

    let configured = capture_json(&vault, &day_file, &config, &["=BUILD"]);
    assert!(warning_codes(&configured).is_empty(), "{configured}");
}

#[test]
fn capture_skips_close_link_inventory_and_unconfigured_vocab() {
    let temp = TempDir::new("bob-cli-capture-theme-warn-skip");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    let day_file = vault.join("2026/20261024.md");
    write_capture_task_settings(&vault);
    write_vocab(&config);
    write_file(&vault.join("sase.md"), "- [ ] #task Work ^foo1\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0800-0825** [t:: 25m]) — SNACK\n\t- [[sase#^foo1]]\n- [ ] () — BUILD\n",
    );

    let close = capture_json(&vault, &day_file, &config, &["x"]);
    assert!(warning_codes(&close).is_empty(), "{close}");

    let link = capture_json(&vault, &day_file, &config, &["@sase:foo1#snack"]);
    assert!(warning_codes(&link).is_empty(), "{link}");

    let inventory = capture_json(&vault, &day_file, &config, &["=LATER"]);
    assert!(warning_codes(&inventory).is_empty(), "{inventory}");

    let none = temp.path().join("none.yml");
    write_file(&none, "plan:\n  max_themes: 30\n");
    let unconfigured_vocab =
        capture_json(&vault, &day_file, &none, &["=CHORE"]);
    assert!(
        warning_codes(&unconfigured_vocab).is_empty(),
        "{unconfigured_vocab}"
    );
}

#[test]
fn capture_deferred_priority_still_warns_when_theme_grows() {
    let temp = TempDir::new("bob-cli-capture-theme-warn-defer");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    let day_file = vault.join("2026/20261024.md");
    write_capture_task_settings(&vault);
    write_vocab(&config);
    write_file(&vault.join("sase.md"), "- [ ] #task Work ^foo1\n");
    write_file(&day_file, "## Pomodoros\n");

    // `$<N>` still writes the unconfigured name on the item, but a
    // future schedule is link-guarded, so today's SNACK count does not
    // grow and the warning stays off. Pair it with `s:0` so the Task
    // Link (and the new SNACK entry) still land.
    let deferred = capture_json(
        &vault,
        &day_file,
        &config,
        &["Do work @sase:bar1#SNACK $1 s:0"],
    );
    assert_eq!(
        warning_codes(&deferred),
        ["theme_not_configured"],
        "{deferred}"
    );
}
