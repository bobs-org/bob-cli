//! `bob gcal login`, `logout`, `doctor`, and `calendars` integration tests.
//!
//! FakeGog records argv and answers from a scenario file. Nothing here
//! touches live Google.

mod gcal_support;

use std::{
    fs,
    io::{Read, Write},
};

use gcal_support::{
    hit_oauth_callback, json_stdout, run_gcal, run_gcal_paste,
    run_gcal_paste_script, run_gcal_paste_script_with, run_gcal_stdin, stderr,
    stdout, wait_printed_auth_url, FakeBrowser, FakeGog, GcalEnv, ACCOUNT,
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
    assert_eq!(doc["client_source"], "stored");
    assert_eq!(doc["client_project"], Value::Null);
    assert_eq!(doc["reconnected"], false);
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
fn login_prints_the_guided_checklist_without_a_client() {
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
        err.contains("Turn on the Calendar API"),
        "expected checklist step 1:\n{err}"
    );
    assert!(
        err.contains("External") && err.contains("publish it"),
        "expected checklist step 2:\n{err}"
    );
    assert!(
        err.contains("Desktop app"),
        "expected checklist step 3:\n{err}"
    );
    assert!(
        err.contains("bob gcal login")
            && err.contains("console.cloud.google.com/auth/clients/create"),
        "expected guided setup links:\n{err}"
    );
}

#[test]
fn login_self_heals_a_stale_saved_grant() {
    let env = GcalEnv::new("bob-cli-gcal-login-self-heal");
    let fake = FakeGog::new(&env, "gog").scenario("stale_grant");
    let output = run_gcal_paste(&env, &fake, &["login", "-f", "json"]);
    assert_success(&output, "login self-heal");
    let doc = json_stdout(&output);
    assert_eq!(doc["reconnected"], true);
    assert_eq!(doc["sign_in"], "paste");
    assert_eq!(doc["already_connected"], false);
    assert!(
        fake.calls().iter().any(|call| {
            call.windows(2).any(|window| window == ["--step", "2"])
        }),
        "self-heal must perform remote step 2: {:?}",
        fake.calls()
    );
}

#[test]
fn login_retries_calendar_api_activation_on_a_tty() {
    let env = GcalEnv::new("bob-cli-gcal-login-api-retry");
    let fake = FakeGog::new(&env, "gog").scenario("api_disabled_once");
    let output = run_gcal_stdin(&env, &fake, &["login", "-f", "json"], "\n");
    assert_success(&output, "login API retry");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert!(stderr(&output).contains("Press Enter to check again"));
    assert!(stderr(&output).contains("project=123456789012"));
    let calendar_calls = fake
        .calls()
        .iter()
        .filter(|call| {
            call.windows(2)
                .any(|window| window == ["calendar", "calendars"])
        })
        .count();
    assert_eq!(calendar_calls, 2, "expected a retry: {:?}", fake.calls());
}

#[test]
fn login_api_disabled_without_tty_returns_project_url() {
    let env = GcalEnv::new("bob-cli-gcal-login-api-nontty");
    let fake = FakeGog::new(&env, "gog").scenario("api_disabled");
    let output = run_gcal(&env, &fake, &["login", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "api_disabled");
    let hint = doc["error"]["hint"].as_str().unwrap_or_default();
    assert!(hint.contains("project=123456789012"), "{doc}");
    assert!(
        hint.contains("your sign-in is kept")
            && hint.contains("bob gcal login"),
        "non-TTY API-disabled hint must keep the grant: {doc}"
    );
}

#[test]
fn login_guides_pasted_client_json_and_adds_json_fields() {
    let env = GcalEnv::new("bob-cli-gcal-login-pasted-client");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let raw = r#"{"installed":{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"pasted-secret","project_id":"my-project"}}"#;
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-f", "json"],
        &format!("{raw}\n{{callback}}\n"),
    );
    assert_success(&output, "guided pasted JSON");
    let doc = json_stdout(&output);
    assert_eq!(doc["client_source"], "pasted");
    assert_eq!(doc["client_project"], "my-project");
    assert_eq!(doc["reconnected"], false);
    assert!(!stderr(&output).contains("pasted-secret"));
}

