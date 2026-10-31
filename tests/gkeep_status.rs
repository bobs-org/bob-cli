//! `bob gkeep status` integration tests.
//!
//! The fake adapter and token stub prove the gates: cache-only runs never
//! spawn either, `-r` contacts the adapter at most once, and every skip
//! reason shows up in the JSON. Nothing here touches live Keep.

mod gkeep_support;

use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
};

use chrono::{Duration, Utc};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use gkeep_support::{
    error_response, note, snapshot_ok, stderr, stdout, FakeAdapter, GkeepEnv,
};

const EMAIL: &str = "bryanbugyi34@gmail.com";
const DEVICE_ID: &str = "3f9c0a1b2c3d4e5f";

/// An isolated vault with a gkeep config, token stub, and fake adapter.
struct Rig {
    env: GkeepEnv,
    fake: FakeAdapter,
    token_command: String,
    token_marker: PathBuf,
}

impl Rig {
    fn new(name: &str) -> Self {
        Self::with_config(name, "")
    }

    fn with_config(name: &str, extra_yaml: &str) -> Self {
        let env = GkeepEnv::new(name);
        let root = root_of(&env);
        let token_marker = root.join("token-ran");
        let script = root.join("token.sh");
        write_script(
            &script,
            &format!(
                "#!/bin/sh\n: > '{}'\nprintf 'aas_et/test-master-token\\n'\n",
                token_marker.display()
            ),
        );
        let token_command = format!("sh '{}'", script.display());
        let rig = Self {
            fake: FakeAdapter::new(&env, "adapter"),
            env,
            token_command,
            token_marker,
        };
        rig.write_config(extra_yaml);
        rig.fake.respond(
            "snapshot",
            &snapshot_ok(
                EMAIL,
                vec![
                    note("Buy milk").id("keep-new-1").build(),
                    note("Wi-Fi password").id("keep-pinned-1").pinned().build(),
                ],
            ),
        );
        rig
    }

    fn write_config(&self, extra_yaml: &str) {
        self.write_config_with_token(&self.token_command, extra_yaml);
    }

    fn write_config_with_token(&self, token_command: &str, extra_yaml: &str) {
        self.env.write_config(&format!(
            "gkeep:\n  email: {EMAIL}\n  device_id: {DEVICE_ID}\n  \
             token_command: \"{}\"\n{extra_yaml}",
            token_command.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }

    fn state_dir(&self) -> PathBuf {
        root_of(&self.env)
            .join("state")
            .join("bob-cli")
            .join("gkeep")
    }

    fn status_path(&self) -> PathBuf {
        self.state_dir().join("status.json")
    }

    fn status_command(&self, args: &[&str]) -> Command {
        let mut command = self.env.command();
        self.fake.install(&mut command);
        command
            .arg("gkeep")
            .arg("status")
            .args(args)
            .env("TZ", "UTC")
            .env_remove("BOB_NOW")
            .env_remove("DATE");
        command
    }

    /// Run `status` with `args` and parse the JSON document.
    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["-f", "json"];
        all.extend_from_slice(args);
        let output = self
            .status_command(&all)
            .output()
            .expect("run bob gkeep status");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout: {}\nstderr: {}",
            stdout(&output),
            stderr(&output)
        );
        serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
            panic!("status JSON: {error}\n{}", stdout(&output))
        })
    }

    fn config_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(EMAIL.as_bytes());
        hasher.update([0u8]);
        hasher.update(DEVICE_ID.as_bytes());
        hasher.update([0u8]);
        hasher.update(self.token_command.as_bytes());
        hasher.update([0u8]);
        hasher.update(b"gkeep_inbox.md");
        hex::encode(hasher.finalize())
    }

    /// Seed `status.json`: a verified observation `observed_ago` old,
    /// then `edit` for failures, budget, and the like.
    fn seed(&self, observed_ago: Duration, edit: impl FnOnce(&mut Value)) {
        let mut doc = json!({
            "cache_version": 1,
            "config_key": self.config_key(),
            "verified": true,
            "observation": {
                "started_at": stamp(Utc::now() - observed_ago),
                "source": "list",
                "pull": {"total": 3, "new": 1, "links": 1, "revised": 0, "pending": 1},
                "stays": {"pinned": 1, "shared": 0, "empty": 0},
            },
            "invalidated_at": null,
            "failure": null,
            "last_contact_at": null,
            "budget": null,
        });
        edit(&mut doc);
        fs::create_dir_all(self.state_dir()).expect("state dir");
        fs::write(
            self.status_path(),
            serde_json::to_string_pretty(&doc).expect("seed json"),
        )
        .expect("write status.json");
    }

    fn cache(&self) -> Value {
        serde_json::from_str(
            &fs::read_to_string(self.status_path()).expect("read status.json"),
        )
        .expect("status.json parses")
    }

    fn adapter_calls(&self) -> usize {
        self.fake.call_count()
    }
}

