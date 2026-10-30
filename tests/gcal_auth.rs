//! `bob gcal login`, `logout`, `doctor`, and `calendars` integration tests.
//!
//! FakeGog records argv and answers from a scenario file. Nothing here
//! touches live Google.

mod gcal_support;

use std::{fs, io::Write};

use gcal_support::{
    json_stdout, run_gcal, run_gcal_paste, run_gcal_stdin, stderr, stdout,
    FakeBrowser, FakeGog, GcalEnv, ACCOUNT, FAKE_REDIRECT,
};
use serde_json::Value;

fn check<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("missing check {id}: {doc}"))
}

fn assert_success(output: &std::process::Output, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{label} expected exit 0:\nstatus: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
}

#[test]
fn login_existing_client_is_already_connected() {
    let env = GcalEnv::new("bob-cli-gcal-login-existing");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["login", "-f", "json"]);
    assert_success(&output, "login existing");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["already_connected"], true);
    assert_eq!(doc["sign_in"], "already_connected");
    assert_eq!(doc["account"], ACCOUNT);
    assert_eq!(doc["client"], "bob-gcal");
    assert!(doc["calendars"].as_u64().unwrap() >= 1);
    assert!(
        !fake.called_with(&["auth", "add"]),
        "already connected must skip auth add:\n{:?}",
        fake.calls()
    );
}

#[test]
fn login_dash_c_stores_the_client() {
    let env = GcalEnv::new("bob-cli-gcal-login-c");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let client = env.root().join("client.json");
    fs::write(
        &client,
        r#"{"installed":{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"test-secret-123","project_id":"my-proj"}}"#,
    )
    .expect("write client json");
    let output = run_gcal_paste(
        &env,
        &fake,
        &[
            "login",
            "-c",
            client.to_str().expect("utf8 path"),
            "-f",
            "json",
        ],
    );
    assert_success(&output, "login -c");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["already_connected"], false);
    assert!(
        fake.called_with(&["auth", "credentials", "set"]),
        "expected credentials set:\n{:?}",
        fake.calls()
    );
    assert!(
        fake.called_with(&["auth", "add"]),
        "expected auth add after -c:\n{:?}",
        fake.calls()
    );
    let set_call = fake
        .calls()
        .into_iter()
        .find(|call| {
            call.windows(3).any(|w| w == ["auth", "credentials", "set"])
        })
        .expect("credentials set call");
    assert!(
        set_call.iter().any(|arg| arg == "-"),
        "credentials set must read from stdin: {set_call:?}"
    );
    assert!(
        !set_call.iter().any(|arg| arg.contains("test-secret-123")),
        "argv must not carry the secret: {set_call:?}"
    );
    assert!(
        !set_call.iter().any(|arg| arg == "--readonly"),
        "storing a client is a write: {set_call:?}"
    );
    let payloads = fake.stdin_payloads();
    assert_eq!(
        payloads.len(),
        1,
        "expected one stdin payload: {payloads:?}"
    );
    assert_eq!(
        payloads[0]["installed"]["client_id"],
        "123456789012-abc.apps.googleusercontent.com"
    );
    assert_eq!(payloads[0]["installed"]["client_secret"], "test-secret-123");
}

#[test]
fn login_reuses_legacy_default_client_without_prompt() {
    let env = GcalEnv::new("bob-cli-gcal-login-reuse");
    let fake = FakeGog::new(&env, "gog").scenario("other_client");
    let output = run_gcal_paste(&env, &fake, &["login", "-f", "json"]);
    assert_success(&output, "login reuse");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert!(
        fake.called_with(&["auth", "credentials", "set"]),
        "reuse must pipe the normalized client:\n{:?}",
        fake.calls()
    );
    let set_call = fake
        .calls()
        .into_iter()
        .find(|call| {
            call.windows(3).any(|w| w == ["auth", "credentials", "set"])
        })
        .expect("credentials set call");
    assert!(
        set_call.iter().any(|arg| arg == "-"),
        "reuse must use `set -`: {set_call:?}"
    );
    assert!(
        !set_call.iter().any(|arg| arg.contains("other-secret")),
        "argv must not carry the secret: {set_call:?}"
    );
    let payloads = fake.stdin_payloads();
    assert_eq!(
        payloads.len(),
        1,
        "expected one stdin payload: {payloads:?}"
    );
    assert_eq!(
        payloads[0]["installed"]["client_id"],
        "123456789012-abc.apps.googleusercontent.com"
    );
    assert_eq!(payloads[0]["installed"]["client_secret"], "other-secret");
}

