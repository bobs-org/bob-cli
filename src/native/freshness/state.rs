//! Pure freshness evaluation, computed at read time and never stored.
//!
//! Owned by `docs/freshness.md`. The JavaScript mirror lives in
//! bob-ledger-tools (`api.freshness.state`, `queue`, `counts`); both
//! sides run the state conformance vectors in that doc verbatim.

use chrono::NaiveDate;

use super::placement::read_freshness;
use crate::native::config::freshness::FreshnessConfig;

/// A task's freshness state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FreshState {
    New,
    Resurfaced,
    Rotten,
    Fresh,
}

impl FreshState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Resurfaced => "resurfaced",
            Self::Rotten => "rotten",
            Self::Fresh => "fresh",
        }
    }

    /// Stable read-time bucket for dashboard gating (`docs/freshness.md`
    /// §4): `new` tasks surface in NEW, `resurfaced` and `rotten` tasks
    /// surface in ROTTEN review, and `fresh` tasks stay in READY.
    pub(crate) fn bucket(self) -> Option<&'static str> {
        match self {
            Self::New => Some("new"),
            Self::Resurfaced | Self::Rotten => Some("rotten"),
            Self::Fresh => None,
        }
    }
}

/// Stable bucket for an evaluated state: out-of-scope (`None`) and
/// `Fresh` map to no bucket, so a null bucket alone never proves a
/// task is Ready — callers must also apply the Ready predicate.
pub(crate) fn bucket_for_state(
    state: Option<FreshState>,
) -> Option<&'static str> {
    state.and_then(FreshState::bucket)
}

/// Which lane a task walks in (`docs/freshness.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    Ready,
    Pending,
    Next,
}

impl Lane {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Pending => "pending",
            Self::Next => "next",
        }
    }
}

/// PRE/POST membership from exact `#gtd` + `#pre`/`#post` tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChecklistKind {
    Pre,
    Post,
}

/// Walk tier, in walk order (`docs/freshness.md` §4):
/// PRE → NEW → PROJECTS → PENDING → NEXT → RECURRING → TICKLER →
/// REFERENCES → ROTTEN → POST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    Pre,
    New,
    Projects,
    Pending,
    Next,
    Recurring,
    Tickler,
    References,
    Rotten,
    Post,
}

impl Tier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pre => "pre",
            Self::New => "new",
            Self::Projects => "projects",
            Self::Pending => "pending",
            Self::Next => "next",
            Self::Recurring => "recurring",
            Self::Tickler => "tickler",
            Self::References => "references",
            Self::Rotten => "rotten",
            Self::Post => "post",
        }
    }

    fn is_checklist(self) -> bool {
        matches!(self, Self::Pre | Self::Post)
    }
}

/// Tracking-task identity: the parsed, exact trailing block ID `prj`
/// on a real task, or — for references — the exact trailing block ID
/// `ref` or a whole-token `#ref` tag (case-insensitive). `^prj-extra`,
/// `#references`, description text, and `[[x#^prj]]` links/embeds are
/// never identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrackerKind {
    Prj,
    Ref,
}

impl TrackerKind {
    #[allow(dead_code)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Prj => "prj",
            Self::Ref => "ref",
        }
    }

    /// Ref-task identity is the `#ref` tag or the exact `^ref` block
    /// ID; `^prj` behavior is unchanged. Tags compare as whole tokens,
    /// case-insensitive (`#REF` qualifies, `#references` does not).
    pub(crate) fn from_tags_and_block_id(
        tags: &[String],
        block_id: Option<&str>,
    ) -> Option<Self> {
        match block_id {
            Some("prj") => Some(Self::Prj),
            Some("ref") => Some(Self::Ref),
            _ => {
                if tags.iter().any(|tag| tag.eq_ignore_ascii_case("#ref")) {
                    Some(Self::Ref)
                } else {
                    None
                }
            }
        }
    }
}

/// Where the effective interval came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntervalSource {
    Task,
    Note,
    Config,
    Default,
    Pending,
    Next,
    Project,
    Reference,
}

impl IntervalSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Note => "note",
            Self::Config => "config",
            Self::Default => "default",
            Self::Pending => "pending",
            Self::Next => "next",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }
}

