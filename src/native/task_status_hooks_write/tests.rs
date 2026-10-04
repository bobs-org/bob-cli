use super::*;
use super::{
    model::{QUIET_PERIOD, RETENTION, TOOL},
    recovery::{
        prune_completed, sha256_hex, unix_secs, vault_hash, RecoveryManifest,
    },
};
use std::{
    cell::{Cell, RefCell},
    ffi::OsStr,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn fixture() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = TempDir::new("bob-cli-guarded-write");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("vault");
    let first = vault.join("one.md");
    let second = vault.join("two.md");
    write_file(&first, "alpha\n");
    write_file(&second, "bravo\n");
    (temp, vault, first, second)
}

fn snapshot(path: &Path, kind: InputKind) -> InputSnapshot {
    capture_required(path, kind).unwrap_or_else(|error| {
        panic!("capture {}: {}", path.display(), error.message())
    })
}

fn plan_for(
    vault: &Path,
    outputs: Vec<(&Path, &str, bool)>,
    extra_inputs: Vec<InputSnapshot>,
    scan_paths: Vec<PathBuf>,
) -> WritePlan {
    let mut inputs = extra_inputs;
    let mut planned = Vec::new();
    for (path, proposed, structural) in outputs {
        let original = snapshot(path, InputKind::Note);
        if !inputs.iter().any(|input| input.path == original.path) {
            inputs.push(original.clone());
        }
        planned.push(
            planned_write(
                path.to_path_buf(),
                &original,
                proposed.as_bytes().to_vec(),
                structural,
            )
            .expect("planned write"),
        );
    }
    WritePlan {
        vault_canonical: vault.canonicalize().expect("canonical vault"),
        inputs,
        scan_paths,
        outputs: planned,
    }
}

fn make_session(
    temp: &TempDir,
    scan_paths: Vec<PathBuf>,
    run_id: &str,
) -> ApplySession {
    let scan = Rc::new(RefCell::new(scan_paths));
    ApplySession {
        tool: TOOL,
        state_home: temp.path().join("state"),
        quiet_period: QUIET_PERIOD,
        retention: RETENTION,
        run_id: run_id.to_string(),
        now_system: Box::new(SystemTime::now),
        sleep: Box::new(|_| {}),
        rescan: Box::new({
            let scan = Rc::clone(&scan);
            move || Ok(scan.borrow().clone())
        }),
        before_preflight: None,
        after_staging: None,
        before_replace: None,
        fail_staging: None,
    }
}

fn live_scan_session(
    temp: &TempDir,
    vault: &Path,
    run_id: &str,
) -> ApplySession {
    let vault = vault.to_path_buf();
    ApplySession {
        tool: TOOL,
        state_home: temp.path().join("state"),
        quiet_period: QUIET_PERIOD,
        retention: RETENTION,
        run_id: run_id.to_string(),
        now_system: Box::new(SystemTime::now),
        sleep: Box::new(|_| {}),
        rescan: Box::new(move || list_markdown(&vault)),
        before_preflight: None,
        after_staging: None,
        before_replace: None,
        fail_staging: None,
    }
}