#[test]
fn login_skips_keyring_held_client() {
    let env = GcalEnv::new("bob-cli-gcal-login-keyring-skip");
    let fake = FakeGog::new(&env, "gog").scenario("keyring_client");
    let output = run_gcal(&env, &fake, &["login", "-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["error"]["kind"], "no_client");
    let hint = doc["error"]["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("keyring"),
        "expected keyring note in hint: {doc}"
    );
    assert!(
        hint.contains("default"),
        "expected client name in keyring note: {doc}"
    );
    assert!(
        !fake.called_with(&["auth", "credentials", "set"]),
        "keyring client must not be copied:\n{:?}",
        fake.calls()
    );
}

#[test]
fn login_dash_c_rejects_web_client() {
    let env = GcalEnv::new("bob-cli-gcal-login-web");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let client = env.root().join("web.json");
    fs::write(
        &client,
        r#"{"web":{"client_id":"1-a.apps.googleusercontent.com","client_secret":"s"}}"#,
    )
    .expect("write web json");
    let output = run_gcal(
        &env,
        &fake,
        &[
            "login",
            "-c",
            client.to_str().expect("utf8 path"),
            "-f",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "client_invalid");
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Desktop app"),
        "{doc}"
    );
}

#[test]
fn login_dash_c_accepts_legacy_flat_file() {
    let env = GcalEnv::new("bob-cli-gcal-login-flat");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let client = env.root().join("flat.json");
    fs::write(
        &client,
        r#"{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"flat-secret"}"#,
    )
    .expect("write flat json");
    let output = run_gcal_paste(
        &env,
        &fake,
        &[
            "login",
            "-c",
            client.to_str().expect("utf8 path"),
            "-f",
            "json",
        ],
    );
    assert_success(&output, "login -c flat");
    let payloads = fake.stdin_payloads();
    assert_eq!(payloads.len(), 1, "{payloads:?}");
    assert_eq!(payloads[0]["installed"]["client_secret"], "flat-secret");
}

#[test]
fn login_dash_c_replaces_existing_client() {
    let env = GcalEnv::new("bob-cli-gcal-login-replace");
    let fake = FakeGog::new(&env, "gog");
    let client = env.root().join("new.json");
    fs::write(
        &client,
        r#"{"installed":{"client_id":"999-abc.apps.googleusercontent.com","client_secret":"new-secret"}}"#,
    )
    .expect("write new json");
    let output = run_gcal_paste(
        &env,
        &fake,
        &[
            "login",
            "-c",
            client.to_str().expect("utf8 path"),
            "-f",
            "json",
        ],
    );
    assert_success(&output, "login -c replace");
    let payloads = fake.stdin_payloads();
    assert_eq!(payloads.len(), 1, "{payloads:?}");
    assert_eq!(
        payloads[0]["installed"]["client_id"],
        "999-abc.apps.googleusercontent.com"
    );
}

#[test]
fn login_keyring_failure_is_classified() {
    let env = GcalEnv::new("bob-cli-gcal-login-keyring-fail");
    let fake = FakeGog::new(&env, "gog").scenario("keyring_fail");
    let client = env.root().join("client.json");
    fs::write(
        &client,
        r#"{"installed":{"client_id":"1-a.apps.googleusercontent.com","client_secret":"s"}}"#,
    )
    .expect("write client json");
    let output = run_gcal(
        &env,
        &fake,
        &[
            "login",
            "-c",
            client.to_str().expect("utf8 path"),
            "-f",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "keyring");
}

#[test]
fn login_prints_the_four_step_checklist_without_a_client() {
    let env = GcalEnv::new("bob-cli-gcal-login-checklist");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let output = run_gcal(&env, &fake, &["login"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let err = stderr(&output);
    assert!(
        err.contains("no bob-gcal OAuth client is stored"),
        "expected no_client message:\n{err}"
    );
    assert!(
        err.contains("Enable the Calendar API"),
        "expected checklist step 1:\n{err}"
    );
    assert!(
        err.contains("In production"),
        "expected checklist step 2:\n{err}"
    );
    assert!(
        err.contains("Desktop app"),
        "expected checklist step 3:\n{err}"
    );
    assert!(
        err.contains("bob gcal login -c"),
        "expected checklist step 4:\n{err}"
    );
}

#[test]
fn login_account_flag_overrides_config() {
    let env = GcalEnv::new("bob-cli-gcal-login-account");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal_paste(
        &env,
        &fake,
        &["login", "-a", "other@example.com", "-f", "json"],
    );
    assert_success(&output, "login -a");
    let doc = json_stdout(&output);
    assert_eq!(doc["account"], "other@example.com");
    assert!(
        fake.called_with(&["auth", "add", "other@example.com"]),
        "auth add must use -a: {:?}",
        fake.calls()
    );
}

#[test]
fn login_minus_m_uses_paste_remote_flow() {
    let env = GcalEnv::new("bob-cli-gcal-login-manual");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal_paste(&env, &fake, &["login", "-m", "-f", "json"]);
    assert_success(&output, "login -m");
    let doc = json_stdout(&output);
    assert_eq!(doc["sign_in"], "paste");
    let add = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|w| w == ["auth", "add"]))
        .expect("auth add");
    assert!(
        add.iter().any(|arg| arg == "--remote"),
        "expected --remote: {add:?}"
    );
    assert!(
        add.iter().any(|arg| arg == "--no-input"),
        "captured auth add keeps --no-input: {add:?}"
    );
    assert!(
        add.iter().any(|arg| arg == "--json"),
        "captured auth add keeps --json: {add:?}"
    );
    assert!(
        !add.iter().any(|arg| arg == "--manual"),
        "bob-owned paste must not pass --manual: {add:?}"
    );
}

#[test]
fn login_ssh_auto_selects_paste() {
    let env = GcalEnv::new("bob-cli-gcal-login-ssh");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let mut command = env.command();
    fake.install(&mut command);
    command.env("SSH_CONNECTION", "1.2.3.4 22 5.6.7.8 22");
    command.env("BOB_GCAL_ASSUME_TTY", "1");
    command.args(["gcal", "login", "-f", "json"]);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn login on ssh");
    let _ = child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(FAKE_REDIRECT.as_bytes());
    let output = child.wait_with_output().expect("wait login on ssh");
    assert_success(&output, "login ssh");
    let doc = json_stdout(&output);
    assert_eq!(doc["sign_in"], "paste");
    let err = stderr(&output);
    assert!(
        err.contains("Open the link") && err.contains("127.0.0.1"),
        "expected paste instructions:\n{err}"
    );
    assert!(
        !err.contains("rerun with `bob gcal login -m`"),
        "old SSH hint must be gone:\n{err}"
    );
    assert!(
        fake.called_with(&["auth", "add", "--remote"]),
        "SSH must use remote paste:\n{:?}",
        fake.calls()
    );
}

#[test]
fn login_paste_junk_then_good_url_reprompts() {
    let env = GcalEnv::new("bob-cli-gcal-login-junk");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let stdin = format!("not-a-url\n{FAKE_REDIRECT}");
    let output =
        run_gcal_stdin(&env, &fake, &["login", "-m", "-f", "json"], &stdin);
    assert_success(&output, "login junk then good");
    let err = stderr(&output);
    assert!(
        err.contains("isn't the address Bob needs"),
        "expected re-prompt message:\n{err}"
    );
}

#[test]
fn login_paste_access_denied_then_q_is_cancelled() {
    let env = GcalEnv::new("bob-cli-gcal-login-denied");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let stdin = "http://127.0.0.1:36015/oauth2/callback?state=fakestate&error=access_denied\nq\n";
    let output =
        run_gcal_stdin(&env, &fake, &["login", "-m", "-f", "json"], stdin);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "cancelled");
}

#[test]
fn login_paste_stale_state_then_fresh_link() {
    let env = GcalEnv::new("bob-cli-gcal-login-stale");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let stdin = format!(
        "http://127.0.0.1:36015/oauth2/callback?state=oldstate&code=fakecode\n\
         {FAKE_REDIRECT}"
    );
    let output =
        run_gcal_stdin(&env, &fake, &["login", "-m", "-f", "json"], &stdin);
    assert_success(&output, "login stale then fresh");
    let err = stderr(&output);
    assert!(
        err.contains("older sign-in"),
        "expected stale-link message:\n{err}"
    );
}

#[test]
fn login_without_manual_never_runs_interactive_auth_add() {
    let env = GcalEnv::new("bob-cli-gcal-login-no-manual");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal_paste(&env, &fake, &["login", "-f", "json"]);
    assert_success(&output, "login auto paste");
    for call in fake.calls() {
        if call.windows(2).any(|w| w == ["auth", "add"]) {
            assert!(
                call.iter().any(|arg| arg == "--remote"),
                "auth add must use --remote: {call:?}"
            );
            assert!(
                !call.iter().any(|arg| arg == "--manual"),
                "auth add must not use --manual: {call:?}"
            );
        }
        assert!(
            !call.iter().any(|arg| arg.contains("secret")),
            "argv must not carry a secret: {call:?}"
        );
    }
}

#[test]
fn login_browser_end_to_end() {
    let env = GcalEnv::new("bob-cli-gcal-login-browser");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let browser = FakeBrowser::new(&env);
    let mut command = env.command();
    fake.install(&mut command);
    browser.install(&mut command);
    command.args(["gcal", "login", "-f", "json"]);
    let output = command.output().expect("run browser login");
    assert_success(&output, "login browser");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["sign_in"], "browser");
    let page = browser.page();
    assert!(
        page.contains("Bob can read your Google Calendar"),
        "expected success page:\n{page}"
    );
    let add = fake
        .calls()
        .into_iter()
        .find(|call| {
            call.windows(2).any(|w| w == ["auth", "add"])
                && call.iter().any(|arg| arg == "--step")
                && call.windows(2).any(|w| w == ["--step", "2"])
        })
        .expect("step 2");
    assert!(
        add.iter().any(|arg| arg == "--auth-url"),
        "step 2 must pass --auth-url: {add:?}"
    );
    let step1 = fake
        .calls()
        .into_iter()
        .find(|call| {
            call.windows(2).any(|w| w == ["auth", "add"])
                && call.windows(2).any(|w| w == ["--step", "1"])
        })
        .expect("step 1");
    assert!(
        step1.iter().any(|arg| arg == "--redirect-uri"),
        "browser step 1 must pass --redirect-uri: {step1:?}"
    );
    assert!(
        step1.iter().any(|arg| arg == "--force-consent"),
        "step 1 must force consent: {step1:?}"
    );
}

#[test]
fn login_browser_access_denied() {
    let env = GcalEnv::new("bob-cli-gcal-login-browser-denied");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let browser = FakeBrowser::new(&env);
    let mut command = env.command();
    fake.install(&mut command);
    browser.deny(&mut command);
    command.args(["gcal", "login", "-f", "json"]);
    let output = command.output().expect("run denied browser login");
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "consent_denied");
    let page = browser.page();
    assert!(
        page.contains("Sign-in didn't finish"),
        "expected failure page:\n{page}"
    );
}