/// Lane for one row: pending for `/`, next for `*`, ready for the
/// Tasks status type TODO, none otherwise.
pub(crate) fn lane_for_row(status: char, is_todo: bool) -> Option<Lane> {
    match status {
        '/' => Some(Lane::Pending),
        '*' => Some(Lane::Next),
        _ if is_todo => Some(Lane::Ready),
        _ => None,
    }
}

/// One input row for the evaluator.
///
/// Callers precompute the scope inputs: `is_todo` is the Tasks status
/// type TODO, `lane_visible` is the NEXT/PENDING lane predicate (not
/// done, not dependency-blocked, not under `_templates` or
/// `_conflicts`, no scheduled date after today, no `#hide`),
/// `is_daily_note` is a canonical daily note (`YYYY/YYYYMMDD.md`),
/// and `is_today` is membership in today's open Pomodoro Task Links.
///
/// Every row uses the ordinary lane-visible predicate with no `#hide`
/// exemption — including exact `^ref` rows (whose transitional bypass
/// was removed at closeout) and exact `^prj` rows (`bob projects sync`
/// owns that tag, so a hidden `^prj` is simply out of scope).
#[derive(Debug, Clone)]
pub(crate) struct FreshnessRow {
    pub(crate) path: String,
    /// 1-based line number, as in JSON and docs.
    pub(crate) line: u32,
    pub(crate) status: char,
    pub(crate) is_todo: bool,
    pub(crate) recurring: bool,
    pub(crate) lane_visible: bool,
    pub(crate) is_daily_note: bool,
    pub(crate) is_today: bool,
    pub(crate) scheduled: Option<NaiveDate>,
    pub(crate) due: Option<NaiveDate>,
    pub(crate) start: Option<NaiveDate>,
    pub(crate) created: Option<NaiveDate>,
    pub(crate) raw_line: String,
    /// The note's raw `task_refresh` frontmatter value, if present.
    pub(crate) note_refresh_raw: Option<String>,
    /// Tracking identity, if any: exact trailing `^prj`, or exact
    /// trailing `^ref` / whole-token `#ref` tag for references.
    pub(crate) tracker: Option<TrackerKind>,
    /// Exact `#gtd` + `#pre`/`#post` membership, if any.
    pub(crate) checklist: Option<ChecklistKind>,
}

/// Per-note Ready-lane counting predicate shared with `note_ready`:
/// visible TODO-type tasks physically resident in that path, not done,
/// dependency-blocked, or future-scheduled, excluding recurring tasks
/// and every `^prj` row. Callers pass only `READY_QUERY` rows, so the
/// lane predicate is already applied; this checks the two row-level
/// exclusions. It includes unconfirmed NEW, TICKLER, ROTTEN, and
/// fresh tasks (including Today-linked rows), ignores `ready_cap`
/// (including `off`), counts no Next/Pending rows, and never rolls up
/// child projects, embeds, backlinks, or parent membership. Uses
/// Tasks status types so custom TODO symbols behave like Ready.
#[allow(dead_code)]
pub(crate) fn is_counted_ready_row(
    is_recurring: bool,
    block_id: Option<&str>,
) -> bool {
    !is_recurring && block_id != Some("prj")
}

/// The evaluated result for one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evaluated {
    /// `None` when out of scope (see S13). Lane rows keep a null
    /// state and bucket but can still carry a walk tier.
    pub(crate) state: Option<FreshState>,
    /// Walk tier (`None` when in no tier). Lane `pending`/`next`
    /// rows carry a tier with a null state.
    pub(crate) tier: Option<Tier>,
    pub(crate) lane: Option<Lane>,
    pub(crate) fresh: Option<NaiveDate>,
    pub(crate) interval_days: u16,
    pub(crate) interval_source: IntervalSource,
    pub(crate) due_on: Option<NaiveDate>,
    pub(crate) days_overdue: Option<i64>,
    /// The valid `[keeps:: N]` semantic count (0 when absent).
    pub(crate) keeps: u32,
    /// A choice is due — not permission to execute an action:
    /// `enabled && lane ready && tier rotten/tickler && keeps >= limit`.
    pub(crate) decide: bool,
    pub(crate) lints: Vec<String>,
}

