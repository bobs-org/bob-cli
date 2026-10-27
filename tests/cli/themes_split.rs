//! `bob themes split`: template, map-driven rewrite, and refusals.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

const NOW: &str = "2026-10-24 12:00:00";

fn split_vault(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: aqua\n---\n",
    );
    write_file(
        &vault.join("2026/20261020.md"),
        "## Pomodoros\n\
- [x] (0800-0830 30m) — BOB\n\
- [x] (0900-0930 30m) — JOB\n\
\t- [[done/job_done#^apply-warp]]\n\
- [x] (1000-1030 30m) — JOB + BUILD\n\
- [ ] () — JOB\n\
- [ ] () — JOB\n",
    );
    write_file(
        &vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — JOB\n- [x] (0800-0830 30m) — GTD\n",
    );
    write_file(
        &vault.join("2026/20261101.md"),
        "## Pomodoros\n\
- [ ] () — JOB\n\
\t- proofreading the cover letter draft for the next wave\n",
    );
    write_file(
        &vault.join("_templates/daily.md"),
        "## Pomodoros\n- [ ] () — JOB\n",
    );
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["config", "user.email", "t@t"]);
    git_in(&vault, ["config", "user.name", "t"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "init"]);
    (vault, cache)
}

fn split_command(vault: &Path, cache: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", cache)
        .env("HOSTNAME", "apollo")
        .env("BOB_CONFIG_FILE", cache.join("missing-bob-config.yml"));
    command
}

fn split_json(
    vault: &Path,
    cache: &Path,
    args: &[&str],
) -> (std::process::Output, Value) {
    let mut command = split_command(vault, cache);
    command
        .args(["themes", "split"])
        .args(args)
        .arg("-f")
        .arg("json");
    let output = command.output().expect("bob themes split");
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("split JSON");
    (output, value)
}

fn template_json(vault: &Path, cache: &Path, theme: &str) -> Value {
    let (output, value) = split_json(vault, cache, &[theme, "-t"]);
    assert_success(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["mode"], "template");
    value
}

fn tsv_from_entries(
    entries: &[Value],
    map_theme: impl Fn(usize, &Value) -> Option<String>,
) -> String {
    let mut out = String::from("# DATE\tREF\tTHEME\tNOTE\n");
    for (index, entry) in entries.iter().enumerate() {
        let Some(theme) = map_theme(index, entry) else {
            continue;
        };
        out.push_str(&format!(
            "{}\t{}\t{theme}\t{}\n",
            entry["date"].as_str().unwrap(),
            entry["ref"].as_str().unwrap(),
            entry["name"].as_str().unwrap_or(""),
        ));
    }
    out
}

fn write_map(vault: &Path, contents: &str) -> std::path::PathBuf {
    let path = vault.join("job.tsv");
    write_file(&path, contents);
    path
}

#[test]
fn wrong_vault_refuses_before_syncing_or_splitting() {
    let temp = TempDir::new("bob-cli-themes-split-wrong-vault");
    let (vault, _) = split_vault(temp.path());
    assert_vault_override_refused(
        bob_command().args(["themes", "split", "JOB", "-m", "job.tsv"]),
        &vault,
    );
}

#[test]
fn themes_split_template_lists_old_entries_and_round_trips_as_keep() {
    let temp = TempDir::new("bob-cli-themes-split-template");
    let (vault, cache) = split_vault(temp.path());

    let output = split_command(&vault, &cache)
        .args(["themes", "split", "JOB", "-t"])
        .output()
        .expect("template");
    assert_success(&output);
    let tsv = stdout(&output);
    assert!(tsv.contains("# DATE\tREF\tTHEME\tNOTE"), "{tsv}");
    assert!(tsv.contains("JOB"), "{tsv}");
    assert!(!tsv.contains("_templates/"), "{tsv}");
    assert!(!tsv.contains("BOB"), "{tsv}");
    assert!(tsv.contains("2026-11-01"), "future:\n{tsv}");
    assert!(tsv.contains("done/job_done#^apply-warp"), "link:\n{tsv}");
    assert!(
        tsv.contains("proofreading the cover letter draft for the next wave"),
        "plain child:\n{tsv}"
    );

    let value = template_json(&vault, &cache, "JOB");
    let entries = value["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 6, "{entries:?}");
    assert!(entries.iter().all(|entry| {
        entry["name"]
            .as_str()
            .is_some_and(|name| name.to_ascii_lowercase().contains("job"))
    }));
    let placeholders: Vec<&Value> = entries
        .iter()
        .filter(|entry| {
            entry["path"] == "2026/20261020.md"
                && entry["name"] == "JOB"
                && entry["time_range"].is_null()
        })
        .collect();
    assert_eq!(placeholders.len(), 2, "{entries:?}");
    assert_eq!(
        placeholders[0]["ref"]
            .as_str()
            .unwrap()
            .split_once(':')
            .unwrap()
            .1,
        placeholders[1]["ref"]
            .as_str()
            .unwrap()
            .split_once(':')
            .unwrap()
            .1,
        "identical placeholders share a digest"
    );
    assert_ne!(placeholders[0]["line"], placeholders[1]["line"]);

    let map = write_map(&vault, &tsv);
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&map), "-d"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unchanged");
}

