//! Read-only `bob bank subscriptions` report.
//!
//! One invocation loads a single subscription registry and one computed
//! ledger snapshot, then presents the same facts every plugin surface
//! reads: per-record ledgers, status-grouped human output, and the JSON
//! contract for the linker, the header strip, and the album card.

use std::{collections::HashMap, path::Path};

use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use crate::native::style::{display_width, pad_right, Styler};

use super::{
    check::{config_finding, Finding, IssueCounts, Severity},
    check_subscriptions,
    config::{parse_bank_config, BankConfig, ConfigDiagnostic, ConfigParse},
    grammar::render_amount,
    model::collect_rows,
    scan::BankScan,
    subscriptions::{
        ledger::{
            compute_ledger, LedgerCharge, LedgerSnapshot, PriceChange,
            TimelineBucket,
        },
        registry::{
            SubscriptionAvailability, SubscriptionCadence, SubscriptionRecord,
            SubscriptionRegistry,
        },
    },
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// The Active group splits into category subgroups past this many records.
const ACTIVE_SPLIT_AT: usize = 8;

const STATUS_ORDER: [&str; 4] =
    ["to_cancel", "to_decide", "active", "cancelled"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CategoryRef {
    pub(crate) key: String,
    pub(crate) emoji: Option<String>,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionEntry {
    pub(crate) record: SubscriptionRecord,
    pub(crate) category: Option<CategoryRef>,
    pub(crate) monthly_cents: Option<i64>,
    pub(crate) charges_count: usize,
    pub(crate) total_cents: Option<i64>,
    pub(crate) latest: Option<LedgerCharge>,
    pub(crate) next_expected: Option<NaiveDate>,
    pub(crate) price_change: Option<PriceChange>,
    pub(crate) still_charging: bool,
    pub(crate) after_cancel: Option<LedgerCharge>,
    pub(crate) timeline: Vec<TimelineBucket>,
    pub(crate) finding_codes: Vec<String>,
    pub(crate) visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusTotal {
    pub(crate) key: String,
    pub(crate) count: usize,
    pub(crate) monthly_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionsReport {
    pub(crate) today: NaiveDate,
    pub(crate) show_all: bool,
    pub(crate) teaching_empty: bool,
    pub(crate) album_stem: Option<String>,
    pub(crate) album_path: Option<String>,
    pub(crate) album_title: Option<String>,
    pub(crate) album_emoji: Option<String>,
    pub(crate) availability: SubscriptionAvailability,
    pub(crate) entries: Vec<SubscriptionEntry>,
    pub(crate) hidden_cancelled: usize,
    pub(crate) monthly_cents: i64,
    pub(crate) yearly_cents: Option<i64>,
    pub(crate) by_status: Vec<StatusTotal>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) issues: IssueCounts,
    pub(crate) config_fingerprint: Option<String>,
    pub(crate) config_error: Option<String>,
}

impl SubscriptionsReport {
    pub(crate) fn build(
        vault: &Path,
        scan: &BankScan,
        today: NaiveDate,
        show_all: bool,
    ) -> Self {
        let journal = super::journal::load(vault).unwrap_or_default();
        let PartialConfig {
            config,
            error: config_error,
            diagnostics: config_diagnostics,
            fingerprint,
        } = load_partial_config(scan);
        let rows = collect_rows(scan, config.as_ref(), Some(&journal));
        let registry = SubscriptionRegistry::load(vault);
        if registry.availability == SubscriptionAvailability::Missing {
            return Self::teaching_empty(today, show_all);
        }
        let ledger = compute_ledger(
            &registry,
            &journal,
            &rows,
            today,
            config.as_ref(),
            &HashMap::new(),
        );
        let mut findings = check_subscriptions::findings(
            config.as_ref(),
            &registry,
            &journal,
            &rows,
            today,
        );
        if !config_diagnostics.is_empty() {
            // Every located config error, in source order, ahead of the
            // subscription findings (the same shape `bob bank check` emits).
            findings.splice(
                0..0,
                config_diagnostics.into_iter().map(config_finding),
            );
        } else if let Some(message) = &config_error {
            findings.insert(
                0,
                Finding {
                    code: "bank_config_invalid",
                    severity: Severity::Error,
                    path: "bank.md".to_owned(),
                    line: None,
                    message: message.clone(),
                    hint: Some(
                        "Fix the bank: mapping; the report below uses the readable part only."
                            .to_owned(),
                    ),
                    day: None,
                    key_path: None,
                    proposal: None,
                },
            );
        }
        Self::from_parts(
            today,
            show_all,
            &registry,
            &ledger,
            findings,
            config.as_ref(),
            fingerprint,
            config_error,
        )
    }

    fn teaching_empty(today: NaiveDate, show_all: bool) -> Self {
        Self {
            today,
            show_all,
            teaching_empty: true,
            album_stem: None,
            album_path: None,
            album_title: None,
            album_emoji: None,
            availability: SubscriptionAvailability::Missing,
            entries: Vec::new(),
            hidden_cancelled: 0,
            monthly_cents: 0,
            yearly_cents: Some(0),
            by_status: Vec::new(),
            findings: Vec::new(),
            issues: IssueCounts {
                errors: 0,
                warnings: 0,
            },
            config_fingerprint: None,
            config_error: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        today: NaiveDate,
        show_all: bool,
        registry: &SubscriptionRegistry,
        ledger: &LedgerSnapshot,
        findings: Vec<Finding>,
        config: Option<&BankConfig>,
        config_fingerprint: Option<String>,
        config_error: Option<String>,
    ) -> Self {
        let mut entries = Vec::new();
        for record in &registry.records {
            let record_ledger = ledger.records.get(&record.path);
            let monthly_cents =
                record_ledger.and_then(|ledger| ledger.monthly_cents);
            let finding_codes = findings
                .iter()
                .filter(|finding| finding.path == record.path)
                .map(|finding| finding.code.to_owned())
                .collect::<Vec<_>>();
            let visible = !record.status.is_cancelled()
                || show_all
                || !finding_codes.is_empty();
            entries.push(SubscriptionEntry {
                category: category_ref(config, record.category.as_deref()),
                charges_count: record_ledger
                    .map(|ledger| ledger.charges.len())
                    .unwrap_or_default(),
                total_cents: record_ledger.and_then(|ledger| ledger.net_cents),
                latest: record_ledger.and_then(|ledger| ledger.latest.clone()),
                next_expected: record_ledger
                    .and_then(|ledger| ledger.next_expected),
                price_change: record_ledger
                    .and_then(|ledger| ledger.price_change.clone()),
                still_charging: record_ledger
                    .is_some_and(|ledger| ledger.still_charging),
                after_cancel: record_ledger
                    .and_then(|ledger| ledger.after_cancel.clone()),
                timeline: record_ledger
                    .map(|ledger| ledger.timeline.clone())
                    .unwrap_or_default(),
                finding_codes,
                visible,
                monthly_cents,
                record: record.clone(),
            });
        }
        entries.sort_by(|left, right| {
            status_rank(left.record.status.as_str())
                .cmp(&status_rank(right.record.status.as_str()))
                .then_with(|| {
                    left.record
                        .title
                        .to_lowercase()
                        .cmp(&right.record.title.to_lowercase())
                })
                .then_with(|| left.record.path.cmp(&right.record.path))
        });
        let hidden_cancelled =
            entries.iter().filter(|entry| !entry.visible).count();
        let mut monthly_cents: i64 = 0;
        for entry in &entries {
            if let Some(cents) = entry.monthly_cents {
                monthly_cents = monthly_cents.saturating_add(cents);
            }
        }
        let yearly_cents = monthly_cents.checked_mul(12);
        let mut by_status = Vec::new();
        for key in STATUS_ORDER {
            let mut total = StatusTotal {
                key: key.to_owned(),
                count: 0,
                monthly_cents: 0,
            };
            for entry in entries
                .iter()
                .filter(|entry| entry.record.status.as_str() == key)
            {
                total.count += 1;
                if let Some(cents) = entry.monthly_cents {
                    total.monthly_cents =
                        total.monthly_cents.saturating_add(cents);
                }
            }
            if total.count > 0 {
                by_status.push(total);
            }
        }
        let errors = findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .count();
        let warnings = findings
            .iter()
            .filter(|finding| finding.severity == Severity::Warning)
            .count();
        Self {
            today,
            show_all,
            teaching_empty: false,
            album_stem: registry.album.as_ref().map(|album| album.stem.clone()),
            album_path: registry.album.as_ref().map(|album| album.path.clone()),
            album_title: registry
                .album
                .as_ref()
                .map(|album| album.title.clone()),
            album_emoji: registry
                .album
                .as_ref()
                .and_then(|album| album.emoji.clone()),
            availability: registry.availability,
            entries,
            hidden_cancelled,
            monthly_cents,
            yearly_cents,
            by_status,
            findings,
            issues: IssueCounts { errors, warnings },
            config_fingerprint,
            config_error,
        }
    }

    pub(crate) fn ok(&self) -> bool {
        self.issues.errors == 0 && self.config_error.is_none()
    }

    pub(crate) fn exit_code(&self) -> i32 {
        if self.ok() {
            0
        } else {
            1
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        if self.teaching_empty {
            return json!({
                "ok": true,
                "schema_version": SCHEMA_VERSION,
                "today": self.today.format("%Y-%m-%d").to_string(),
                "teaching_empty": true,
                "all": self.show_all,
                "album": null,
                "config_fingerprint": null,
                "totals": {
                    "monthly_cents": 0,
                    "yearly_cents": 0,
                    "by_status": {},
                },
                "subscriptions": [],
                "hidden_cancelled": 0,
                "issues": {"errors": 0, "warnings": 0},
                "findings": [],
            });
        }
        json!({
            "ok": self.ok(),
            "schema_version": SCHEMA_VERSION,
            "today": self.today.format("%Y-%m-%d").to_string(),
            "teaching_empty": false,
            "all": self.show_all,
            "album": match (&self.album_stem, &self.album_path, &self.album_title) {
                (Some(stem), Some(path), Some(title)) => json!({
                    "stem": stem,
                    "path": path,
                    "title": title,
                    "emoji": self.album_emoji,
                }),
                _ => Value::Null,
            },
            "config_fingerprint": self.config_fingerprint,
            "totals": {
                "monthly_cents": self.monthly_cents,
                "yearly_cents": self.yearly_cents,
                "by_status": self.by_status.iter().map(|total| {
                    (total.key.clone(), json!({
                        "count": total.count,
                        "monthly_cents": total.monthly_cents,
                    }))
                }).collect::<serde_json::Map<_, _>>(),
            },
            "subscriptions": self.entries.iter().map(entry_json).collect::<Vec<_>>(),
            "hidden_cancelled": self.hidden_cancelled,
            "issues": {
                "errors": self.issues.errors,
                "warnings": self.issues.warnings,
            },
            "findings": self.findings.iter().map(finding_json).collect::<Vec<_>>(),
        })
    }
}

/// The readable part of `bank:` plus every located diagnostic, so an
/// invalid config reports each error on its own line instead of one
/// unlocated first message.
struct PartialConfig {
    config: Option<BankConfig>,
    error: Option<String>,
    diagnostics: Vec<ConfigDiagnostic>,
    fingerprint: Option<String>,
}

fn load_partial_config(scan: &BankScan) -> PartialConfig {
    let Some(hub) = &scan.hub else {
        return PartialConfig {
            config: None,
            error: Some("bank.md is missing".to_owned()),
            diagnostics: Vec::new(),
            fingerprint: None,
        };
    };
    let fingerprint = super::config::config_fingerprint(&hub.contents);
    match parse_bank_config(&hub.contents) {
        ConfigParse::Ok(config) => PartialConfig {
            config: Some(config),
            error: None,
            diagnostics: Vec::new(),
            fingerprint,
        },
        ConfigParse::MissingMapping => PartialConfig {
            config: None,
            error: Some("bank.md has no bank: mapping".to_owned()),
            diagnostics: Vec::new(),
            fingerprint,
        },
        ConfigParse::Invalid {
            diagnostics,
            config,
        } => {
            let first = diagnostics
                .first()
                .map(|diagnostic| {
                    format!("{}: {}", diagnostic.key_path, diagnostic.message)
                })
                .unwrap_or_else(|| "invalid bank configuration".to_owned());
            PartialConfig {
                config,
                error: Some(first),
                diagnostics,
                fingerprint,
            }
        }
    }
}

fn category_ref(
    config: Option<&BankConfig>,
    key: Option<&str>,
) -> Option<CategoryRef> {
    let key = key?;
    let entry = config?.category(key);
    Some(CategoryRef {
        key: key.to_owned(),
        emoji: entry.map(|entry| entry.emoji.clone()),
        label: entry.map(|entry| entry.label.clone()),
    })
}

fn status_rank(status: &str) -> usize {
    STATUS_ORDER
        .iter()
        .position(|key| *key == status)
        .unwrap_or(usize::MAX)
}

fn entry_json(entry: &SubscriptionEntry) -> Value {
    let record = &entry.record;
    json!({
        "stem": record.stem,
        "path": record.path,
        "title": record.title,
        "status": record.status.as_str(),
        "status_label": record.status_label,
        "cadence": record.cadence.as_str(),
        "pricing": record.pricing.as_str(),
        "amount_cents": record.amount_cents,
        "category": match &entry.category {
            Some(category) => json!({
                "key": category.key,
                "emoji": category.emoji,
                "label": category.label,
            }),
            None => Value::Null,
        },
        "match": record.needles,
        "url": record.url,
        "renews": record.renews.map(|date| date.format("%Y-%m-%d").to_string()),
        "ended": record.ended.map(|date| date.format("%Y-%m-%d").to_string()),
        "monthly_cents": entry.monthly_cents,
        "charges": {
            "count": entry.charges_count,
            "total_cents": entry.total_cents,
            "latest": entry.latest.as_ref().map(charge_json),
        },
        "next_expected": entry.next_expected.map(|date| date.format("%Y-%m-%d").to_string()),
        "price_change": entry.price_change.as_ref().map(|change| json!({
            "from_cents": change.from_cents,
            "to_cents": change.to_cents,
            "since": change.since.format("%Y-%m-%d").to_string(),
        })),
        "still_charging": entry.still_charging,
        "after_cancel": entry.after_cancel.as_ref().map(charge_json),
        "timeline": entry.timeline.iter().map(|bucket| json!({
            "start": bucket.start.format("%Y-%m-%d").to_string(),
            "due": bucket.due.format("%Y-%m-%d").to_string(),
            "state": bucket.state.as_str(),
        })).collect::<Vec<_>>(),
        "findings": entry.finding_codes,
    })
}

fn charge_json(charge: &LedgerCharge) -> Value {
    json!({
        "date": charge.date.format("%Y-%m-%d").to_string(),
        "cents": charge.cents,
        "path": charge.path,
        "line": charge.line,
        "id": charge.id,
        "journal_only": charge.journal_only,
    })
}

fn finding_json(finding: &Finding) -> Value {
    let mut value = json!({
        "code": finding.code,
        "severity": severity_str(finding.severity),
        "path": finding.path,
        "line": finding.line,
        "message": finding.message,
        "hint": finding.hint,
    });
    if let Some(key_path) = &finding.key_path {
        value["key_path"] = json!(key_path);
    }
    if let Some(proposal) = &finding.proposal {
        value["proposal"] = proposal.clone();
    }
    value
}

fn cadence_label(cadence: SubscriptionCadence) -> &'static str {
    match cadence {
        SubscriptionCadence::Monthly => "monthly",
        SubscriptionCadence::FourWeekly => "every 4 weeks",
        SubscriptionCadence::Quarterly => "quarterly",
        SubscriptionCadence::Yearly => "yearly",
    }
}

/// Spending magnitude without a sign: `$9.99`, never `+$9.99`.
fn display_money(cents: i64) -> String {
    render_amount(-cents.saturating_abs())
}

fn month_label(date: NaiveDate, today: NaiveDate) -> String {
    if date.year() == today.year() {
        date.format("%b").to_string()
    } else {
        date.format("%b %Y").to_string()
    }
}

fn day_short(date: NaiveDate) -> String {
    date.format("%b %-d").to_string()
}

fn row_flags(entry: &SubscriptionEntry, today: NaiveDate) -> Vec<String> {
    let mut flags = Vec::new();
    if entry.after_cancel.is_some() {
        flags.push("after cancel".to_owned());
    } else if entry.still_charging {
        flags.push("still charging".to_owned());
    }
    if let Some(change) = &entry.price_change {
        flags.push(format!(
            "↑ {} since {}",
            display_money(change.to_cents),
            month_label(change.since, today)
        ));
    }
    if entry.record.pricing
        == super::subscriptions::registry::SubscriptionPricing::Variable
    {
        flags.push("usage-based".to_owned());
    }
    flags
}

pub(crate) fn format_human(report: &SubscriptionsReport) -> String {
    let styler = Styler::detect();
    let mut out = String::new();
    if report.teaching_empty {
        out.push_str(&styler.bold("🔁 Subscriptions"));
        out.push('\n');
        out.push_str("No subscriptions album yet.\n");
        out.push_str("Create it with:\n");
        out.push_str(
            "  bob albums create bank subscriptions -n subscription -e 🔁 -t Subscriptions -s 'What we pay for on repeat.'\n",
        );
        out.push_str("See docs/bank.md for the subscriptions contract.\n");
        return out;
    }
    let yearly = match report.yearly_cents {
        Some(yearly) => format!("≈ {} a year", display_money(yearly)),
        None => "≈ — a year".to_owned(),
    };
    out.push_str(&styler.bold("🔁 Subscriptions"));
    out.push_str(&format!(
        "   {} a month · {}\n",
        format!("≈ {}", display_money(report.monthly_cents)),
        yearly
    ));
    out.push_str(&format!("   {}\n", counts_line(report)));

    if report.availability == SubscriptionAvailability::Invalid {
        out.push_str(
            "subscription records are unavailable; run `bob albums check`\n",
        );
    }
    if let Some(error) = &report.config_error {
        out.push_str(&format!("bank config invalid: {error}\n"));
    }

    let visible = report
        .entries
        .iter()
        .filter(|entry| entry.visible)
        .collect::<Vec<_>>();
    if visible.is_empty() {
        if report.hidden_cancelled > 0 {
            out.push_str(&format!(
                "No live subscriptions ({} cancelled hidden; rerun with -a).\n",
                report.hidden_cancelled
            ));
        } else {
            out.push_str("No subscription records yet.\n");
        }
        return out;
    }

    for status in STATUS_ORDER {
        let group = visible
            .iter()
            .filter(|entry| entry.record.status.as_str() == status)
            .copied()
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        let label = group
            .first()
            .map(|entry| entry.record.status_label.as_str())
            .unwrap_or(status);
        out.push('\n');
        out.push_str(&styler.bold(label));
        out.push('\n');
        if status == "active" && group.len() > ACTIVE_SPLIT_AT {
            print_category_subgroups(&mut out, &group, report.today, 4);
        } else {
            print_entries(&mut out, &group, report.today, 4);
        }
    }
    if report.hidden_cancelled > 0 && !report.show_all {
        out.push_str(&format!(
            "\n{} cancelled hidden; rerun with -a to show them.\n",
            report.hidden_cancelled
        ));
    }
    out
}

fn counts_line(report: &SubscriptionsReport) -> String {
    let mut parts = Vec::new();
    for total in &report.by_status {
        if total.count == 0 {
            continue;
        }
        match total.key.as_str() {
            "to_cancel" => parts.push(format!("{} to cancel", total.count)),
            "to_decide" => parts.push(format!("{} to decide", total.count)),
            _ => {}
        }
    }
    if report.findings.is_empty() {
        parts.push("no findings".to_owned());
    } else {
        let count = report.findings.len();
        parts.push(if count == 1 {
            "1 finding".to_owned()
        } else {
            format!("{count} findings")
        });
    }
    format!("{} — bob bank check", parts.join(" · "))
}

fn category_subgroup_key(entry: &SubscriptionEntry) -> (String, String) {
    match &entry.category {
        Some(category) => (
            category
                .label
                .clone()
                .unwrap_or_else(|| category.key.clone()),
            category.emoji.clone().unwrap_or_default(),
        ),
        None => ("No category".to_owned(), String::new()),
    }
}

fn print_category_subgroups(
    out: &mut String,
    group: &[&SubscriptionEntry],
    today: NaiveDate,
    indent: usize,
) {
    let mut order = Vec::<(String, String)>::new();
    for entry in group {
        let key = category_subgroup_key(entry);
        if !order.contains(&key) {
            order.push(key);
        }
    }
    for (label, emoji) in order {
        let heading = if emoji.is_empty() {
            label.clone()
        } else {
            format!("{emoji} {label}")
        };
        out.push_str(&format!("{}{}\n", " ".repeat(indent), heading));
        let members = group
            .iter()
            .filter(|entry| {
                category_subgroup_key(entry) == (label.clone(), emoji.clone())
            })
            .copied()
            .collect::<Vec<_>>();
        print_entries(out, &members, today, indent + 4);
    }
}

fn print_entries(
    out: &mut String,
    group: &[&SubscriptionEntry],
    today: NaiveDate,
    indent: usize,
) {
    struct Row {
        title: String,
        amount: String,
        cadence: String,
        last: String,
        next: String,
        flags: String,
    }
    let rows = group
        .iter()
        .map(|entry| {
            let last = entry
                .latest
                .as_ref()
                .map(|charge| format!("last {}", day_short(charge.date)))
                .unwrap_or_else(|| "last —".to_owned());
            let next = entry
                .next_expected
                .map(|date| format!("next ≈ {}", day_short(date)))
                .unwrap_or_else(|| "next —".to_owned());
            Row {
                title: entry.record.title.clone(),
                amount: display_money(entry.record.amount_cents),
                cadence: cadence_label(entry.record.cadence).to_owned(),
                last,
                next,
                flags: row_flags(entry, today).join(" · "),
            }
        })
        .collect::<Vec<_>>();
    let width = |pick: fn(&Row) -> &str| {
        rows.iter()
            .map(|row| display_width(pick(row)))
            .max()
            .unwrap_or(0)
    };
    let title_w = width(|row| &row.title);
    let amount_w = width(|row| &row.amount);
    let cadence_w = width(|row| &row.cadence);
    let last_w = width(|row| &row.last);
    let next_w = width(|row| &row.next);
    let pad = " ".repeat(indent);
    for row in &rows {
        let mut line = format!(
            "{pad}{}  {}  {}  {}  {}",
            pad_right(&row.title, title_w),
            pad_left(&row.amount, amount_w),
            pad_right(&row.cadence, cadence_w),
            pad_right(&row.last, last_w),
            pad_right(&row.next, next_w),
        );
        if !row.flags.is_empty() {
            line.push_str(&format!("  {}", row.flags));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
}

fn pad_left(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{}{text}", " ".repeat(padding))
}

pub(crate) fn print_human(report: &SubscriptionsReport) {
    print!("{}", format_human(report));
}

fn severity_str(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::native::bank::{
        config::{parse_bank_config, ConfigParse},
        subscriptions::{
            ledger::{LedgerCharge, RecordLedger},
            registry::{
                InvalidNeedle, SubscriptionAvailability, SubscriptionCadence,
                SubscriptionPricing, SubscriptionStatus,
            },
        },
    };

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 15).expect("date")
    }

    fn date(month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, month, day).expect("date")
    }

    fn config() -> BankConfig {
        let body = "---\nbank:\n  accounts:\n    checking:\n      kind: checking\n      start: 2026-09-01\n  categories:\n    streaming: 📡 Streaming\n    gaming: 🎮 Gaming\n---\n";
        match parse_bank_config(body) {
            ConfigParse::Ok(config) => config,
            other => panic!("expected ok, got {other:?}"),
        }
    }

    fn record(
        stem: &str,
        status: SubscriptionStatus,
        amount_cents: i64,
        cadence: SubscriptionCadence,
    ) -> SubscriptionRecord {
        let label = match status {
            SubscriptionStatus::ToCancel => "✂️ To cancel",
            SubscriptionStatus::ToDecide => "🤔 To decide",
            SubscriptionStatus::Active => "💚 Active",
            SubscriptionStatus::Cancelled => "🚫 Cancelled",
        };
        SubscriptionRecord {
            stem: stem.to_owned(),
            path: format!("subscriptions/{stem}.md"),
            link_target: format!("subscriptions/{stem}"),
            title: format!("{stem} title"),
            status,
            status_label: label.to_owned(),
            amount_cents,
            cadence,
            pricing: SubscriptionPricing::Fixed,
            needles: vec![stem.to_ascii_uppercase()],
            invalid_needles: Vec::<InvalidNeedle>::new(),
            category: Some("streaming".to_owned()),
            url: None,
            renews: None,
            ended: None,
        }
    }

    fn charge(day: u32, cents: i64) -> LedgerCharge {
        LedgerCharge {
            date: date(11, day),
            cents,
            path: Some("bank/2026-11.md".to_owned()),
            line: Some(day as usize),
            id: None,
            account: "checking".to_owned(),
            journal_only: true,
        }
    }

    fn ledger_for(
        record: &SubscriptionRecord,
        charges: Vec<LedgerCharge>,
        monthly_cents: i64,
    ) -> RecordLedger {
        let latest = charges.last().cloned();
        RecordLedger {
            record_path: record.path.clone(),
            charges,
            refunds: Vec::new(),
            gross_cents: None,
            refund_cents: None,
            net_cents: Some(999),
            latest,
            next_expected: Some(date(12, 3)),
            monthly_cents: Some(monthly_cents),
            price_change: None,
            still_charging: false,
            after_cancel: None,
            after_cancel_charges: Vec::new(),
            timeline: Vec::new(),
        }
    }

    fn registry(records: Vec<SubscriptionRecord>) -> SubscriptionRegistry {
        SubscriptionRegistry {
            availability: SubscriptionAvailability::Available,
            album: Some(
                super::super::subscriptions::registry::SubscriptionAlbum {
                    stem: "subscriptions".to_owned(),
                    path: "subscriptions.md".to_owned(),
                    title: "Subscriptions".to_owned(),
                    directory: "subscriptions".to_owned(),
                    emoji: Some("🔁".to_owned()),
                },
            ),
            records,
            diagnostics: Vec::new(),
        }
    }

    fn snapshot(ledgers: Vec<RecordLedger>) -> LedgerSnapshot {
        LedgerSnapshot {
            records: ledgers
                .into_iter()
                .map(|ledger| (ledger.record_path.clone(), ledger))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        }
    }

    fn build(
        records: Vec<SubscriptionRecord>,
        ledgers: Vec<RecordLedger>,
        show_all: bool,
    ) -> SubscriptionsReport {
        let registry = registry(records);
        let ledger = snapshot(ledgers);
        let config = config();
        SubscriptionsReport::from_parts(
            today(),
            show_all,
            &registry,
            &ledger,
            Vec::new(),
            Some(&config),
            Some("sha256:abc".to_owned()),
            None,
        )
    }

    #[test]
    fn groups_follow_action_order_and_hide_quiet_cancelled() {
        let active = record(
            "tunely",
            SubscriptionStatus::Active,
            999,
            SubscriptionCadence::Monthly,
        );
        let to_decide = record(
            "cloudbox",
            SubscriptionStatus::ToDecide,
            1999,
            SubscriptionCadence::Monthly,
        );
        let to_cancel = record(
            "ledger",
            SubscriptionStatus::ToCancel,
            600,
            SubscriptionCadence::FourWeekly,
        );
        let cancelled = record(
            "oldbox",
            SubscriptionStatus::Cancelled,
            500,
            SubscriptionCadence::Monthly,
        );
        let ledgers = [&active, &to_decide, &to_cancel, &cancelled]
            .into_iter()
            .map(|record| {
                ledger_for(
                    record,
                    vec![charge(3, -record.amount_cents)],
                    record.amount_cents,
                )
            })
            .collect::<Vec<_>>();
        let report = build(
            vec![active, to_decide, to_cancel, cancelled],
            ledgers,
            false,
        );
        let order = report
            .entries
            .iter()
            .map(|entry| entry.record.status.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            vec!["to_cancel", "to_decide", "active", "cancelled"]
        );
        assert!(report.entries[3].record.status.is_cancelled());
        assert!(!report.entries[3].visible);
        assert_eq!(report.hidden_cancelled, 1);

        let human = format_human(&report);
        assert!(human.contains("🔁 Subscriptions"));
        let cancel_at = human.find("✂️ To cancel").expect("cancel group");
        let decide_at = human.find("🤔 To decide").expect("decide group");
        let active_at = human.find("💚 Active").expect("active group");
        assert!(cancel_at < decide_at && decide_at < active_at);
        assert!(!human.contains("oldbox title"));
        assert!(human.contains("1 cancelled hidden; rerun with -a"));
        assert!(human.contains(
            "1 to cancel · 1 to decide · no findings — bob bank check"
        ));
    }

    #[test]
    fn cancelled_with_a_finding_stays_visible() {
        let cancelled = record(
            "oldbox",
            SubscriptionStatus::Cancelled,
            500,
            SubscriptionCadence::Monthly,
        );
        let registry = registry(vec![cancelled.clone()]);
        let ledger =
            snapshot(vec![ledger_for(&cancelled, vec![charge(3, -500)], 0)]);
        let config = config();
        let finding = Finding {
            code: "bank_subscription_cancelled_charge",
            severity: Severity::Warning,
            path: cancelled.path.clone(),
            line: None,
            message: "charged after cancel".to_owned(),
            hint: None,
            day: None,
            key_path: None,
            proposal: None,
        };
        let report = SubscriptionsReport::from_parts(
            today(),
            false,
            &registry,
            &ledger,
            vec![finding],
            Some(&config),
            None,
            None,
        );
        assert!(report.entries[0].visible);
        assert_eq!(
            report.entries[0].finding_codes,
            vec!["bank_subscription_cancelled_charge"]
        );
        assert_eq!(report.hidden_cancelled, 0);
        let human = format_human(&report);
        assert!(human.contains("🚫 Cancelled"));
        assert!(human.contains("1 finding"));
    }

    #[test]
    fn totals_sum_monthly_equivalents_by_status() {
        let monthly = record(
            "tunely",
            SubscriptionStatus::Active,
            999,
            SubscriptionCadence::Monthly,
        );
        let four_weekly = record(
            "ledger",
            SubscriptionStatus::Active,
            600,
            SubscriptionCadence::FourWeekly,
        );
        let yearly = record(
            "craft",
            SubscriptionStatus::Active,
            12_000,
            SubscriptionCadence::Yearly,
        );
        let cancelled = record(
            "oldbox",
            SubscriptionStatus::Cancelled,
            50_000,
            SubscriptionCadence::Monthly,
        );
        // 600 every 4 weeks is 650 a month with half-up rounding.
        let ledgers = [&monthly, &four_weekly, &yearly, &cancelled]
            .into_iter()
            .map(|record| {
                let monthly_cents = match record.cadence {
                    SubscriptionCadence::Monthly => record.amount_cents,
                    SubscriptionCadence::FourWeekly => 650,
                    SubscriptionCadence::Yearly => 1000,
                    SubscriptionCadence::Quarterly => 0,
                };
                let monthly_cents = if record.status.is_cancelled() {
                    0
                } else {
                    monthly_cents
                };
                ledger_for(
                    record,
                    vec![charge(3, -record.amount_cents)],
                    monthly_cents,
                )
            })
            .collect::<Vec<_>>();
        let report =
            build(vec![monthly, four_weekly, yearly, cancelled], ledgers, true);
        assert_eq!(report.monthly_cents, 999 + 650 + 1000);
        assert_eq!(report.yearly_cents, Some((999 + 650 + 1000) * 12));
        let active = report
            .by_status
            .iter()
            .find(|total| total.key == "active")
            .expect("active total");
        assert_eq!(active.count, 3);
        assert_eq!(active.monthly_cents, 999 + 650 + 1000);

        let json = report.to_json();
        assert_eq!(json["totals"]["monthly_cents"], 2649);
        assert_eq!(json["totals"]["yearly_cents"], 31788);
        assert_eq!(json["totals"]["by_status"]["active"]["count"], 3);
        assert_eq!(json["subscriptions"].as_array().expect("array").len(), 4);
    }

    #[test]
    fn json_entry_carries_the_plugin_contract() {
        let mut active = record(
            "tunely",
            SubscriptionStatus::Active,
            999,
            SubscriptionCadence::Monthly,
        );
        active.url = Some("https://example.com/manage".to_owned());
        let mut ledger = ledger_for(&active, vec![charge(3, -999)], 999);
        ledger.net_cents = Some(999);
        let report = build(vec![active], vec![ledger], false);
        let entry = &report.to_json()["subscriptions"][0];
        assert_eq!(entry["stem"], "tunely");
        assert_eq!(entry["path"], "subscriptions/tunely.md");
        assert_eq!(entry["status"], "active");
        assert_eq!(entry["status_label"], "💚 Active");
        assert_eq!(entry["cadence"], "monthly");
        assert_eq!(entry["pricing"], "fixed");
        assert_eq!(entry["amount_cents"], 999);
        assert_eq!(entry["category"]["key"], "streaming");
        assert_eq!(entry["category"]["emoji"], "📡");
        assert_eq!(entry["category"]["label"], "Streaming");
        assert_eq!(entry["match"], serde_json::json!(["TUNELY"]));
        assert_eq!(entry["url"], "https://example.com/manage");
        assert_eq!(entry["monthly_cents"], 999);
        assert_eq!(entry["charges"]["count"], 1);
        assert_eq!(entry["charges"]["total_cents"], 999);
        assert_eq!(entry["charges"]["latest"]["date"], "2026-11-03");
        assert_eq!(entry["next_expected"], "2026-12-03");
        assert_eq!(entry["still_charging"], false);
        let envelope = report.to_json();
        assert_eq!(envelope["album"]["emoji"], "🔁");
        assert_eq!(envelope["ok"], true);
        assert_eq!(report.to_json()["schema_version"], 1);
    }

    #[test]
    fn human_row_pins_tabular_money_and_flags() {
        let mut to_cancel = record(
            "ledger",
            SubscriptionStatus::ToCancel,
            600,
            SubscriptionCadence::FourWeekly,
        );
        to_cancel.title = "Daily Ledger".to_owned();
        let mut ledger = ledger_for(&to_cancel, vec![charge(10, -600)], 650);
        ledger.still_charging = true;
        ledger.latest = Some(charge(10, -600));
        ledger.next_expected = Some(date(12, 8));
        let report = build(vec![to_cancel], vec![ledger], false);
        let human = format_human(&report);
        let line = human
            .lines()
            .find(|line| line.contains("Daily Ledger"))
            .expect("row");
        assert!(line.contains("$6.00"), "amount: {line}");
        assert!(line.contains("every 4 weeks"), "cadence: {line}");
        assert!(line.contains("last Nov 10"), "last: {line}");
        assert!(line.contains("next ≈ Dec 8"), "next: {line}");
        assert!(line.contains("still charging"), "flag: {line}");
        assert!(human.contains("≈ $6.50 a month"), "header: {human}");
    }

    #[test]
    fn teaching_empty_reports_the_create_command() {
        let report = SubscriptionsReport::teaching_empty(today(), false);
        assert_eq!(report.exit_code(), 0);
        let human = format_human(&report);
        assert!(human.contains("No subscriptions album yet."));
        assert!(human.contains("bob albums create bank subscriptions"));
        let json = report.to_json();
        assert_eq!(json["teaching_empty"], true);
        assert_eq!(json["album"], serde_json::Value::Null);
        assert_eq!(json["subscriptions"], serde_json::json!([]));
        assert_eq!(json["ok"], true);
    }

    #[test]
    fn active_group_splits_by_category_when_large() {
        let mut records = Vec::new();
        let mut ledgers = Vec::new();
        for index in 0..9 {
            let stem = format!("service{index:02}");
            let mut entry = record(
                &stem,
                SubscriptionStatus::Active,
                100,
                SubscriptionCadence::Monthly,
            );
            entry.title = format!("Service {index:02}");
            if index % 3 == 0 {
                entry.category = Some("gaming".to_owned());
            }
            ledgers.push(ledger_for(&entry, vec![charge(3, -100)], 100));
            records.push(entry);
        }
        let report = build(records, ledgers, false);
        let human = format_human(&report);
        assert!(human.contains("🎮 Gaming"), "split:\n{human}");
        assert!(human.contains("📡 Streaming"), "split:\n{human}");
    }
}
