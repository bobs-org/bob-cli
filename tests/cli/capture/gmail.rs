//! Capture stays offline; only the detached worker may fetch Gmail.
use crate::support::*;
use serde_json::Value;
use std::{fs, process::Command};

const URL: &str =
    "https://mail.google.com/mail/u/0/#inbox/FMfcgzQhWTsNRgxMhQNwhZDfgqwXpPsx";
const NOTE: &str = "mail/2026/lease.md";

struct Fixture {
    temp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new("capture-gmail");
        fs::create_dir_all(temp.path().join("vault/mail/2026")).unwrap();
        fs::write(temp.path().join("config.yml"), "gmail:\n  account: me@example.com\nurl_routing:\n  exclude_hosts: []\n").unwrap();
        write_executable(
            &temp.path().join("gog"),
            "#!/bin/sh\necho called >> \"$GOG_CALLS\"\nexit 1\n",
        );
        Self { temp }
    }
    fn command(&self, subcommand: &str) -> Command {
        let mut cmd = bob_command();
        cmd.arg(subcommand)
            .args(["-f", "json"])
            .env("BOB_DIR", self.temp.path().join("vault"))
            .env("BOB_CONFIG_FILE", self.temp.path().join("config.yml"))
            .env("BOB_GMAIL_GOG", self.temp.path().join("gog"))
            .env("GOG_CALLS", self.temp.path().join("calls"))
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("BOB_NOW", "2026-11-06 12:00:00");
        cmd
    }
    fn saved(&self) {
        fs::write(self.temp.path().join("vault").join(NOTE), "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: me@example.com\ngmail_thread_id: 1a0b9eb2cc26ab96\ntitle: Lease renewal\nwho: Dana\nmessages: 3\n---\nPrivate synthetic body\n").unwrap();
    }
    fn json(&self, command: &mut Command) -> Value {
        let output = command.output().unwrap();
        assert_success(&output);
        assert!(
            !self.temp.path().join("calls").exists(),
            "capture invoked gog"
        );
        serde_json::from_str(&stdout(&output)).unwrap()
    }
}

#[test]
fn gmail_spans_preserve_identity_and_do_not_route_to_references() {
    let f = Fixture::new();
    for url in [URL.to_string(), "https://mail.google.com/mail/?authuser=me@example.com#all/1a0b9eb2cc26ab96".into(), "https://mail.google.com/mail/?authuser=x%40y#all/1a0b9eb2cc26ab96".into()] {
        let draft = format!("Répondre [my label]({url})");
        let json = f.json(f.command("capture-parse").arg("--").arg(&draft));
        assert_eq!(json["mode"], "task");
        assert_eq!(json["grammar"], 18);
        assert_eq!(json["diagnostics"], serde_json::json!([]));
        let span = json["spans"].as_array().unwrap().iter().find(|span| span["kind"] == "gmail_url").unwrap();
        assert_eq!(span["gmail"]["verdict"], "decoded");
        assert_eq!(span["key"], "1a0b9eb2cc26ab96");
        assert_eq!(&draft[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize], url);
    }
    let json = f.json(
        f.command("capture-parse")
            .arg("--")
            .arg(format!("{URL}\nhttps://example.com/article")),
    );
    assert_eq!(json["items"][0]["mode"], "task");
    assert_eq!(json["items"][1]["mode"], "ref");
    f.saved();
    let json = f.json(f.command("capture-parse").arg("--").arg(URL));
    assert_eq!(json["spans"][0]["gmail"]["verdict"], "known");
    assert_eq!(json["spans"][0]["title"], "✉️ Lease renewal · Dana");
}

