//! Enrollment: which Gmail threads sync (or legacy pull) has taken on.
//!
//! A thread is enrolled once any host's `.bob/gmail/*.jsonl` journal holds a
//! valid `gmail.pulled` (legacy, read-only), `gmail.filed`, or
//! `gmail.adopted` event for it. Enrollment never expires: removing the
//! Gmail label or closing, moving, merging, or deleting the task leaves it
//! enrolled, so an enrolled thread is never filed again. Whether it is
//! *watched* right now is derived on every run from the open tasks that
//! link its mail note ([`super::scan`]).
//!
//! Event values are body-free: identity, vault-relative paths, the filing
//! route, and how it was filed. Never a subject, snippet, or body.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Map, Value};

use super::{
    scan::{WatchScan, WatchingTask},
    ThreadKey,
};
use crate::native::{
    gmail::journal::{self, NAMESPACE},
    pref_journal::{self, StoredEvent, MAX_MALFORMED_WARNINGS},
};

const FILED_KIND: &str = "gmail.filed";
const ADOPTED_KIND: &str = "gmail.adopted";

/// Which journal event enrolled a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentKind {
    /// Legacy `bob gmail pull` filing; still read, never newly written.
    Pulled,
    /// `bob gmail sync` wrote the thread's task row.
    Filed,
    /// An open task already linked the thread's mail note.
    Adopted,
}

/// Where a `gmail.filed` row went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FiledVia {
    /// The routing label's area or project note.
    Route,
    /// The sync inbox, because no routing label was present.
    Inbox,
    /// The sync inbox with a managed route-problem child.
    Problem,
}

impl FiledVia {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Inbox => "inbox",
            Self::Problem => "problem",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "route" => Some(Self::Route),
            "inbox" => Some(Self::Inbox),
            "problem" => Some(Self::Problem),
            _ => None,
        }
    }
}

/// How a thread's enrollment reads in reports: `already_pulled` when only
/// legacy pull events exist, `already_filed` once sync filed or adopted it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentOrigin {
    Legacy,
    Sync,
}

/// One folded enrollment event. Provenance (`via`, `route`) and the task
/// destination stay available for later health work (route repair).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnrollmentRecord {
    pub(crate) kind: EnrollmentKind,
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) host: String,
    /// Vault-relative mail-note path, when the event named one.
    pub(crate) note: Option<String>,
    /// Vault-relative note holding the task row: `task_file` for sync
    /// events, `inbox` for legacy pull events.
    pub(crate) task_file: Option<String>,
    /// `gmail.filed` only.
    pub(crate) via: Option<FiledVia>,
    /// `gmail.filed` only, when the row was routed.
    pub(crate) route: Option<String>,
    journal_path: String,
    journal_line: u32,
}

/// Every host's enrollment events, folded by thread in writer order.
#[derive(Debug, Clone, Default)]
pub(crate) struct EnrollmentFold {
    by_thread: BTreeMap<ThreadKey, Vec<EnrollmentRecord>>,
    warnings: Vec<String>,
}

impl EnrollmentFold {
    /// Fold `.bob/gmail/*.jsonl` across every host. Malformed lines and
    /// enrollment events without a valid identity are skipped with bounded
    /// warnings; other `gmail.*` kinds are ignored.
    pub(crate) fn load(vault: &Path) -> io::Result<Self> {
        let (events, warnings) = pref_journal::read_raw(vault, NAMESPACE)?;
        Ok(Self::from_events(events, warnings))
    }

    fn from_events(
        events: Vec<StoredEvent>,
        mut warnings: Vec<String>,
    ) -> Self {
        let mut by_thread: BTreeMap<ThreadKey, Vec<EnrollmentRecord>> =
            BTreeMap::new();
        let mut malformed = 0usize;
        for event in events {
            let kind = match event.kind.as_str() {
                journal::PULLED_KIND => EnrollmentKind::Pulled,
                FILED_KIND => EnrollmentKind::Filed,
                ADOPTED_KIND => EnrollmentKind::Adopted,
                _ => continue,
            };
            let Some((key, record)) = parse_record(kind, &event) else {
                malformed += 1;
                if malformed <= MAX_MALFORMED_WARNINGS {
                    warnings.push(format!(
                        "skipping malformed Gmail enrollment event {}:{}",
                        event.path, event.line
                    ));
                }
                continue;
            };
            by_thread.entry(key).or_default().push(record);
        }
        if malformed > MAX_MALFORMED_WARNINGS {
            warnings.push(format!(
                "skipped {malformed} malformed Gmail enrollment events"
            ));
        }
        for records in by_thread.values_mut() {
            pref_journal::sort_by_writer_order(records, |record| {
                (
                    record.ts,
                    record.host.as_str(),
                    record.journal_path.as_str(),
                    record.journal_line,
                )
            });
        }
        Self {
            by_thread,
            warnings,
        }
    }