fn list_markdown(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(vault)? {
        let path = entry?.path();
        if path.extension().and_then(OsStr::to_str) == Some("md") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn apply_ok(plan: &WritePlan, session: &ApplySession) -> ApplyOutcome {
    apply_plan(plan, session).unwrap_or_else(|error| {
        panic!("{} ({})", error.message, error.reason.as_str())
    })
}

fn set_mtime(path: &Path, time: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .expect("open for mtime")
        .set_modified(time)
        .expect("set mtime");
}

#[test]
fn unchanged_read_set_applies_and_records_recovery_bytes() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![
            (first.as_path(), "alpha-new\n", false),
            (second.as_path(), "bravo-new\n", false),
        ],
        Vec::new(),
        scan.clone(),
    );
    let session = make_session(&temp, scan, "apply-ok");
    let outcome = apply_ok(&plan, &session);
    let ApplyOutcome::Applied {
        applied_files,
        recovery_directory,
    } = outcome
    else {
        panic!("expected applied outcome");
    };
    assert_eq!(applied_files.len(), 2);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha-new\n");
    assert_eq!(fs::read_to_string(&second).unwrap(), "bravo-new\n");
    let original0 = fs::read(recovery_directory.join("0000.original")).unwrap();
    let proposed0 = fs::read(recovery_directory.join("0001.proposed")).unwrap();
    assert_eq!(original0, b"alpha\n");
    assert_eq!(proposed0, b"bravo-new\n");
    let manifest: RecoveryManifest = serde_json::from_str(
        &fs::read_to_string(recovery_directory.join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.tool, TOOL);
    assert_eq!(manifest.outcome, "applied");
    assert_eq!(manifest.notes[0].original_hash, sha256_hex(b"alpha\n"));
    assert_eq!(manifest.notes[1].proposed_hash, sha256_hex(b"bravo-new\n"));
    assert_eq!(manifest.notes[0].state, "applied");
}

#[test]
fn noop_creates_no_recovery_or_staging() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first, second];
    let plan = WritePlan {
        vault_canonical: vault.canonicalize().unwrap(),
        inputs: Vec::new(),
        scan_paths: scan.clone(),
        outputs: Vec::new(),
    };
    let session = make_session(&temp, scan, "noop");
    assert!(matches!(apply_ok(&plan, &session), ApplyOutcome::NoOp));
    assert!(!temp.path().join("state/bob-cli/task-status-hooks").exists());
    let foreign = vault.join(".one.md.999.tmp");
    write_file(&foreign, "leave-me");
    assert_eq!(fs::read_to_string(&foreign).unwrap(), "leave-me");
}

#[test]
fn equal_length_change_with_restored_mtime_prevents_write() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        vec![snapshot(&second, InputKind::Note)],
        scan.clone(),
    );
    let mtime = fs::metadata(&first).unwrap().modified().unwrap();
    let mut session = make_session(&temp, scan, "mtime-restore");
    session.before_preflight = Some(Box::new({
        let first = first.clone();
        move || {
            fs::write(&first, "ALPHA\n").unwrap();
            set_mtime(&first, mtime);
        }
    }));
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "ALPHA\n");
    assert!(error.applied_files.is_empty());
}

#[test]
fn fresh_attempt_after_vault_changed_replans_from_intervening_edit() {
    // Models what the `bob task reconcile` retry controller does across
    // two independent attempts: attempt 1 sees an editor's intervening
    // save and stops without writing; attempt 2 is a wholly fresh
    // snapshot/plan/apply, not a replay of attempt 1's stale plan, so it
    // observes the edit and preserves it.
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];

    let attempt_one_plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-planned\n", false)],
        vec![snapshot(&second, InputKind::Note)],
        scan.clone(),
    );
    let mut attempt_one_session =
        make_session(&temp, scan.clone(), "attempt-1");
    attempt_one_session.before_preflight = Some(Box::new({
        let first = first.clone();
        move || fs::write(&first, "alpha-from-editor\n").unwrap()
    }));
    let error = apply_plan(&attempt_one_plan, &attempt_one_session)
        .expect_err("attempt 1 must see the intervening edit");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert!(error.applied_files.is_empty());
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha-from-editor\n");

    // The lock and every attempt-1 snapshot are dropped between
    // attempts; attempt 2 rebuilds everything from scratch.
    let refreshed = snapshot(&first, InputKind::Note);
    assert_eq!(
            refreshed.utf8_contents().unwrap().as_deref(),
            Some("alpha-from-editor\n"),
            "a fresh attempt must observe the intervening edit, not attempt 1's stale bytes"
        );
    let attempt_two_plan = plan_for(
        &vault,
        vec![(
            first.as_path(),
            "alpha-from-editor\nplanned-addition\n",
            false,
        )],
        vec![snapshot(&second, InputKind::Note)],
        scan,
    );
    let attempt_two_session =
        make_session(&temp, vec![first.clone(), second.clone()], "attempt-2");
    let outcome = apply_ok(&attempt_two_plan, &attempt_two_session);
    assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
    assert_eq!(
        fs::read_to_string(&first).unwrap(),
        "alpha-from-editor\nplanned-addition\n",
        "the successful retry must preserve the editor's intervening change"
    );
}