#[test]
fn login_discovers_a_downloaded_client() {
    let env = GcalEnv::new("bob-cli-gcal-login-downloads");
    let downloads = env.root().join("home").join("Downloads");
    fs::create_dir_all(&downloads).expect("create Downloads");
    fs::write(
        downloads.join("client_secret_download.json"),
        r#"{"installed":{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"download-secret","project_id":"download-project"}}"#,
    )
    .expect("write downloaded client");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-f", "json"],
        "\n{callback}\n",
    );
    assert_success(&output, "downloaded client");
    let doc = json_stdout(&output);
    assert_eq!(doc["client_source"], "downloads");
    assert_eq!(doc["client_project"], "download-project");
}

#[test]
fn login_account_picker_accepts_a_number() {
    let env = GcalEnv::new("bob-cli-gcal-login-account-picker");
    env.write_config("gcal:\n  writer: test-host\n");
    let fake = FakeGog::new(&env, "gog").scenario("picker_accounts");
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-f", "json"],
        "2\n{callback}\n",
    );
    assert_success(&output, "account picker");
    let doc = json_stdout(&output);
    assert_eq!(doc["account"], "work@example.com");
}

#[test]
fn login_suggests_config_when_bob_accounts_remain_ambiguous() {
    let env = GcalEnv::new("bob-cli-gcal-login-account-tip");
    env.write_config("gcal:\n  writer: test-host\n");
    let fake = FakeGog::new(&env, "gog").scenario("two_bob_accounts");
    let output =
        run_gcal_paste_script(&env, &fake, &["login"], "2\n{callback}\n");
    assert_success(&output, "account tip");
    assert!(fake.called_with(&["auth", "add", "work@example.com"]));
    let err = stderr(&output);
    assert!(
        err.contains("add this to ~/.config/bob/config.yml"),
        "{err}"
    );
    assert!(err.contains("account: work@example.com"), "{err}");
}

#[test]
fn login_human_transcript_stays_on_stderr_and_mentions_remote_writer() {
    let env = GcalEnv::new("bob-cli-gcal-login-human-transcript");
    env.write_config("gcal:\n  account: you@example.com\n  writer: the-mac\n");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal_paste(&env, &fake, &["login"]);
    assert_success(&output, "human login");
    assert!(stdout(&output).is_empty(), "human mode wrote stdout");
    let err = stderr(&output);
    assert!(err.contains("Google Calendar · read-only"), "{err}");
    assert!(err.contains("sync runs on the-mac"), "{err}");
    assert!(err.contains("Connected. Bob can read"), "{err}");
    assert!(
        err.contains("Next  bob gcal sync -d     preview today's Events\n        bob gcal calendars   choose which calendars to sync\n  Undo  bob gcal logout      removes only Bob's Calendar grant"),
        "expected the stable summary block:\n{err}"
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
    let steps: Vec<_> = fake
        .calls()
        .into_iter()
        .filter(|call| call.windows(2).any(|w| w == ["auth", "add"]))
        .collect();
    assert_eq!(steps.len(), 2, "expected remote steps 1 and 2: {steps:?}");
    for step in &steps {
        assert!(
            step.iter().any(|arg| arg == "--force-consent"),
            "both remote steps must pass --force-consent: {step:?}"
        );
        assert!(
            step.iter().any(|arg| arg == "--redirect-uri"),
            "both remote steps must pass --redirect-uri: {step:?}"
        );
    }
    let step2 = steps
        .iter()
        .find(|call| call.windows(2).any(|w| w == ["--step", "2"]))
        .expect("step 2");
    let auth_url = step2
        .windows(2)
        .find(|window| window[0] == "--auth-url")
        .map(|window| window[1].as_str())
        .expect("step 2 --auth-url");
    assert!(
        auth_url.contains("127.0.0.1:"),
        "callback must keep the bound port: {auth_url}"
    );
    assert!(
        !auth_url.starts_with("http://127.0.0.1/oauth2/callback"),
        "callback must not drop the port: {auth_url}"
    );
    assert!(
        auth_url.contains("%2B") && auth_url.contains("%2F"),
        "callback must keep percent-encoded code/state: {auth_url}"
    );
}

#[test]
fn login_ssh_auto_selects_paste() {
    let env = GcalEnv::new("bob-cli-gcal-login-ssh");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal_paste_script_with(
        &env,
        &fake,
        &["login", "-f", "json"],
        "{callback}\n",
        |command| {
            command.env("SSH_CONNECTION", "1.2.3.4 22 5.6.7.8 22");
        },
    );
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
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-m", "-f", "json"],
        "not-a-url\n{callback}\n",
    );
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
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-m", "-f", "json"],
        "{denied}\nq\n",
    );
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "cancelled");
}