    /// Whether any host ever filed, adopted, or pulled this thread.
    pub(crate) fn is_enrolled(&self, key: &ThreadKey) -> bool {
        self.by_thread.contains_key(key)
    }

    /// `Legacy` when only `gmail.pulled` events enrolled the thread.
    pub(crate) fn origin(&self, key: &ThreadKey) -> Option<EnrollmentOrigin> {
        let records = self.by_thread.get(key)?;
        Some(
            if records
                .iter()
                .all(|record| record.kind == EnrollmentKind::Pulled)
            {
                EnrollmentOrigin::Legacy
            } else {
                EnrollmentOrigin::Sync
            },
        )
    }

    /// Every record for the thread, oldest first in writer order.
    pub(crate) fn records(&self, key: &ThreadKey) -> &[EnrollmentRecord] {
        self.by_thread.get(key).map_or(&[], Vec::as_slice)
    }

    /// The newest record for the thread in writer order.
    pub(crate) fn latest(&self, key: &ThreadKey) -> Option<&EnrollmentRecord> {
        self.records(key).last()
    }

    /// Every enrolled thread, sorted by key.
    pub(crate) fn threads(&self) -> impl Iterator<Item = &ThreadKey> {
        self.by_thread.keys()
    }

    /// Malformed-line and malformed-event warnings, body-free.
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

fn parse_record(
    kind: EnrollmentKind,
    event: &StoredEvent,
) -> Option<(ThreadKey, EnrollmentRecord)> {
    let object = event.value.get("value")?.as_object()?;
    let text = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let key = ThreadKey::new(&text("account")?, &text("thread_id")?)?;
    let (task_file, via, route) = match kind {
        EnrollmentKind::Pulled => (text("inbox"), None, None),
        EnrollmentKind::Filed => (
            text("task_file"),
            text("via").as_deref().and_then(FiledVia::parse),
            text("route"),
        ),
        EnrollmentKind::Adopted => (text("task_file"), None, None),
    };
    Some((
        key,
        EnrollmentRecord {
            kind,
            ts: event.ts,
            host: event.host.clone(),
            note: text("note"),
            task_file,
            via,
            route,
            journal_path: event.path.clone(),
            journal_line: event.line,
        },
    ))
}

/// A new enrollment event. Append it only after the vault writes it
/// describes are durable; a lost append is repaired by adoption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnrollmentEvent {
    /// Sync wrote the thread's task row into `task_file`.
    Filed {
        key: ThreadKey,
        note: String,
        task_file: String,
        via: FiledVia,
        route: Option<String>,
    },
    /// An open task in `task_file` already linked the thread's mail note.
    Adopted {
        key: ThreadKey,
        note: String,
        task_file: String,
    },
}

impl EnrollmentEvent {
    pub(crate) fn key(&self) -> &ThreadKey {
        match self {
            Self::Filed { key, .. } | Self::Adopted { key, .. } => key,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Filed { .. } => FILED_KIND,
            Self::Adopted { .. } => ADOPTED_KIND,
        }
    }

    /// The body-free event value.
    fn value(&self) -> Value {
        let mut value = Map::new();
        let key = self.key();
        value.insert("account".into(), json!(key.account()));
        value.insert("thread_id".into(), json!(key.thread_id()));
        match self {
            Self::Filed {
                note,
                task_file,
                via,
                route,
                ..
            } => {
                value.insert("note".into(), json!(note));
                value.insert("task_file".into(), json!(task_file));
                value.insert("via".into(), json!(via.as_str()));
                if let Some(route) = route {
                    value.insert("route".into(), json!(route));
                }
            }
            Self::Adopted {
                note, task_file, ..
            } => {
                value.insert("note".into(), json!(note));
                value.insert("task_file".into(), json!(task_file));
            }
        }
        Value::Object(value)
    }
}

