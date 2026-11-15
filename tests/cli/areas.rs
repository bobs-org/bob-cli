//! `bob areas list` and `check` contract tests.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn vault_fixture(name: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    (temp, vault)
}

fn areas_command(vault: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("areas")
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1");
    command
}

fn write_note(vault: &Path, name: &str, contents: &str) {
    write_file(&vault.join(name), contents);
}

fn area_note(parent: &str, extra: &str, title: &str) -> String {
    format!(
        "---\ntype: \"[[area]]\"\nparent: \"[[{parent}]]\"\n{extra}---\n\n# {title}\n"
    )
}

fn tree_vault(vault: &Path) {
    write_note(vault, "area.md", "# Area root\n");
    write_note(vault, "body.md", &area_note("area", "", "Body"));
    write_note(
        vault,
        "cash.md",
        &area_note(
            "area",
            "color: magenta\nsummary: Money and bills.\n",
            "Cash",
        ),
    );
    write_note(vault, "bank.md", &area_note("cash", "", "Bank"));
}

#[test]
fn areas_help_lists_list_and_check() {
    let output = bob_command()
        .args(["areas", "--help"])
        .output()
        .expect("run bob areas --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Bare `bob areas` runs the read-only list")
            && help.contains("docs/areas.md")
            && help.contains("check")
            && help.contains("list")
            && help.contains("sync")
            && help.contains("(default)"),
        "expected areas group help:\n{help}"
    );
    assert_text_order(&help, &["\n  check ", "\n  list ", "\n  sync "]);

    let list = bob_command()
        .args(["areas", "list", "--help"])
        .output()
        .expect("run bob areas list --help");
    assert_success(&list);
    let list_help = stdout(&list);
    assert!(
        list_help.contains("-b, --bob-dir")
            && list_help.contains("-f, --format")
            && list_help.contains("-h, --help")
            && list_help.contains("docs/areas.md")
            && list_help.contains("always exits 0"),
        "expected list help:\n{list_help}"
    );
    assert_text_order(
        &list_help,
        &["-b, --bob-dir", "-f, --format", "-h, --help"],
    );

    let check = bob_command()
        .args(["areas", "check", "--help"])
        .output()
        .expect("run bob areas check --help");
    assert_success(&check);
    let check_help = stdout(&check);
    assert!(
        check_help.contains("Exits 1 on any error finding")
            && check_help.contains("docs/areas.md"),
        "expected check help:\n{check_help}"
    );

    let sync = bob_command()
        .args(["areas", "sync", "--help"])
        .output()
        .expect("run bob areas sync --help");
    assert_success(&sync);
    let sync_help = stdout(&sync);
    assert!(
        sync_help.contains("-b, --bob-dir")
            && sync_help.contains("-d, --dry-run")
            && sync_help.contains("-f, --format")
            && sync_help.contains("-h, --help")
            && sync_help.contains("docs/areas.md")
            && sync_help.contains("areas: sync catalogs"),
        "expected sync help:\n{sync_help}"
    );
    assert_text_order(
        &sync_help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
        ],
    );
}

#[test]
fn areas_bare_defaults_to_list() {
    let (_temp, vault) = vault_fixture("areas-bare");
    tree_vault(&vault);
    let bare = areas_command(&vault).output().expect("bare");
    let explicit = areas_command(&vault)
        .arg("list")
        .output()
        .expect("explicit");
    assert!(bare.status.success(), "{}", format_output(&bare));
    assert!(explicit.status.success(), "{}", format_output(&explicit));
    assert_eq!(stdout(&bare), stdout(&explicit));
    let text = stdout(&bare);
    assert!(
        text.contains("Areas · 3 areas · 2 catalogs")
            && text.contains("body")
            && text.contains("cash")
            && text.contains("bank")
            && text.contains("├─")
            && text.contains("└─"),
        "text:\n{text}"
    );
}