#[test]
fn replacement_inode_prevents_write() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let planned_identity = plan.outputs[0].identity.clone();
    let mut session = make_session(&temp, scan, "inode");
    session.before_preflight = Some(Box::new({
        let first = first.clone();
        move || {
            fs::remove_file(&first).unwrap();
            fs::write(&first, "alpha\n").unwrap();
            let replacement_identity = snapshot(&first, InputKind::Note)
                .identity()
                .unwrap()
                .clone();
            if replacement_identity == planned_identity {
                let mut permissions = fs::metadata(&first)
                    .expect("replacement metadata")
                    .permissions();
                permissions.set_readonly(true);
                fs::set_permissions(&first, permissions)
                    .expect("force replacement identity change");
            }
        }
    }));
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
}

#[test]
fn deletion_prevents_write() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let mut session = make_session(&temp, scan, "delete");
    session.before_preflight = Some(Box::new({
        let first = first.clone();
        move || fs::remove_file(&first).unwrap()
    }));
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert!(!first.exists());
}

#[cfg(unix)]
#[test]
fn symlink_substitution_prevents_write() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let mut session = make_session(&temp, scan, "symlink");
    session.before_preflight = Some(Box::new({
        let first = first.clone();
        let second = second.clone();
        move || {
            fs::remove_file(&first).unwrap();
            std::os::unix::fs::symlink(&second, &first).unwrap();
        }
    }));
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert!(matches!(
        error.reason,
        ReasonCode::UnsupportedFile | ReasonCode::VaultChanged
    ));
    assert_eq!(fs::read_to_string(&second).unwrap(), "bravo\n");
}

#[test]
fn changed_tasks_settings_prevent_write() {
    let (temp, vault, first, second) = fixture();
    let settings =
        vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    write_file(&settings, "{\"globalFilter\":\"#task\"}\n");
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        vec![snapshot(&settings, InputKind::TasksSettings)],
        scan.clone(),
    );
    let mut session = make_session(&temp, scan, "settings");
    session.before_preflight = Some(Box::new({
        let settings = settings.clone();
        move || fs::write(&settings, "{\"globalFilter\":\"\"}\n").unwrap()
    }));
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
}

#[test]
fn changed_previous_daily_prevents_write() {
    let (temp, vault, first, second) = fixture();
    let previous = vault.join("2026/20260101.md");
    write_file(&previous, "## Pomodoros\n");
    let scan = vec![first.clone(), second.clone(), previous.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        vec![snapshot(&previous, InputKind::PreviousDaily)],
        scan.clone(),
    );
    let mut session = make_session(&temp, scan, "previous");
    session.before_preflight = Some(Box::new({
        let previous = previous.clone();
        move || fs::write(&previous, "## Pomodoros\n\n- extra\n").unwrap()
    }));
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
}

#[test]
fn new_or_deleted_scan_candidate_invalidates_plan() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let live = Rc::new(RefCell::new(scan.clone()));
    let mut session = make_session(&temp, scan.clone(), "scan-new");
    session.rescan = Box::new({
        let live = Rc::clone(&live);
        move || Ok(live.borrow().clone())
    });
    session.before_preflight = Some(Box::new({
        let live = Rc::clone(&live);
        let extra = vault.join("extra.md");
        move || {
            fs::write(&extra, "new\n").unwrap();
            live.borrow_mut().push(extra.clone());
        }
    }));
    let error = apply_plan(&plan, &session).expect_err("new file");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");

    live.borrow_mut().clone_from(&scan);
    let mut session = make_session(&temp, scan.clone(), "scan-deleted");
    session.rescan = Box::new({
        let live = Rc::clone(&live);
        move || Ok(live.borrow().clone())
    });
    session.before_preflight = Some(Box::new({
        let live = Rc::clone(&live);
        let second = second.clone();
        move || {
            live.borrow_mut().retain(|path| path != &second);
        }
    }));
    let error = apply_plan(&plan, &session).expect_err("deleted file");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
}

