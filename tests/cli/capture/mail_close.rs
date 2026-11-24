//! Last-gate Gmail Task close confirmation for `bob capture`.
//!
//! A real run that would close a Gmail Task refuses unless
//! `--mail-close DIGEST` matches. Dry-run reports the same `mail_close`
//! payload and writes nothing.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-11-24 12:00:00";
const MAIL_NOTE: &str = "mail/2026/yogi.md";
const MAIL: &str = concat!(
    "---\n",
    "type: \"[[mail]]\"\n",
    "content: untrusted-email\n",
    "gmail_thread_id: 1a0b9eb2cc26ab96\n",
    "title: Yogi - Senior SWE - Technical Screen\n",
    "who: Derrick Blakely\n",
    "turn: yours\n",
    "turn_since: 2026-11-23T12:00:00\n",
    "gmail_url: https://mail.google.com/mail/u/0/#inbox/FMfcgzQhWTsNRgxMhQNwhZDfgqwXpPsx\n",
    "---\n",
    "Private synthetic body\n",
);
const GMAIL_TASK: &str =
    "- [ ] #task Reply to Yogi screen [[mail/2026/yogi]] ^yogi\n";

struct Fixture {
    _temp: TempDir,
    vault: PathBuf,
    day: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        write_toggle_task_settings(&vault);
        fs::create_dir_all(vault.join("mail/2026")).expect("mail folder");
        write_file(&vault.join(MAIL_NOTE), MAIL);
        write_file(&vault.join("job.md"), GMAIL_TASK);
        let day = vault.join("20261124.md");
        write_file(&day, "## Pomodoros\n- [ ] (**0900-0930**) — MAIL\n");
        Self {
            _temp: temp,
            vault,
            day,
        }
    }

    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        vault_bytes(&self.vault)
    }

    fn run(&self, flags: &[&str], text: &str, human: bool) -> Output {
        let mut command = bob_command();
        command.arg("capture").arg("-b").arg(&self.vault);
        if human {
            command.env("NO_COLOR", "1");
        } else {
            command.arg("-f").arg("json");
        }
        command
            .args(flags)
            .arg("--")
            .arg(text)
            .env("BOB_DAY_FILE", &self.day)
            .env("BOB_NOW", NOW)
            .output()
            .expect("run bob capture")
    }

    fn json(&self, flags: &[&str], text: &str) -> Value {
        let output = self.run(flags, text, false);
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    }

    fn fail(&self, flags: &[&str], text: &str) -> (Output, Value) {
        let output = self.run(flags, text, false);
        assert!(!output.status.success(), "{}", format_output(&output));
        let json: Value =
            serde_json::from_str(stdout(&output).trim()).expect("error JSON");
        assert_eq!(json["ok"], false);
        (output, json)
    }
}

fn vault_bytes(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = fs::read_dir(&dir).expect("read vault dir");
        for entry in read {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir()
                && path.file_name().is_some_and(|name| name == ".obsidian")
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(vault)
                    .expect("relative")
                    .to_string_lossy()
                    .replace('\\', "/");
                entries.push((relative, fs::read(&path).expect("read file")));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn assert_guarded(json: &Value) {
    let mail = &json["mail_close"];
    assert!(mail.is_object(), "{json}");
    assert_eq!(mail["verb"], "complete");
    assert_eq!(mail["title"], "Complete this email task?");
    assert_eq!(mail["tone"], "warning");
    assert_eq!(mail["tasks"].as_array().unwrap().len(), 1);
    let task = &mail["tasks"][0];
    assert_eq!(task["path"], "job.md");
    assert_eq!(task["line"], 1);
    assert_eq!(task["block_id"], "yogi");
    assert_eq!(task["before"], " ");
    assert_eq!(task["after"], "x");
    assert_eq!(task["verb"], "complete");
    assert_eq!(task["mails"][0]["note"], MAIL_NOTE);
    assert_eq!(
        task["mails"][0]["title"],
        "Yogi - Senior SWE - Technical Screen"
    );
    let digest = mail["digest"].as_str().expect("digest");
    assert_eq!(digest.len(), 12);
    assert!(digest.chars().all(|ch| ch.is_ascii_hexdigit()));
    let confirm = mail["confirm"].as_str().expect("confirm");
    assert!(
        confirm.contains(&format!("--mail-close '{digest}'")),
        "{confirm}"
    );
}

fn digest_of(json: &Value) -> String {
    json["mail_close"]["digest"]
        .as_str()
        .expect("digest")
        .to_string()
}

#[test]
fn bang_complete_refuses_without_digest_and_writes_nothing() {
    let fx = Fixture::new("bob-cli-mail-close-bang-refuse");
    let before = fx.snapshot();
    let (output, json) = fx.fail(&[], "!job:yogi");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(json["code"], "mail_close_confirmation_required");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("pass --mail-close"),
        "{json}"
    );
    assert_guarded(&json);
    assert_eq!(fx.snapshot(), before, "refusal must write nothing");
}

#[test]
fn bang_complete_dry_run_reports_mail_close_and_writes_nothing() {
    let fx = Fixture::new("bob-cli-mail-close-bang-dry");
    let before = fx.snapshot();
    let json = fx.json(&["--dry-run"], "!job:yogi");
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], true);
    assert_guarded(&json);
    assert_eq!(fx.snapshot(), before, "dry-run must write nothing");
}

