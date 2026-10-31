//! Host-local Keep status cache: the default-pull observation that
//! `bob gkeep status` will serve.
//!
//! Lives at `bob_cli_state_dir()/gkeep/status.json` (directory `0700`,
//! file `0600`). Recording is best-effort: a failed write warns on
//! stderr and never changes a command's exit code or stdout.

use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    config::GkeepConfig,
    ledger::{Journal, Ledger},
    model::KeepNote,
    plan::{
        classify_with_imports, load_gkeep_routing, NoteState, Plan, PlanAction,
        PlanOptions,
    },
    ui, GkeepError,
};
use crate::native::env as bob_env;

const CACHE_VERSION: u32 = 1;
const STATUS_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_LOCK_POLL: Duration = Duration::from_millis(10);

/// Namespace for the persisted Keep status cache.
pub(super) struct StatusCache;

/// Persisted cache document (`cache_version: 1`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct CacheDoc {
    pub(super) cache_version: u32,
    pub(super) config_key: String,
    pub(super) verified: bool,
    pub(super) observation: Option<Observation>,
    pub(super) invalidated_at: Option<String>,
    pub(super) failure: Option<Failure>,
    pub(super) last_contact_at: Option<String>,
    pub(super) budget: Option<Budget>,
}

/// One default-pull observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Observation {
    pub(super) started_at: String,
    pub(super) source: String,
    pub(super) pull: PullCounts,
    pub(super) stays: StayCounts,
}

/// Actionable Keep counts: a partition of `total`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(super) struct PullCounts {
    pub(super) total: usize,
    pub(super) new: usize,
    pub(super) links: usize,
    pub(super) revised: usize,
    pub(super) pending: usize,
}

/// Notes a default pull leaves in Keep.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(super) struct StayCounts {
    pub(super) pinned: usize,
    pub(super) shared: usize,
    pub(super) empty: usize,
}

/// The last recorded adapter failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Failure {
    pub(super) state: String,
    pub(super) kind: String,
    pub(super) message: String,
    pub(super) hint: Option<String>,
    pub(super) since: String,
    pub(super) last_at: String,
    pub(super) count: u32,
}

/// Automatic-check budget for the next phase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Budget {
    pub(super) day: String,
    pub(super) used: u32,
}

impl StatusCache {
    /// Load the cache for `config`. Missing, corrupt, version-mismatched,
    /// or different-key files load as empty.
    ///
    /// Production readers land in the auto-checks phase.
    #[allow(dead_code)]
    pub(super) fn load(config: &GkeepConfig) -> CacheDoc {
        match with_lock(&status_dir(), || Ok(load_doc(&status_dir(), config))) {
            Ok(doc) => doc,
            Err(_) => empty_doc(config),
        }
    }

    /// Record a successful default-pull observation.
    pub(super) fn record_observation(
        config: &GkeepConfig,
        observation: Observation,
    ) {
        warn_write(record_observation_at(&status_dir(), config, observation));
    }

    /// Record an adapter failure. Setup errors and unmapped kinds are
    /// ignored.
    pub(super) fn record_failure(
        config: &GkeepConfig,
        error: &GkeepError,
        started_at: DateTime<Utc>,
    ) {
        let Some(state) = failure_state(error.kind()) else {
            return;
        };
        warn_write(record_failure_at(
            &status_dir(),
            config,
            error,
            state,
            started_at,
        ));
    }

    /// Mark the cached count stale without clearing it.
    pub(super) fn invalidate(config: &GkeepConfig, at: DateTime<Utc>) {
        warn_write(invalidate_at(&status_dir(), config, at));
    }
}

impl Observation {
    /// Count a classified plan the way a default pull would act.
    ///
    /// `new` excludes `CreateRef` notes (those are `links`). Archived
    /// notes never count.
    pub(super) fn from_default_plan(
        plan: &Plan,
        started_at: DateTime<Utc>,
        source: &str,
    ) -> Self {
        let mut new = 0;
        let mut links = 0;
        let mut revised = 0;
        let mut pending = 0;
        let mut pinned = 0;
        let mut shared = 0;
        let mut empty = 0;
        for planned in &plan.notes {
            if matches!(planned.action, PlanAction::CreateRef) {
                links += 1;
                continue;
            }
            match planned.state {
                NoteState::New => new += 1,
                NoteState::Revised => revised += 1,
                NoteState::Pending => pending += 1,
                NoteState::Pinned => pinned += 1,
                NoteState::Shared => shared += 1,
                NoteState::Empty => empty += 1,
                NoteState::Archived => {}
            }
        }
        Self {
            started_at: fmt_utc(started_at),
            source: source.to_string(),
            pull: PullCounts {
                total: new + links + revised + pending,
                new,
                links,
                revised,
                pending,
            },
            stays: StayCounts {
                pinned,
                shared,
                empty,
            },
        }
    }
}