fn root_of(env: &GkeepEnv) -> PathBuf {
    env.vault()
        .parent()
        .expect("vault has a parent")
        .to_path_buf()
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions =
            fs::metadata(path).expect("stat script").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod script");
    }
}

fn stamp(at: chrono::DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn ago(minutes: i64) -> String {
    stamp(Utc::now() - Duration::minutes(minutes))
}

fn lock(path: &Path, exclusive: bool) -> File {
    fs::create_dir_all(path.parent().expect("lock parent")).expect("lock dir");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .expect("open lock");
    if exclusive {
        FileExt::try_lock_exclusive(&file).expect("take lock");
    } else {
        FileExt::try_lock_shared(&file).expect("take shared lock");
    }
    file
}

#[test]
fn cache_only_never_spawns_the_adapter_or_reads_the_token() {
    let rig = Rig::new("bob-cli-gkeep-status-cache-only");
    rig.seed(Duration::minutes(10), |_| {});
    fs::write(
        rig.env.vault().join("gkeep_inbox.md"),
        "- [ ] Call the bank #task\n- [x] Done thing #task\n",
    )
    .expect("write target");

    let doc = rig.json(&[]);
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["configured"], true);
    assert_eq!(doc["keep"]["state"], "ok");
    assert_eq!(doc["keep"]["verified"], true);
    assert_eq!(doc["keep"]["stale"], false);
    assert_eq!(doc["keep"]["busy"], false);
    assert_eq!(doc["keep"]["in_flight"], false);
    assert_eq!(doc["keep"]["source"], "list");
    assert_eq!(doc["keep"]["pull"]["total"], 3);
    assert_eq!(doc["keep"]["stays"]["pinned"], 1);
    assert!(doc["keep"]["error"].is_null());
    assert!(doc["keep"].get("refresh").is_none());
    assert!(doc["keep"]["next_check_at"].is_string());
    assert_eq!(doc["inbox"]["path"], "gkeep_inbox.md");
    assert_eq!(doc["inbox"]["exists"], true);
    assert_eq!(doc["inbox"]["open"], 1);

    assert_eq!(rig.adapter_calls(), 0, "no adapter spawn");
    assert!(!rig.token_marker.exists(), "no token read");
}

#[test]
fn human_output_is_plain_when_piped() {
    let rig = Rig::new("bob-cli-gkeep-status-human");
    rig.seed(Duration::minutes(4), |_| {});
    fs::write(rig.env.vault().join("gkeep_inbox.md"), "- [ ] One #task\n")
        .expect("write target");
    let output = rig.status_command(&[]).output().expect("run status");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(!text.contains('\u{1b}'), "no ANSI when piped:\n{text}");
    assert!(
        text.contains("3 to pull · 1 new · 1 link → reading queue · 1 pending archive · 1 pinned stays"),
        "{text}"
    );
    assert!(text.contains("checked 4m ago"), "{text}");
    assert!(text.contains("1 open in gkeep_inbox.md"), "{text}");
    assert!(text.contains("→ bob gkeep pull"), "{text}");
    assert_eq!(rig.adapter_calls(), 0);
}

#[test]
fn a_fresh_host_is_unverified_with_no_inbox_file() {
    let rig = Rig::new("bob-cli-gkeep-status-unverified");
    let doc = rig.json(&[]);
    assert_eq!(doc["keep"]["state"], "unverified");
    assert_eq!(doc["keep"]["verified"], false);
    assert!(doc["keep"]["pull"].is_null());
    assert!(doc["keep"]["stays"].is_null());
    assert!(doc["keep"]["checked_at"].is_null());
    assert_eq!(doc["inbox"]["exists"], false);
    assert!(doc["inbox"]["open"].is_null());
    assert!(
        !rig.state_dir().join("status.json").exists(),
        "reading never writes the cache"
    );
}