/// Whether a decision is due for a Ready-lane row in `tier` with
/// `keeps` counted keeps under `config`.
pub(crate) fn decide_for(
    lane: Option<Lane>,
    tier: Option<Tier>,
    keeps: u32,
    config: &FreshnessConfig,
) -> bool {
    let due_tier = matches!(tier, Some(Tier::Rotten) | Some(Tier::Tickler));
    config.decay.enabled
        && lane == Some(Lane::Ready)
        && due_tier
        && keeps >= u32::from(config.decay.keeps)
}

/// Exact whole-token tag match, case-insensitive. `#gtd/pre` is not
/// `#gtd`, and `#pressed_juice` is not `#pre`.
pub(crate) fn checklist_from_tags(tags: &[String]) -> Option<ChecklistKind> {
    let has =
        |needle: &str| tags.iter().any(|tag| tag.eq_ignore_ascii_case(needle));
    if has("#gtd") && has("#pre") {
        Some(ChecklistKind::Pre)
    } else if has("#gtd") && has("#post") {
        Some(ChecklistKind::Post)
    } else {
        None
    }
}

fn checklist_in_scope(row: &FreshnessRow) -> bool {
    row.checklist.is_some()
        && matches!(row.status, ' ' | '*' | '/' | '?')
        && row.lane_visible
}

fn overlay_checklist(
    row: &FreshnessRow,
    mut evaluated: Evaluated,
) -> Evaluated {
    let Some(kind) = row.checklist else {
        return evaluated;
    };
    if !checklist_in_scope(row) {
        return evaluated;
    }
    evaluated.tier = Some(match kind {
        ChecklistKind::Pre => Tier::Pre,
        ChecklistKind::Post => Tier::Post,
    });
    evaluated.due_on = None;
    evaluated.days_overdue = None;
    evaluated.decide = false;
    evaluated
}

/// The earliest of the row's valid `scheduled`, `due`, and `start`
/// dates (Tasks' "happens" date); none when it has none of them.
pub(crate) fn occurs_on(row: &FreshnessRow) -> Option<NaiveDate> {
    [row.scheduled, row.due, row.start]
        .into_iter()
        .flatten()
        .min()
}

/// Recurring overlay (`docs/freshness.md` §4): an open, visible,
/// non-checklist recurring row whose occurrence date has arrived
/// walks in RECURRING. Applied after the checklist overlay so no
/// early return in the evaluator can drop it. A checklist member
/// never reaches here with a recurring tier.
fn overlay_recurring(
    row: &FreshnessRow,
    mut evaluated: Evaluated,
    today: NaiveDate,
) -> Evaluated {
    if row.checklist.is_some() {
        return evaluated;
    }
    if !row.recurring {
        return evaluated;
    }
    if evaluated.lane.is_none() || !row.lane_visible {
        return evaluated;
    }
    if row.is_daily_note || row.is_today {
        return evaluated;
    }
    let Some(occurs) = occurs_on(row) else {
        return evaluated;
    };
    if occurs > today {
        return evaluated;
    }
    evaluated.tier = Some(Tier::Recurring);
    evaluated.due_on = Some(occurs);
    evaluated.days_overdue =
        Some(today.signed_duration_since(occurs).num_days());
    evaluated.decide = false;
    evaluated
}