#[test]
fn themes_split_dry_run_reports_flags_and_leaves_vault_untouched() {
    let temp = TempDir::new("bob-cli-themes-split-dry");
    let (vault, cache) = split_vault(temp.path());
    let value = template_json(&vault, &cache, "JOB");
    let entries = value["entries"].as_array().unwrap();
    let tsv = tsv_from_entries(entries, |index, entry| {
        match (index, entry["name"].as_str().unwrap_or("")) {
            (_, "JOB + BUILD") => Some("BUILD".into()),
            (0, _) => Some("CHORE".into()),
            (2, _) => Some("WRITE".into()),
            (3, _) => Some("PLAN".into()),
            (4, _) => Some("JOB".into()),
            _ => None,
        }
    });
    let map = write_map(&vault, &tsv);
    let before_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    let before_day =
        fs::read_to_string(vault.join("2026/20261020.md")).unwrap();

    let output = split_command(&vault, &cache)
        .args(["themes", "split", "JOB", "-m", path_str(&map), "-d"])
        .output()
        .expect("dry run");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("CHORE"), "{text}");
    assert!(text.contains("WRITE"), "{text}");
    assert!(text.contains("new"), "{text}");
    assert!(text.contains("exempt"), "{text}");
    assert!(text.contains("kept"), "{text}");
    assert!(text.contains("unmapped"), "{text}");

    let (_, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&map), "-d"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["mode"], "preview");
    assert_eq!(value["kept"], 1);
    assert_eq!(value["unmapped"], 1);
    let targets = value["targets"].as_array().unwrap();
    let write = targets
        .iter()
        .find(|target| target["name"] == "WRITE")
        .expect("WRITE");
    assert_eq!(write["new"], true);
    let plan = targets
        .iter()
        .find(|target| target["name"] == "PLAN")
        .expect("PLAN");
    assert_eq!(plan["exempt"], true);

    assert_eq!(
        fs::read_to_string(vault.join("2026/20261020.md")).unwrap(),
        before_day
    );
    assert_eq!(stdout(&git_in(&vault, ["rev-parse", "HEAD"])), before_head);
    assert!(!vault.join(".bob/themes/apollo.jsonl").exists());
}