#[test]
fn unconfigured_hosts_print_configured_false_and_exit_zero() {
    let rig = Rig::new("bob-cli-gkeep-status-unconfigured");
    for config in ["", "other: 1\n", "gkeep:\n  target: gkeep_inbox.md\n"] {
        rig.env.write_config(config);
        let doc = rig.json(&["-r"]);
        assert_eq!(doc["configured"], false, "{config:?}");
        assert_eq!(doc["keep"]["state"], "unconfigured");
        assert_eq!(doc["keep"]["refresh"]["ran"], false);
        assert_eq!(doc["keep"]["refresh"]["skipped"], "unconfigured");
        assert!(doc["keep"]["next_check_at"].is_null());
    }
    fs::remove_file(rig.env.config()).expect("remove config");
    let doc = rig.json(&[]);
    assert_eq!(doc["configured"], false, "missing config file");
    assert_eq!(rig.adapter_calls(), 0);

    rig.env.write_config("");
    let output = rig.status_command(&[]).output().expect("run status");
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output)
        .contains("not set up on this host · run bob gkeep login"));
}

#[test]
fn invalid_config_exits_two_with_a_json_error_document() {
    let rig = Rig::new("bob-cli-gkeep-status-bad-config");
    for config in [
        "gkeep: [unclosed\n",
        "gkeep:\n  email: not-an-email\n",
        "gkeep:\n  email: a@b.c\n  check_interval: 1m\n",
    ] {
        rig.env.write_config(config);
        let output = rig
            .status_command(&["-f", "json"])
            .output()
            .expect("run status");
        assert_eq!(output.status.code(), Some(2), "{config:?}");
        let doc: Value = serde_json::from_str(&stdout(&output))
            .unwrap_or_else(|error| panic!("{error}: {}", stdout(&output)));
        assert_eq!(doc["schema_version"], 1);
        assert_eq!(doc["ok"], false);
        assert_eq!(doc["error"]["kind"], "config");
        assert!(doc["error"]["message"].is_string());

        let human = rig.status_command(&[]).output().expect("run human");
        assert_eq!(human.status.code(), Some(2));
        assert!(
            stderr(&human).contains("bob gkeep status"),
            "{}",
            stderr(&human)
        );
    }
    assert_eq!(rig.adapter_calls(), 0);
}

#[test]
fn usage_errors_exit_two() {
    let rig = Rig::new("bob-cli-gkeep-status-usage");
    for args in [&["-m", "soon"][..], &["-f", "yaml"], &["--bogus"]] {
        let output = rig.status_command(args).output().expect("run status");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn refresh_runs_one_check_and_records_the_observation() {
    let rig = Rig::new("bob-cli-gkeep-status-refresh");
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true);
    assert!(doc["keep"]["refresh"]["skipped"].is_null());
    assert_eq!(doc["keep"]["state"], "ok");
    assert_eq!(doc["keep"]["verified"], true);
    assert_eq!(doc["keep"]["source"], "status");
    assert_eq!(doc["keep"]["pull"]["total"], 1);
    assert_eq!(doc["keep"]["pull"]["new"], 1);
    assert_eq!(doc["keep"]["stays"]["pinned"], 1);

    assert_eq!(rig.adapter_calls(), 1);
    assert!(rig.token_marker.exists(), "token command ran");
    let request: Value = serde_json::from_str(&rig.fake.request("snapshot", 1))
        .expect("request");
    assert_eq!(request["op"], "snapshot");
    assert_eq!(request["include_archived"], false);

    let cache = rig.cache();
    assert_eq!(cache["budget"]["used"], 1, "automatic check counts");
    assert!(cache["last_contact_at"].is_string());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(rig.status_path())
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert!(
        !fs::read_to_string(rig.status_path())
            .expect("cache text")
            .contains("aas_et/"),
        "the cache never holds a token"
    );
}

#[test]
fn a_fresh_cache_skips_and_reports_the_next_check() {
    let rig = Rig::new("bob-cli-gkeep-status-fresh");
    rig.seed(Duration::minutes(5), |_| {});
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], false);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "fresh");
    assert!(doc["keep"]["next_check_at"].is_string());
    assert_eq!(rig.adapter_calls(), 0);
    assert!(!rig.token_marker.exists());
    let human = rig
        .status_command(&["-r"])
        .output()
        .expect("run human status");
    assert!(
        stdout(&human).contains("skipped: fresh · next check "),
        "{}",
        stdout(&human)
    );
}