/// Append one enrollment event to this host's journal with one `O_APPEND`
/// write.
pub(crate) fn append(
    vault: &Path,
    host: &str,
    event: &EnrollmentEvent,
) -> io::Result<()> {
    let ts = pref_journal::event_timestamp()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let mut line = serde_json::to_string(&json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": event.kind(),
        "value": event.value(),
    }))
    .map_err(io::Error::other)?;
    line.push('\n');
    pref_journal::append_line(vault, NAMESPACE, host, &line)
}

/// Record an open task that already links an unenrolled thread's mail
/// note: no new row is filed, and the apply step appends `gmail.adopted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdoptionIntent {
    pub(crate) key: ThreadKey,
    /// Vault-relative mail-note path the task links.
    pub(crate) note: String,
    /// Vault-relative note of the first watching task (by path, then line).
    pub(crate) task_file: String,
    /// 1-based line of that task, for the apply step's recheck.
    pub(crate) task_line: usize,
    /// How many open tasks watch the thread.
    pub(crate) watchers: usize,
}

impl AdoptionIntent {
    pub(crate) fn event(&self) -> EnrollmentEvent {
        EnrollmentEvent::Adopted {
            key: self.key.clone(),
            note: self.note.clone(),
            task_file: self.task_file.clone(),
        }
    }
}

/// What a run does with one thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CandidateDecision {
    /// Already enrolled: never filed again, whatever its tasks look like.
    Enrolled(EnrollmentOrigin),
    /// Pending sync-recovery evidence: the replay owns this thread, so the
    /// current run neither files nor adopts it again.
    InFlight,
    /// Unenrolled, but an open task links its mail note: adopt it.
    Adopt(AdoptionIntent),
    /// Unenrolled and unlinked: a new filing candidate.
    File,
}

/// The enrollment fold joined with one watch scan. Deciding never writes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Membership<'a> {
    fold: &'a EnrollmentFold,
    scan: &'a WatchScan,
    in_flight: Option<&'a BTreeSet<ThreadKey>>,
}

impl<'a> Membership<'a> {
    pub(crate) fn new(fold: &'a EnrollmentFold, scan: &'a WatchScan) -> Self {
        Self {
            fold,
            scan,
            in_flight: None,
        }
    }

    /// Mark threads with pending sync-recovery evidence as in-flight. Their
    /// recorded `gmail.filed` or `gmail.adopted` event is appended by the
    /// recovery replay; they are never filed or adopted again.
    pub(crate) fn with_in_flight(
        self,
        in_flight: &'a BTreeSet<ThreadKey>,
    ) -> Self {
        Self {
            in_flight: Some(in_flight),
            ..self
        }
    }

    /// Decide one thread: enrolled threads are never filed again, linked
    /// unenrolled threads are adopted, and the rest are filing candidates.
    /// In-flight threads are skipped: recovery replays them.
    pub(crate) fn decide(&self, key: &ThreadKey) -> CandidateDecision {
        if let Some(origin) = self.fold.origin(key) {
            return CandidateDecision::Enrolled(origin);
        }
        if self.in_flight.is_some_and(|set| set.contains(key)) {
            return CandidateDecision::InFlight;
        }
        match self.adoption(key) {
            Some(intent) => CandidateDecision::Adopt(intent),
            None => CandidateDecision::File,
        }
    }

    /// Every unenrolled thread of `account` that an open task links, from
    /// the local scan alone: no Gmail label or search is consulted, so a
    /// filing whose journal append was lost is adopted even after its label
    /// is gone. Sorted by thread key.
    pub(crate) fn adoptions(&self, account: &str) -> Vec<AdoptionIntent> {
        self.scan
            .threads()
            .filter(|key| key.is_account(account))
            .filter(|key| !self.fold.is_enrolled(key))
            .filter_map(|key| self.adoption(key))
            .collect()
    }