/// Scan the vault the way `list` does (no warnings) and classify with
/// default options plus default gkeep URL routing.
pub(super) fn observe_default(
    notes: &[KeepNote],
    bob_dir: &Path,
    started_at: DateTime<Utc>,
    source: &str,
) -> Option<Observation> {
    let ledger = Ledger::scan(bob_dir).ok()?;
    let store_files = super::imports::read_all(bob_dir).ok()?;
    let journal_path = bob_env::bob_cli_state_dir()
        .join("gkeep")
        .join("journal.jsonl");
    let journal = Journal::read(&journal_path).ok()?;
    let routing = load_gkeep_routing(false).unwrap_or(None);
    let plan = classify_with_imports(
        notes,
        &ledger,
        &journal,
        &store_files,
        &PlanOptions {
            routing,
            ..PlanOptions::default()
        },
    );
    Some(Observation::from_default_plan(&plan, started_at, source))
}

fn status_dir() -> PathBuf {
    bob_env::bob_cli_state_dir().join("gkeep")
}

fn config_key(config: &GkeepConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(config.email().as_bytes());
    hasher.update([0u8]);
    hasher.update(config.device_id().as_bytes());
    hasher.update([0u8]);
    hasher.update(config.token_command().as_bytes());
    hasher.update([0u8]);
    hasher.update(config.target().as_bytes());
    hex::encode(hasher.finalize())
}

fn fmt_utc(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn parse_utc(stamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn stamp_ge(left: &str, right: &str) -> bool {
    match (parse_utc(left), parse_utc(right)) {
        (Some(left), Some(right)) => left >= right,
        _ => true,
    }
}

fn failure_state(kind: &str) -> Option<&'static str> {
    match kind {
        "network" | "timeout" => Some("offline"),
        "rate_limit" => Some("rate_limited"),
        "auth" => Some("auth"),
        "adapter" | "protocol" | "internal" | "dependency" => Some("error"),
        _ => None,
    }
}

fn one_line(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Keep check failed")
        .to_string()
}

fn empty_doc(config: &GkeepConfig) -> CacheDoc {
    CacheDoc {
        cache_version: CACHE_VERSION,
        config_key: config_key(config),
        verified: false,
        observation: None,
        invalidated_at: None,
        failure: None,
        last_contact_at: None,
        budget: None,
    }
}

fn load_doc(dir: &Path, config: &GkeepConfig) -> CacheDoc {
    let path = dir.join("status.json");
    let Ok(text) = fs::read_to_string(path) else {
        return empty_doc(config);
    };
    let Ok(doc) = serde_json::from_str::<CacheDoc>(&text) else {
        return empty_doc(config);
    };
    if doc.cache_version != CACHE_VERSION
        || doc.config_key != config_key(config)
    {
        return empty_doc(config);
    }
    doc
}

fn apply_observation(doc: &mut CacheDoc, observation: Observation) -> bool {
    if let Some(current) = &doc.observation
        && !stamp_ge(&observation.started_at, &current.started_at)
    {
        return false;
    }
    if let Some(invalidated) = &doc.invalidated_at
        && !stamp_ge(&observation.started_at, invalidated)
    {
        return false;
    }
    doc.verified = true;
    doc.observation = Some(observation);
    doc.failure = None;
    doc.invalidated_at = None;
    true
}

fn apply_failure(
    doc: &mut CacheDoc,
    failure: Failure,
    started_at: &str,
) -> bool {
    if let Some(current) = &doc.observation
        && !stamp_ge(started_at, &current.started_at)
    {
        return false;
    }
    match &mut doc.failure {
        Some(existing) if existing.state == failure.state => {
            existing.count = existing.count.saturating_add(1);
            existing.last_at = failure.last_at;
            existing.message = failure.message;
            existing.hint = failure.hint;
            existing.kind = failure.kind;
        }
        _ => {
            doc.failure = Some(failure);
        }
    }
    true
}

fn write_doc(dir: &Path, doc: &CacheDoc) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        let _ = builder.create(dir);
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let path = dir.join("status.json");
    let temp = dir.join(format!("status.json.tmp{}", std::process::id()));
    let mut text =
        serde_json::to_string_pretty(doc).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(&temp, &text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

fn with_lock<T>(
    dir: &Path,
    op: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let file = bob_env::create_state_lock_file(dir, "status.lock")?;
    lock_exclusive(&file, STATUS_LOCK_TIMEOUT)?;
    op()
}

fn lock_exclusive(file: &fs::File, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if start.elapsed() >= timeout {
                    return Err(error);
                }
                thread::sleep(STATUS_LOCK_POLL);
            }
            Err(error) => return Err(error),
        }
    }
}