#[test]
fn max_age_overrides_the_interval() {
    let rig = Rig::new("bob-cli-gkeep-status-max-age");
    rig.seed(Duration::minutes(10), |_| {});
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "fresh");
    let doc = rig.json(&["-r", "-m", "5m"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true);
    assert_eq!(rig.adapter_calls(), 1);
}

#[test]
fn forced_checks_ignore_freshness_but_not_the_floor() {
    let rig = Rig::new("bob-cli-gkeep-status-forced");
    rig.seed(Duration::minutes(1), |_| {});
    let doc = rig.json(&["-r", "-m", "0"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true, "{doc}");
    assert_eq!(rig.adapter_calls(), 1);
    assert!(
        rig.cache()["budget"].is_null(),
        "a forced check does not spend the automatic budget"
    );

    let doc = rig.json(&["-r", "-m", "0"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], false);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "floor");
    assert_eq!(rig.adapter_calls(), 1, "the floor stopped the second call");
}

#[test]
fn check_interval_off_blocks_refresh_unless_max_age_is_given() {
    let rig =
        Rig::with_config("bob-cli-gkeep-status-off", "  check_interval: off\n");
    rig.seed(Duration::hours(5), |_| {});
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "off");
    assert!(doc["keep"]["next_check_at"].is_null());
    assert_eq!(rig.adapter_calls(), 0);

    let doc = rig.json(&["-r", "-m", "1h"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true);
    assert_eq!(rig.adapter_calls(), 1);
}

#[test]
fn false_check_interval_also_turns_checks_off() {
    let rig = Rig::with_config(
        "bob-cli-gkeep-status-false",
        "  check_interval: false\n",
    );
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "off");
}

#[test]
fn a_held_single_flight_lock_skips_with_in_flight() {
    let rig = Rig::new("bob-cli-gkeep-status-in-flight");
    let _held = lock(&rig.state_dir().join("status-check.lock"), true);
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "in_flight");
    assert_eq!(doc["keep"]["in_flight"], true);
    assert_eq!(rig.adapter_calls(), 0);

    let cache_only = rig.json(&[]);
    assert_eq!(cache_only["keep"]["in_flight"], true);
}

#[test]
fn a_running_pull_skips_with_busy() {
    let rig = Rig::new("bob-cli-gkeep-status-busy");
    let _held = lock(&rig.state_dir().join("pull.lock"), true);
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "busy");
    assert_eq!(doc["keep"]["busy"], true);
    assert_eq!(doc["keep"]["in_flight"], false);
    assert_eq!(rig.adapter_calls(), 0);
    assert_eq!(rig.json(&[])["keep"]["busy"], true);
}

#[test]
fn sign_in_errors_refuse_even_forced_checks() {
    let rig = Rig::new("bob-cli-gkeep-status-auth");
    rig.seed(Duration::hours(3), |doc| {
        doc["failure"] = json!({
            "state": "auth", "kind": "auth", "message": "credentials rejected",
            "hint": "run `bob gkeep login`", "since": ago(30),
            "last_at": ago(30), "count": 1,
        });
    });
    let doc = rig.json(&["-r", "-m", "0"]);
    assert_eq!(doc["keep"]["state"], "auth");
    assert_eq!(doc["keep"]["stale"], true);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "auth");
    assert!(doc["keep"]["next_check_at"].is_null());
    assert_eq!(doc["keep"]["error"]["kind"], "auth");
    assert_eq!(doc["keep"]["error"]["failures"], 1);
    assert_eq!(rig.adapter_calls(), 0);

    let human = rig.status_command(&[]).output().expect("human");
    assert!(
        stdout(&human).contains("sign-in expired · run bob gkeep login"),
        "{}",
        stdout(&human)
    );
}