#[test]
fn login_paste_stale_state_then_fresh_link() {
    let env = GcalEnv::new("bob-cli-gcal-login-stale");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-m", "-f", "json"],
        "http://127.0.0.1:36015/oauth2/callback?state=oldstate&code=fakecode\n\
         {callback}\n",
    );
    assert_success(&output, "login stale then fresh");
    let err = stderr(&output);
    assert!(
        err.contains("older sign-in"),
        "expected stale-link message:\n{err}"
    );
    assert!(
        err.contains("https://accounts.google.com/o/oauth2/auth"),
        "stale paste must reprint the current link:\n{err}"
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
fn login_already_connected_human_transcript_includes_step_two_and_summary() {
    let env = GcalEnv::new("bob-cli-gcal-login-already-human");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["login"]);
    assert_success(&output, "already connected human");
    assert!(stdout(&output).is_empty(), "human mode wrote stdout");
    let err = stderr(&output);
    assert!(
        err.contains("2/3  Sign in") && err.contains("already connected"),
        "rerun must render step 2:\n{err}"
    );
    assert!(
        err.contains("3/3  Check"),
        "rerun must still check calendars:\n{err}"
    );
    assert!(
        err.contains("Connected. Bob can read"),
        "rerun must print the summary:\n{err}"
    );
    assert!(
        err.contains("Next  bob gcal sync -d")
            && err.contains("Undo  bob gcal logout"),
        "rerun must print Next/Undo:\n{err}"
    );
}

#[test]
fn login_check_accepts_primary_calendar_real_id() {
    let env = GcalEnv::new("bob-cli-gcal-login-primary-id");
    env.write_config(&format!(
        "gcal:\n  account: {ACCOUNT}\n  writer: test-host\n  calendars:\n    - id: {ACCOUNT}\n"
    ));
    let fake = FakeGog::new(&env, "gog");
    let output = run_gcal(&env, &fake, &["login"]);
    assert_success(&output, "primary real id");
    let err = stderr(&output);
    assert!(
        !err.contains("configured calendars not found"),
        "primary calendar real id must match:\n{err}"
    );
}

#[test]
fn login_paste_denied_then_enter_retries_with_a_fresh_state() {
    let env = GcalEnv::new("bob-cli-gcal-login-denied-retry");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-m", "-f", "json"],
        "{denied}\n\n{callback}\n",
    );
    assert_success(&output, "denied then retry");
    let step1: Vec<_> = fake
        .calls()
        .into_iter()
        .filter(|call| {
            call.windows(2).any(|w| w == ["auth", "add"])
                && call.windows(2).any(|w| w == ["--step", "1"])
        })
        .collect();
    assert_eq!(
        step1.len(),
        2,
        "denial retry must begin a new step 1: {step1:?}"
    );
    let uri = |call: &Vec<String>| {
        call.windows(2)
            .find(|window| window[0] == "--redirect-uri")
            .map(|window| window[1].clone())
            .expect("redirect-uri")
    };
    assert_ne!(
        uri(&step1[0]),
        uri(&step1[1]),
        "fresh retry must bind a new redirect URI: {step1:?}"
    );
}

#[test]
fn login_paste_retry_exhaustion_is_stale_link() {
    let env = GcalEnv::new("bob-cli-gcal-login-retry-exhaust");
    let fake = FakeGog::new(&env, "gog").scenario("exchange_invalid_grant");
    let output = run_gcal_paste_script(
        &env,
        &fake,
        &["login", "-m", "-f", "json"],
        "{callback}\n{callback}\n",
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "stale_link");
}

#[test]
fn login_downloads_q_cancels_and_junk_reprompts() {
    let env = GcalEnv::new("bob-cli-gcal-login-downloads-q");
    let downloads = env.root().join("home").join("Downloads");
    fs::create_dir_all(&downloads).expect("create Downloads");
    fs::write(
        downloads.join("client_secret_download.json"),
        r#"{"installed":{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"download-secret","project_id":"download-project"}}"#,
    )
    .expect("write downloaded client");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let output =
        run_gcal_stdin(&env, &fake, &["login", "-f", "json"], "maybe\nq\n");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let doc = json_stdout(&output);
    assert_eq!(doc["error"]["kind"], "cancelled");
    assert!(
        stderr(&output).contains("Please answer Y, n, or q."),
        "junk must re-prompt:\n{}",
        stderr(&output)
    );
}

