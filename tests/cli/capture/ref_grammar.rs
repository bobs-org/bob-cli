//! URL-list split and routing-off CLI tests (phase grammar).
//!
//! The list split is lexical and live: a pasted URL block becomes one
//! task per line. Reference claiming stays off, so a lone bare URL is
//! still a task.

use crate::support::*;
use std::fs;

#[test]
fn capture_url_list_writes_one_task_per_line() {
    let temp = TempDir::new("bob-cli-capture-url-list");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let draft = "https://a.example/1\nhttps://b.example/2";
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-R")
        .arg(draft)
        .env("BOB_NOW", "2026-10-07 13:40:00")
        .output()
        .expect("run URL-list capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "task");
    assert_eq!(captures[0]["text"], "https://a.example/1");
    assert_eq!(captures[1]["kind"], "task");
    assert_eq!(captures[1]["text"], "https://b.example/2");
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        concat!(
            "- [ ] #task https://a.example/1 [created::2026-10-07]\n",
            "- [ ] #task https://b.example/2 [created::2026-10-07]\n",
        )
    );
}

#[test]
fn capture_lone_bare_url_stays_a_task_while_routing_is_off() {
    let temp = TempDir::new("bob-cli-capture-lone-url");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-R")
        .arg("https://example.com/post")
        .env("BOB_NOW", "2026-10-07 13:40:00")
        .output()
        .expect("run lone-URL capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "task");
    assert_eq!(json["text"], "https://example.com/post");
    assert_eq!(
        json["task_line"],
        "- [ ] #task https://example.com/post [created::2026-10-07]"
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task https://example.com/post [created::2026-10-07]\n"
    );
}