#[test]
fn bang_complete_matching_digest_writes() {
    let fx = Fixture::new("bob-cli-mail-close-bang-confirm");
    let digest = digest_of(&fx.json(&["--dry-run"], "!job:yogi"));
    let json = fx.json(&["--mail-close", &digest], "!job:yogi");
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["kind"], "task_complete");
    assert_eq!(json["mail_close"]["digest"], digest);
    let job = fs::read_to_string(fx.vault.join("job.md")).expect("job");
    assert!(job.contains("- [x] #task Reply to Yogi screen"), "{job}");
}

#[test]
fn bang_complete_mismatch_refuses_and_writes_nothing() {
    let fx = Fixture::new("bob-cli-mail-close-bang-mismatch");
    let before = fx.snapshot();
    let (output, json) =
        fx.fail(&["--mail-close", "deadbeefdead"], "!job:yogi");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert_eq!(json["code"], "mail_close_confirmation_mismatch");
    assert_guarded(&json);
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn digest_with_nothing_to_guard_is_ignored() {
    let fx = Fixture::new("bob-cli-mail-close-ignored-digest");
    let json = fx.json(&["--mail-close", "deadbeefdead"], "Buy milk @job");
    assert_eq!(json["ok"], true);
    assert!(json.get("mail_close").is_none(), "{json}");
    let job = fs::read_to_string(fx.vault.join("job.md")).expect("job");
    assert!(job.contains("Buy milk"), "{job}");
}

#[test]
fn non_email_complete_stays_unguarded() {
    let fx = Fixture::new("bob-cli-mail-close-non-email");
    write_file(&fx.vault.join("sase.md"), "- [ ] #task Ship parser ^ship\n");
    let json = fx.json(&[], "!sase:ship");
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "task_complete");
    assert!(json.get("mail_close").is_none(), "{json}");
}

#[test]
fn toggle_that_leaves_the_task_open_is_not_guarded() {
    let fx = Fixture::new("bob-cli-mail-close-toggle");
    let json = fx.json(&[], "@job+yogi!");
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "task_toggle");
    assert!(json.get("mail_close").is_none(), "{json}");
    let job = fs::read_to_string(fx.vault.join("job.md")).expect("job");
    assert!(job.contains("- [*] #task"), "{job}");
}

#[test]
fn same_capture_create_and_close_is_exempt() {
    let fx = Fixture::new("bob-cli-mail-close-same-capture");
    let json = fx.json(
        &[],
        "@job^follow Follow up [[mail/2026/yogi]]\n\n!job:follow",
    );
    assert_eq!(json["ok"], true);
    assert!(json.get("mail_close").is_none(), "{json}");
    let job = fs::read_to_string(fx.vault.join("job.md")).expect("job");
    assert!(
        job.contains("- [x] #task Follow up [[mail/2026/yogi]]"),
        "{job}"
    );
}