#[test]
fn login_paste_without_tty_is_needs_tty() {
    let env = GcalEnv::new("bob-cli-gcal-login-nontty");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal(&env, &fake, &["login", "-m", "-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "needs_tty");
}

#[test]
fn login_rejects_account_without_at() {
    let env = GcalEnv::new("bob-cli-gcal-login-bad-email");
    env.write_config("gcal:\n  account: not-an-email\n");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["login"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("gcal.account"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn logout_force_removes_the_grant_and_prints_revoke_url() {
    let env = GcalEnv::new("bob-cli-gcal-logout");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["logout", "-y", "-f", "json"]);
    assert_success(&output, "logout -y");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["account"], ACCOUNT);
    assert_eq!(doc["client"], "bob-gcal");
    assert_eq!(
        doc["revoke_url"],
        "https://myaccount.google.com/permissions"
    );
    assert!(
        fake.called_with(&["auth", "remove", ACCOUNT]),
        "expected auth remove: {:?}",
        fake.calls()
    );
    let remove = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|w| w == ["auth", "remove"]))
        .expect("auth remove");
    assert!(
        remove.iter().any(|arg| arg == "--force"),
        "logout passes --force to gog: {remove:?}"
    );
    assert!(
        remove.iter().any(|arg| arg == "bob-gcal"),
        "logout is scoped to bob-gcal: {remove:?}"
    );
}

#[test]
fn logout_without_force_on_a_pipe_exits_2() {
    let env = GcalEnv::new("bob-cli-gcal-logout-nontty");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["logout"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("logout -y")
            || stderr(&output).contains("--force"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn doctor_json_healthy_setup() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-ok");
    env.prepare_healthy_vault();
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_success(&output, "doctor ok");
    let doc = json_stdout(&output);
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["host"], "test-host");
    let checks = doc["checks"].as_array().expect("checks");
    let ids: Vec<&str> = checks
        .iter()
        .map(|check| check["id"].as_str().expect("id"))
        .collect();
    assert_eq!(
        ids,
        [
            "gog",
            "client",
            "account",
            "token",
            "calendars",
            "default_root",
            "today",
            "journal",
            "journal_malformed",
            "writer",
            "last_sync",
        ]
    );
    for check in checks {
        if check["id"] == "last_sync" {
            assert_eq!(check["status"], "skip", "expected skip: {check}");
            continue;
        }
        assert_eq!(check["status"], "ok", "expected ok: {check}");
    }
}

#[test]
fn doctor_reports_gog_missing() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-gog-missing");
    let empty = env.root().join("empty-bin");
    fs::create_dir_all(&empty).expect("empty bin");
    let output = env
        .command()
        .env("PATH", &empty)
        .env_remove("BOB_GCAL_GOG")
        .args(["gcal", "doctor", "-f", "json"])
        .output()
        .expect("doctor without gog");
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], false);
    let gog = &doc["checks"][0];
    assert_eq!(gog["id"], "gog");
    assert_eq!(gog["status"], "fail");
    assert!(
        gog["summary"].as_str().unwrap_or_default().contains("gog"),
        "{gog}"
    );
}