    fn adoption(&self, key: &ThreadKey) -> Option<AdoptionIntent> {
        let mut watchers = self.scan.watchers(key);
        let first: &WatchingTask = watchers.next()?;
        let note = first.target_note(key)?.to_string();
        Some(AdoptionIntent {
            key: key.clone(),
            note,
            task_file: first.path.clone(),
            task_line: first.line,
            watchers: 1 + watchers.count(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::native::gmail::sync::watch::watched_keys;

    fn key(account: &str, thread: &str) -> ThreadKey {
        ThreadKey::new(account, thread).unwrap()
    }

    fn write_journal(vault: &Path, host: &str, lines: &[String]) {
        let dir = vault.join(".bob/gmail");
        fs::create_dir_all(&dir).unwrap();
        let mut text = lines.join("\n");
        text.push('\n');
        fs::write(dir.join(format!("{host}.jsonl")), text).unwrap();
    }

    fn event(ts: &str, host: &str, kind: &str, value: Value) -> String {
        serde_json::to_string(&json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": kind,
            "value": value,
        }))
        .unwrap()
    }

    #[test]
    fn folds_legacy_and_new_enrollment_across_hosts() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        write_journal(
            vault,
            "athena",
            &[
                event(
                    "2026-11-01T09:00:00-04:00",
                    "athena",
                    "gmail.pulled",
                    json!({
                        "account": "Me@Example.com",
                        "thread_id": "AABB01",
                        "inbox": "gmail_inbox.md",
                        "note": "mail/2026/a.md",
                    }),
                ),
                event(
                    "2026-11-02T09:00:00-04:00",
                    "athena",
                    "gmail.save",
                    json!({"account": "me@example.com", "thread_id": "ccdd02"}),
                ),
            ],
        );
        write_journal(
            vault,
            "mac",
            &[
                event(
                    "2026-11-03T09:00:00-04:00",
                    "mac",
                    "gmail.filed",
                    json!({
                        "account": "me@example.com",
                        "thread_id": "eeff03",
                        "note": "mail/2026/c.md",
                        "task_file": "job.md",
                        "via": "route",
                        "route": "job",
                    }),
                ),
                event(
                    "2026-11-04T09:00:00-04:00",
                    "mac",
                    "gmail.adopted",
                    json!({
                        "account": "me@example.com",
                        "thread_id": "aabb01",
                        "note": "mail/2026/a.md",
                        "task_file": "cash.md",
                    }),
                ),
                event(
                    "2026-11-05T09:00:00-04:00",
                    "mac",
                    "gmail.pulled",
                    json!({
                        "account": "me@example.com",
                        "thread_id": "112233",
                        "inbox": "gmail_inbox.md",
                        "note": "mail/2026/d.md",
                    }),
                ),
            ],
        );
        let fold = EnrollmentFold::load(vault).unwrap();
        assert!(fold.warnings().is_empty(), "{:?}", fold.warnings());
        assert_eq!(fold.threads().count(), 3);
        // A save event never enrolls.
        assert!(!fold.is_enrolled(&key("me@example.com", "ccdd02")));

        let legacy_then_adopted = key("me@example.com", "aabb01");
        assert_eq!(
            fold.origin(&legacy_then_adopted),
            Some(EnrollmentOrigin::Sync)
        );
        let records = fold.records(&legacy_then_adopted);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].kind, EnrollmentKind::Pulled);
        assert_eq!(records[0].task_file.as_deref(), Some("gmail_inbox.md"));
        assert_eq!(records[0].host, "athena");
        let latest = fold.latest(&legacy_then_adopted).unwrap();
        assert_eq!(latest.kind, EnrollmentKind::Adopted);
        assert_eq!(latest.task_file.as_deref(), Some("cash.md"));

        let filed = fold.latest(&key("me@example.com", "eeff03")).unwrap();
        assert_eq!(filed.kind, EnrollmentKind::Filed);
        assert_eq!(filed.via, Some(FiledVia::Route));
        assert_eq!(filed.route.as_deref(), Some("job"));
        assert_eq!(filed.note.as_deref(), Some("mail/2026/c.md"));

