//! `bob gkeep login` and `bob gkeep doctor` integration tests.
//!
//! The fake adapter answers `exchange`, `snapshot`, and `ping`; stub
//! shell scripts backed by a temp file stand in for the token store and
//! reader. Nothing here touches live Keep.

mod gkeep_support;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use gkeep_support::{
    exchange_ok, note, ping_ok, snapshot_ok, stderr, stdout, FakeAdapter,
    GkeepEnv,
};

/// A `bob` command pointed at the fake adapter with isolated state and
/// cache dirs under the env's temp root.
fn auth_command(env: &GkeepEnv, fake: &FakeAdapter) -> Command {
    let mut command = env.command();
    fake.install(&mut command);
    let root = env
        .vault()
        .parent()
        .expect("vault has a parent")
        .to_path_buf();
    command.env("XDG_STATE_HOME", root.join("state"));
    command.env("XDG_CACHE_HOME", root.join("cache"));
    command
}

/// The temp root behind `env` (`<root>/vault` is the vault).
fn root_of(env: &GkeepEnv) -> PathBuf {
    env.vault()
        .parent()
        .expect("vault has a parent")
        .to_path_buf()
}

/// Single-quote a path for shell and YAML embedding.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// Make `path` executable on unix.
fn chmod_exec(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions =
            fs::metadata(path).expect("stat script").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod script");
    }
}

/// A `token_command` stub that prints `token`.
fn write_token_reader(root: &Path, token: &str) -> PathBuf {
    let path = root.join("read-token.sh");
    fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{token}'\n"))
        .expect("write token reader");
    chmod_exec(&path);
    path
}

/// A `token_store_command` stub that saves stdin to `token_file`.
fn write_token_store(root: &Path, token_file: &Path) -> PathBuf {
    let path = root.join("store-token.sh");
    fs::write(
        &path,
        format!("#!/bin/sh\ncat > {}\n", shell_quote(token_file)),
    )
    .expect("write token store");
    chmod_exec(&path);
    path
}

/// A `token_store_command` stub that always fails.
fn write_failing_store(root: &Path) -> PathBuf {
    let path = root.join("failing-store.sh");
    fs::write(&path, "#!/bin/sh\nexit 3\n").expect("write failing store");
    chmod_exec(&path);
    path
}

/// Overwrite the env config with the auth token commands.
fn write_auth_config(env: &GkeepEnv, token_command: &str, store_command: &str) {
    env.write_config(&format!(
        "gkeep:\n  email: bryanbugyi34@gmail.com\n  token_command: \
         {token_command}\n  token_store_command: {store_command}\n"
    ));
}

/// Run `bob gkeep login` with `stdin_text` on stdin.
fn run_login(
    env: &GkeepEnv,
    fake: &FakeAdapter,
    extra_args: &[&str],
    stdin_text: &str,
) -> Output {
    let mut command = auth_command(env, fake);
    command.arg("gkeep").arg("login");
    for arg in extra_args {
        command.arg(arg);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gkeep login");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin_text.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for login")
}

/// Run `bob gkeep doctor`.
fn run_doctor(
    env: &GkeepEnv,
    fake: &FakeAdapter,
    extra_args: &[&str],
) -> Output {
    let mut command = auth_command(env, fake);
    command.arg("gkeep").arg("doctor");
    for arg in extra_args {
        command.arg(arg);
    }
    command.output().expect("run bob gkeep doctor")
}

/// A minimal target note with a Tasks section.
fn write_target(env: &GkeepEnv) {
    fs::write(
        env.vault().join("gkeep_inbox.md"),
        "---\ntitle: inbox\n---\n\n- intro\n\n## Tasks\n",
    )
    .expect("write target note");
}