#[test]
fn areas_list_json_golden_shape() {
    let (_temp, vault) = vault_fixture("areas-json");
    tree_vault(&vault);
    let output = areas_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("run list json");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse json");
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "list");
    assert_eq!(payload["root"]["path"], "area.md");
    assert_eq!(payload["root"]["stem"], "area");
    assert_eq!(payload["root"]["state"], "found");
    let areas = payload["areas"].as_array().unwrap();
    assert_eq!(areas.len(), 3);
    assert_eq!(areas[0]["stem"], "body");
    assert_eq!(areas[0]["depth"], 1);
    assert_eq!(areas[1]["stem"], "cash");
    assert_eq!(areas[1]["path"], "cash.md");
    assert_eq!(areas[1]["link"], "cash");
    assert_eq!(areas[1]["title"], "Cash");
    assert_eq!(areas[1]["titled"], true);
    assert_eq!(areas[1]["summary"], "Money and bills.");
    assert_eq!(areas[1]["parent"], "area.md");
    assert_eq!(areas[1]["depth"], 1);
    assert_eq!(areas[1]["top"], "cash");
    assert_eq!(areas[1]["hue"], "magenta");
    assert_eq!(areas[1]["hue_source"], "color");
    assert_eq!(areas[1]["children"], serde_json::json!(["bank.md"]));
    assert_eq!(areas[2]["stem"], "bank");
    assert_eq!(areas[2]["parent"], "cash.md");
    assert_eq!(areas[2]["depth"], 2);
    assert_eq!(areas[2]["top"], "cash");
    let catalogs = payload["catalogs"].as_array().unwrap();
    assert_eq!(catalogs.len(), 2);
    assert_eq!(catalogs[0]["host"], "area");
    assert_eq!(catalogs[0]["kind"], "root");
    assert_eq!(catalogs[0]["state"], "missing");
    assert_eq!(catalogs[1]["host"], "cash");
    assert_eq!(catalogs[1]["kind"], "area");
    assert_eq!(catalogs[1]["state"], "missing");
    assert_eq!(payload["counts"]["areas"], 3);
    assert_eq!(payload["counts"]["top_level"], 2);
    assert_eq!(payload["counts"]["hosts"], 2);
    assert_eq!(payload["counts"]["errors"], 0);
    assert!(payload["counts"]["warnings"].as_u64().unwrap() >= 2);
}

#[test]
fn areas_check_exits_zero_when_clean() {
    let (_temp, vault) = vault_fixture("areas-clean");
    write_note(&vault, "area.md", "# Area root\n");
    let output = areas_command(&vault)
        .arg("check")
        .output()
        .expect("check clean");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert!(text.contains("areas match the contract"), "text:\n{text}");
}

#[test]
fn areas_check_exits_one_on_error_finding() {
    let (_temp, vault) = vault_fixture("areas-error");
    write_note(&vault, "area.md", "# Area root\n");
    write_note(
        &vault,
        "orphan.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Orphan\n",
    );
    let output = areas_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check error");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "check");
    let codes: Vec<&str> = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"area_parent_missing"), "codes: {codes:?}");
}

#[test]
fn areas_check_exits_one_on_damaged_catalog() {
    let (_temp, vault) = vault_fixture("areas-damaged");
    write_note(
        &vault,
        "area.md",
        "# Area root\n\n## Areas\n\n> [!areas\n> - partial\n",
    );
    write_note(&vault, "cash.md", &area_note("area", "", "Cash"));
    let output = areas_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check damaged");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    let catalogs = payload["catalogs"].as_array().unwrap();
    assert!(
        catalogs.iter().any(|catalog| catalog["state"] == "damaged"
            && catalog["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["code"] == "area_catalog_damaged")),
        "catalogs: {catalogs:?}"
    );
}

#[test]
fn areas_check_exits_zero_with_only_warnings() {
    let (_temp, vault) = vault_fixture("areas-warnings");
    tree_vault(&vault);
    let output = areas_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check warnings");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], true);
    assert!(payload["counts"]["errors"].as_u64().unwrap() == 0);
    assert!(payload["counts"]["warnings"].as_u64().unwrap() > 0);
    let codes: Vec<&str> = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"area_catalog_stale"), "codes: {codes:?}");
}

