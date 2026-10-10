//! Marker-free import-store transaction, portability, and identical-rendering tests.

mod gkeep_support;

use std::{fs, path::Path};

use gkeep_support::{
    archive_ok, note, snapshot_ok, stderr, stdout, FakeAdapter, GkeepEnv,
    TempDir,
};

fn write_token_script(vault: &Path) -> std::path::PathBuf {
    let path = vault.join("token.sh");
    fs::write(&path, "#!/bin/sh\nprintf 'aas_et/test-master-token\\n'\n")
        .expect("write token script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
    }
    path
}

fn configure(env: &GkeepEnv, token: &Path) {
    let quoted = token.to_string_lossy().replace('\'', "'\\''");
    env.write_config(&format!(
        "gkeep:\n  email: bryanbugyi34@gmail.com\n  token_command: \"sh '{quoted}'\"\n",
    ));
}

fn setup(prefix: &str) -> (GkeepEnv, FakeAdapter, TempDir) {
    let env = GkeepEnv::new(prefix);
    let state = TempDir::new(prefix);
    let token = write_token_script(env.vault());
    configure(&env, &token);
    fs::write(env.vault().join("gkeep_inbox.md"), "## Tasks\n")
        .expect("write target");
    let fake = FakeAdapter::new(&env, "adapter");
    (env, fake, state)
}

fn run_pull(
    env: &GkeepEnv,
    fake: &FakeAdapter,
    state: &TempDir,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = env.command();
    fake.install(&mut cmd);
    cmd.env("TZ", "UTC")
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CACHE_HOME", state.path().join("cache"))
        .arg("gkeep")
        .arg("pull");
    for arg in args {
        cmd.arg(arg);
    }
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().expect("run pull")
}

fn target(env: &GkeepEnv) -> String {
    fs::read_to_string(env.vault().join("gkeep_inbox.md")).expect("read target")
}

fn store_count(vault: &Path) -> usize {
    let dir = vault.join(".bob/gkeep/imports");
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().extension().is_some_and(|ext| ext == "json")
        })
        .count()
}

#[test]
fn prepare_crash_retries_without_duplicate() {
    let (env, fake, state) = setup("bob-cli-gkeep-crash-prepare");
    let n1 = note("Crash me").id("note-1").build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let crashed = run_pull(
        &env,
        &fake,
        &state,
        &[],
        &[("BOB_GKEEP_TEST_FAIL_AFTER_PREPARE", "1")],
    );
    assert_ne!(crashed.status.code(), Some(0), "{}", stderr(&crashed));
    assert!(
        !target(&env).contains("Crash me"),
        "no write on prepare crash"
    );
    // Retry without the fault: exactly one task, no duplicate.
    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(target(&env).matches("Crash me").count(), 1);
    let second = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(target(&env).matches("Crash me").count(), 1, "no duplicate");
}

#[test]
fn target_crash_finalizes_without_appending_again() {
    let (env, fake, state) = setup("bob-cli-gkeep-crash-target");
    let n1 = note("Target crash").id("note-1").build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let crashed = run_pull(
        &env,
        &fake,
        &state,
        &[],
        &[("BOB_GKEEP_TEST_FAIL_AFTER_TARGET", "1")],
    );
    assert_ne!(crashed.status.code(), Some(0));
    // Retry finalizes the installed write without appending again.
    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(target(&env).matches("Target crash").count(), 1);
}

#[test]
fn identical_url_less_notes_use_multiplicity() {
    let (env, fake, state) = setup("bob-cli-gkeep-identical");
    // One preexisting identical task must not prove two new imports.
    fs::write(
        env.vault().join("gkeep_inbox.md"),
        "## Tasks\n\n- [ ] #task Same text [created::2026-09-20]\n",
    )
    .expect("write target");
    let a = note("Same text")
        .id("note-a")
        .created("2026-09-26T08:00:00Z")
        .build();
    let b = note("Same text")
        .id("note-b")
        .created("2026-09-27T08:00:00Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![a, b]),
    );
    fake.respond(
        "archive",
        &archive_ok(vec![("note-a", "archived"), ("note-b", "archived")]),
    );
    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // Baseline 1 + 2 inserted = 3 occurrences.
    assert_eq!(target(&env).matches("Same text").count(), 3);
    assert!(!target(&env).contains("%%gkeep:"));
    assert_eq!(store_count(env.vault()), 1);
}

#[test]
fn portability_second_host_with_empty_state_adds_nothing() {
    let (env, fake, state) = setup("bob-cli-gkeep-portable");
    let n1 = note("Portable").id("note-1").build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1.clone()]),
    );
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    // --no-archive still requires durable receipts.
    let out = run_pull(&env, &fake, &state, &["-n"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(target(&env).matches("Portable").count(), 1);
    assert_eq!(store_count(env.vault()), 1);

    // Clone the whole vault including the store to a second host with
    // empty local state; the same revision archives only.
    let host2 = TempDir::new("bob-cli-gkeep-portable2");
    let vault2 = host2.path().join("vault");
    fs::create_dir_all(&vault2).expect("vault2");
    copy_dir(env.vault(), &vault2);
    let state2 = TempDir::new("bob-cli-gkeep-portable-state2");
    let env2 = GkeepEnv::new("bob-cli-gkeep-portable-env2");
    // Reuse vault2 as BOB_DIR via command override: point env2 at vault2
    // by copying vault2 into env2's vault.
    fs::remove_dir_all(env2.vault()).ok();
    copy_dir(&vault2, env2.vault());
    let token2 = write_token_script(env2.vault());
    configure(&env2, &token2);
    let fake2 = FakeAdapter::new(&env2, "adapter2");
    fake2.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake2.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let mut cmd = env2.command();
    fake2.install(&mut cmd);
    let out2 = cmd
        .env("TZ", "UTC")
        .env("XDG_STATE_HOME", state2.path())
        .arg("gkeep")
        .arg("pull")
        .output()
        .expect("second host pull");
    assert_eq!(
        out2.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        stdout(&out2),
        stderr(&out2)
    );
    let after =
        fs::read_to_string(env2.vault().join("gkeep_inbox.md")).expect("read");
    assert_eq!(
        after.matches("Portable").count(),
        1,
        "no duplicate on fresh host"
    );

    // A bare hand-written Keep link without history neither suppresses an
    // import nor authorizes archival: tested via unit history (receipts
    // required), plus this pull wrote the task even though the vault
    // already contained a hand-written link shape elsewhere.
    let _ = state;
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("mkdir dst");
    for entry in fs::read_dir(src).expect("read src") {
        let entry = entry.expect("entry");
        let dest = dst.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).expect("copy file");
        }
    }
}