#[test]
fn themes_split_write_commits_without_journal_and_invalidates_cache() {
    let temp = TempDir::new("bob-cli-themes-split-write");
    let (vault, cache) = split_vault(temp.path());
    let value = template_json(&vault, &cache, "JOB");
    let entries = value["entries"].as_array().unwrap();
    let tsv = tsv_from_entries(entries, |_, entry| {
        Some(match entry["name"].as_str().unwrap_or("") {
            "JOB + BUILD" => "BUILD".into(),
            _ => "CHORE".into(),
        })
    });
    let map = write_map(&vault, &tsv);

    let output = split_command(&vault, &cache)
        .args(["themes", "split", "JOB", "-m", path_str(&map), "-o"])
        .output()
        .expect("write");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("committed"), "{text}");
    assert!(
        text.contains("bob themes split: JOB → BUILD, CHORE"),
        "{text}"
    );
    assert!(
        text.contains("JOB no longer appears in any daily note"),
        "{text}"
    );
    assert!(!vault.join(".bob/themes/apollo.jsonl").exists());

    let day20 = fs::read_to_string(vault.join("2026/20261020.md")).unwrap();
    assert!(day20.contains("— CHORE\n"), "{day20}");
    assert!(day20.contains("— BUILD\n"), "{day20}");
    assert!(!day20.contains("JOB"), "{day20}");
    assert!(day20.contains("[[done/job_done#^apply-warp]]"), "{day20}");
    let template =
        fs::read_to_string(vault.join("_templates/daily.md")).unwrap();
    assert!(template.contains("— JOB"), "{template}");

    let log = git_in(&vault, ["log", "--oneline"]);
    assert_eq!(stdout(&log).lines().count(), 2);
    let show = git_in(&vault, ["show", "--stat", "--oneline", "HEAD"]);
    let stat = stdout(&show);
    assert!(stat.contains("2026/20261020.md"), "{stat}");
    assert!(!stat.contains(".bob/themes/"), "{stat}");

    let listed = split_command(&vault, &cache)
        .args(["themes", "-a", "-f", "json"])
        .output()
        .expect("list");
    assert_success(&listed);
    let value: Value = serde_json::from_str(stdout(&listed).trim()).unwrap();
    let keys: Vec<&str> = value["themes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|theme| theme["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"chore"), "{keys:?}");
    assert!(keys.contains(&"build"), "{keys:?}");
    assert!(!keys.contains(&"job"), "{keys:?}");
}

#[test]
fn themes_split_refusals_are_json_on_nonzero_exit() {
    let temp = TempDir::new("bob-cli-themes-split-refusals");
    let (vault, cache) = split_vault(temp.path());
    let template = template_json(&vault, &cache, "JOB");
    let entries = template["entries"].as_array().unwrap();
    let job_ref = entries[0]["ref"].as_str().unwrap();
    let job_date = entries[0]["date"].as_str().unwrap();

    let (output, value) = split_json(&vault, &cache, &["NOPE", "-t"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "unknown_theme");

    let short = write_map(&vault, &format!("{job_date}\t{job_ref}\n"));
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&short), "-d"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["code"], "invalid_map");

    let bad_date =
        write_map(&vault, &format!("13-10-2026\t{job_ref}\tCHORE\n"));
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&bad_date), "-d"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["code"], "invalid_map");

    let missing = write_map(&vault, &format!("2020-01-01\t{job_ref}\tCHORE\n"));
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&missing), "-d"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["code"], "invalid_map");

    let dup = write_map(
        &vault,
        &format!(
            "{job_date}\t{job_ref}\tCHORE\n{job_date}\t{job_ref}\tWRITE\n"
        ),
    );
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&dup), "-d"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["code"], "invalid_map");

    let plus = write_map(&vault, &format!("{job_date}\t{job_ref}\tA+B\n"));
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&plus), "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "invalid_name");

    let blank = write_map(&vault, &format!("{job_date}\t{job_ref}\t\n"));
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&blank), "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "invalid_name");

    let mut stale_digest = job_ref.to_string();
    stale_digest.replace_range(stale_digest.len() - 1.., "0");
    if stale_digest == job_ref {
        stale_digest.replace_range(stale_digest.len() - 1.., "1");
    }
    let stale =
        write_map(&vault, &format!("{job_date}\t{stale_digest}\tCHORE\n"));
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&stale), "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "stale_entry");
    assert_eq!(value["hint"], "regenerate with -t");

    let bob = template_json(&vault, &cache, "BOB");
    let bob_entry = &bob["entries"][0];
    let no_old = write_map(
        &vault,
        &format!(
            "{}\t{}\tCHORE\n",
            bob_entry["date"].as_str().unwrap(),
            bob_entry["ref"].as_str().unwrap()
        ),
    );
    let (output, value) =
        split_json(&vault, &cache, &["JOB", "-m", path_str(&no_old), "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(value["code"], "stale_entry");
}

#[test]
fn themes_split_stdin_map_works() {
    let temp = TempDir::new("bob-cli-themes-split-stdin");
    let (vault, cache) = split_vault(temp.path());
    let tsv = stdout(
        &split_command(&vault, &cache)
            .args(["themes", "split", "JOB", "-t"])
            .output()
            .expect("template"),
    );
    let mut child = split_command(&vault, &cache)
        .args(["themes", "split", "JOB", "-m", "-", "-d", "-f", "json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin split");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(tsv.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("stdin split");
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("stdin JSON");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(value["code"], "unchanged");
}