#[test]
fn login_via_stdin_exchanges_stores_and_verifies() {
    let env = GkeepEnv::new("bob-cli-gkeep-login-ok");
    let root = root_of(&env);
    let token_file = root.join("token.txt");
    let reader = write_token_reader(&root, "unused-until-stored");
    let store = write_token_store(&root, &token_file);
    // The reader prints whatever the store saved.
    fs::write(
        &reader,
        format!("#!/bin/sh\ncat {}\n", shell_quote(&token_file)),
    )
    .expect("rewrite token reader");
    chmod_exec(&reader);
    write_auth_config(
        &env,
        &reader.to_string_lossy(),
        &store.to_string_lossy(),
    );

    let fake = FakeAdapter::new(&env, "login-ok");
    fake.respond("exchange", &exchange_ok("aas_et/fresh-token"));
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("Call dentist").id("n1").build(),
                note("Hardware store").id("n2").build(),
            ],
        ),
    );

    let output = run_login(&env, &fake, &[], "oauth2_4/test-cookie\n");
    assert!(
        output.status.success(),
        "expected success:\nstdout:\n{}\nstderr:\n{}",
        stdout(&output),
        stderr(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("exchanged for a master token"),
        "exchange line:\n{out}"
    );
    assert!(
        out.contains("stored and readable via")
            && out.contains(&reader.to_string_lossy().into_owned()),
        "store line names the token command:\n{out}"
    );
    assert!(
        out.contains("Google Keep reachable · 2 notes in inbox"),
        "reachability line:\n{out}"
    );

    // The token reached the store file and matches the exchange result.
    let stored = fs::read_to_string(&token_file).expect("read stored token");
    assert_eq!(stored.trim(), "aas_et/fresh-token");

    // The cookie traveled on stdin, never in argv; snapshot verified.
    let exchange = fake.request("exchange", 1);
    assert!(
        exchange.contains("oauth2_4/test-cookie"),
        "cookie on stdin:\n{exchange}"
    );
    assert_eq!(fake.argv("exchange", 1), "");
    assert_eq!(fake.call_count(), 2);
}

#[test]
fn login_email_override_wins_and_warns_on_cookie_shape() {
    let env = GkeepEnv::new("bob-cli-gkeep-login-override");
    let root = root_of(&env);
    let token_file = root.join("token.txt");
    let store = write_token_store(&root, &token_file);
    let reader = root.join("read-token.sh");
    fs::write(
        &reader,
        format!("#!/bin/sh\ncat {}\n", shell_quote(&token_file)),
    )
    .expect("write token reader");
    chmod_exec(&reader);
    write_auth_config(
        &env,
        &reader.to_string_lossy(),
        &store.to_string_lossy(),
    );

    let fake = FakeAdapter::new(&env, "login-override");
    fake.respond("exchange", &exchange_ok("aas_et/fresh-token"));
    fake.respond(
        "snapshot",
        &snapshot_ok("other@example.com", vec![note("Solo").id("n9").build()]),
    );

    let output =
        run_login(&env, &fake, &["-e", "other@example.com"], "not-a-cookie\n");
    assert!(
        output.status.success(),
        "warn-but-continue succeeds:\nstdout:\n{}\nstderr:\n{}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("oauth2_4"),
        "shape warning:\n{}",
        stderr(&output)
    );
    let exchange = fake.request("exchange", 1);
    assert!(
        exchange.contains("other@example.com"),
        "override email exchanged:\n{exchange}"
    );
    assert!(
        stdout(&output).contains("1 note in inbox"),
        "singular count:\n{}",
        stdout(&output)
    );
}

#[test]
fn login_preflight_failure_leaves_the_cookie_unconsumed() {
    let env = GkeepEnv::new("bob-cli-gkeep-login-preflight");
    write_auth_config(
        &env,
        "cat /nonexistent/token.txt",
        "/nonexistent-bob-store-cmd",
    );
    let fake = FakeAdapter::new(&env, "login-preflight");
    fake.respond("exchange", &exchange_ok("aas_et/unused"));

    let output = run_login(&env, &fake, &[], "oauth2_4/single-use-cookie\n");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("token store command not found"),
        "preflight error:\n{}",
        stderr(&output)
    );
    assert_eq!(fake.call_count(), 0, "no exchange call consumed the cookie");
}

#[test]
fn login_missing_email_is_a_setup_error() {
    let env = GkeepEnv::new("bob-cli-gkeep-login-no-email");
    env.write_config("gkeep:\n  target: gkeep_inbox.md\n");
    let fake = FakeAdapter::new(&env, "login-no-email");
    fake.respond("exchange", &exchange_ok("aas_et/unused"));

    let output = run_login(&env, &fake, &[], "oauth2_4/cookie\n");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("gkeep.email"),
        "email hint:\n{}",
        stderr(&output)
    );
    assert_eq!(fake.call_count(), 0);
}