#[test]
fn edit_between_staging_and_revalidate_survives() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let mut session = make_session(&temp, scan, "after-stage");
    session.after_staging = Some(Box::new({
        let first = first.clone();
        move || fs::write(&first, "user-save\n").unwrap()
    }));
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "user-save\n");
    assert!(error.applied_files.is_empty());
    assert!(error.recovery_directory.is_some());
}

#[test]
fn edit_before_later_replacement_reports_partial() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![
            (first.as_path(), "alpha-new\n", false),
            (second.as_path(), "bravo-new\n", false),
        ],
        Vec::new(),
        scan.clone(),
    );
    let mut session = make_session(&temp, scan, "partial");
    let calls = Rc::new(Cell::new(0_u32));
    session.before_replace = Some(Box::new({
        let second = second.clone();
        let calls = Rc::clone(&calls);
        move |path| {
            let count = calls.get();
            calls.set(count + 1);
            if count == 1 && path == second {
                fs::write(&second, "keep-me\n").unwrap();
            }
        }
    }));
    let error = apply_plan(&plan, &session).expect_err("partial");
    assert_eq!(error.reason, ReasonCode::PartialApply);
    assert_eq!(error.applied_files, vec![first.clone()]);
    assert_eq!(error.deferred_files, vec![second.clone()]);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha-new\n");
    assert_eq!(fs::read_to_string(&second).unwrap(), "keep-me\n");
    let recovery = error.recovery_directory.expect("recovery");
    let manifest: RecoveryManifest = serde_json::from_str(
        &fs::read_to_string(recovery.join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.outcome, "partial");
    assert_eq!(
        fs::read(recovery.join("0001.original")).unwrap(),
        b"bravo\n"
    );
    assert_eq!(
        fs::read(recovery.join("0001.proposed")).unwrap(),
        b"bravo-new\n"
    );
}

#[cfg(unix)]
#[test]
fn exclusive_temp_and_mode_and_foreign_temps() {
    let (temp, vault, first, second) = fixture();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&first, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let foreign = vault.join(".one.md.99999.tmp");
    let colliding = vault.join(".one.md.bob-tsh.mode.0.0.tmp");
    write_file(&foreign, "foreign\n");
    write_file(&colliding, "collision\n");
    let session = make_session(&temp, scan, "mode");
    apply_ok(&plan, &session);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha-new\n");
    assert_eq!(fs::read_to_string(&foreign).unwrap(), "foreign\n");
    assert_eq!(fs::read_to_string(&colliding).unwrap(), "collision\n");
    let mode = fs::metadata(&first).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

#[test]
fn staging_failure_preserves_notes_and_foreign_temps() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![
            (first.as_path(), "alpha-new\n", false),
            (second.as_path(), "bravo-new\n", false),
        ],
        Vec::new(),
        scan.clone(),
    );
    let foreign = vault.join(".two.md.123.tmp");
    write_file(&foreign, "foreign\n");
    let mut session = make_session(&temp, scan, "stage-fail");
    session.fail_staging = Some(Box::new({
        let second = second.clone();
        move |path| {
            if path == second {
                Err(io::Error::other("injected staging failure"))
            } else {
                Ok(())
            }
        }
    }));
    let error = apply_plan(&plan, &session).expect_err("stage fail");
    assert_eq!(error.reason, ReasonCode::Io);
    assert!(error.applied_files.is_empty());
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(&second).unwrap(), "bravo\n");
    assert_eq!(fs::read_to_string(&foreign).unwrap(), "foreign\n");
    let leftover = fs::read_dir(&vault)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".bob-tsh.stage-fail.")
        });
    assert!(!leftover, "run staging files should be removed");
}

