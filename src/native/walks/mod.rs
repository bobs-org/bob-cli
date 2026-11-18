//! Walk attention: one read-only fact per bracket-walk family.
//!
//! `s` review, `w` lanes, and `b` bank share the predicates in
//! `docs/walks.md` with the walker footer. A failure in one family
//! leaves the others free to report.
//! `w` reports `counts.excess`, the Mac `w` count.
//!
//! The `s` mark is on exactly when `]s` has a row to land on: attention
//! iff `walk > 0`, with `tier` set to the first non-empty group in walk
//! order (PRE → NEW → PROJECTS → PENDING → NEXT → RECURRING → TICKLER →
//! ROTTEN → POST).

pub(crate) mod cli;

use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use super::{
    bank::{
        config::{parse_bank_config, ConfigParse},
        grammar::{month_abbr, weekday_abbr},
        journal,
        list::{self, ListFilter},
        model::collect_rows,
        scan::{is_teaching_empty, scan_bank},
    },
    config::{self, ConfigError, PlanConfig},
    freshness::{
        scan::{review_counts, row_bucket, scan, RowCtx, ScanError, Snapshot},
        state::ByTier,
    },
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneCount {
    pub(crate) lane: &'static str,
    pub(crate) label: &'static str,
    pub(crate) count: u32,
    pub(crate) cap: u32,
    pub(crate) over: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FamilyBody {
    Review {
        walk: u32,
        commitments: u32,
    },
    Lanes {
        lanes: Vec<LaneCount>,
        excess: u32,
    },
    Bank {
        waiting: u32,
        oldest_waiting: Option<NaiveDate>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FamilyReport {
    pub(crate) family: &'static str,
    pub(crate) name: &'static str,
    pub(crate) available: bool,
    pub(crate) attention: bool,
    pub(crate) tier: Option<&'static str>,
    pub(crate) label: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) body: Option<FamilyBody>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkReport {
    pub(crate) date: NaiveDate,
    pub(crate) walks: [FamilyReport; 3],
}

pub(crate) enum VaultError {
    Unusable(String),
}

impl VaultError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Unusable(message) => message,
        }
    }
}

pub(crate) fn build_report(bob_dir: &Path) -> Result<WalkReport, VaultError> {
    if !bob_dir.is_dir() {
        return Err(VaultError::Unusable(format!(
            "vault root is not a directory: {}",
            bob_dir.display()
        )));
    }
    let today = super::env::current_datetime().date();
    let snapshot = scan(bob_dir).map_err(scan_reason);
    let plan =
        config::load_plan_config(&config::config_path()).map_err(plan_reason);
    Ok(WalkReport {
        date: today,
        walks: [
            review_family(snapshot.as_ref()),
            lanes_family(snapshot.as_ref(), plan.as_ref()),
            bank_family(bob_dir, today),
        ],
    })
}

fn scan_reason(error: ScanError) -> String {
    error.message().to_string()
}

fn plan_reason(error: ConfigError) -> String {
    error.to_string()
}

fn review_family(snapshot: Result<&Snapshot, &String>) -> FamilyReport {
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(reason) => return unavailable("s", "review", reason),
    };
    let counts = review_counts(snapshot);
    let commitments = commitment_total(&counts.by_tier);
    let tier = if counts.walk > 0 {
        first_walk_tier(&counts.by_tier)
    } else {
        None
    };
    // `walk` is `by_tier.sum()`, so a positive walk with no tier is
    // impossible; report calm rather than panic if it ever happens.
    let attention = tier.is_some();
    let label = tier.map(|tier| {
        review_label(counts.walk, commitments, footer_tier_label(tier))
    });
    FamilyReport {
        family: "s",
        name: "review",
        available: true,
        attention,
        tier,
        label,
        reason: None,
        body: Some(FamilyBody::Review {
            walk: counts.walk,
            commitments,
        }),
    }
}