fn warn_write(result: io::Result<()>) {
    if let Err(error) = result {
        ui::warn(&format!("could not update Keep status cache: {error}"));
    }
}

fn record_observation_at(
    dir: &Path,
    config: &GkeepConfig,
    observation: Observation,
) -> io::Result<()> {
    with_lock(dir, || {
        let mut doc = load_doc(dir, config);
        if apply_observation(&mut doc, observation) {
            write_doc(dir, &doc)?;
        }
        Ok(())
    })
}

fn record_failure_at(
    dir: &Path,
    config: &GkeepConfig,
    error: &GkeepError,
    state: &str,
    started_at: DateTime<Utc>,
) -> io::Result<()> {
    let stamp = fmt_utc(started_at);
    let failure = Failure {
        state: state.to_string(),
        kind: error.kind().to_string(),
        message: one_line(error.message()),
        hint: error.hint().map(str::to_string),
        since: stamp.clone(),
        last_at: stamp.clone(),
        count: 1,
    };
    with_lock(dir, || {
        let mut doc = load_doc(dir, config);
        if apply_failure(&mut doc, failure, &stamp) {
            write_doc(dir, &doc)?;
        }
        Ok(())
    })
}

fn invalidate_at(
    dir: &Path,
    config: &GkeepConfig,
    at: DateTime<Utc>,
) -> io::Result<()> {
    with_lock(dir, || {
        let mut doc = load_doc(dir, config);
        doc.invalidated_at = Some(fmt_utc(at));
        write_doc(dir, &doc)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gkeep::model::{KeepContent, KeepNoteKind};
    use crate::native::gkeep::plan::{Plan, PlanSummary, PlannedNote};

    fn config() -> GkeepConfig {
        GkeepConfig::for_tests("bryan@example.com", "abc123", 30)
    }

    fn other_config() -> GkeepConfig {
        GkeepConfig::for_tests("other@example.com", "abc123", 30)
    }

    fn ts(stamp: &str) -> DateTime<Utc> {
        parse_utc(stamp).expect("test timestamp")
    }

    fn sample_observation(started_at: &str, new: usize) -> Observation {
        Observation {
            started_at: started_at.to_string(),
            source: "list".to_string(),
            pull: PullCounts {
                total: new,
                new,
                links: 0,
                revised: 0,
                pending: 0,
            },
            stays: StayCounts::default(),
        }
    }

    fn sample_note(id: &str) -> KeepNote {
        KeepNote {
            id: id.to_string(),
            server_id: None,
            kind: KeepNoteKind::Note,
            content: KeepContent {
                title: "Call the bank".to_string(),
                text: String::new(),
                items: Vec::new(),
            },
            pinned: false,
            archived: false,
            shared: false,
            labels: Vec::new(),
            attachments: Vec::new(),
            links: Vec::new(),
            created: "2026-10-31T12:00:00Z".to_string(),
            edited: "2026-10-31T12:00:00Z".to_string(),
            url: None,
        }
    }

    fn planned(
        note: KeepNote,
        state: NoteState,
        action: PlanAction,
    ) -> PlannedNote {
        PlannedNote {
            ref_: "abcd123".to_string(),
            note,
            state,
            action,
            skip_reason: None,
            ref_intent: None,
            ref_route: None,
        }
    }

    #[test]
    fn file_mode_is_0600() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config();
        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:00:00Z", 1),
        )
        .expect("write");
        let path = dir.path().join("status.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path)
                .expect("stat status.json")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = path;
    }

    #[test]
    fn corrupt_and_mismatched_key_load_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config();
        fs::write(dir.path().join("status.json"), "{not json").expect("write");
        let loaded = load_doc(dir.path(), &config);
        assert!(!loaded.verified);
        assert!(loaded.observation.is_none());

        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:00:00Z", 2),
        )
        .expect("write");
        let mismatched = load_doc(dir.path(), &other_config());
        assert!(mismatched.observation.is_none());
        assert_eq!(mismatched.config_key, config_key(&other_config()));
    }

    #[test]
    fn newest_observation_wins() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config();
        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:00:00Z", 1),
        )
        .expect("first");
        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:01:00Z", 3),
        )
        .expect("second");
        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T13:59:00Z", 9),
        )
        .expect("older ignored");
        let loaded = load_doc(dir.path(), &config);
        let observation = loaded.observation.expect("observation");
        assert_eq!(observation.pull.new, 3);
        assert_eq!(observation.started_at, "2026-10-31T14:01:00Z");
        assert!(loaded.verified);
    }

    #[test]
    fn older_failure_is_ignored() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config();
        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:00:00Z", 1),
        )
        .expect("observation");
        let error = GkeepError::runtime("network", "down".to_string());
        record_failure_at(
            dir.path(),
            &config,
            &error,
            "offline",
            ts("2026-10-31T13:59:00Z"),
        )
        .expect("older failure");
        let loaded = load_doc(dir.path(), &config);
        assert!(loaded.failure.is_none());
        assert!(loaded.verified);

        record_failure_at(
            dir.path(),
            &config,
            &error,
            "offline",
            ts("2026-10-31T14:00:00Z"),
        )
        .expect("same-time failure");
        let loaded = load_doc(dir.path(), &config);
        let failure = loaded.failure.expect("failure");
        assert_eq!(failure.state, "offline");
        assert_eq!(failure.count, 1);
        assert!(loaded.observation.is_some());
    }

    #[test]
    fn invalidation_drops_older_observations() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config();
        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:00:00Z", 1),
        )
        .expect("observation");
        invalidate_at(dir.path(), &config, ts("2026-10-31T14:05:00Z"))
            .expect("invalidate");
        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:04:00Z", 0),
        )
        .expect("older after invalidate");
        let loaded = load_doc(dir.path(), &config);
        assert_eq!(
            loaded.invalidated_at.as_deref(),
            Some("2026-10-31T14:05:00Z")
        );
        assert_eq!(
            loaded.observation.as_ref().map(|obs| obs.pull.new),
            Some(1)
        );

        record_observation_at(
            dir.path(),
            &config,
            sample_observation("2026-10-31T14:05:00Z", 4),
        )
        .expect("new enough");
        let loaded = load_doc(dir.path(), &config);
        assert!(loaded.invalidated_at.is_none());
        assert_eq!(loaded.observation.expect("obs").pull.new, 4);
    }

    #[test]
    fn setup_errors_are_not_mapped() {
        assert_eq!(failure_state("config"), None);
        assert_eq!(failure_state("token"), None);
        assert_eq!(failure_state("uv"), None);
        assert_eq!(failure_state("network"), Some("offline"));
        assert_eq!(failure_state("timeout"), Some("offline"));
        assert_eq!(failure_state("rate_limit"), Some("rate_limited"));
        assert_eq!(failure_state("auth"), Some("auth"));
        assert_eq!(failure_state("adapter"), Some("error"));
    }

    #[test]
    fn from_default_plan_splits_create_ref_into_links() {
        let notes = vec![
            planned(sample_note("n1"), NoteState::New, PlanAction::Write),
            planned(sample_note("n2"), NoteState::New, PlanAction::CreateRef),
            planned(
                sample_note("n3"),
                NoteState::Pending,
                PlanAction::ArchiveOnly,
            ),
            planned(sample_note("n4"), NoteState::Pinned, PlanAction::Skip),
        ];
        let plan = Plan {
            notes,
            summary: PlanSummary::default(),
        };
        let observation = Observation::from_default_plan(
            &plan,
            ts("2026-10-31T14:00:00Z"),
            "list",
        );
        assert_eq!(observation.pull.new, 1);
        assert_eq!(observation.pull.links, 1);
        assert_eq!(observation.pull.pending, 1);
        assert_eq!(observation.pull.total, 3);
        assert_eq!(observation.stays.pinned, 1);
        assert_eq!(observation.source, "list");
    }

    #[test]
    fn same_state_failure_increments_count() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config();
        let error = GkeepError::runtime("network", "down".to_string());
        record_failure_at(
            dir.path(),
            &config,
            &error,
            "offline",
            ts("2026-10-31T14:00:00Z"),
        )
        .expect("first");
        record_failure_at(
            dir.path(),
            &config,
            &error,
            "offline",
            ts("2026-10-31T14:01:00Z"),
        )
        .expect("second");
        let loaded = load_doc(dir.path(), &config);
        let failure = loaded.failure.expect("failure");
        assert_eq!(failure.count, 2);
        assert_eq!(failure.since, "2026-10-31T14:00:00Z");
        assert_eq!(failure.last_at, "2026-10-31T14:01:00Z");
    }
}