#[test]
fn all_or_nothing_refusal_leaves_other_items_unwritten() {
    let fx = Fixture::new("bob-cli-mail-close-all-or-nothing");
    let before = fx.snapshot();
    let (_, json) = fx.fail(&[], "!job:yogi\n\nBuy milk @job");
    assert_eq!(json["code"], "mail_close_confirmation_required");
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn complete_offer_guards_a_successor_gmail_task() {
    let fx = Fixture::new("bob-cli-mail-close-complete-offer");
    write_file(
        &fx.vault.join("sase.md"),
        "- [ ] #task Bar [id:: bar] ^bar\n",
    );
    write_file(
        &fx.vault.join("job.md"),
        concat!(
            "- [?] #task Reply to Yogi screen [[mail/2026/yogi]] ",
            "[dependsOn:: bar] ^yogi\n",
        ),
    );
    let dry = fx.json(&["--dry-run", "-x", "1"], "!sase:bar");
    assert_eq!(dry["mail_close"]["verb"], "complete");
    assert_eq!(dry["mail_close"]["tasks"][0]["path"], "job.md");
    assert_eq!(dry["mail_close"]["tasks"][0]["before"], "?");
    assert_eq!(dry["mail_close"]["tasks"][0]["after"], "x");
    let before = fx.snapshot();
    let (_, json) = fx.fail(&["-x", "1"], "!sase:bar");
    assert_eq!(json["code"], "mail_close_confirmation_required");
    assert_eq!(fx.snapshot(), before);
    let digest = digest_of(&dry);
    let real = fx.json(&["-x", "1", "--mail-close", &digest], "!sase:bar");
    assert_eq!(real["ok"], true);
    let job = fs::read_to_string(fx.vault.join("job.md")).expect("job");
    assert!(job.contains("- [x] #task Reply to Yogi screen"), "{job}");
}

#[test]
fn cancel_offer_guards_a_successor_gmail_task() {
    let fx = Fixture::new("bob-cli-mail-close-cancel-offer");
    write_file(
        &fx.vault.join("sase.md"),
        "- [ ] #task Bar [id:: bar] ^bar\n",
    );
    write_file(
        &fx.vault.join("job.md"),
        concat!(
            "- [?] #task Reply to Yogi screen [[mail/2026/yogi]] ",
            "[dependsOn:: bar] ^yogi\n",
        ),
    );
    let dry = fx.json(&["--dry-run", "-X", "1"], "!sase:bar");
    let mail = &dry["mail_close"];
    assert_eq!(mail["verb"], "cancel");
    assert_eq!(mail["title"], "Cancel this email task?");
    assert_eq!(mail["tasks"][0]["after"], "-");
    assert_eq!(mail["tasks"][0]["verb"], "cancel");
    let digest = digest_of(&dry);
    let real = fx.json(&["-X", "1", "--mail-close", &digest], "!sase:bar");
    assert_eq!(real["ok"], true);
    let job = fs::read_to_string(fx.vault.join("job.md")).expect("job");
    assert!(job.contains("- [-] #task Reply to Yogi screen"), "{job}");
}

#[test]
fn relation_complete_guards_the_source_gmail_task() {
    let fx = Fixture::new("bob-cli-mail-close-relation-complete");
    let config = fx.vault.join("config.yml");
    write_file(
        &config,
        "relations:\n  complete_labels:\n    - implemented by\n",
    );
    write_file(
        &fx.vault.join("dev.md"),
        "- [ ] #task Sell stocks now ^sell-now\n",
    );
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&fx.vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("@job+yogi ~dev:sell-now|\"implemented by\"")
        .env("BOB_DAY_FILE", &fx.day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("relation complete dry-run");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("JSON");
    assert_guarded(&json);
}

#[test]
fn relation_cancel_guards_the_source_gmail_task() {
    let fx = Fixture::new("bob-cli-mail-close-relation-cancel");
    let config = fx.vault.join("config.yml");
    write_file(
        &config,
        "relations:\n  cancel_labels:\n    - duplicate of\n",
    );
    write_file(
        &fx.vault.join("dev.md"),
        "- [ ] #task Sell stocks now ^sell-now\n",
    );
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&fx.vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("@job+yogi ~dev:sell-now|\"duplicate of\"")
        .env("BOB_DAY_FILE", &fx.day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("relation cancel dry-run");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("JSON");
    assert_eq!(json["mail_close"]["verb"], "cancel");
    assert_eq!(json["mail_close"]["tasks"][0]["after"], "-");
}

#[test]
fn pomodoro_x_bang_guards_linked_gmail_task() {
    let fx = Fixture::new("bob-cli-mail-close-pomodoro");
    write_file(
        &fx.day,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — MAIL\n",
            "  - [[job#^yogi]]\n",
        ),
    );
    let json = fx.json(&["--dry-run"], "x!");
    assert_guarded(&json);
    let before = fx.snapshot();
    let (_, refused) = fx.fail(&[], "x!");
    assert_eq!(refused["code"], "mail_close_confirmation_required");
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn merge_retirement_is_exempt() {
    let fx = Fixture::new("bob-cli-mail-close-merge");
    write_file(
        &fx.vault.join("inbox.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Existing target ^t\n",
        ),
    );
    write_file(
        &fx.vault.join("job.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Reply to Yogi screen [[mail/2026/yogi]] ^yogi\n",
        ),
    );
    let json = fx.json(&[], "@job+yogi > @inbox+t");
    assert_eq!(json["ok"], true);
    assert!(json.get("mail_close").is_none(), "{json}");
    let job = fs::read_to_string(fx.vault.join("job.md")).expect("job");
    assert!(job.contains("merged into"), "{job}");
}

#[test]
fn human_refusal_prints_the_card_and_writes_nothing() {
    let fx = Fixture::new("bob-cli-mail-close-human");
    let before = fx.snapshot();
    let output = fx.run(&[], "!job:yogi", true);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let err = stderr(&output);
    let out = stdout(&output);
    assert!(
        err.contains("pass --mail-close"),
        "stderr:\n{err}\n{}",
        format_output(&output)
    );
    assert!(
        out.contains("Complete this email task?"),
        "stdout:\n{out}\n{}",
        format_output(&output)
    );
    assert!(out.contains("Nothing was written."), "stdout:\n{out}");
    assert!(
        out.contains("Confirm with:") && out.contains("--mail-close"),
        "stdout:\n{out}"
    );
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn dry_run_and_refusal_share_the_same_payload() {
    let fx = Fixture::new("bob-cli-mail-close-parity");
    let dry = fx.json(&["--dry-run"], "!job:yogi");
    let (_, refused) = fx.fail(&[], "!job:yogi");
    assert_eq!(dry["mail_close"], refused["mail_close"]);
}