#[test]
fn doctor_reports_too_old_gog() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-too-old");
    let fake = FakeGog::new(&env, "gog").scenario("too_old");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    let gog = &doc["checks"][0];
    assert_eq!(gog["id"], "gog");
    assert_eq!(gog["status"], "fail");
    assert!(
        gog["summary"].as_str().unwrap_or_default().contains("0.43"),
        "{gog}"
    );
}

#[test]
fn doctor_reports_no_client() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-no-client");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    let client = doc["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "client")
        .expect("client check");
    assert_eq!(client["status"], "fail");
}

#[test]
fn doctor_reports_not_logged_in() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-logged-out");
    let fake = FakeGog::new(&env, "gog").scenario("not_logged_in");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    assert_eq!(check(&doc, "account")["status"], "fail");
}

#[test]
fn doctor_maps_api_disabled_on_token_refresh() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-api");
    let fake = FakeGog::new(&env, "gog").scenario("api_disabled");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    let token = check(&doc, "token");
    assert_eq!(token["status"], "fail");
    let summary = token["summary"].as_str().unwrap_or_default();
    let fix = token["fix"].as_str().unwrap_or_default();
    assert!(
        summary.contains("Calendar API") || fix.contains("calendar-json"),
        "{token}"
    );
}

#[test]
fn doctor_maps_invalid_grant() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-grant");
    let fake = FakeGog::new(&env, "gog").scenario("invalid_grant");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    let blob = format!("{}", check(&doc, "token"));
    assert!(
        blob.contains("7 days")
            || blob.contains("invalid_grant")
            || blob.contains("publish"),
        "{blob}"
    );
}