fn lanes_family(
    snapshot: Result<&Snapshot, &String>,
    plan: Result<&PlanConfig, &String>,
) -> FamilyReport {
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(reason) => return unavailable("w", "lanes", reason),
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(reason) => return unavailable("w", "lanes", reason),
    };
    let pending = count_lane(snapshot.pending.iter(), |_| true);
    let next = count_lane(snapshot.next.iter(), |_| true);
    let ready = count_lane(snapshot.ready.iter(), |row| {
        !matches!(
            row_bucket(row, snapshot.today, &snapshot.config).0,
            Some("new" | "rotten")
        )
    });
    let lanes = vec![
        lane("pending", "WIP", pending, plan.max_pending()),
        lane("next", "NEXT", next, plan.max_next()),
        lane("ready", "READY", ready, plan.max_ready()),
    ];
    let over: Vec<&LaneCount> = lanes.iter().filter(|item| item.over).collect();
    let attention = !over.is_empty();
    let label = attention.then(|| lanes_over_label(&over));
    FamilyReport {
        family: "w",
        name: "lanes",
        available: true,
        attention,
        tier: attention.then_some("over"),
        label,
        reason: None,
        body: Some(FamilyBody::Lanes {
            excess: lanes_excess(&lanes),
            lanes,
        }),
    }
}

fn lanes_excess(lanes: &[LaneCount]) -> u32 {
    lanes
        .iter()
        .map(|lane| lane.count.saturating_sub(lane.cap))
        .sum()
}

fn count_lane<'a, I, F>(rows: I, extra: F) -> u32
where
    I: Iterator<Item = &'a RowCtx>,
    F: Fn(&RowCtx) -> bool,
{
    rows.filter(|row| counts_in_lane(row) && extra(row)).count() as u32
}

fn counts_in_lane(row: &RowCtx) -> bool {
    !row.is_today && !is_vault_root_dash(&row.task.path)
}

fn is_vault_root_dash(path: &str) -> bool {
    path.eq_ignore_ascii_case("dash.md")
}

fn lane(
    key: &'static str,
    label: &'static str,
    count: u32,
    cap: u32,
) -> LaneCount {
    LaneCount {
        lane: key,
        label,
        count,
        cap,
        over: count > cap,
    }
}

fn bank_family(bob_dir: &Path, today: NaiveDate) -> FamilyReport {
    let scan = scan_bank(bob_dir);
    if !scan.io_errors.is_empty() {
        return unavailable("b", "bank", &scan.io_errors.join("; "));
    }
    if is_teaching_empty(&scan) {
        return bank_available(0, None);
    }
    let config = match scan.hub.as_ref() {
        None => return unavailable("b", "bank", "bank.md is missing"),
        Some(hub) => match parse_bank_config(&hub.contents) {
            ConfigParse::Ok(config) => config,
            ConfigParse::MissingMapping => {
                return unavailable(
                    "b",
                    "bank",
                    "bank.md has no bank: mapping",
                );
            }
            ConfigParse::Invalid { diagnostics, .. } => {
                let first = diagnostics
                    .first()
                    .map(|diagnostic| {
                        format!(
                            "{}: {}",
                            diagnostic.key_path, diagnostic.message
                        )
                    })
                    .unwrap_or_else(|| "invalid bank configuration".to_owned());
                return unavailable("b", "bank", &first);
            }
        },
    };
    let journal = journal::load(bob_dir).ok();
    let rows = collect_rows(&scan, Some(&config), journal.as_ref());
    let heading_lines = list::collect_heading_lines(&scan);
    let report = list::build(
        rows,
        today,
        ListFilter {
            all: false,
            month: None,
        },
        Vec::new(),
        Vec::new(),
        heading_lines,
    );
    let waiting = u32::try_from(report.waiting).unwrap_or(u32::MAX);
    let oldest_waiting = report.days.first().map(|day| day.date);
    bank_available(waiting, oldest_waiting)
}

fn bank_available(
    waiting: u32,
    oldest_waiting: Option<NaiveDate>,
) -> FamilyReport {
    let attention = waiting > 0;
    let label = attention.then(|| bank_label(waiting, oldest_waiting));
    FamilyReport {
        family: "b",
        name: "bank",
        available: true,
        attention,
        tier: attention.then_some("waiting"),
        label,
        reason: None,
        body: Some(FamilyBody::Bank {
            waiting,
            oldest_waiting,
        }),
    }
}

