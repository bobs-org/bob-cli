//! `bob gkeep status`: the cached Keep count plus the live inbox count.
//!
//! Default runs are cache only: no token read, no adapter, no network,
//! no ledger scan. `-r` runs at most one never-prompting Keep check when
//! bob's gates allow it. The gates are a pure function of the cache
//! document, the config, the clock, `-m`, and two lock probes, so
//! clients (the Obsidian footer) ask and bob decides.

use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::Path,
    time::Duration,
};

use chrono::{DateTime, Duration as TimeDelta, FixedOffset, Utc};
use fs2::FileExt;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    adapter::{AdapterClient, Credentials},
    config::{CheckInterval, GkeepConfig, DEFAULT_TARGET},
    ledger::{is_open_status, read_target_tasks_with_imports},
    pull::pull_lock_path,
    status_cache::{
        fmt_utc, observe_default, parse_utc, status_dir, CacheDoc, Failure,
        PullCounts, StatusCache, StayCounts,
    },
    ui, GkeepError, StatusArgs,
};
use crate::native::{env as bob_env, note_tasks, style::Styler};

/// Minimum spacing between status-initiated Keep contacts.
const FLOOR: TimeDelta = TimeDelta::seconds(60);
/// Automatic checks allowed per local day.
const DAILY_BUDGET: u32 = 48;
/// Adapter deadline for a status check, independent of
/// `gkeep.timeout_secs`.
const ADAPTER_DEADLINE: Duration = Duration::from_secs(30);

/// Why `-r` did not contact Keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Skip {
    Unconfigured,
    Auth,
    Busy,
    InFlight,
    Floor,
    Backoff,
    Off,
    Fresh,
    Budget,
}

impl Skip {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::Auth => "auth",
            Self::Busy => "busy",
            Self::InFlight => "in_flight",
            Self::Floor => "floor",
            Self::Backoff => "backoff",
            Self::Off => "off",
            Self::Fresh => "fresh",
            Self::Budget => "budget",
        }
    }
}

/// The moment a status call runs at, in Bob's local day.
#[derive(Debug, Clone)]
pub(super) struct Clock {
    pub(super) now: DateTime<Utc>,
    /// `YYYY-MM-DD` of the local day (the budget key).
    pub(super) day: String,
    /// The next local midnight, when a spent budget resets.
    pub(super) next_midnight: DateTime<Utc>,
    /// Bob's UTC offset now, for clock times in human output.
    pub(super) offset: FixedOffset,
}

impl Clock {
    fn current() -> Self {
        let now = ui::now_utc();
        let local = bob_env::current_datetime();
        let date = local.date();
        let midnight = date
            .succ_opt()
            .unwrap_or(date)
            .and_hms_opt(0, 0, 0)
            .unwrap_or(local);
        let drift = (local - now.naive_utc()).num_seconds();
        let quarter = ((drift as f64) / 900.0).round() as i32 * 900;
        Self {
            now,
            day: date.format("%Y-%m-%d").to_string(),
            next_midnight: midnight.and_utc()
                - TimeDelta::seconds(i64::from(quarter)),
            offset: FixedOffset::east_opt(quarter).unwrap_or_else(|| {
                FixedOffset::east_opt(0).expect("zero offset")
            }),
        }
    }

    fn hhmm(&self, at: DateTime<Utc>) -> String {
        at.with_timezone(&self.offset).format("%H:%M").to_string()
    }
}

/// Everything the gates read.
pub(super) struct GateInput<'a> {
    pub(super) doc: &'a CacheDoc,
    pub(super) interval: CheckInterval,
    /// `-m`, when given.
    pub(super) max_age: Option<Duration>,
    /// `-r -m 0`.
    pub(super) forced: bool,
    pub(super) busy: bool,
    pub(super) in_flight: bool,
    pub(super) clock: &'a Clock,
}

/// The ordered gates after "configured": `Ok(())` means contact Keep.
pub(super) fn evaluate(input: &GateInput<'_>) -> Result<(), Skip> {
    let doc = input.doc;
    let now = input.clock.now;
    let failure = doc.failure.as_ref();
    if failure.is_some_and(|failure| failure.state == "auth") {
        return Err(Skip::Auth);
    }
    if input.busy {
        return Err(Skip::Busy);
    }
    if input.in_flight {
        return Err(Skip::InFlight);
    }
    if let Some(last) = doc.last_contact_at.as_deref().and_then(parse_utc)
        && now < last + FLOOR
        && last <= now + FLOOR
    {
        return Err(Skip::Floor);
    }
    if let Some(failure) = failure
        && let Some(end) = backoff_end(failure)
        && now < end
        && (failure.state == "rate_limited" || !input.forced)
    {
        return Err(Skip::Backoff);
    }
    let max_age = match (input.max_age, input.interval) {
        (Some(max_age), _) => Some(max_age),
        (None, CheckInterval::Every(interval)) => Some(interval),
        (None, CheckInterval::Off) => None,
    };
    let Some(max_age) = max_age else {
        return Err(Skip::Off);
    };
    if doc.invalidated_at.is_none()
        && let Some(observation) = &doc.observation
        && let Some(started) = parse_utc(&observation.started_at)
        && now - started < to_delta(max_age)
    {
        return Err(Skip::Fresh);
    }
    if !input.forced && budget_spent(doc, input.clock) {
        return Err(Skip::Budget);
    }
    Ok(())
}