#[test]
fn retention_keeps_incomplete_and_prunes_old_completed() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let hash = vault_hash(&vault.canonicalize().unwrap());
    let root = temp
        .path()
        .join("state/bob-cli/task-status-hooks")
        .join(&hash);
    let old_complete = root.join("old-complete");
    let old_partial = root.join("old-partial");
    let other = root.join("other-state.txt");
    fs::create_dir_all(&old_complete).unwrap();
    fs::create_dir_all(&old_partial).unwrap();
    write_file(&other, "leave");
    let now = SystemTime::now();
    let old = unix_secs(now).saturating_sub(31 * 24 * 60 * 60);
    write_file(
        &old_complete.join("manifest.json"),
        &format!(
            r#"{{"tool":"{TOOL}","schema_version":1,"vault":"x","vault_hash":"{hash}","run_id":"old-complete","started_at":"{old}","started_at_unix":{old},"completed_at":"{old}","completed_at_unix":{old},"outcome":"applied","notes":[]}}"#
        ),
    );
    write_file(
        &old_partial.join("manifest.json"),
        &format!(
            r#"{{"tool":"{TOOL}","schema_version":1,"vault":"x","vault_hash":"{hash}","run_id":"old-partial","started_at":"{old}","started_at_unix":{old},"completed_at":null,"completed_at_unix":null,"outcome":"partial","notes":[]}}"#
        ),
    );
    let session = make_session(&temp, scan, "retain");
    apply_ok(&plan, &session);
    assert!(!old_complete.exists());
    assert!(old_partial.exists());
    assert_eq!(fs::read_to_string(&other).unwrap(), "leave");
}

#[test]
fn quiet_period_skips_wait_for_stable_files_and_status_only() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let slept = Rc::new(Cell::new(None));
    let now = SystemTime::now();
    set_mtime(&first, now - Duration::from_secs(3));
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", true)],
        Vec::new(),
        scan.clone(),
    );
    let mut session = make_session(&temp, scan.clone(), "quiet-stable");
    session.now_system = Box::new(move || now);
    session.sleep = Box::new({
        let slept = Rc::clone(&slept);
        move |duration| slept.set(Some(duration))
    });
    apply_ok(&plan, &session);
    assert_eq!(slept.get(), None);

    write_file(&first, "alpha\n");
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    set_mtime(&first, now);
    slept.set(None);
    let mut session = make_session(&temp, scan, "quiet-status");
    session.now_system = Box::new(move || now);
    session.sleep = Box::new({
        let slept = Rc::clone(&slept);
        move |duration| slept.set(Some(duration))
    });
    apply_ok(&plan, &session);
    assert_eq!(slept.get(), None);
}

#[test]
fn quiet_period_waits_once_then_defers_if_changed() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let now = SystemTime::now();
    set_mtime(&first, now - Duration::from_millis(500));
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", true)],
        Vec::new(),
        scan.clone(),
    );
    let slept = Rc::new(Cell::new(None));
    let mut session = make_session(&temp, scan, "quiet-wait");
    session.now_system = Box::new(move || now);
    session.sleep = Box::new({
        let slept = Rc::clone(&slept);
        let first = first.clone();
        move |duration| {
            slept.set(Some(duration));
            fs::write(&first, "typed\n").unwrap();
        }
    });
    let error = apply_plan(&plan, &session).expect_err("defer");
    assert_eq!(slept.get(), Some(Duration::from_millis(1500)));
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "typed\n");
}