fn unavailable(
    family: &'static str,
    name: &'static str,
    reason: &str,
) -> FamilyReport {
    FamilyReport {
        family,
        name,
        available: false,
        attention: false,
        tier: None,
        label: None,
        reason: Some(reason.to_string()),
        body: None,
    }
}

fn commitment_total(by_tier: &ByTier) -> u32 {
    by_tier.pre
        + by_tier.new
        + by_tier.projects
        + by_tier.pending
        + by_tier.next
        + by_tier.recurring
        + by_tier.tickler
}

fn first_walk_tier(by_tier: &ByTier) -> Option<&'static str> {
    [
        ("pre", by_tier.pre),
        ("new", by_tier.new),
        ("projects", by_tier.projects),
        ("pending", by_tier.pending),
        ("next", by_tier.next),
        ("recurring", by_tier.recurring),
        ("tickler", by_tier.tickler),
        ("rotten", by_tier.rotten),
        ("post", by_tier.post),
    ]
    .into_iter()
    .find(|(_, count)| *count > 0)
    .map(|(tier, _)| tier)
}

pub(crate) fn footer_tier_label(tier: &str) -> &'static str {
    match tier {
        "pending" => "WIP",
        "recurring" => "RECUR",
        "tickler" => "TICKS",
        "rotten" => "ROTTEN",
        "post" => "POST",
        "pre" => "PRE",
        "new" => "NEW",
        "projects" => "PROJECTS",
        "next" => "NEXT",
        _ => "",
    }
}

pub(crate) fn review_label(walk: u32, commitments: u32, first: &str) -> String {
    match commitments {
        1 => format!("Review: {walk} due · 1 commitment — {first} first"),
        0 => format!("Review: {walk} due · commitments done — {first} first"),
        _ => format!(
            "Review: {walk} due · {commitments} commitments — {first} first"
        ),
    }
}

pub(crate) fn lanes_over_label(over: &[&LaneCount]) -> String {
    let parts = over
        .iter()
        .map(|lane| format!("{} {}/{}", lane.label, lane.count, lane.cap))
        .collect::<Vec<_>>()
        .join(" · ");
    format!("Lanes over limit: {parts}")
}

pub(crate) fn bank_label(waiting: u32, oldest: Option<NaiveDate>) -> String {
    match oldest {
        Some(date) => {
            format!("Bank: {waiting} waiting · oldest {}", long_day_label(date))
        }
        None => format!("Bank: {waiting} waiting"),
    }
}

pub(crate) fn long_day_label(date: NaiveDate) -> String {
    format!(
        "{} {} {}",
        weekday_abbr(date),
        month_abbr(date.month()),
        date.day()
    )
}

impl WalkReport {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "ok": true,
            "schema_version": SCHEMA_VERSION,
            "date": self.date.format("%Y-%m-%d").to_string(),
            "walks": self.walks.iter().map(family_json).collect::<Vec<_>>(),
        })
    }
}