#[test]
fn login_store_failure_writes_a_recovery_file() {
    let env = GkeepEnv::new("bob-cli-gkeep-login-recovery");
    let root = root_of(&env);
    let reader = write_token_reader(&root, "aas_et/stale");
    let store = write_failing_store(&root);
    write_auth_config(
        &env,
        &reader.to_string_lossy(),
        &store.to_string_lossy(),
    );
    let fake = FakeAdapter::new(&env, "login-recovery");
    fake.respond("exchange", &exchange_ok("aas_et/fresh-token"));

    let output = run_login(&env, &fake, &[], "oauth2_4/test-cookie\n");
    assert_eq!(output.status.code(), Some(1));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        !combined.contains("aas_et/fresh-token"),
        "the token is never printed:\n{combined}"
    );
    assert!(
        combined.contains("master_token.recovered")
            && combined.contains("delete the file"),
        "recovery instructions:\n{combined}"
    );

    let recovered = root
        .join("state")
        .join("bob-cli")
        .join("gkeep")
        .join("master_token.recovered");
    let saved = fs::read_to_string(&recovered).expect("recovery file");
    assert_eq!(saved.trim(), "aas_et/fresh-token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&recovered)
            .expect("stat recovery")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "recovery file is 0600");
    }
}

#[test]
fn login_readback_mismatch_writes_a_recovery_file() {
    let env = GkeepEnv::new("bob-cli-gkeep-login-mismatch");
    let root = root_of(&env);
    // The store saves a different token than the exchange produced.
    let reader = write_token_reader(&root, "aas_et/other-token");
    let store = root.join("store-token.sh");
    fs::write(&store, "#!/bin/sh\ncat >/dev/null\nexit 0\n")
        .expect("write store stub");
    chmod_exec(&store);
    write_auth_config(
        &env,
        &reader.to_string_lossy(),
        &store.to_string_lossy(),
    );
    let fake = FakeAdapter::new(&env, "login-mismatch");
    fake.respond("exchange", &exchange_ok("aas_et/fresh-token"));

    let output = run_login(&env, &fake, &[], "oauth2_4/test-cookie\n");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("did not match"),
        "mismatch error:\n{}",
        stderr(&output)
    );
    let recovered = root
        .join("state")
        .join("bob-cli")
        .join("gkeep")
        .join("master_token.recovered");
    let saved = fs::read_to_string(&recovered).expect("recovery file");
    assert_eq!(saved.trim(), "aas_et/fresh-token");
}

#[test]
fn doctor_all_ok_reports_the_checklist() {
    let env = GkeepEnv::new("bob-cli-gkeep-doctor-ok");
    let root = root_of(&env);
    let reader = write_token_reader(&root, "aas_et/test-master-token");
    write_auth_config(&env, &reader.to_string_lossy(), "true");
    write_target(&env);
    assert!(
        Command::new("git")
            .arg("init")
            .arg(env.vault())
            .output()
            .expect("git init")
            .status
            .success(),
        "git init for the worktree check"
    );
    // A fresh state cache reads as just synced.
    let cache = root.join("cache").join("bob-cli").join("gkeep");
    fs::create_dir_all(&cache).expect("create cache dir");
    fs::write(cache.join("state.json"), "{}").expect("write state cache");

    let fake = FakeAdapter::new(&env, "doctor-ok");
    fake.respond("ping", &ping_ok());
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("Call dentist").id("n1").build(),
                note("Pinned").id("p1").pinned().build(),
            ],
        ),
    );

    let output = run_doctor(&env, &fake, &[]);
    assert!(
        output.status.success(),
        "expected success:\nstdout:\n{}\nstderr:\n{}",
        stdout(&output),
        stderr(&output)
    );
    let out = stdout(&output);
    assert!(out.contains("Google Keep doctor"), "header:\n{out}");
    for row in [
        "config", "account", "token", "adapter", "keep", "target", "git",
    ] {
        assert!(out.contains(row), "missing {row} row:\n{out}");
    }
    assert!(
        out.contains("master token (aas_et/…)"),
        "token shape label:\n{out}"
    );
    assert!(
        out.contains("reachable · 2 in inbox · 1 pinned"),
        "keep counts:\n{out}"
    );
    assert!(out.contains("just synced"), "state cache age:\n{out}");
    assert!(out.contains("Tasks section found"), "target row:\n{out}");
    assert!(out.contains("Git worktree"), "git row:\n{out}");
    assert!(out.contains("ok all checks passed"), "summary:\n{out}");
    assert!(
        !out.contains("aas_et/test-master-token"),
        "no token in output:\n{out}"
    );
    assert!(
        !out.contains("\u{1b}[") && !stderr(&output).contains("\u{1b}["),
        "plain output when piped"
    );
}