#[test]
fn areas_list_exits_zero_with_error_findings() {
    let (_temp, vault) = vault_fixture("areas-list-errors");
    write_note(&vault, "area.md", "# Area root\n");
    write_note(
        &vault,
        "orphan.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Orphan\n",
    );
    let output = areas_command(&vault)
        .arg("list")
        .output()
        .expect("list with errors");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert!(text.contains("Unplaced"), "text:\n{text}");
    assert!(text.contains("area_parent_missing"), "text:\n{text}");
}

#[test]
fn areas_vault_bytes_unchanged_by_list_and_check() {
    let (_temp, vault) = vault_fixture("areas-readonly");
    tree_vault(&vault);
    let before: Vec<(PathBuf, String)> = fs::read_dir(&vault)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let contents = fs::read_to_string(&path).unwrap();
            (path, contents)
        })
        .collect();
    for args in [&["list"][..], &["check"][..], &["list", "-f", "json"][..]] {
        let mut command = areas_command(&vault);
        command.args(args);
        let output = command.output().expect("run");
        assert!(
            output.status.success() || args[0] == "check",
            "{}",
            format_output(&output)
        );
    }
    for (path, contents) in before {
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }
}

// ---------------------------------------------------------------- sync

fn sync_vault(temp: &TempDir, name: &str) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    let _ = name;
    write_realistic_tree(&vault);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "areas fixture"]);
    git_in(&vault, ["push", "-q"]);
    vault
}

fn write_realistic_tree(vault: &Path) {
    write_note(vault, "area.md", "# Area root\n");
    for (stem, parent) in [
        ("body", "area"),
        ("cash", "area"),
        ("bank", "cash"),
        ("recur", "cash"),
        ("dev", "area"),
        ("gtd", "area"),
        ("gtd_daily", "gtd"),
        ("inbox", "gtd"),
        ("gkeep_inbox", "inbox"),
        ("gmail_inbox", "inbox"),
        ("mac_inbox", "inbox"),
        ("job", "area"),
        ("love", "area"),
        ("abby", "love"),
        ("family", "love"),
        ("fun", "love"),
        ("kelly", "love"),
        ("vacation", "love"),
        ("meet", "area"),
    ] {
        write_note(vault, &format!("{stem}.md"), &area_note(parent, "", stem));
    }
}

fn sync_run(
    vault: &Path,
    temp: &TempDir,
    args: &[&str],
) -> std::process::Output {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("create home");
    let mut command = bob_command();
    command
        .arg("areas")
        .args(args)
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env("TZ", "UTC")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    command.output().expect("run bob areas")
}

fn sync_json(output: &std::process::Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!(
            "one JSON envelope expected ({error}):\n{}",
            format_output(output)
        )
    })
}

fn vault_head(vault: &Path) -> String {
    stdout(&git_in(vault, ["rev-parse", "HEAD"]))
        .trim()
        .to_string()
}

fn vault_subject(vault: &Path) -> String {
    stdout(&git_in(vault, ["log", "-1", "--format=%s"]))
        .trim()
        .to_string()
}

fn commits_since(vault: &Path, base: &str) -> usize {
    stdout(&git_in(
        vault,
        ["rev-list", "--count", &format!("{base}..HEAD")],
    ))
    .trim()
    .parse()
    .expect("count")
}

fn read_note(vault: &Path, name: &str) -> String {
    fs::read_to_string(vault.join(name)).expect("read note")
}

const SYNC_HOSTS: [&str; 5] =
    ["area.md", "cash.md", "gtd.md", "inbox.md", "love.md"];