fn to_delta(duration: Duration) -> TimeDelta {
    TimeDelta::seconds(i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
}

fn budget_spent(doc: &CacheDoc, clock: &Clock) -> bool {
    doc.budget
        .as_ref()
        .is_some_and(|b| b.day == clock.day && b.used >= DAILY_BUDGET)
}

/// When the failure's backoff window closes; `None` for states that
/// never back off (`auth`) or unknown ones.
///
/// From the failure count `n`: `offline` 5 min × 2^(n−1) capped at 2 h,
/// `error` 30 min × 2^(n−1) capped at 2 h, `rate_limited` 60 min ×
/// 2^(n−1) capped at 6 h, `locked` a flat 30 min; each ±10% by a
/// jitter derived from `last_at`, so repeated renders agree.
pub(super) fn backoff_end(failure: &Failure) -> Option<DateTime<Utc>> {
    let last = parse_utc(&failure.last_at)?;
    let doublings = failure.count.saturating_sub(1).min(16);
    let grow = |base_min: i64, cap_min: i64| {
        (base_min.saturating_mul(1 << doublings)).min(cap_min)
    };
    let minutes = match failure.state.as_str() {
        "offline" => grow(5, 120),
        "error" => grow(30, 120),
        "rate_limited" => grow(60, 360),
        "locked" => 30,
        _ => return None,
    };
    let permille = jitter_permille(&failure.last_at);
    Some(last + TimeDelta::seconds(minutes * 60 * permille / 1000))
}

/// A deterministic factor in 900..=1100 (per mille) from `seed`.
fn jitter_permille(seed: &str) -> i64 {
    let digest = Sha256::digest(seed.as_bytes());
    let value = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    900 + i64::try_from(value % 201).expect("small")
}

/// The earliest moment an automatic `-r` could contact Keep; `None`
/// when automatic checks will not run (`off`, `auth`).
pub(super) fn next_check_at(
    doc: &CacheDoc,
    interval: CheckInterval,
    clock: &Clock,
) -> Option<DateTime<Utc>> {
    let CheckInterval::Every(interval) = interval else {
        return None;
    };
    if doc.failure.as_ref().is_some_and(|f| f.state == "auth") {
        return None;
    }
    let observed = doc
        .observation
        .as_ref()
        .and_then(|o| parse_utc(&o.started_at));
    let mut at = match observed {
        Some(started) if doc.invalidated_at.is_none() => {
            started + to_delta(interval)
        }
        _ => clock.now,
    };
    if let Some(end) = doc.failure.as_ref().and_then(backoff_end) {
        at = at.max(end);
    }
    if let Some(last) = doc.last_contact_at.as_deref().and_then(parse_utc) {
        at = at.max(last + FLOOR);
    }
    if budget_spent(doc, clock) {
        at = at.max(clock.next_midnight);
    }
    Some(at)
}

/// `bob gkeep status`.
pub(crate) fn run(args: &StatusArgs) -> i32 {
    let format = args.error_format();
    let config = match GkeepConfig::resolve_optional() {
        Ok(config) => config,
        Err(error) => return ui::report_error("status", &error, format),
    };
    let bob_dir = args.bob_dir();
    let clock = Clock::current();
    let inbox = read_inbox(
        &bob_dir,
        config.as_ref().map_or(DEFAULT_TARGET, GkeepConfig::target),
    );
    let Some(config) = config else {
        let refresh = args.refresh.then_some(Refresh {
            ran: false,
            skipped: Some(Skip::Unconfigured),
        });
        render(args, &unconfigured_view(inbox, refresh), &clock);
        return 0;
    };

    let busy = pull_busy();
    let (in_flight, guard) = if args.refresh {
        match take_check_lock() {
            CheckLock::Held(file) => (false, Some(file)),
            CheckLock::Taken => (true, None),
            CheckLock::Unavailable => (true, None),
        }
    } else {
        (check_lock_taken(), None)
    };

    let mut refresh = None;
    if args.refresh {
        let doc = StatusCache::load(&config);
        let input = GateInput {
            doc: &doc,
            interval: config.check_interval(),
            max_age: args.max_age,
            forced: args.forced(),
            busy,
            in_flight,
            clock: &clock,
        };
        refresh = Some(match evaluate(&input) {
            Err(skip) => Refresh {
                ran: false,
                skipped: Some(skip),
            },
            Ok(()) => {
                run_check(&config, &bob_dir, &clock, args.forced());
                Refresh {
                    ran: true,
                    skipped: None,
                }
            }
        });
    }
    drop(guard);

    let doc = StatusCache::load(&config);
    let view =
        View::build(&config, &doc, &clock, busy, in_flight, refresh, inbox);
    render(args, &view, &clock);
    0
}

/// One never-prompting Keep check; records the observation or failure.
fn run_check(
    config: &GkeepConfig,
    bob_dir: &Path,
    clock: &Clock,
    forced: bool,
) {
    StatusCache::begin_check(config, clock.now, &clock.day, forced);
    let started_at = ui::now_utc();
    let token = match config.read_token_unattended() {
        Ok(token) => token,
        Err(error) => {
            StatusCache::record_failure(config, &error, started_at);
            return;
        }
    };
    let client = match AdapterClient::resolve(config) {
        Ok(client) => client.with_timeout(ADAPTER_DEADLINE),
        Err(error) => {
            // A missing `uv` is a setup error for a human command; in the
            // background it must still back off, so record it as an
            // adapter failure.
            let mut failure =
                GkeepError::runtime("adapter", error.message().to_string());
            if let Some(hint) = error.hint() {
                failure = failure.with_hint(hint);
            }
            StatusCache::record_failure(config, &failure, started_at);
            return;
        }
    };
    let credentials = Credentials::from_config(config, &token);
    let notes = match client.snapshot(&credentials, false, None) {
        Ok(notes) => notes,
        Err(error) => {
            StatusCache::record_failure(config, &error, started_at);
            return;
        }
    };
    match observe_default(&notes, bob_dir, started_at, "status") {
        Some(observation) => {
            StatusCache::record_observation(config, observation);
        }
        None => StatusCache::record_failure(
            config,
            &GkeepError::runtime(
                "internal",
                "could not read the vault to classify Keep notes".to_string(),
            ),
            started_at,
        ),
    }
}

/// Whether a running `bob gkeep pull` holds `pull.lock`: a momentary
/// shared lock that a real pull's retry loop tolerates.
fn pull_busy() -> bool {
    lock_taken(&pull_lock_path())
}

fn check_lock_taken() -> bool {
    lock_taken(&status_dir().join("status-check.lock"))
}

fn lock_taken(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).open(path) else {
        return false;
    };
    match FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        Err(error) => error.kind() == ErrorKind::WouldBlock,
    }
}