#[test]
fn future_mtime_defers_without_sleeping() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let now = SystemTime::now();
    set_mtime(&first, now + Duration::from_secs(30));
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", true)],
        Vec::new(),
        scan.clone(),
    );
    let slept = Rc::new(Cell::new(false));
    let mut session = make_session(&temp, scan, "future");
    session.now_system = Box::new(move || now);
    session.sleep = Box::new({
        let slept = Rc::clone(&slept);
        move |_| slept.set(true)
    });
    let error = apply_plan(&plan, &session).expect_err("future");
    assert_eq!(error.reason, ReasonCode::QuietPeriod);
    assert!(!slept.get());
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
}

#[cfg(unix)]
#[test]
fn multiply_linked_output_is_rejected() {
    let (temp, vault, first, second) = fixture();
    let linked = vault.join("link.md");
    fs::hard_link(&first, &linked).unwrap();
    let scan = vec![first.clone(), second.clone(), linked.clone()];
    let original = snapshot(&first, InputKind::Note);
    let error =
        planned_write(first.clone(), &original, b"alpha-new\n".to_vec(), false)
            .expect_err("nlink");
    assert!(matches!(error, CaptureError::Unsupported { .. }));
    let _ = (temp, scan);
}

#[test]
fn tool_scopes_recovery_root_manifest_and_pruning() {
    let (temp, vault, first, second) = fixture();
    let scan = vec![first.clone(), second.clone()];
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan.clone(),
    );
    let mut session = make_session(&temp, scan, "randomize-run");
    session.tool = "randomize";
    let outcome = apply_ok(&plan, &session);
    let ApplyOutcome::Applied {
        recovery_directory, ..
    } = outcome
    else {
        panic!("expected applied outcome");
    };
    let expected_root = temp.path().join("state/bob-cli/randomize");
    assert!(recovery_directory.starts_with(&expected_root));
    let manifest: RecoveryManifest = serde_json::from_str(
        &fs::read_to_string(recovery_directory.join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.tool, "randomize");

    // Pruning under the randomize root removes an expired randomize
    // manifest but leaves another tool's manifest alone.
    let hash = vault_hash(&vault.canonicalize().unwrap());
    let root = expected_root.join(&hash);
    let now = SystemTime::now();
    let old = unix_secs(now).saturating_sub(31 * 24 * 60 * 60);
    let foreign = root.join("foreign-complete");
    let own = root.join("own-complete");
    fs::create_dir_all(&foreign).unwrap();
    fs::create_dir_all(&own).unwrap();
    write_file(
        &foreign.join("manifest.json"),
        &format!(
            r#"{{"tool":"task-status-hooks","schema_version":1,"vault":"x","vault_hash":"{hash}","run_id":"foreign-complete","started_at":"{old}","started_at_unix":{old},"completed_at":"{old}","completed_at_unix":{old},"outcome":"applied","notes":[]}}"#
        ),
    );
    write_file(
        &own.join("manifest.json"),
        &format!(
            r#"{{"tool":"randomize","schema_version":1,"vault":"x","vault_hash":"{hash}","run_id":"own-complete","started_at":"{old}","started_at_unix":{old},"completed_at":"{old}","completed_at_unix":{old},"outcome":"applied","notes":[]}}"#
        ),
    );
    prune_completed(&expected_root, "randomize", now, RETENTION).unwrap();
    assert!(foreign.exists(), "other tool records must be kept");
    assert!(!own.exists(), "expired own-tool records must be pruned");
}

#[test]
fn live_rescan_sees_new_vault_file() {
    let (temp, vault, first, second) = fixture();
    let scan = list_markdown(&vault).unwrap();
    let plan = plan_for(
        &vault,
        vec![(first.as_path(), "alpha-new\n", false)],
        Vec::new(),
        scan,
    );
    let mut session = live_scan_session(&temp, &vault, "live-scan");
    session.before_preflight = Some(Box::new({
        let extra = vault.join("extra.md");
        move || fs::write(&extra, "new\n").unwrap()
    }));
    let error = apply_plan(&plan, &session).expect_err("rescan");
    assert_eq!(error.reason, ReasonCode::VaultChanged);
    assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
    let _ = second;
}