#[test]
fn rate_limits_refuse_forced_checks() {
    let rig = Rig::new("bob-cli-gkeep-status-rate-limit");
    rig.seed(Duration::hours(3), |doc| {
        doc["failure"] = json!({
            "state": "rate_limited", "kind": "rate_limit", "message": "slow down",
            "hint": null, "since": ago(5), "last_at": ago(5), "count": 1,
        });
    });
    let doc = rig.json(&["-r", "-m", "0"]);
    assert_eq!(doc["keep"]["state"], "rate_limited");
    assert_eq!(doc["keep"]["refresh"]["skipped"], "backoff");
    assert!(doc["keep"]["next_check_at"].is_string());
    assert_eq!(rig.adapter_calls(), 0);
}

#[test]
fn offline_backoff_blocks_automatic_checks_but_not_forced_ones() {
    let rig = Rig::new("bob-cli-gkeep-status-offline-backoff");
    let seed = |rig: &Rig| {
        rig.seed(Duration::hours(3), |doc| {
            doc["failure"] = json!({
                "state": "offline", "kind": "network", "message": "down",
                "hint": null, "since": ago(2), "last_at": ago(2), "count": 1,
            });
        });
    };
    seed(&rig);
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "backoff");
    assert_eq!(rig.adapter_calls(), 0);

    let doc = rig.json(&["-r", "-m", "0"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true);
    assert_eq!(doc["keep"]["state"], "ok", "success clears the failure");
    assert!(doc["keep"]["error"].is_null());
    assert_eq!(rig.adapter_calls(), 1);
}

#[test]
fn the_daily_budget_stops_automatic_checks_only() {
    let rig = Rig::new("bob-cli-gkeep-status-budget");
    let today = Utc::now().format("%Y-%m-%d").to_string();
    rig.seed(Duration::hours(3), |doc| {
        doc["budget"] = json!({"day": today, "used": 48});
    });
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "budget");
    let next = doc["keep"]["next_check_at"].as_str().expect("next check");
    assert!(next.ends_with("T00:00:00Z"), "waits for midnight: {next}");
    assert_eq!(rig.adapter_calls(), 0);

    let doc = rig.json(&["-r", "-m", "0"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true);
    assert_eq!(rig.adapter_calls(), 1);
}

#[test]
fn a_token_command_that_needs_stdin_is_locked_and_never_calls_keep() {
    let rig = Rig::new("bob-cli-gkeep-status-locked");
    rig.write_config_with_token("read -r line; echo \"$line\"", "");
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true);
    assert_eq!(doc["keep"]["state"], "locked");
    assert_eq!(doc["keep"]["error"]["kind"], "locked");
    assert_eq!(doc["keep"]["error"]["failures"], 1);
    assert!(doc["keep"]["error"]["hint"].is_string());
    assert!(doc["keep"]["next_check_at"].is_string(), "locked backs off");
    assert_eq!(rig.adapter_calls(), 0);

    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["skipped"], "floor");
}

#[test]
fn the_token_command_runs_without_a_tty_and_with_pinentry_errors() {
    let rig = Rig::new("bob-cli-gkeep-status-hardened");
    let script = root_of(&rig.env).join("hardened-token.sh");
    write_script(
        &script,
        "#!/bin/sh\n\
         [ -z \"${GPG_TTY+x}\" ] || { echo 'GPG_TTY leaked' >&2; exit 7; }\n\
         case \"$PASSWORD_STORE_GPG_OPTS\" in\n\
           '--quiet --pinentry-mode error --batch') ;;\n\
           *) echo \"bad opts: $PASSWORD_STORE_GPG_OPTS\" >&2; exit 8;;\n\
         esac\n\
         printf 'aas_et/hardened-token\\n'\n",
    );
    rig.write_config_with_token(&format!("sh '{}'", script.display()), "");
    let output = rig
        .status_command(&["-r", "-f", "json"])
        .env("GPG_TTY", "/dev/pts/9")
        .env("PASSWORD_STORE_GPG_OPTS", "--quiet")
        .output()
        .expect("run status");
    assert_eq!(output.status.code(), Some(0));
    let doc: Value = serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(doc["keep"]["state"], "ok", "{doc}");
    assert_eq!(rig.adapter_calls(), 1);
    let request = rig.fake.request("snapshot", 1);
    assert!(request.contains("aas_et/hardened-token"));
    assert!(!stdout(&output).contains("aas_et/"), "no token on stdout");
    assert!(!stderr(&output).contains("aas_et/"), "no token on stderr");
}