/// Evaluate one row for `today` under `config`.
pub(crate) fn evaluate(
    row: &FreshnessRow,
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Evaluated {
    let base = evaluate_without_checklist(row, today, config);
    let checked = overlay_checklist(row, base);
    overlay_recurring(row, checked, today)
}

fn evaluate_without_checklist(
    row: &FreshnessRow,
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Evaluated {
    let read = read_freshness(&row.raw_line, today);
    let mut lints = read.lints.clone();

    let (note_interval, note_lint) =
        parse_note_refresh(row.note_refresh_raw.as_deref());
    if let Some(lint) = note_lint {
        push_lint(&mut lints, &lint);
    }

    let lane = lane_for_row(row.status, row.is_todo);
    let lane_days: Option<u16> = match lane {
        Some(Lane::Pending) => config.pending_interval,
        Some(Lane::Next) => config.next_interval,
        _ => None,
    };
    // Explicit tracker cadences override every other level for that
    // tracker type (`docs/freshness.md` §2). A tracker without a
    // configured cadence uses the Ready chain in every lane: the
    // weekly reminder must not become a daily lane review. Only Ready
    // refs keep the tracker cadence and REFERENCES tier; lane refs
    // (`[*]`/`[/]`) are ordinary lane rows (`docs/freshness.md` §6,
    // the J4 split). `^prj` behavior is unchanged.
    let is_prj = row.tracker == Some(TrackerKind::Prj);
    let is_ref =
        row.tracker == Some(TrackerKind::Ref) && lane == Some(Lane::Ready);
    let is_tracker = is_prj || is_ref;
    let tracker_override: Option<(u16, IntervalSource)> = if is_prj {
        config
            .project_interval
            .map(|days| (days, IntervalSource::Project))
    } else if is_ref {
        config
            .reference_interval
            .map(|days| (days, IntervalSource::Reference))
    } else {
        None
    };
    let (ready_days, ready_source) =
        interval_for(read.refresh, note_interval, config);
    let (interval_days, interval_source) = match tracker_override {
        Some(explicit) => explicit,
        None if is_tracker => (ready_days, ready_source),
        None => match (lane, lane_days) {
            (Some(Lane::Pending), Some(days)) => {
                (days, IntervalSource::Pending)
            }
            (Some(Lane::Next), Some(days)) => (days, IntervalSource::Next),
            _ => (ready_days, ready_source),
        },
    };

    let walk_scope = lane.is_some()
        && row.lane_visible
        && !row.recurring
        && !row.is_daily_note
        && !row.is_today;

    // Lane due date for walked ordinary lanes: fresh + lane interval,
    // or none when never stamped. Trackers never use the lane
    // interval here; their lane arithmetic lives below.
    let lane_due_on: Option<NaiveDate> = match (lane, lane_days, read.fresh) {
        (Some(Lane::Pending) | Some(Lane::Next), Some(days), Some(fresh)) => {
            fresh
                .checked_add_days(chrono::Days::new(u64::from(days)))
                .or(Some(fresh))
        }
        _ => None,
    };
    let lane_due = match (lane, lane_days) {
        (Some(Lane::Pending) | Some(Lane::Next), Some(days)) => {
            match read.fresh {
                None => true,
                Some(fresh) => {
                    let due = fresh
                        .checked_add_days(chrono::Days::new(u64::from(days)))
                        .unwrap_or(fresh);
                    today >= due
                }
            }
        }
        _ => false,
    };
    let lane_days_overdue: Option<i64> = match lane_due_on {
        Some(due) if today >= due => {
            Some(today.signed_duration_since(due).num_days())
        }
        Some(_) => None,
        None => None,
    };

    let in_scope = row.is_todo
        && row.lane_visible
        && !row.recurring
        && !row.is_daily_note
        && !row.is_today;

    // Effective resurfacing schedule: the inline schedule, once due,
    // participates in the existing resurfacing rule. Tracker rows use
    // the same ordinary inline handling as every other row.
    let resurfaced_on: Option<NaiveDate> = if !in_scope {
        None
    } else if let Some(scheduled) = row.scheduled
        && read
            .fresh
            .is_some_and(|fresh| fresh < scheduled && scheduled <= today)
    {
        Some(scheduled)
    } else {
        None
    };

    // Ready state: lane rows keep a null state (the lane early-return
    // below reports it); Ready rows evaluate normally, trackers
    // included. A hidden `^prj` never reaches here with `in_scope`
    // true because it uses the ordinary lane-visible predicate.
    let state: Option<FreshState> = if !in_scope
        || matches!(lane, Some(Lane::Pending) | Some(Lane::Next))
    {
        None
    } else {
        match read.fresh {
            None => Some(FreshState::New),
            Some(fresh) => {
                if resurfaced_on.is_some() {
                    Some(FreshState::Resurfaced)
                } else {
                    let due = fresh
                        .checked_add_days(chrono::Days::new(u64::from(
                            interval_days,
                        )))
                        .unwrap_or(fresh);
                    if today >= due {
                        Some(FreshState::Rotten)
                    } else {
                        Some(FreshState::Fresh)
                    }
                }
            }
        }
    };

    // Tracker lane due date uses the effective (tracker-override or
    // Ready-chain) interval, never the lane interval; a disabled lane
    // walk does not disable the reminder. Lane trackers are due when
    // never stamped, resurfaced, or at/over the effective interval.
    let tracker_lane_eligible = is_tracker
        && matches!(lane, Some(Lane::Pending) | Some(Lane::Next))
        && walk_scope;
    let tracker_lane_due = tracker_lane_eligible
        && (read.fresh.is_none()
            || resurfaced_on.is_some()
            || read.fresh.is_some_and(|fresh| {
                let due = fresh
                    .checked_add_days(chrono::Days::new(u64::from(
                        interval_days,
                    )))
                    .unwrap_or(fresh);
                today >= due
            }));
    let tracker_lane_due_on: Option<NaiveDate> = if !tracker_lane_due {
        None
    } else if let Some(scheduled) = resurfaced_on {
        Some(scheduled)
    } else {
        read.fresh.map(|fresh| {
            fresh
                .checked_add_days(chrono::Days::new(u64::from(interval_days)))
                .unwrap_or(fresh)
        })
    };

    // Tier: PROJECTS is every due `^prj` (Ready NEW/RESURFACED/ROTTEN
    // states, plus every due lane tracker with its actual lane
    // retained) and REFERENCES every due Ready `#ref`/`^ref` row; both
    // are checked before
    // NEW and the lane tiers, so a never-confirmed Ready reference
    // walks in REFERENCES, never NEW. PENDING/NEXT are due walked
    // ordinary lanes; TICKLER/ROTTEN are the Ready resurfaced/rotten
    // states. Tracker tiers never precede NEW for ordinary tasks.
    let tracker_ready_due = is_tracker
        && lane == Some(Lane::Ready)
        && matches!(
            state,
            Some(FreshState::New)
                | Some(FreshState::Resurfaced)
                | Some(FreshState::Rotten)
        );
    let tracker_lane_due_row = tracker_lane_eligible && tracker_lane_due;
    let tier: Option<Tier> = if is_prj
        && (tracker_ready_due || tracker_lane_due_row)
    {
        Some(Tier::Projects)
    } else if is_ref && (tracker_ready_due || tracker_lane_due_row) {
        Some(Tier::References)
    } else if lane == Some(Lane::Ready)
        && state == Some(FreshState::New)
        && !is_tracker
    {
        Some(Tier::New)
    } else if lane == Some(Lane::Pending)
        && walk_scope
        && lane_due
        && !is_tracker
    {
        Some(Tier::Pending)
    } else if lane == Some(Lane::Next) && walk_scope && lane_due && !is_tracker
    {
        Some(Tier::Next)
    } else if state == Some(FreshState::Resurfaced) {
        Some(Tier::Tickler)
    } else if state == Some(FreshState::Rotten) {
        Some(Tier::Rotten)
    } else {
        None
    };

    // A choice is due — never permission to act — for Ready due
    // rows at or over the keep limit.
    let keeps = read.keeps;
    let decide = decide_for(lane, tier, keeps, config);

    // Per-row dates: lane rows use the lane due date; Ready rows use
    // the state due date. Lane tracker rows keep their actual lane
    // with null state but use the effective tracker due metadata; a
    // disabled lane walk does not clear it.
    if matches!(lane, Some(Lane::Pending) | Some(Lane::Next)) {
        if is_tracker {
            let days_overdue = match tracker_lane_due_on {
                Some(due) if today >= due => {
                    Some(today.signed_duration_since(due).num_days())
                }
                _ => None,
            };
            return Evaluated {
                state: None,
                tier,
                lane,
                fresh: read.fresh,
                interval_days,
                interval_source,
                due_on: tracker_lane_due_on,
                days_overdue,
                keeps,
                decide,
                lints,
            };
        }
        let due_on = match read.fresh {
            Some(_) => lane_due_on,
            None => None,
        };
        let days_overdue = match read.fresh {
            Some(_) => lane_days_overdue,
            None => None,
        };
        // An unwalked lane falls back to the Ready chain interval
        // with no due date (L4).
        let (due_on, days_overdue) = match lane_days {
            Some(_) => (due_on, days_overdue),
            None => (None, None),
        };
        return Evaluated {
            state: None,
            tier,
            lane,
            fresh: read.fresh,
            interval_days,
            interval_source,
            due_on,
            days_overdue,
            keeps,
            decide,
            lints,
        };
    }

    match state {
        None => Evaluated {
            state: None,
            tier: None,
            lane,
            fresh: read.fresh,
            interval_days,
            interval_source,
            due_on: None,
            days_overdue: None,
            keeps,
            decide,
            lints,
        },
        Some(FreshState::New) => Evaluated {
            state,
            tier,
            lane,
            fresh: None,
            interval_days,
            interval_source,
            due_on: None,
            days_overdue: None,
            keeps,
            decide,
            lints,
        },
        Some(FreshState::Resurfaced) => {
            let fresh = read.fresh.expect("resurfaced has fresh");
            let scheduled = resurfaced_on
                .or(row.scheduled)
                .expect("resurfaced has schedule");
            let days_overdue =
                today.signed_duration_since(scheduled).num_days();
            Evaluated {
                state,
                tier,
                lane,
                fresh: Some(fresh),
                interval_days,
                interval_source,
                due_on: Some(scheduled),
                days_overdue: Some(days_overdue),
                keeps,
                decide,
                lints,
            }
        }
        Some(FreshState::Rotten) => {
            let fresh = read.fresh.expect("rotten has fresh");
            let due = fresh
                .checked_add_days(chrono::Days::new(u64::from(interval_days)))
                .unwrap_or(fresh);
            let days_overdue = today.signed_duration_since(due).num_days();
            Evaluated {
                state,
                tier,
                lane,
                fresh: Some(fresh),
                interval_days,
                interval_source,
                due_on: Some(due),
                days_overdue: Some(days_overdue),
                keeps,
                decide,
                lints,
            }
        }
        Some(FreshState::Fresh) => {
            let fresh = read.fresh.expect("fresh has fresh");
            let due = fresh
                .checked_add_days(chrono::Days::new(u64::from(interval_days)))
                .unwrap_or(fresh);
            Evaluated {
                state,
                tier,
                lane,
                fresh: Some(fresh),
                interval_days,
                interval_source,
                due_on: Some(due),
                days_overdue: None,
                keeps,
                decide,
                lints,
            }
        }
    }
}

/// Effective interval and where it came from: the task's
/// `[refresh:: N]`, then the note's `task_refresh`, then
/// `freshness.interval`, then 7.
fn interval_for(
    task_refresh: Option<u16>,
    note_refresh: Option<u16>,
    config: &FreshnessConfig,
) -> (u16, IntervalSource) {
    if let Some(days) = task_refresh {
        return (days, IntervalSource::Task);
    }
    if let Some(days) = note_refresh {
        return (days, IntervalSource::Note);
    }
    if config.interval_from_config {
        return (config.interval, IntervalSource::Config);
    }
    if config.interval != 7 {
        // A programmatically built config with a non-default interval
        // but no explicit flag still counts as configured.
        return (config.interval, IntervalSource::Config);
    }
    (7, IntervalSource::Default)
}

/// Parse a note's raw `task_refresh` frontmatter value.
fn parse_note_refresh(raw: Option<&str>) -> (Option<u16>, Option<String>) {
    let Some(raw) = raw else {
        return (None, None);
    };
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    if trimmed.is_empty() {
        return (None, None);
    }
    match trimmed.parse::<i64>() {
        Ok(number) if (1..=365).contains(&number) => {
            (Some(number as u16), None)
        }
        _ => (None, Some("task_refresh_invalid".to_string())),
    }
}

/// One entry of the review queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueEntry {
    pub(crate) rank: u32,
    pub(crate) tier: Tier,
    /// `None` for `[?]` checklist rows (and any other null-lane
    /// checklist member). Ordinary walk rows always carry a lane.
    pub(crate) lane: Option<Lane>,
    /// `None` for lane `pending`/`next` rows (S13).
    pub(crate) state: Option<FreshState>,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) due_on: Option<NaiveDate>,
    pub(crate) days_overdue: Option<i64>,
    pub(crate) interval_days: u16,
    pub(crate) created: Option<NaiveDate>,
    /// The valid `[keeps:: N]` semantic count (0 when absent).
    pub(crate) keeps: u32,
    /// A choice is due for this row — not permission to act.
    pub(crate) decide: bool,
}