#[test]
fn areas_sync_writes_five_hosts_in_one_commit_then_noops() {
    let temp = TempDir::new("areas-sync-once");
    let vault = sync_vault(&temp, "areas-sync-once");
    let base = vault_head(&vault);

    let output = sync_run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    let payload = sync_json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["dry_run"], false);
    assert_eq!(
        payload["changed"],
        serde_json::json!(SYNC_HOSTS),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        payload["commit"]["subject"],
        "areas: sync catalogs (5 notes)"
    );
    assert_eq!(payload["sync"]["pre"], "ok");
    assert_eq!(payload["sync"]["post"], "ok");
    assert_eq!(commits_since(&vault, &base), 1, "one catalog commit");
    assert_eq!(vault_subject(&vault), "areas: sync catalogs (5 notes)");

    let area = read_note(&vault, "area.md");
    for row in [
        "> - **[[body]]**",
        "> - **[[cash]]**",
        ">   - **[[bank]]**",
        ">   - **[[recur]]**",
        "> - **[[love]]**",
        ">   - **[[abby]]**",
    ] {
        assert!(area.contains(row), "area.md missing {row}:\n{area}");
    }
    let love = read_note(&vault, "love.md");
    assert!(love.contains("## Areas"), "{love}");
    assert!(love.contains("> [!areas]"), "{love}");
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["show", "--name-only", "--format=", "HEAD"]
        ))
        .lines()
        .collect::<Vec<_>>(),
        SYNC_HOSTS
    );

    // The second run is a byte-for-byte no-op with no commit.
    let files: Vec<(String, String)> = SYNC_HOSTS
        .iter()
        .map(|name| ((*name).to_string(), read_note(&vault, name)))
        .collect();
    let synced_head = vault_head(&vault);
    let output = sync_run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    let payload = sync_json(&output);
    assert_eq!(payload["changed"], serde_json::json!([]));
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(vault_head(&vault), synced_head);
    for (name, before) in files {
        assert_eq!(read_note(&vault, &name), before);
    }
    let text = stdout(&sync_run(&vault, &temp, &["sync"]));
    assert!(text.contains("area catalogs are current"), "{text}");
}

#[test]
fn areas_sync_dry_run_prints_diffs_and_writes_nothing() {
    let temp = TempDir::new("areas-sync-dry");
    let vault = sync_vault(&temp, "areas-sync-dry");
    let head_before = vault_head(&vault);
    let area_before = read_note(&vault, "area.md");

    let output = sync_run(&vault, &temp, &["sync", "-d"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("+++ b/area.md"), "{text}");
    assert!(text.contains("> [!areas]"), "{text}");
    assert!(text.contains("dry run: 5 catalogs would change"), "{text}");

    let output = sync_run(&vault, &temp, &["sync", "--dry-run", "-f", "json"]);
    assert_success(&output);
    let payload = sync_json(&output);
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["commit"], Value::Null);
    let changes = payload["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 5);
    assert!(
        changes[0]["diff"]
            .as_str()
            .unwrap()
            .contains("+++ b/area.md"),
        "{changes:?}"
    );

    assert_eq!(vault_head(&vault), head_before);
    assert_eq!(read_note(&vault, "area.md"), area_before);
    assert_eq!(stdout(&git_in(&vault, ["status", "--porcelain"])), "");
}

#[test]
fn areas_sync_skips_damaged_host_and_applies_the_rest() {
    let temp = TempDir::new("areas-sync-damaged");
    let vault = sync_vault(&temp, "areas-sync-damaged");
    let damaged = format!("{}\n> [!areas\n", read_note(&vault, "area.md"));
    write_file(&vault.join("area.md"), &damaged);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "damage area"]);
    let base = vault_head(&vault);

    let output = sync_run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = sync_json(&output);
    assert_eq!(payload["ok"], false);
    let finding = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["code"] == "area_catalog_damaged")
        .expect("damaged finding");
    assert_eq!(finding["path"], "area.md");
    assert_eq!(
        read_note(&vault, "area.md"),
        damaged,
        "damaged host untouched"
    );
    assert_eq!(
        payload["changed"],
        serde_json::json!(["cash.md", "gtd.md", "inbox.md", "love.md"])
    );
    assert_eq!(commits_since(&vault, &base), 1);
    assert_eq!(vault_subject(&vault), "areas: sync catalogs (4 notes)");

    let output = sync_run(&vault, &temp, &["check", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn areas_sync_reparents_kelly_under_family_in_one_commit() {
    let temp = TempDir::new("areas-sync-reparent");
    let vault = sync_vault(&temp, "areas-sync-reparent");
    assert_success(&sync_run(&vault, &temp, &["sync"]));

    write_note(&vault, "kelly.md", &area_note("family", "", "kelly"));
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "reparent kelly"]);
    let base = vault_head(&vault);
    let output = sync_run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    let payload = sync_json(&output);
    assert_eq!(
        payload["changed"],
        serde_json::json!(["area.md", "family.md", "love.md"])
    );
    assert_eq!(commits_since(&vault, &base), 1);
    let family = read_note(&vault, "family.md");
    assert!(family.contains("[[kelly]]"), "{family}");
    let love = read_note(&vault, "love.md");
    assert!(love.contains("[[family]]"), "{love}");
    assert!(love.contains("[[kelly]]"), "{love}");
}