#[test]
fn adapter_failures_map_to_states_and_back_off() {
    for (kind, state) in [
        ("network", "offline"),
        ("rate_limit", "rate_limited"),
        ("auth", "auth"),
        ("internal", "error"),
    ] {
        let rig = Rig::new(&format!("bob-cli-gkeep-status-fail-{kind}"));
        rig.fake
            .respond("snapshot", &error_response(kind, "it broke"));
        let doc = rig.json(&["-r"]);
        assert_eq!(doc["keep"]["refresh"]["ran"], true, "{kind}");
        assert_eq!(doc["keep"]["state"], state, "{kind}");
        assert_eq!(doc["keep"]["verified"], false, "{kind}");
        assert_eq!(doc["keep"]["error"]["failures"], 1, "{kind}");
        assert!(doc["keep"]["pull"].is_null(), "{kind}");
        if state == "auth" {
            assert!(doc["keep"]["next_check_at"].is_null(), "{kind}");
        } else {
            assert!(doc["keep"]["next_check_at"].is_string(), "{kind}");
        }
    }
}

#[test]
fn a_failed_check_keeps_the_last_count_as_stale() {
    let rig = Rig::new("bob-cli-gkeep-status-stale");
    rig.seed(Duration::hours(2), |_| {});
    rig.fake
        .respond("snapshot", &error_response("network", "unreachable"));
    let doc = rig.json(&["-r"]);
    assert_eq!(doc["keep"]["refresh"]["ran"], true);
    assert_eq!(doc["keep"]["state"], "offline");
    assert_eq!(doc["keep"]["stale"], true);
    assert_eq!(doc["keep"]["verified"], true);
    assert_eq!(doc["keep"]["pull"]["total"], 3, "last-known count");
}

#[test]
fn status_never_pulls_or_archives() {
    let rig = Rig::new("bob-cli-gkeep-status-read-only");
    let before = fs::read_dir(rig.env.vault()).expect("list vault").count();
    let _ = rig.json(&["-r"]);
    assert_eq!(rig.adapter_calls(), 1, "one snapshot, nothing else");
    assert!(rig
        .fake
        .request("snapshot", 1)
        .contains("\"op\":\"snapshot\""));
    assert_eq!(
        fs::read_dir(rig.env.vault()).expect("list vault").count(),
        before,
        "the vault is untouched"
    );
}

#[test]
fn list_refreshes_the_cache_status_serves() {
    let rig = Rig::new("bob-cli-gkeep-status-after-list");
    let mut list = rig.env.command();
    rig.fake.install(&mut list);
    let output = list
        .args(["gkeep", "list", "-s", "keep", "-f", "json"])
        .env("TZ", "UTC")
        .output()
        .expect("run list");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let calls = rig.adapter_calls();

    let doc = rig.json(&[]);
    assert_eq!(doc["keep"]["state"], "ok");
    assert_eq!(doc["keep"]["source"], "list");
    assert_eq!(doc["keep"]["pull"]["total"], 1);
    assert_eq!(rig.adapter_calls(), calls, "status itself called nothing");
}

#[test]
fn a_status_probe_does_not_break_a_real_pull_lock() {
    // The busy probe takes a momentary shared lock on pull.lock; a real
    // pull started in that window retries instead of reporting another
    // pull. Here the probe has finished, so the lock must be free.
    let rig = Rig::new("bob-cli-gkeep-status-probe-release");
    let _ = rig.json(&["-r"]);
    let held = lock(&rig.state_dir().join("pull.lock"), true);
    drop(held);
}

#[test]
fn help_documents_the_options() {
    let rig = Rig::new("bob-cli-gkeep-status-help");
    let output = rig.status_command(&["--help"]).output().expect("help");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    for needle in [
        "-b, --bob-dir",
        "-f, --format",
        "-h, --help",
        "-m, --max-age",
        "-r, --refresh",
        "bob gkeep status -r -m 0 -f json",
    ] {
        assert!(text.contains(needle), "help lacks {needle:?}:\n{text}");
    }
}