/// Compare `created` with missing dates always last, in both
/// ascending and descending keys.
fn compare_created(
    a: Option<NaiveDate>,
    b: Option<NaiveDate>,
    descending: bool,
) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) if descending => y.cmp(&x),
        (Some(x), Some(y)) => x.cmp(&y),
    }
}

/// The review queue in tier order PRE → NEW → PROJECTS → PENDING →
/// NEXT → RECURRING → TICKLER → REFERENCES → ROTTEN → POST, with
/// each tier's comparator from `docs/freshness.md` §4.
pub(crate) fn queue(
    rows: &[FreshnessRow],
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Vec<QueueEntry> {
    let mut entries: Vec<QueueEntry> = Vec::new();

    for row in rows {
        let evaluated = evaluate(row, today, config);
        let Some(tier) = evaluated.tier else {
            continue;
        };
        if evaluated.lane.is_none() && !tier.is_checklist() {
            continue;
        }
        entries.push(QueueEntry {
            rank: 0,
            tier,
            lane: evaluated.lane,
            state: evaluated.state,
            path: row.path.clone(),
            line: row.line,
            due_on: evaluated.due_on,
            days_overdue: evaluated.days_overdue,
            interval_days: evaluated.interval_days,
            created: row.created,
            keeps: evaluated.keeps,
            decide: evaluated.decide,
        });
    }

    entries.sort_by(|a, b| {
        let tier_order = a.tier.cmp(&b.tier);
        if tier_order != std::cmp::Ordering::Equal {
            return tier_order;
        }
        match a.tier {
            Tier::Pre | Tier::Post | Tier::New => {
                a.path.cmp(&b.path).then(a.line.cmp(&b.line))
            }
            Tier::Recurring => a
                .due_on
                .cmp(&b.due_on)
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line)),
            Tier::Projects | Tier::Pending | Tier::Next | Tier::References => {
                // Never-stamped (`due_on` none) first, then due_on,
                // created, path, line.
                a.due_on
                    .cmp(&b.due_on)
                    .then(compare_created(a.created, b.created, false))
                    .then(a.path.cmp(&b.path))
                    .then(a.line.cmp(&b.line))
            }
            Tier::Tickler => a
                .due_on
                .cmp(&b.due_on)
                .then(compare_created(a.created, b.created, true))
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line)),
            Tier::Rotten => a
                .interval_days
                .cmp(&b.interval_days)
                .then(a.due_on.cmp(&b.due_on))
                .then(compare_created(a.created, b.created, true))
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line)),
        }
    });

    for (index, entry) in entries.iter_mut().enumerate() {
        entry.rank = index as u32 + 1;
    }
    entries
}

