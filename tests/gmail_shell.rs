//! `bob gmail` shell phase: noun group, config, login, logout, doctor.

mod gmail_support;

use std::fs;

use gmail_support::{
    json_stdout, run_gmail, run_gmail_paste, run_gmail_stdin, stderr, stdout,
    FakeGog, GmailEnv, ACCOUNT,
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
fn bare_lists_no_saved_threads_offline() {
    let env = GmailEnv::new("bob-cli-gmail-bare");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gmail(&env, &fake, &["-f", "json"]);
    assert_success(&output, "bare json");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["threads"].as_array().unwrap().len(), 0);
    assert!(
        !fake.called_with(&["gmail", "search"]),
        "bare list must make no Gmail calls:\n{:?}",
        fake.calls()
    );
}

#[test]
fn bare_human_lists_no_saved_threads() {
    let env = GmailEnv::new("bob-cli-gmail-bare-human");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gmail(&env, &fake, &[]);
    assert_success(&output, "bare human");
    assert!(stdout(&output).contains("No saved threads yet"));
    assert!(
        !fake.called_with(&["gmail", "search"]),
        "bare list must make no Gmail calls:\n{:?}",
        fake.calls()
    );
}

#[test]
fn login_existing_grant_is_already_connected() {
    let env = GmailEnv::new("bob-cli-gmail-login-existing");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gmail(&env, &fake, &["login", "-f", "json"]);
    assert_success(&output, "login existing");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["already_connected"], true);
    assert_eq!(doc["sign_in"], "already_connected");
    assert_eq!(doc["client_source"], "stored");
    assert_eq!(doc["account"], ACCOUNT);
    assert_eq!(doc["client"], "bob-gmail");
    assert_eq!(doc["scope"], "readonly");
    assert!(
        !fake.called_with(&["auth", "add"]),
        "already connected must skip auth add:\n{:?}",
        fake.calls()
    );
    assert!(
        fake.called_with(&["gmail", "search"]),
        "check step must fetch one search page:\n{:?}",
        fake.calls()
    );
}

#[test]
fn login_compose_requests_extra_scopes() {
    let env = GmailEnv::new("bob-cli-gmail-login-compose");
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let client = env.root().join("client.json");
    fs::write(
        &client,
        r#"{"installed":{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"test-secret-123","project_id":"my-proj"}}"#,
    )
    .expect("write client json");
    let output = run_gmail_paste(
        &env,
        &fake,
        &[
            "login",
            "-c",
            client.to_str().expect("utf8 path"),
            "-s",
            "compose",
            "-f",
            "json",
        ],
    );
    assert_success(&output, "login compose");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["scope"], "compose");
    let auth_add = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|w| w == ["auth", "add"]))
        .expect("auth add call");
    assert!(
        auth_add.iter().any(|arg| arg == "--gmail-scope"),
        "login must request --gmail-scope readonly: {auth_add:?}"
    );
    assert!(
        auth_add.iter().any(|arg| arg == "readonly"),
        "login must request readonly scope: {auth_add:?}"
    );
    assert!(
        auth_add.iter().any(|arg| arg == "--extra-scopes"),
        "compose login must pass --extra-scopes: {auth_add:?}"
    );
    assert!(
        auth_add.iter().any(|arg| arg.contains("gmail.compose")),
        "compose login must request the compose URI: {auth_add:?}"
    );
}

#[test]
fn logout_removes_only_bob_gmail_grant() {
    let env = GmailEnv::new("bob-cli-gmail-logout");
    let fake = FakeGog::new(&env, "gog");
    let output =
        run_gmail_stdin(&env, &fake, &["logout", "-y", "-f", "json"], "");
    assert_success(&output, "logout");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["account"], ACCOUNT);
    assert_eq!(doc["client"], "bob-gmail");
    assert!(
        fake.called_with(&["auth", "remove"]),
        "logout must call auth remove:\n{:?}",
        fake.calls()
    );
}

#[test]
fn doctor_reports_healthy_setup() {
    let env = GmailEnv::new("bob-cli-gmail-doctor-ok");
    env.write_journal_rules();
    let fake = FakeGog::new(&env, "gog");
    let output = run_gmail(&env, &fake, &["doctor", "-f", "json"]);
    assert_success(&output, "doctor ok");
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(check(&doc, "gog")["status"], "ok");
    assert_eq!(check(&doc, "client")["status"], "ok");
    assert_eq!(check(&doc, "account")["status"], "ok");
    assert_eq!(check(&doc, "token")["status"], "ok");
    assert_eq!(check(&doc, "scopes")["status"], "ok");
}

#[test]
fn doctor_maps_invalid_grant() {
    let env = GmailEnv::new("bob-cli-gmail-doctor-grant");
    env.write_journal_rules();
    let fake = FakeGog::new(&env, "gog").scenario("invalid_grant");
    let output = run_gmail(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    assert_eq!(doc["ok"], false);
    assert_eq!(check(&doc, "token")["status"], "fail");
    assert!(
        check(&doc, "token")["fix"]
            .as_str()
            .unwrap_or_default()
            .contains("bob gmail login"),
        "invalid_grant must point at login: {doc}"
    );
}

#[test]
fn doctor_reports_no_client() {
    let env = GmailEnv::new("bob-cli-gmail-doctor-noclient");
    env.write_journal_rules();
    let fake = FakeGog::new(&env, "gog").scenario("no_client");
    let output = run_gmail(&env, &fake, &["doctor", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc = json_stdout(&output);
    assert_eq!(check(&doc, "client")["status"], "fail");
}

#[test]
fn config_errors_name_the_key() {
    let env = GmailEnv::new("bob-cli-gmail-config-key");
    let fake = FakeGog::new(&env, "gog");
    env.write_config("gmail:\n  timeout_secs: soon\n");
    let output = run_gmail(&env, &fake, &["doctor", "-f", "json"]);
    let doc = json_stdout(&output);
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("gmail.timeout_secs")
            || doc["checks"].as_array().is_some_and(|checks| checks
                .iter()
                .any(|row| row["summary"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("gmail.timeout_secs"))),
        "config error must name the key: {doc}"
    );
}

#[test]
fn gmail_reads_use_read_only_no_send_flags() {
    let env = GmailEnv::new("bob-cli-gmail-read-flags");
    let fake = FakeGog::new(&env, "gog");
    let output = run_gmail(&env, &fake, &["login", "-f", "json"]);
    assert_success(&output, "login flags");
    let search = fake
        .calls()
        .into_iter()
        .find(|call| call.windows(2).any(|w| w == ["gmail", "search"]))
        .expect("search call");
    assert!(
        search.iter().any(|arg| arg == "--readonly"),
        "reads must pass --readonly: {search:?}"
    );
    assert!(
        search.iter().any(|arg| arg == "--gmail-no-send"),
        "reads must pass --gmail-no-send: {search:?}"
    );
    assert!(
        search.iter().any(|arg| arg == "--enable-commands"),
        "reads must restrict --enable-commands: {search:?}"
    );
    assert!(
        !search.iter().any(|arg| arg == "--sanitize-content"),
        "reads must never pass --sanitize-content: {search:?}"
    );
    assert!(
        !search.iter().any(|arg| arg == "--wrap-untrusted"),
        "reads must never pass --wrap-untrusted: {search:?}"
    );
}