enum CheckLock {
    /// We hold the single-flight lock for the whole check.
    Held(File),
    /// Another `status -r` holds it.
    Taken,
    /// The lock file cannot be created (unwritable state directory).
    Unavailable,
}

fn take_check_lock() -> CheckLock {
    let file = match bob_env::create_state_lock_file(
        &status_dir(),
        "status-check.lock",
    ) {
        Ok(file) => file,
        Err(error) => {
            ui::warn(&format!(
                "could not create the Keep status-check lock: {error}"
            ));
            return CheckLock::Unavailable;
        }
    };
    match file.try_lock_exclusive() {
        Ok(()) => CheckLock::Held(file),
        Err(error) if error.kind() == ErrorKind::WouldBlock => CheckLock::Taken,
        Err(error) => {
            ui::warn(&format!(
                "could not take the Keep status-check lock: {error}"
            ));
            CheckLock::Unavailable
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refresh {
    ran: bool,
    skipped: Option<Skip>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct InboxView {
    path: String,
    exists: bool,
    open: Option<usize>,
}

/// The target note's open top-level task count; reads that file alone.
fn read_inbox(bob_dir: &Path, target_rel: &str) -> InboxView {
    let path = bob_dir.join(target_rel);
    let (exists, open) = match fs::read_to_string(&path) {
        Ok(contents) => {
            let settings = note_tasks::read_settings(bob_dir);
            let open =
                read_target_tasks_with_imports(&contents, &settings, &[])
                    .iter()
                    .filter(|task| is_open_status(task.status_symbol))
                    .count();
            (true, Some(open))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (false, None),
        Err(_) => (true, None),
    };
    InboxView {
        path: target_rel.to_string(),
        exists,
        open,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ErrorView {
    kind: String,
    message: String,
    hint: Option<String>,
    since: String,
    failures: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RefreshView {
    ran: bool,
    skipped: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct KeepView {
    state: &'static str,
    verified: bool,
    stale: bool,
    in_flight: bool,
    busy: bool,
    checked_at: Option<String>,
    next_check_at: Option<String>,
    source: Option<String>,
    pull: Option<PullCounts>,
    stays: Option<StayCounts>,
    error: Option<ErrorView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh: Option<RefreshView>,
}

/// The public status document (`schema_version: 1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct View {
    schema_version: u32,
    ok: bool,
    configured: bool,
    keep: KeepView,
    inbox: InboxView,
}

fn unconfigured_view(inbox: InboxView, refresh: Option<Refresh>) -> View {
    View {
        schema_version: 1,
        ok: true,
        configured: false,
        keep: KeepView {
            state: "unconfigured",
            verified: false,
            stale: false,
            in_flight: false,
            busy: false,
            checked_at: None,
            next_check_at: None,
            source: None,
            pull: None,
            stays: None,
            error: None,
            refresh: refresh.map(refresh_view),
        },
        inbox,
    }
}

fn refresh_view(refresh: Refresh) -> RefreshView {
    RefreshView {
        ran: refresh.ran,
        skipped: refresh.skipped.map(Skip::as_str),
    }
}

fn failure_state_name(state: &str) -> &'static str {
    match state {
        "offline" => "offline",
        "locked" => "locked",
        "rate_limited" => "rate_limited",
        "auth" => "auth",
        _ => "error",
    }
}

impl View {
    fn build(
        config: &GkeepConfig,
        doc: &CacheDoc,
        clock: &Clock,
        busy: bool,
        in_flight: bool,
        refresh: Option<Refresh>,
        inbox: InboxView,
    ) -> Self {
        let state = match (&doc.failure, &doc.observation) {
            (Some(failure), _) => failure_state_name(&failure.state),
            (None, Some(_)) if doc.verified => "ok",
            _ => "unverified",
        };
        let stale = doc.observation.is_some()
            && (doc.failure.is_some() || doc.invalidated_at.is_some());
        let observation = doc.observation.as_ref();
        View {
            schema_version: 1,
            ok: true,
            configured: true,
            keep: KeepView {
                state,
                verified: doc.verified,
                stale,
                in_flight,
                busy,
                checked_at: observation.map(|o| o.started_at.clone()),
                next_check_at: next_check_at(
                    doc,
                    config.check_interval(),
                    clock,
                )
                .map(fmt_utc),
                source: observation.map(|o| o.source.clone()),
                pull: observation.map(|o| o.pull.clone()),
                stays: observation.map(|o| o.stays.clone()),
                error: doc.failure.as_ref().map(|failure| ErrorView {
                    kind: failure.kind.clone(),
                    message: failure.message.clone(),
                    hint: failure.hint.clone(),
                    since: failure.since.clone(),
                    failures: failure.count,
                }),
                refresh: refresh.map(refresh_view),
            },
            inbox,
        }
    }
}

fn render(args: &StatusArgs, view: &View, clock: &Clock) {
    if args.format.is_json() {
        println!(
            "{}",
            serde_json::to_string_pretty(view).expect("status serializes")
        );
    } else {
        print!("{}", human(view, clock, &Styler::detect()));
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn stays_text(stays: &StayCounts) -> Vec<String> {
    [
        (stays.pinned, "pinned"),
        (stays.shared, "shared"),
        (stays.empty, "empty"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| {
        format!(
            "{count} {label} {}",
            if count == 1 { "stays" } else { "stay" }
        )
    })
    .collect()
}

fn pull_text(pull: &PullCounts) -> Vec<String> {
    let mut parts = vec![format!("{} to pull", pull.total)];
    if pull.new > 0 {
        parts.push(format!("{} new", pull.new));
    }
    if pull.links > 0 {
        parts.push(format!(
            "{} → reading queue",
            plural(pull.links, "link", "links")
        ));
    }
    if pull.revised > 0 {
        parts.push(format!("{} revised", pull.revised));
    }
    if pull.pending > 0 {
        parts.push(format!("{} pending archive", pull.pending));
    }
    parts
}

fn ago(now: DateTime<Utc>, then: &str) -> Option<String> {
    let then = parse_utc(then)?;
    let secs = (now - then).num_seconds();
    Some(if secs < 60 {
        "just now".to_string()
    } else {
        format!("{} ago", ui::format_age(now.timestamp(), then.timestamp()))
    })
}

/// Human output (colored through `Styler`, plain when piped).
fn human(view: &View, clock: &Clock, styler: &Styler) -> String {
    let keep = &view.keep;
    let dot = " · ";
    let mut out = String::new();
    let label = styler.yellow("💡 Keep ");
    let mut hint: Option<String> = None;
    let line = match keep.state {
        "unconfigured" => format!(
            "{} {} run bob gkeep login",
            styler.dim("not set up on this host"),
            styler.dim("·")
        ),
        "auth" => {
            format!("{} · run bob gkeep login", styler.red("sign-in expired"))
        }
        "unverified" => "not checked yet · run bob gkeep status -r".to_string(),
        _ => {
            let age = keep
                .checked_at
                .as_deref()
                .and_then(|at| ago(clock.now, at))
                .map(|age| {
                    format!("   {}", styler.dim(&format!("checked {age}")))
                });
            let at = |stamp: &str| {
                parse_utc(stamp)
                    .map(|at| clock.hhmm(at))
                    .unwrap_or_default()
            };
            let next = keep
                .next_check_at
                .as_deref()
                .map(|stamp| format!("{dot}next try about {}", at(stamp)))
                .unwrap_or_default();
            let summary = keep.pull.as_ref().map(|pull| {
                let mut parts = pull_text(pull);
                let mut stays = stays_text(
                    keep.stays.as_ref().unwrap_or(&StayCounts::default()),
                );
                if pull.total == 0 {
                    parts = vec![styler.green("✓ clear")];
                }
                parts.append(&mut stays);
                parts
            });
            match (&keep.error, summary, &keep.checked_at) {
                (None, Some(parts), _) if !keep.stale => {
                    format!("{}{}", parts.join(dot), age.unwrap_or_default())
                }
                (None, Some(parts), Some(checked)) => format!(
                    "{}, as of {}{dot}a pull changed Keep since{dot}rechecking soon",
                    parts.join(dot),
                    at(checked)
                ),
                (Some(error), summary, checked) => {
                    hint = error.hint.clone();
                    let head = match (summary, checked) {
                        (Some(parts), Some(checked)) => {
                            format!("{}, as of {}", parts.join(dot), at(checked))
                        }
                        _ => "no count yet".to_string(),
                    };
                    let problem = match keep.state {
                        "offline" => {
                            format!("offline since {}", at(&error.since))
                        }
                        "rate_limited" => {
                            "Google asked Bob to slow down".to_string()
                        }
                        "locked" => "can't read the Keep token without a \
                                     passphrase prompt"
                            .to_string(),
                        _ => format!("check failed: {}", error.message),
                    };
                    format!("{head}{dot}{}{next}", styler.yellow(&problem))
                }
                _ => "not checked yet · run bob gkeep status -r".to_string(),
            }
        }
    };
    out.push_str(&format!("{label} {line}\n"));
    if let Some(hint) = hint {
        out.push_str(&styler.dim(&format!("         hint: {hint}\n")));
    }

    let inbox = &view.inbox;
    let inbox_line = match (inbox.exists, inbox.open) {
        (false, _) => format!("{} not found", inbox.path),
        (true, None) => format!("{} could not be read", inbox.path),
        (true, Some(open)) => format!("{open} open in {}", inbox.path),
    };
    out.push_str(&format!("{} {inbox_line}\n", styler.cyan("📥 Inbox")));

    if let Some(refresh) = &keep.refresh {
        let note = match refresh.skipped {
            None if keep.error.is_some() => {
                "check failed; see above".to_string()
            }
            None => "checked Keep just now".to_string(),
            Some(skip) => {
                let next = keep
                    .next_check_at
                    .as_deref()
                    .and_then(parse_utc)
                    .filter(|_| {
                        matches!(skip, "fresh" | "backoff" | "floor" | "budget")
                    })
                    .map(|at| format!("{dot}next check {}", clock.hhmm(at)))
                    .unwrap_or_default();
                format!("skipped: {skip}{next}")
            }
        };
        out.push_str(&styler.dim(&note));
        out.push('\n');
    }
    if keep.pull.as_ref().is_some_and(|pull| pull.total > 0) {
        out.push_str(&format!("→ {}\n", styler.bold("bob gkeep pull")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gkeep::status_cache::{Budget, Observation};

    fn ts(stamp: &str) -> DateTime<Utc> {
        parse_utc(stamp).expect("test timestamp")
    }

    fn clock_at(stamp: &str) -> Clock {
        let now = ts(stamp);
        Clock {
            now,
            day: now.format("%Y-%m-%d").to_string(),
            next_midnight: ts("2026-11-01T00:00:00Z"),
            offset: FixedOffset::east_opt(0).expect("utc"),
        }
    }

    const NOW: &str = "2026-10-31T14:00:00Z";
    const THIRTY: CheckInterval =
        CheckInterval::Every(Duration::from_secs(30 * 60));

    fn empty_doc() -> CacheDoc {
        CacheDoc {
            cache_version: 1,
            config_key: "key".to_string(),
            verified: false,
            observation: None,
            invalidated_at: None,
            failure: None,
            last_contact_at: None,
            budget: None,
        }
    }

    fn observed(started: &str) -> Observation {
        Observation {
            started_at: started.to_string(),
            source: "list".to_string(),
            pull: PullCounts {
                total: 3,
                new: 1,
                links: 1,
                revised: 0,
                pending: 1,
            },
            stays: StayCounts {
                pinned: 1,
                shared: 0,
                empty: 0,
            },
        }
    }

    fn doc_observed(started: &str) -> CacheDoc {
        CacheDoc {
            verified: true,
            observation: Some(observed(started)),
            ..empty_doc()
        }
    }

    fn failing(state: &str, count: u32, last_at: &str) -> Failure {
        Failure {
            state: state.to_string(),
            kind: "network".to_string(),
            message: "down".to_string(),
            hint: None,
            since: last_at.to_string(),
            last_at: last_at.to_string(),
            count,
        }
    }

    fn gate(
        doc: &CacheDoc,
        max_age: Option<Duration>,
        forced: bool,
    ) -> Result<(), Skip> {
        gate_with(doc, THIRTY, max_age, forced, false, false)
    }

    fn gate_with(
        doc: &CacheDoc,
        interval: CheckInterval,
        max_age: Option<Duration>,
        forced: bool,
        busy: bool,
        in_flight: bool,
    ) -> Result<(), Skip> {
        evaluate(&GateInput {
            doc,
            interval,
            max_age,
            forced,
            busy,
            in_flight,
            clock: &clock_at(NOW),
        })
    }

    #[test]
    fn no_observation_runs_and_a_young_one_is_fresh() {
        assert_eq!(gate(&empty_doc(), None, false), Ok(()));
        let young = doc_observed("2026-10-31T13:40:00Z");
        assert_eq!(gate(&young, None, false), Err(Skip::Fresh));
        let old = doc_observed("2026-10-31T13:20:00Z");
        assert_eq!(gate(&old, None, false), Ok(()));
    }

    #[test]
    fn max_age_overrides_the_interval_and_zero_forces() {
        let young = doc_observed("2026-10-31T13:40:00Z");
        let ten_minutes = Some(Duration::from_secs(600));
        assert_eq!(gate(&young, ten_minutes, false), Ok(()));
        let zero = Some(Duration::ZERO);
        assert_eq!(gate(&young, zero, true), Ok(()));
        let long = Some(Duration::from_secs(7200));
        let older = doc_observed("2026-10-31T12:30:00Z");
        assert_eq!(gate(&older, long, false), Err(Skip::Fresh));
    }

    #[test]
    fn invalidation_makes_a_young_observation_due() {
        let mut doc = doc_observed("2026-10-31T13:55:00Z");
        assert_eq!(gate(&doc, None, false), Err(Skip::Fresh));
        doc.invalidated_at = Some("2026-10-31T13:58:00Z".to_string());
        assert_eq!(gate(&doc, None, false), Ok(()));
    }

    #[test]
    fn off_blocks_only_unforced_refresh_without_max_age() {
        let doc = doc_observed("2026-10-31T10:00:00Z");
        let off = |max_age, forced| {
            gate_with(&doc, CheckInterval::Off, max_age, forced, false, false)
        };
        assert_eq!(off(None, false), Err(Skip::Off));
        assert_eq!(off(Some(Duration::from_secs(60)), false), Ok(()));
        assert_eq!(off(Some(Duration::ZERO), true), Ok(()));
    }

    #[test]
    fn auth_refuses_even_a_forced_check() {
        let mut doc = doc_observed("2026-10-31T10:00:00Z");
        doc.failure = Some(failing("auth", 1, "2026-10-31T10:30:00Z"));
        assert_eq!(gate(&doc, None, false), Err(Skip::Auth));
        assert_eq!(gate(&doc, Some(Duration::ZERO), true), Err(Skip::Auth));
    }

    #[test]
    fn busy_and_in_flight_come_before_floor_and_freshness() {
        let doc = doc_observed("2026-10-31T13:59:00Z");
        let busy = gate_with(&doc, THIRTY, None, false, true, true);
        assert_eq!(busy, Err(Skip::Busy));
        let flight = gate_with(&doc, THIRTY, None, false, false, true);
        assert_eq!(flight, Err(Skip::InFlight));
        let forced =
            gate_with(&doc, THIRTY, Some(Duration::ZERO), true, true, false);
        assert_eq!(forced, Err(Skip::Busy));
    }

    #[test]
    fn floor_blocks_within_sixty_seconds_even_when_forced() {
        let mut doc = doc_observed("2026-10-31T10:00:00Z");
        doc.last_contact_at = Some("2026-10-31T13:59:30Z".to_string());
        assert_eq!(gate(&doc, None, false), Err(Skip::Floor));
        assert_eq!(gate(&doc, Some(Duration::ZERO), true), Err(Skip::Floor));
        doc.last_contact_at = Some("2026-10-31T13:58:59Z".to_string());
        assert_eq!(gate(&doc, None, false), Ok(()));
    }

    #[test]
    fn rate_limit_backoff_refuses_forced_checks_too() {
        let mut doc = doc_observed("2026-10-31T10:00:00Z");
        doc.failure = Some(failing("rate_limited", 1, "2026-10-31T13:30:00Z"));
        assert_eq!(gate(&doc, None, false), Err(Skip::Backoff));
        assert_eq!(gate(&doc, Some(Duration::ZERO), true), Err(Skip::Backoff));
        doc.failure = Some(failing("rate_limited", 1, "2026-10-31T12:30:00Z"));
        assert_eq!(gate(&doc, None, false), Ok(()));
    }

    #[test]
    fn offline_locked_and_error_backoff_yield_to_a_forced_check() {
        for state in ["offline", "locked", "error"] {
            let mut doc = doc_observed("2026-10-31T10:00:00Z");
            doc.failure = Some(failing(state, 1, "2026-10-31T13:58:00Z"));
            assert_eq!(gate(&doc, None, false), Err(Skip::Backoff), "{state}");
            assert_eq!(
                gate(&doc, Some(Duration::ZERO), true),
                Ok(()),
                "{state} forced"
            );
        }
    }

    #[test]
    fn budget_blocks_automatic_checks_until_the_next_day() {
        let mut doc = doc_observed("2026-10-31T10:00:00Z");
        doc.budget = Some(Budget {
            day: "2026-10-31".to_string(),
            used: 48,
        });
        assert_eq!(gate(&doc, None, false), Err(Skip::Budget));
        assert_eq!(gate(&doc, Some(Duration::ZERO), true), Ok(()));
        doc.budget = Some(Budget {
            day: "2026-10-31".to_string(),
            used: 47,
        });
        assert_eq!(gate(&doc, None, false), Ok(()));
        doc.budget = Some(Budget {
            day: "2026-10-30".to_string(),
            used: 48,
        });
        assert_eq!(gate(&doc, None, false), Ok(()), "yesterday's budget");
    }

    #[test]
    fn backoff_grows_doubles_and_caps() {
        let at = |state: &str, count: u32| {
            let last = "2026-10-31T00:00:00Z";
            let failure = failing(state, count, last);
            let end = backoff_end(&failure).expect("backoff");
            (end - ts(last)).num_seconds()
        };
        let within = |secs: i64, minutes: i64| {
            let nominal = minutes * 60;
            secs >= nominal * 9 / 10 && secs <= nominal * 11 / 10
        };
        assert!(within(at("offline", 1), 5));
        assert!(within(at("offline", 2), 10));
        assert!(within(at("offline", 3), 20));
        assert!(within(at("offline", 20), 120), "capped at 2 h");
        assert!(within(at("error", 1), 30));
        assert!(within(at("error", 2), 60));
        assert!(within(at("error", 9), 120));
        assert!(within(at("rate_limited", 1), 60));
        assert!(within(at("rate_limited", 2), 120));
        assert!(within(at("rate_limited", 99), 360), "capped at 6 h");
        assert!(within(at("locked", 1), 30));
        assert!(within(at("locked", 7), 30), "flat");
        assert!(backoff_end(&failing("auth", 1, NOW)).is_none());
    }

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        let a = jitter_permille("2026-10-31T00:00:00Z");
        assert_eq!(a, jitter_permille("2026-10-31T00:00:00Z"));
        let mut seen = std::collections::BTreeSet::new();
        for minute in 0..60 {
            let value =
                jitter_permille(&format!("2026-10-31T00:{minute:02}:00Z"));
            assert!((900..=1100).contains(&value), "{value}");
            seen.insert(value);
        }
        assert!(seen.len() > 10, "jitter varies with its seed");
    }

    #[test]
    fn next_check_at_cases() {
        let clock = clock_at(NOW);
        let next = |doc: &CacheDoc, interval| {
            next_check_at(doc, interval, &clock).map(fmt_utc)
        };
        // No observation: now.
        assert_eq!(
            next(&empty_doc(), THIRTY).as_deref(),
            Some("2026-10-31T14:00:00Z")
        );
        // Observation plus the interval.
        let doc = doc_observed("2026-10-31T13:50:00Z");
        assert_eq!(next(&doc, THIRTY).as_deref(), Some("2026-10-31T14:20:00Z"));
        // Off and auth never schedule.
        assert_eq!(next(&doc, CheckInterval::Off), None);
        let mut authed = doc.clone();
        authed.failure = Some(failing("auth", 1, "2026-10-31T13:55:00Z"));
        assert_eq!(next(&authed, THIRTY), None);
        // Invalidated: now.
        let mut invalidated = doc.clone();
        invalidated.invalidated_at = Some("2026-10-31T13:58:00Z".to_string());
        assert_eq!(
            next(&invalidated, THIRTY).as_deref(),
            Some("2026-10-31T14:00:00Z")
        );
        // The floor lifts the time.
        let mut floored = invalidated.clone();
        floored.last_contact_at = Some("2026-10-31T13:59:30Z".to_string());
        assert_eq!(
            next(&floored, THIRTY).as_deref(),
            Some("2026-10-31T14:00:30Z")
        );
        // A backoff window lifts the time past the interval.
        let mut backing = doc.clone();
        backing.failure =
            Some(failing("rate_limited", 1, "2026-10-31T13:55:00Z"));
        let end = next_check_at(&backing, THIRTY, &clock).expect("scheduled");
        assert!(end >= ts("2026-10-31T14:49:00Z"), "{end}");
        // A spent budget waits for midnight.
        let mut spent = doc.clone();
        spent.budget = Some(Budget {
            day: "2026-10-31".to_string(),
            used: 48,
        });
        assert_eq!(
            next(&spent, THIRTY).as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
    }

    #[test]
    fn skip_names_match_the_contract() {
        let names: Vec<_> = [
            Skip::Unconfigured,
            Skip::Auth,
            Skip::Busy,
            Skip::InFlight,
            Skip::Floor,
            Skip::Backoff,
            Skip::Off,
            Skip::Fresh,
            Skip::Budget,
        ]
        .into_iter()
        .map(Skip::as_str)
        .collect();
        assert_eq!(
            names,
            [
                "unconfigured",
                "auth",
                "busy",
                "in_flight",
                "floor",
                "backoff",
                "off",
                "fresh",
                "budget"
            ]
        );
    }

    fn view_for(doc: &CacheDoc, refresh: Option<Refresh>) -> View {
        let config = GkeepConfig::for_tests("bryan@example.com", "abc123", 30);
        View::build(
            &config,
            doc,
            &clock_at(NOW),
            false,
            false,
            refresh,
            InboxView {
                path: "gkeep_inbox.md".to_string(),
                exists: true,
                open: Some(1),
            },
        )
    }

    #[test]
    fn view_states_follow_the_cache() {
        let ok = view_for(&doc_observed("2026-10-31T13:56:00Z"), None);
        assert_eq!(ok.keep.state, "ok");
        assert!(ok.keep.verified && !ok.keep.stale);

        let fresh = view_for(&empty_doc(), None);
        assert_eq!(fresh.keep.state, "unverified");
        assert!(fresh.keep.pull.is_none());

        let mut offline = doc_observed("2026-10-31T13:56:00Z");
        offline.failure = Some(failing("offline", 2, "2026-10-31T13:57:00Z"));
        let offline = view_for(&offline, None);
        assert_eq!(offline.keep.state, "offline");
        assert!(offline.keep.stale);
        assert_eq!(offline.keep.error.as_ref().map(|e| e.failures), Some(2));

        let mut invalid = doc_observed("2026-10-31T13:56:00Z");
        invalid.invalidated_at = Some("2026-10-31T13:58:00Z".to_string());
        let invalid = view_for(&invalid, None);
        assert_eq!(invalid.keep.state, "ok");
        assert!(invalid.keep.stale);
    }

    #[test]
    fn json_matches_the_contract_shape() {
        let view = view_for(
            &doc_observed("2026-10-31T13:56:00Z"),
            Some(Refresh {
                ran: false,
                skipped: Some(Skip::Fresh),
            }),
        );
        let json = serde_json::to_value(&view).expect("serializes");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["ok"], true);
        assert_eq!(json["configured"], true);
        let keep = &json["keep"];
        assert_eq!(keep["state"], "ok");
        assert_eq!(keep["checked_at"], "2026-10-31T13:56:00Z");
        assert_eq!(keep["next_check_at"], "2026-10-31T14:26:00Z");
        assert_eq!(keep["source"], "list");
        assert_eq!(keep["pull"]["total"], 3);
        assert_eq!(keep["stays"]["pinned"], 1);
        assert!(keep["error"].is_null());
        assert_eq!(keep["refresh"]["ran"], false);
        assert_eq!(keep["refresh"]["skipped"], "fresh");
        assert_eq!(json["inbox"]["open"], 1);

        let plain = serde_json::to_value(view_for(&empty_doc(), None))
            .expect("serializes");
        assert!(plain["keep"].get("refresh").is_none());
        assert!(plain["keep"]["pull"].is_null());
    }

    #[test]
    fn human_lines_for_every_state() {
        let styler = Styler::plain();
        let clock = clock_at(NOW);
        let text = |doc: &CacheDoc, refresh| {
            human(&view_for(doc, refresh), &clock, &styler)
        };

        let ok = text(&doc_observed("2026-10-31T13:56:00Z"), None);
        assert!(
            ok.contains(
                "3 to pull · 1 new · 1 link → reading queue · 1 pending \
                 archive · 1 pinned stays   checked 4m ago"
            ),
            "{ok}"
        );
        assert!(ok.contains("📥 Inbox 1 open in gkeep_inbox.md"), "{ok}");
        assert!(ok.trim_end().ends_with("→ bob gkeep pull"), "{ok}");

        let mut clear = doc_observed("2026-10-31T13:56:00Z");
        if let Some(observation) = &mut clear.observation {
            observation.pull = PullCounts::default();
        }
        let clear = text(&clear, None);
        assert!(
            clear.contains("✓ clear · 1 pinned stays   checked 4m ago"),
            "{clear}"
        );
        assert!(!clear.contains("bob gkeep pull"), "{clear}");

        let mut offline = doc_observed("2026-10-31T14:02:00Z");
        offline.observation.as_mut().expect("obs").started_at =
            "2026-10-31T13:02:00Z".to_string();
        let mut failure = failing("offline", 1, "2026-10-31T13:05:00Z");
        failure.hint = Some("check your network".to_string());
        offline.failure = Some(failure);
        let offline = text(&offline, None);
        assert!(
            offline.contains(
                "as of 13:02 · offline since 13:05 · next try about "
            ),
            "{offline}"
        );
        assert!(offline.contains("hint: check your network"), "{offline}");

        let unchecked = text(&empty_doc(), None);
        assert!(
            unchecked.contains("not checked yet · run bob gkeep status -r"),
            "{unchecked}"
        );

        let mut authed = empty_doc();
        authed.failure = Some(failing("auth", 1, "2026-10-31T13:05:00Z"));
        let authed = text(&authed, None);
        assert!(
            authed.contains("sign-in expired · run bob gkeep login"),
            "{authed}"
        );

        let skipped = text(
            &doc_observed("2026-10-31T13:56:00Z"),
            Some(Refresh {
                ran: false,
                skipped: Some(Skip::Fresh),
            }),
        );
        assert!(
            skipped.contains("skipped: fresh · next check 14:26"),
            "{skipped}"
        );
        let ran = text(
            &doc_observed("2026-10-31T13:56:00Z"),
            Some(Refresh {
                ran: true,
                skipped: None,
            }),
        );
        assert!(ran.contains("checked Keep just now"), "{ran}");

        let unconfigured = human(
            &unconfigured_view(
                InboxView {
                    path: "gkeep_inbox.md".to_string(),
                    exists: false,
                    open: None,
                },
                None,
            ),
            &clock,
            &styler,
        );
        assert!(unconfigured
            .contains("not set up on this host · run bob gkeep login"));
        assert!(unconfigured.contains("gkeep_inbox.md not found"));
    }

    #[test]
    fn inbox_counts_the_parity_fixture() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(
            dir.path().join("gkeep_inbox.md"),
            "---\nparent: \"[[inbox]]\"\n---\n\n# Keep inbox\n\n\
             - [ ] Call the bank #task\n\
             - [?] Blocked thing #task\n\
             - [*] Next thing #task\n\
             - [/] In progress thing #task\n\
             - [x] Done thing #task\n\
             - [x] Done uppercase #task\n\
             - [-] Cancelled thing #task\n\
             - [ ] Grocery list #task\n  - [ ] milk\n  - [ ] eggs\n  \
             - [ ] Hand-nested subtask #task\n\
             - [ ] Untagged top-level line\n\n\
             ```text\n- [ ] Fenced #task line\n```\n",
        )
        .expect("write target");
        let inbox = read_inbox(dir.path(), "gkeep_inbox.md");
        assert!(inbox.exists);
        assert_eq!(inbox.open, Some(5));

        let missing = read_inbox(dir.path(), "absent.md");
        assert!(!missing.exists);
        assert_eq!(missing.open, None);
    }
}