/// Tier histogram for the full queue: each key counts its walk tier,
/// and `walk` is their sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ByTier {
    pub(crate) pre: u32,
    pub(crate) new: u32,
    pub(crate) projects: u32,
    pub(crate) pending: u32,
    pub(crate) next: u32,
    pub(crate) recurring: u32,
    pub(crate) tickler: u32,
    pub(crate) references: u32,
    pub(crate) rotten: u32,
    pub(crate) post: u32,
}

impl ByTier {
    pub(crate) fn sum(self) -> u32 {
        self.pre
            + self.new
            + self.projects
            + self.pending
            + self.next
            + self.recurring
            + self.tickler
            + self.references
            + self.rotten
            + self.post
    }
}

/// Whole-vault counts for the review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Ready-state due (NEW + RESURFACED + ROTTEN) over the full
    /// review universe, including eligible Ready trackers once each.
    /// Pending/Next rows retain null state and never contribute.
    pub(crate) due: u32,
    pub(crate) new: u32,
    pub(crate) resurfaced: u32,
    pub(crate) rotten: u32,
    /// In-scope FRESH tasks.
    pub(crate) fresh: u32,
    /// Due `[/]` lane tasks (equals its tier count).
    pub(crate) pending_due: u32,
    /// Due `[*]` lane tasks (equals its tier count).
    pub(crate) next_due: u32,
    /// Due `PROJECTS` rows (equals its tier count).
    pub(crate) projects_due: u32,
    /// Due `REFERENCES` rows (equals its tier count).
    pub(crate) references_due: u32,
    /// Due `PRE` checklist rows (equals its tier count).
    pub(crate) pre_due: u32,
    /// Due `POST` checklist rows (equals its tier count).
    pub(crate) post_due: u32,
    /// Due `RECURRING` rows (equals its tier count).
    pub(crate) recurring_due: u32,
    /// Tier histogram for the actual full queue.
    pub(crate) by_tier: ByTier,
    /// Full queue length, before any `--limit` (`walk = sum(by_tier)`).
    pub(crate) walk: u32,
    /// Queue rows with a decision due (Ready rotten/tickler at or
    /// over the keep limit while decay is enabled).
    pub(crate) decide: u32,
    /// Tasks of any status outside `_templates` / `_conflicts` whose
    /// `fresh` equals today.
    pub(crate) refreshed_today: u32,
    /// Stamps today outside the lanes: `fresh` today whose status
    /// symbol is neither `/` nor `*`.
    pub(crate) upkeep_today: u32,
    pub(crate) budget: Option<u32>,
    pub(crate) budget_met: bool,
}