#[test]
fn doctor_cookie_token_fails_and_keep_skips() {
    let env = GkeepEnv::new("bob-cli-gkeep-doctor-cookie");
    let root = root_of(&env);
    let reader = write_token_reader(&root, "oauth2_4/stale-cookie");
    write_auth_config(&env, &reader.to_string_lossy(), "true");
    write_target(&env);

    let fake = FakeAdapter::new(&env, "doctor-cookie");
    fake.respond("ping", &ping_ok());

    let output = run_doctor(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(1));
    let out = stdout(&output);
    assert!(out.contains("sign-in cookie"), "cookie failure:\n{out}");
    assert!(out.contains("bob gkeep login"), "login hint:\n{out}");
    assert!(
        out.contains("skipped"),
        "keep skips without a token:\n{out}"
    );
    assert!(
        out.contains("error 1 check failed"),
        "failure summary:\n{out}"
    );
    assert!(
        !out.contains("oauth2_4/stale-cookie"),
        "no token in output:\n{out}"
    );
    assert_eq!(
        fake.call_count(),
        1,
        "only ping ran; snapshot never attempted"
    );
}

#[test]
fn doctor_adapter_crash_skips_keep() {
    let env = GkeepEnv::new("bob-cli-gkeep-doctor-crash");
    let root = root_of(&env);
    let reader = write_token_reader(&root, "aas_et/test-master-token");
    write_auth_config(&env, &reader.to_string_lossy(), "true");
    write_target(&env);

    let fake = FakeAdapter::new(&env, "doctor-crash");
    fake.set_exit("ping", 3);

    let output = run_doctor(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(1));
    let out = stdout(&output);
    assert!(out.contains("crashed (exit 3)"), "crash reported:\n{out}");
    assert!(out.contains("skipped"), "keep skips:\n{out}");
    assert_eq!(fake.call_count(), 1);
}

#[test]
fn doctor_missing_target_fails() {
    let env = GkeepEnv::new("bob-cli-gkeep-doctor-target");
    let root = root_of(&env);
    let reader = write_token_reader(&root, "aas_et/test-master-token");
    write_auth_config(&env, &reader.to_string_lossy(), "true");
    // No gkeep_inbox.md on purpose.

    let fake = FakeAdapter::new(&env, "doctor-target");
    fake.respond("ping", &ping_ok());
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("Call dentist").id("n1").build()],
        ),
    );

    let output = run_doctor(&env, &fake, &[]);
    assert_eq!(output.status.code(), Some(1));
    let out = stdout(&output);
    assert!(
        out.contains("gkeep_inbox.md · missing"),
        "missing target:\n{out}"
    );
    assert!(
        out.contains("create it or set gkeep.target"),
        "target hint:\n{out}"
    );
}

#[test]
fn doctor_json_reports_the_check_shape() {
    let env = GkeepEnv::new("bob-cli-gkeep-doctor-json");
    let root = root_of(&env);
    let reader = write_token_reader(&root, "aas_et/test-master-token");
    write_auth_config(&env, &reader.to_string_lossy(), "true");
    write_target(&env);

    let fake = FakeAdapter::new(&env, "doctor-json");
    fake.respond("ping", &ping_ok());
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("Call dentist").id("n1").build()],
        ),
    );

    // The temp vault is not a Git worktree, so `git` warns but ok stays
    // true.
    let output = run_doctor(&env, &fake, &["-f", "json"]);
    assert!(
        output.status.success(),
        "warnings still exit 0:\nstdout:\n{}\nstderr:\n{}",
        stdout(&output),
        stderr(&output)
    );
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("valid JSON report");
    assert_eq!(report["schema_version"], serde_json::json!(1));
    assert_eq!(report["ok"], serde_json::json!(true));
    let checks = report["checks"].as_array().expect("checks array");
    assert_eq!(checks.len(), 7);
    let names: Vec<&str> = checks
        .iter()
        .map(|check| check["name"].as_str().expect("check name"))
        .collect();
    assert_eq!(
        names,
        vec!["config", "account", "token", "adapter", "keep", "target", "git"]
    );
    for check in checks {
        assert!(
            ["ok", "warn", "fail", "skip"]
                .contains(&check["status"].as_str().expect("status")),
            "known status: {check}"
        );
        assert!(check["summary"].is_string(), "string summary: {check}");
        assert!(
            check
                .as_object()
                .expect("check object")
                .contains_key("hint"),
            "hint key: {check}"
        );
    }
    let git = checks
        .iter()
        .find(|check| check["name"] == "git")
        .expect("git");
    assert_eq!(git["status"], serde_json::json!("warn"));
    assert!(
        !stdout(&output).contains("aas_et/test-master-token"),
        "no token in JSON"
    );
}