#[test]
fn gmail_preview_and_submit_link_saved_threads_offline_in_the_batch() {
    let f = Fixture::new();
    f.saved();
    let draft = format!("Reply {URL}\n- Keep [my words]({URL})\n\nOther https://mail.google.com/mail/#all/1a0b9eb2cc26ab97");
    let preview = f.json(f.command("capture").args(["-d", "--"]).arg(&draft));
    assert_eq!(preview["captures"][0]["gmail"][0]["verdict"], "known");
    assert_eq!(
        preview["captures"][0]["gmail"][1]["link"],
        "[[mail/2026/lease|my words]]"
    );
    assert_eq!(preview["captures"][1]["gmail"][0]["verdict"], "decoded");
    assert!(!f.temp.path().join("vault/mac_inbox.md").exists());
    assert!(!f.temp.path().join("state/bob-cli/gmail").exists());
    f.json(f.command("capture").arg("--").arg(&draft));
    let text =
        fs::read_to_string(f.temp.path().join("vault/mac_inbox.md")).unwrap();
    assert!(text.contains("Reply [[mail/2026/lease|✉️ Lease renewal · Dana]]"));
    assert!(text.contains("Keep [[mail/2026/lease|my words]]"));
    assert!(!text.contains(URL));
    assert!(text.contains("#all/1a0b9eb2cc26ab97"));
    assert!(!text.contains("[fresh::"));
}

#[test]
fn gmail_keeps_refused_unknown_code_and_other_accounts_unchanged() {
    let f = Fixture::new();
    f.saved();
    let note = f.temp.path().join("vault").join(NOTE);
    fs::write(
        &note,
        fs::read_to_string(&note)
            .unwrap()
            .replace("me@example.com", "other@example.com"),
    )
    .unwrap();
    let draft = format!("{URL}\n\nhttps://mail.google.com/mail/#inbox/unknown\n\nhttps://mail.google.com/mail/?compose=new\n\nLiteral `{URL}`");
    let json = f.json(f.command("capture").args(["-d", "--"]).arg(&draft));
    assert_eq!(json["captures"][0]["gmail"][0]["verdict"], "decoded");
    assert_eq!(json["captures"][1]["gmail"][0]["verdict"], "unresolved");
    assert_eq!(json["captures"][2]["gmail"][0]["verdict"], "refused");
    assert!(json["captures"][3].get("gmail").is_none());
    let search = "https://mail.google.com/mail/#search/rfc822msgid:synthetic%40example.com";
    let json = f.json(f.command("capture").args(["-d", "--"]).arg(search));
    assert_eq!(json["gmail"][0]["verdict"], "search");
    assert_eq!(json["text"], search);
}

#[test]
fn gmail_failed_capture_and_unsigned_capture_never_queue() {
    let f = Fixture::new();
    let output = f
        .command("capture")
        .env("BOB_GMAIL_KICK", "on")
        .arg("--")
        .arg(format!("{URL}\n\n@does-not-exist+id note"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!f.temp.path().join("vault/mac_inbox.md").exists());
    assert!(!f.temp.path().join("state/bob-cli/gmail").exists());
    f.json(
        f.command("capture")
            .env("BOB_GMAIL_KICK", "on")
            .arg("--")
            .arg(URL),
    );
    assert!(!f.temp.path().join("state/bob-cli/gmail").exists());
}

#[test]
fn gmail_preserves_authored_labels_wrappers_and_duplicate_identity() {
    let f = Fixture::new();
    f.saved();
    let words = format!("Re: {}", "keep my wording ".repeat(6));
    let draft = format!("[{words}]({URL})\n- [angle](<{URL}>)\n- `{URL}`");
    let json = f.json(f.command("capture").args(["-d", "--"]).arg(&draft));
    assert_eq!(
        json["gmail"][0]["link"],
        format!("[[mail/2026/lease|{words}]]")
    );
    assert_eq!(json["gmail"].as_array().unwrap().len(), 1);
    assert!(json["sub_bullets"][0]
        .as_str()
        .unwrap()
        .contains(&format!("[angle](<{URL}>)")));
    fs::copy(
        f.temp.path().join("vault").join(NOTE),
        f.temp.path().join("vault/mail/duplicate.md"),
    )
    .unwrap();
    let json = f.json(f.command("capture").args(["-d", "--"]).arg(URL));
    assert_eq!(json["gmail"][0]["verdict"], "decoded");
    assert_eq!(json["text"], URL);
}