fn family_json(family: &FamilyReport) -> Value {
    let mut value = json!({
        "family": family.family,
        "name": family.name,
        "available": family.available,
        "attention": family.attention,
        "tier": family.tier,
        "label": family.label,
    });
    let object = value.as_object_mut().expect("family object");
    if let Some(reason) = &family.reason {
        object.insert("reason".to_string(), json!(reason));
    }
    match &family.body {
        Some(FamilyBody::Review { walk, commitments }) => {
            object.insert(
                "counts".to_string(),
                json!({ "walk": walk, "commitments": commitments }),
            );
        }
        Some(FamilyBody::Lanes { lanes, excess }) => {
            object.insert("counts".to_string(), json!({ "excess": excess }));
            object.insert(
                "lanes".to_string(),
                json!(lanes
                    .iter()
                    .map(|lane| json!({
                        "lane": lane.lane,
                        "label": lane.label,
                        "count": lane.count,
                        "cap": lane.cap,
                        "over": lane.over,
                    }))
                    .collect::<Vec<_>>()),
            );
        }
        Some(FamilyBody::Bank {
            waiting,
            oldest_waiting,
        }) => {
            object.insert("counts".to_string(), json!({ "waiting": waiting }));
            object.insert(
                "oldest_waiting".to_string(),
                json!(oldest_waiting
                    .map(|date| date.format("%Y-%m-%d").to_string())),
            );
        }
        None => {}
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn review_label_matches_the_walker_footer() {
        assert_eq!(
            review_label(14, 4, "NEXT"),
            "Review: 14 due · 4 commitments — NEXT first"
        );
        assert_eq!(footer_tier_label("tickler"), "TICKS");
        assert_eq!(footer_tier_label("pending"), "WIP");
        assert_eq!(footer_tier_label("recurring"), "RECUR");
    }

    #[test]
    fn lane_and_bank_labels_match_the_walker_footer() {
        let pending = lane("pending", "WIP", 12, 10);
        let next = lane("next", "NEXT", 16, 15);
        assert_eq!(
            lanes_over_label(&[&pending, &next]),
            "Lanes over limit: WIP 12/10 · NEXT 16/15"
        );
        let date = NaiveDate::from_ymd_opt(2026, 11, 3).expect("date");
        assert_eq!(long_day_label(date), "Tue Nov 3");
        assert_eq!(
            bank_label(3, Some(date)),
            "Bank: 3 waiting · oldest Tue Nov 3"
        );
        assert_eq!(bank_label(3, None), "Bank: 3 waiting");
    }

    #[test]
    fn lane_excess_sums_only_over_cap_rows() {
        let over = [
            lane("pending", "WIP", 12, 10),
            lane("next", "NEXT", 3, 15),
            lane("ready", "READY", 101, 100),
        ];
        let over_count = over.iter().filter(|item| item.over).count();
        assert_eq!(lanes_excess(&over), 3);
        assert_eq!(over_count > 0, lanes_excess(&over) > 0);

        let at_or_under = [
            lane("pending", "WIP", 10, 10),
            lane("next", "NEXT", 0, 15),
            lane("ready", "READY", 0, 100),
        ];
        let over_count = at_or_under.iter().filter(|item| item.over).count();
        assert_eq!(lanes_excess(&at_or_under), 0);
        assert_eq!(over_count > 0, lanes_excess(&at_or_under) > 0);
    }

    #[test]
    fn first_walk_tier_covers_all_nine_in_walk_order() {
        let by_tier = ByTier {
            new: 1,
            rotten: 4,
            post: 2,
            next: 3,
            ..ByTier::default()
        };
        assert_eq!(commitment_total(&by_tier), 4);
        assert_eq!(first_walk_tier(&by_tier), Some("new"));
        assert_eq!(
            first_walk_tier(&ByTier {
                rotten: 2,
                post: 1,
                ..ByTier::default()
            }),
            Some("rotten")
        );
        assert_eq!(
            first_walk_tier(&ByTier {
                post: 1,
                ..ByTier::default()
            }),
            Some("post")
        );
        assert_eq!(first_walk_tier(&ByTier::default()), None);
        assert_eq!(footer_tier_label("rotten"), "ROTTEN");
        assert_eq!(footer_tier_label("post"), "POST");
    }

    #[test]
    fn review_label_covers_all_three_commitment_forms() {
        assert_eq!(
            review_label(3, 3, "PRE"),
            "Review: 3 due · 3 commitments — PRE first"
        );
        assert_eq!(
            review_label(2, 1, "NEW"),
            "Review: 2 due · 1 commitment — NEW first"
        );
        assert_eq!(
            review_label(1, 0, "ROTTEN"),
            "Review: 1 due · commitments done — ROTTEN first"
        );
        assert_eq!(
            review_label(1, 0, "POST"),
            "Review: 1 due · commitments done — POST first"
        );
    }
}