/// Count the review states over `rows`. Callers pass the combined
/// ready ∪ pending ∪ next rows for the tier counts; `refreshed_today`
/// and `upkeep_today` are computed over the same slice here and
/// overwritten from the all-status rows by the CLI.
pub(crate) fn counts(
    rows: &[FreshnessRow],
    today: NaiveDate,
    config: &FreshnessConfig,
) -> Counts {
    let mut due = 0;
    let mut new = 0;
    let mut resurfaced = 0;
    let mut rotten = 0;
    let mut fresh = 0;
    let mut by_tier = ByTier::default();
    let mut decide = 0;
    let mut refreshed_today = 0;
    let mut upkeep_today = 0;

    for row in rows {
        let evaluated = evaluate(row, today, config);
        if !is_excluded_count_path(&row.path) && evaluated.fresh == Some(today)
        {
            refreshed_today += 1;
            if row.status != '/' && row.status != '*' {
                upkeep_today += 1;
            }
        }
        if evaluated.decide {
            decide += 1;
        }
        // State totals count evaluated Ready states (a due Ready
        // tracker contributes its state to new/resurfaced/rotten and
        // its queue row to PROJECTS or REFERENCES); tier totals count
        // the actual full queue.
        match evaluated.state {
            Some(FreshState::New) => {
                new += 1;
                due += 1;
            }
            Some(FreshState::Resurfaced) => {
                resurfaced += 1;
                due += 1;
            }
            Some(FreshState::Rotten) => {
                rotten += 1;
                due += 1;
            }
            Some(FreshState::Fresh) => {
                fresh += 1;
            }
            None => {}
        }
        match evaluated.tier {
            Some(Tier::Pre) => by_tier.pre += 1,
            Some(Tier::New) => by_tier.new += 1,
            Some(Tier::Projects) => by_tier.projects += 1,
            Some(Tier::Pending) => by_tier.pending += 1,
            Some(Tier::Next) => by_tier.next += 1,
            Some(Tier::Recurring) => by_tier.recurring += 1,
            Some(Tier::Tickler) => by_tier.tickler += 1,
            Some(Tier::References) => by_tier.references += 1,
            Some(Tier::Rotten) => by_tier.rotten += 1,
            Some(Tier::Post) => by_tier.post += 1,
            None => {}
        }
    }

    let budget = config.rotten_daily_budget;
    let budget_met =
        budget.is_some_and(|goal| upkeep_today >= goal && new == 0);

    Counts {
        due,
        new,
        resurfaced,
        rotten,
        fresh,
        pending_due: by_tier.pending,
        next_due: by_tier.next,
        projects_due: by_tier.projects,
        references_due: by_tier.references,
        pre_due: by_tier.pre,
        post_due: by_tier.post,
        recurring_due: by_tier.recurring,
        by_tier,
        walk: by_tier.sum(),
        decide,
        refreshed_today,
        upkeep_today,
        budget,
        budget_met,
    }
}

/// Paths under `_templates` or `_conflicts` never count toward
/// `refreshed_today` or `upkeep_today`. Shared with `scan` so there
/// is one excluded-path helper.
pub(crate) fn is_excluded_count_path(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "_templates" || segment == "_conflicts")
}

fn push_lint(lints: &mut Vec<String>, code: &str) {
    if !lints.iter().any(|lint| lint == code) {
        lints.push(code.to_string());
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod state_tests;