#[test]
fn login_browser_denied_then_retry_succeeds() {
    let env = GcalEnv::new("bob-cli-gcal-login-browser-retry");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let browser = FakeBrowser::new(&env);
    let mut command = env.command();
    fake.install(&mut command);
    browser.deny_once(&mut command);
    command.env("BOB_GCAL_ASSUME_TTY", "1");
    command.args(["gcal", "login", "-f", "json"]);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn browser retry");
    let write_result =
        child.stdin.take().expect("piped stdin").write_all(b"\n");
    if let Err(error) = write_result {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "write stdin");
    }
    let output = child.wait_with_output().expect("wait browser retry");
    assert_success(&output, "browser denied then retry");
    let doc = json_stdout(&output);
    assert_eq!(doc["sign_in"], "browser");
    let page = browser.page();
    assert!(
        page.contains("Bob can read your Google Calendar"),
        "expected success page after retry:\n{page}"
    );
}

#[test]
fn login_browser_opener_spawn_fail_prints_fallback_and_accepts_callback() {
    let env = GcalEnv::new("bob-cli-gcal-login-opener-fail");
    let fake = FakeGog::new(&env, "gog").scenario("needs_auth");
    let mut command = env.command();
    fake.install(&mut command);
    command.env("BOB_GCAL_BROWSER", env.root().join("missing-browser"));
    command.env("BOB_GCAL_CONSENT_SECS", "30");
    command.args(["gcal", "login"]);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn opener-fail login");
    let stderr_pipe = child.stderr.take().expect("piped stderr");
    let printed = wait_printed_auth_url(stderr_pipe);
    hit_oauth_callback(&printed.url);
    let mut stdout_buf = Vec::new();
    if let Some(mut handle) = child.stdout.take() {
        handle.read_to_end(&mut stdout_buf).expect("read stdout");
    }
    let status = child.wait().expect("wait opener-fail login");
    let stderr_buf = printed.finish();
    let output = std::process::Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    };
    assert_success(&output, "opener spawn fail");
    let err = stderr(&output);
    assert!(
        err.contains("couldn't open a browser"),
        "spawn failure must print the fallback hint:\n{err}"
    );
}

#[test]
fn logout_case_insensitive_duplicate_emails_count_as_one() {
    let env = GcalEnv::new("bob-cli-gcal-logout-dup-case");
    env.write_config("gcal:\n  writer: test-host\n");
    let fake = FakeGog::new(&env, "gog").scenario("dup_case_accounts");
    let output = run_gcal(&env, &fake, &["logout", "-y", "-f", "json"]);
    assert_success(&output, "logout dup case");
    let doc = json_stdout(&output);
    assert_eq!(doc["account"], ACCOUNT);
    assert!(
        fake.called_with(&["auth", "remove", ACCOUNT]),
        "logout must remove the first unique email: {:?}",
        fake.calls()
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
    assert_eq!(client["fix"], "run `bob gcal login`");
}

#[test]
fn doctor_shows_the_stored_client_project() {
    let env = GcalEnv::new("bob-cli-gcal-doctor-client-project");
    let fake = FakeGog::new(&env, "gog").scenario("stored_project");
    let output = run_gcal(&env, &fake, &["doctor", "-f", "json"]);
    let doc = json_stdout(&output);
    assert_eq!(check(&doc, "client")["status"], "ok");
    assert!(check(&doc, "client")["summary"]
        .as_str()
        .unwrap_or_default()
        .contains("project doctor-project"));
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
    assert!(blob.contains("signs in again automatically"), "{blob}");
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
fn calendars_prefers_the_sole_bob_account_over_multiple_gog_accounts() {
    let env = GcalEnv::new("bob-cli-gcal-calendars-bob-account");
    env.write_config("gcal:\n  writer: test-host\n");
    let fake = FakeGog::new(&env, "gog").scenario("two_default_accounts");
    let output = run_gcal(&env, &fake, &["calendars", "-f", "json"]);
    assert_success(&output, "calendars account resolution");
    assert!(
        fake.called_with(&["--account", ACCOUNT]),
        "expected the sole bob-gcal account despite multiple default accounts: {:?}",
        fake.calls()
    );
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