        assert_eq!(
            fold.origin(&key("ME@example.com", "112233")),
            Some(EnrollmentOrigin::Legacy)
        );
        let threads: Vec<_> =
            fold.threads().map(|key| key.thread_id()).collect();
        assert_eq!(threads, vec!["112233", "aabb01", "eeff03"]);
    }

    #[test]
    fn malformed_lines_and_identities_warn_without_enrolling() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        write_journal(
            vault,
            "athena",
            &[
                "{not json".to_string(),
                event(
                    "2026-11-01T09:00:00-04:00",
                    "athena",
                    "gmail.filed",
                    json!({"account": "me@example.com", "thread_id": "not-hex"}),
                ),
                event(
                    "2026-11-01T09:00:00-04:00",
                    "athena",
                    "gmail.adopted",
                    json!({"thread_id": "aabb"}),
                ),
                event(
                    "2026-11-01T09:00:00-04:00",
                    "athena",
                    "gmail.filed",
                    json!({
                        "account": "me@example.com",
                        "thread_id": "aabb",
                        "via": "carrier-pigeon",
                    }),
                ),
            ],
        );
        let fold = EnrollmentFold::load(vault).unwrap();
        assert_eq!(fold.threads().count(), 1);
        let record = fold.latest(&key("me@example.com", "aabb")).unwrap();
        // A valid identity enrolls even when optional provenance is junk.
        assert_eq!(record.via, None);
        assert_eq!(
            fold.warnings(),
            &[
                "skipping malformed gmail journal line athena.jsonl:1"
                    .to_string(),
                "skipping malformed Gmail enrollment event athena.jsonl:2"
                    .to_string(),
                "skipping malformed Gmail enrollment event athena.jsonl:3"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn missing_journal_is_an_empty_fold() {
        let temp = tempfile::tempdir().unwrap();
        let fold = EnrollmentFold::load(temp.path()).unwrap();
        assert_eq!(fold.threads().count(), 0);
        assert!(fold.warnings().is_empty());
    }

    #[test]
    fn appended_events_are_body_free_and_fold_back() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let filed = EnrollmentEvent::Filed {
            key: key("Me@Example.com", "AABB"),
            note: "mail/2026/lease.md".to_string(),
            task_file: "job.md".to_string(),
            via: FiledVia::Route,
            route: Some("job".to_string()),
        };
        let problem = EnrollmentEvent::Filed {
            key: key("me@example.com", "ccdd"),
            note: "mail/2026/kickoff.md".to_string(),
            task_file: "gmail_inbox.md".to_string(),
            via: FiledVia::Problem,
            route: None,
        };
        let adopted = EnrollmentEvent::Adopted {
            key: key("me@example.com", "eeff"),
            note: "mail/2026/quote.md".to_string(),
            task_file: "cash.md".to_string(),
        };
        for event in [&filed, &problem, &adopted] {
            append(vault, "athena", event).unwrap();
        }
        let text =
            fs::read_to_string(vault.join(".bob/gmail/athena.jsonl")).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["kind"], "gmail.filed");
        assert_eq!(
            lines[0]["value"],
            json!({
                "account": "me@example.com",
                "thread_id": "aabb",
                "note": "mail/2026/lease.md",
                "task_file": "job.md",
                "via": "route",
                "route": "job",
            })
        );
        assert_eq!(
            lines[1]["value"],
            json!({
                "account": "me@example.com",
                "thread_id": "ccdd",
                "note": "mail/2026/kickoff.md",
                "task_file": "gmail_inbox.md",
                "via": "problem",
            })
        );
        assert_eq!(lines[2]["kind"], "gmail.adopted");
        assert_eq!(
            lines[2]["value"],
            json!({
                "account": "me@example.com",
                "thread_id": "eeff",
                "note": "mail/2026/quote.md",
                "task_file": "cash.md",
            })
        );
        for line in &lines {
            assert_eq!(line["v"], 1);
            assert_eq!(line["host"], "athena");
            let value = line["value"].as_object().unwrap();
            for forbidden in ["subject", "snippet", "body", "title", "who"] {
                assert!(!value.contains_key(forbidden));
            }
        }
        assert_eq!(super::super::super::journal::malformed_count(vault), 0);

        let fold = EnrollmentFold::load(vault).unwrap();
        assert_eq!(fold.threads().count(), 3);
        assert_eq!(
            fold.latest(&key("me@example.com", "ccdd")).unwrap().via,
            Some(FiledVia::Problem)
        );
        assert_eq!(
            fold.origin(&key("me@example.com", "eeff")),
            Some(EnrollmentOrigin::Sync)
        );
    }

    /// A vault with indexed mail notes for membership tests.
    struct MemberVault {
        _temp: tempfile::TempDir,
        root: std::path::PathBuf,
    }

    impl MemberVault {
        fn new(threads: &[(&str, &str)]) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            let vault = Self { _temp: temp, root };
            for (account, thread) in threads {
                vault.write(
                    &format!("mail/2026/{thread}.md"),
                    &format!(
                        "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\n\
                         gmail_account: {account}\ngmail_thread_id: {thread}\n\
                         title: Subject {thread}\nwho: Dana\n---\nbody\n"
                    ),
                );
            }
            vault
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn fold_and_scan(&self) -> (EnrollmentFold, WatchScan) {
            let index = crate::native::gmail::note::index::scan(
                &self.root,
                Path::new("mail"),
                "✉️ {subject} · {who}",
            )
            .unwrap();
            let scan = super::super::scan::scan_watching_tasks(
                &self.root,
                Path::new("mail"),
                &index,
            )
            .unwrap();
            (EnrollmentFold::load(&self.root).unwrap(), scan)
        }

        fn append(&self, event: &EnrollmentEvent) {
            append(&self.root, "athena", event).unwrap();
        }
    }

    const ME: &str = "me@example.com";

    #[test]
    fn open_linked_unenrolled_threads_are_adopted_from_the_local_scan() {
        let vault = MemberVault::new(&[(ME, "aa01"), (ME, "bb02")]);
        // No label, no search, no journal: only the open task links it.
        vault.write("cash.md", "- [ ] #task Pay\n\t- [[mail/2026/aa01]]\n");
        vault.write("job.md", "- [*] #task Also [[mail/2026/aa01|x]]\n");
        let (fold, scan) = vault.fold_and_scan();
        let membership = Membership::new(&fold, &scan);
        let intent = AdoptionIntent {
            key: key(ME, "aa01"),
            note: "mail/2026/aa01.md".to_string(),
            task_file: "cash.md".to_string(),
            task_line: 1,
            watchers: 2,
        };
        assert_eq!(membership.adoptions(ME), vec![intent.clone()]);
        assert_eq!(
            membership.decide(&key(ME, "aa01")),
            CandidateDecision::Adopt(intent.clone())
        );
        // An unlinked, unenrolled thread is a new filing candidate.
        assert_eq!(
            membership.decide(&key(ME, "bb02")),
            CandidateDecision::File
        );
        // Not enrolled yet, so not watched until the run adopts it.
        assert!(watched_keys(ME, &fold, &scan, &[]).is_empty());
        assert_eq!(
            intent.event(),
            EnrollmentEvent::Adopted {
                key: key(ME, "aa01"),
                note: "mail/2026/aa01.md".to_string(),
                task_file: "cash.md".to_string(),
            }
        );
    }

    #[test]
    fn a_lost_journal_append_is_repaired_by_adoption_then_stays_enrolled() {
        let vault = MemberVault::new(&[(ME, "aa01")]);
        // The filed row is durable, but its gmail.filed append was lost.
        vault.write(
            "gmail_inbox.md",
            "## Tasks\n- [ ] #task [[mail/2026/aa01|✉️ Subject aa01 · Dana]] \
             [created:: 2026-11-07]\n",
        );
        let (fold, scan) = vault.fold_and_scan();
        let membership = Membership::new(&fold, &scan);
        let CandidateDecision::Adopt(intent) =
            membership.decide(&key(ME, "aa01"))
        else {
            panic!("expected adoption");
        };
        assert_eq!(intent.task_file, "gmail_inbox.md");
        assert_eq!(intent.task_line, 2);
        vault.append(&intent.event());

        let (fold, scan) = vault.fold_and_scan();
        let membership = Membership::new(&fold, &scan);
        assert!(membership.adoptions(ME).is_empty());
        assert_eq!(
            membership.decide(&key(ME, "aa01")),
            CandidateDecision::Enrolled(EnrollmentOrigin::Sync)
        );
        assert_eq!(watched_keys(ME, &fold, &scan, &[]), vec![key(ME, "aa01")]);
    }

    #[test]
    fn enrollment_outlives_closed_and_deleted_tasks_and_reopening_resumes() {
        let vault = MemberVault::new(&[(ME, "aa01"), (ME, "bb02")]);
        vault.append(&EnrollmentEvent::Filed {
            key: key(ME, "aa01"),
            note: "mail/2026/aa01.md".to_string(),
            task_file: "job.md".to_string(),
            via: FiledVia::Route,
            route: Some("job".to_string()),
        });
        write_journal(
            &vault.root,
            "mac",
            &[event(
                "2026-11-01T09:00:00-04:00",
                "mac",
                "gmail.pulled",
                json!({
                    "account": ME,
                    "thread_id": "bb02",
                    "inbox": "gmail_inbox.md",
                    "note": "mail/2026/bb02.md",
                }),
            )],
        );
        // Closed task, deleted task: still enrolled, never filed again.
        vault.write("job.md", "- [x] #task Done [[mail/2026/aa01]]\n");
        let (fold, scan) = vault.fold_and_scan();
        let membership = Membership::new(&fold, &scan);
        assert_eq!(
            membership.decide(&key(ME, "aa01")),
            CandidateDecision::Enrolled(EnrollmentOrigin::Sync)
        );
        assert_eq!(
            membership.decide(&key(ME, "bb02")),
            CandidateDecision::Enrolled(EnrollmentOrigin::Legacy)
        );
        assert!(watched_keys(ME, &fold, &scan, &[]).is_empty());
        assert!(membership.adoptions(ME).is_empty());

        // Reopening one and linking the other from a new open task resumes
        // both watches without a label or a new enrollment event.
        vault.write("job.md", "- [ ] #task Reopened [[mail/2026/aa01]]\n");
        vault.write("body.md", "- [/] #task New\n\t- [[mail/2026/bb02]]\n");
        let (fold, scan) = vault.fold_and_scan();
        let membership = Membership::new(&fold, &scan);
        assert_eq!(
            watched_keys(ME, &fold, &scan, &[]),
            vec![key(ME, "aa01"), key(ME, "bb02")]
        );
        assert!(membership.adoptions(ME).is_empty());
        assert_eq!(fold.threads().count(), 2);
    }

    #[test]
    fn in_flight_threads_are_skipped_not_filed_or_adopted() {
        let vault = MemberVault::new(&[(ME, "aa01"), (ME, "bb02")]);
        vault.write("job.md", "- [ ] #task Mine [[mail/2026/aa01]]\n");
        let (fold, scan) = vault.fold_and_scan();
        let plain = Membership::new(&fold, &scan);
        assert!(matches!(
            plain.decide(&key(ME, "aa01")),
            CandidateDecision::Adopt(_)
        ));
        assert_eq!(plain.decide(&key(ME, "bb02")), CandidateDecision::File);
        // A thread with pending recovery evidence is an in-flight filing:
        // never filed again, even when an open task links it.
        let in_flight = BTreeSet::from([key(ME, "aa01"), key(ME, "bb02")]);
        let membership =
            Membership::new(&fold, &scan).with_in_flight(&in_flight);
        assert_eq!(
            membership.decide(&key(ME, "aa01")),
            CandidateDecision::InFlight
        );
        assert_eq!(
            membership.decide(&key(ME, "bb02")),
            CandidateDecision::InFlight
        );
    }

    #[test]
    fn adoption_and_watching_stay_within_the_run_account() {
        let vault =
            MemberVault::new(&[(ME, "aa01"), ("other@example.com", "bb02")]);
        vault.write(
            "job.md",
            "- [ ] #task Mine [[mail/2026/aa01]]\n\
             - [ ] #task Theirs [[mail/2026/bb02]]\n",
        );
        let (fold, scan) = vault.fold_and_scan();
        let membership = Membership::new(&fold, &scan);
        let adopted: Vec<_> = membership
            .adoptions("ME@example.com")
            .into_iter()
            .map(|intent| intent.key)
            .collect();
        assert_eq!(adopted, vec![key(ME, "aa01")]);
        assert_eq!(scan.watchers(&key("other@example.com", "bb02")).count(), 1);
    }
}