#[test]
fn areas_and_projects_catalogs_share_one_section_order() {
    for (name, first, second) in [
        ("areas-first", "areas", "projects"),
        ("projects-first", "projects", "areas"),
    ] {
        let temp = TempDir::new(name);
        let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
        write_note(&vault, "area.md", "# Area root\n");
        write_note(&vault, "cash.md", &area_note("area", "", "cash"));
        write_note(&vault, "bank.md", &area_note("cash", "", "bank"));
        write_note(
            &vault,
            "proj.md",
            "---\ntype: \"[[project]]\"\nparent: \"[[cash]]\"\nstatus: wip\n---\n\n- [ ] #task Finish #prj ^prj\n",
        );
        git_in(&vault, ["add", "-A"]);
        git_in(&vault, ["commit", "-q", "-m", "areas projects fixture"]);
        git_in(&vault, ["push", "-q"]);

        let home = temp.path().join("home-dir");
        fs::create_dir_all(&home).expect("create home");
        let run = |noun: &str| {
            bob_command()
                .arg(noun)
                .arg("sync")
                .env("BOB_DIR", &vault)
                .env("NO_COLOR", "1")
                .env("TZ", "UTC")
                .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(&temp))
                .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(&temp))
                .env("HOME", &home)
                .env("HOSTNAME", "Athena Test")
                .env("XDG_CACHE_HOME", temp.path().join("cache"))
                .output()
                .expect("run sync")
        };
        let first_out = run(first);
        assert_success(&first_out);
        let second_out = run(second);
        assert_success(&second_out);
        let cash = read_note(&vault, "cash.md");
        let areas_at = cash.find("## Areas").expect("areas heading:\n{cash}");
        let projects_at =
            cash.find("## Projects").expect("projects heading:\n{cash}");
        assert!(
            areas_at < projects_at,
            "## Areas must precede ## Projects in {name}:\n{cash}"
        );
        assert!(cash.contains("> [!areas]"), "{cash}");
        assert!(cash.contains("> [!projects]"), "{cash}");
    }
}

#[test]
fn areas_nightly_runs_the_areas_step_before_projects() {
    let temp = TempDir::new("areas-nightly");
    let vault = sync_vault(&temp, "areas-nightly");
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("create home");
    let output = bob_command()
        .arg("nightly")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(&temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(&temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly");
    assert_success(&output);
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "step 5/9 · areas — Repair generated area catalogs",
            "step 6/9 · projects",
        ],
    );
    assert!(
        out.contains("areas: sync catalogs (5 notes)")
            || out.contains("synced 5 catalogs"),
        "{out}"
    );
}