#[test]
fn doctor_fails_invalid_default_root() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-root");
    env.write_today();
    env.write_journal_rules();
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    assert_eq!(check(&doc, "default_root")["status"], "fail");
}

#[test]
fn doctor_fails_missing_journal_rules() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-journal");
    env.write_meet_area();
    env.write_today();
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    let journal = check(&doc, "journal");
    assert_eq!(journal["status"], "fail");
    let summary = journal["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("gitignore") || summary.contains("gitattributes"),
        "{journal}"
    );
}

#[test]
fn doctor_fails_foreign_writer() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-writer");
    env.prepare_healthy_vault();
    env.write_config(&format!(
        "gcal:\n  account: {ACCOUNT}\n  writer: other-host\n"
    ));
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    let writer = check(&doc, "writer");
    assert_eq!(writer["status"], "fail");
    assert!(
        writer["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("other-host"),
        "{writer}"
    );
}

#[test]
fn calendars_table_and_json() {
    let env = GcalEnv::new("bob-cli-gcal-calendars");
    let fake = FakeGog::new(&env, "gog");
    let table = run_gcal(&env, &fake, &["calendars"]);
    assert_success(&table, "calendars table");
    let text = stdout(&table);
    for needle in ["ID", "NAME", "ROLE", "COLOUR", "SYNC", "ROOT", "Personal"] {
        assert!(text.contains(needle), "expected {needle} in:\n{text}");
    }
    assert!(text.contains("yes"), "primary should be synced:\n{text}");

    let json = run_gcal(&env, &fake, &["calendars", "-f", "json"]);
    assert_success(&json, "calendars json");
    let doc = json_stdout(&json);
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["ok"], true);
    let calendars = doc["calendars"].as_array().expect("calendars");
    assert!(calendars.len() >= 2);
    let primary = calendars
        .iter()
        .find(|row| row["primary"] == true)
        .expect("primary");
    assert_eq!(primary["synced"], true);
    assert_eq!(primary["root"], "meet");
    assert_eq!(primary["name"], "Personal");
}

#[test]
fn config_error_names_the_key_and_exits_2() {
    let env = GcalEnv::new("bob-cli-gcal-config-error");
    env.write_config("gcal:\n  timeout_secs: soon\n");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["calendars"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let err = stderr(&output);
    assert!(
        err.contains("gcal.timeout_secs"),
        "expected named key:\n{err}"
    );
}

#[test]
fn common_flags_precede_the_subcommand() {
    let env = GcalEnv::new("bob-cli-gcal-argv-order");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["calendars", "-f", "json"]);
    assert_success(&output, "calendars argv");
    let calendars = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|w| w == ["calendar", "calendars"]))
        .expect("calendars call");
    let calendar_at = calendars
        .iter()
        .position(|arg| arg == "calendar")
        .expect("calendar");
    let json_at = calendars
        .iter()
        .position(|arg| arg == "--json")
        .expect("json");
    let client_at = calendars
        .iter()
        .position(|arg| arg == "--client")
        .expect("client");
    assert!(json_at < calendar_at, "{calendars:?}");
    assert!(client_at < calendar_at, "{calendars:?}");
    assert_eq!(calendars[client_at + 1], "bob-gcal");
}
