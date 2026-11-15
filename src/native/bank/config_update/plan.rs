//! Plan a config update against the current vault snapshot.

use chrono::NaiveDate;

use super::ops::{self, InverseOp, OpError, OpResult, UsageIndex};
use super::protocol::Envelope;
use crate::native::bank::config::{self, ConfigParse};
use crate::native::bank::config_report::ConfigReport;
use crate::native::bank::journal::{self, JournalSnapshot};
use crate::native::bank::scan::{self, BankScan};

#[derive(Debug, Clone)]
pub(crate) struct HotkeyShift {
    pub(crate) key: String,
    pub(crate) from: Option<char>,
    pub(crate) to: Option<char>,
}

#[derive(Debug)]
pub(crate) struct UpdatePlan {
    pub(crate) before: String,
    pub(crate) after: String,
    pub(crate) changed: bool,
    pub(crate) before_fingerprint: String,
    pub(crate) after_fingerprint: String,
    pub(crate) consequence: String,
    pub(crate) per_op: Vec<OpResult>,
    pub(crate) inverse_ops: Vec<InverseOp>,
    pub(crate) shifted: Vec<HotkeyShift>,
    pub(crate) free: Vec<char>,
    pub(crate) categories: serde_json::Value,
}

pub(crate) fn load_scan(vault: &std::path::Path) -> Result<BankScan, OpError> {
    let scan = scan::scan_bank(vault);
    if let Some(first) = scan.io_errors.first() {
        return Err(OpError::new(
            "io",
            format!("the vault scan was incomplete: {first}"),
            Some(
                "fix the unreadable paths, then retry; nothing was written"
                    .to_owned(),
            ),
        ));
    }
    Ok(scan)
}

pub(crate) fn plan_update(
    scan: &BankScan,
    journal: &JournalSnapshot,
    envelope: &Envelope,
    today: NaiveDate,
) -> Result<UpdatePlan, OpError> {
    let hub = scan.hub.as_ref().ok_or_else(|| {
        OpError::new(
            "invalid_config",
            "fix bank: in YAML first",
            Some("bank.md is missing".to_owned()),
        )
    })?;
    let before = hub.contents.clone();
    let parsed = config::parse_bank_config(&before);
    let config = match parsed {
        ConfigParse::Ok(config) => config,
        ConfigParse::MissingMapping | ConfigParse::Invalid { .. } => {
            return Err(OpError::new(
                "invalid_config",
                "fix bank: in YAML first",
                Some(
                    "bob never edits a config it cannot fully parse".to_owned(),
                ),
            ));
        }
    };
    let before_fingerprint =
        config::config_fingerprint(&before).ok_or_else(|| {
            OpError::new(
                "invalid_config",
                "fix bank: in YAML first",
                Some("bank: mapping has no fingerprint".to_owned()),
            )
        })?;
    if before_fingerprint != envelope.expect {
        let mut error = OpError::new(
            "stale_config",
            format!(
                "config fingerprint changed: expected {}, current {before_fingerprint}",
                envelope.expect
            ),
            Some("refresh the setup report and preview the operation again".to_owned()),
        );
        error.key_path = Some("bank".to_owned());
        return Err(error);
    }
    let usage = UsageIndex::from_scan(scan, &config);
    let (after, per_op, inverse_ops) = ops::apply_ops(
        &before,
        &envelope.ops,
        &envelope.restorations,
        &usage,
        0,
    )?;
    let after_parsed = config::parse_bank_config(&after);
    let ConfigParse::Ok(_) = after_parsed else {
        return Err(OpError::new(
            "post_image_mismatch",
            "planned bank: bytes do not re-parse to a valid configuration",
            Some("nothing was written".to_owned()),
        ));
    };
    let after_fingerprint =
        config::config_fingerprint(&after).ok_or_else(|| {
            OpError::new(
                "post_image_mismatch",
                "planned bank: bytes have no fingerprint",
                Some("nothing was written".to_owned()),
            )
        })?;
    let planned_config = ops::parse_live(&after)?;
    let before_keys = hotkey_map(&config);
    let after_keys = hotkey_map(&planned_config);
    let mut shifted = Vec::new();
    let mut names = before_keys.keys().cloned().collect::<Vec<_>>();
    for key in after_keys.keys() {
        if !names.iter().any(|name| name == key) {
            names.push(key.clone());
        }
    }
    names.sort();
    for key in names {
        let from = before_keys.get(&key).copied().flatten();
        let to = after_keys.get(&key).copied().flatten();
        if from != to {
            shifted.push(HotkeyShift { key, from, to });
        }
    }
    let claimed: std::collections::HashSet<char> = planned_config
        .categories
        .iter()
        .filter_map(|category| category.hotkey)
        .collect();
    let free = config::HOTKEY_ORDER
        .chars()
        .filter(|ch| *ch != config::RESERVED_HOTKEY && !claimed.contains(ch))
        .collect();
    let mut scan_after = scan.clone();
    if let Some(hub) = scan_after.hub.as_mut() {
        hub.contents = after.clone();
    }
    let report = ConfigReport::build(&scan_after, journal, today);
    let categories = report.to_json()["categories"].clone();
    let mut consequence = per_op
        .iter()
        .map(|result| result.summary.clone())
        .collect::<Vec<_>>()
        .join("; ");
    if consequence.is_empty() {
        consequence = if after == before {
            "no changes".to_owned()
        } else {
            "updates bank:".to_owned()
        };
    }
    if !shifted.is_empty() && !consequence.contains("keys shift") {
        let shifts = shifted
            .iter()
            .filter_map(|shift| {
                let from = shift.from?;
                let to = shift.to?;
                Some(format!(
                    "{} {}→{}",
                    shift.key,
                    from.to_ascii_uppercase(),
                    to.to_ascii_uppercase()
                ))
            })
            .collect::<Vec<_>>();
        if !shifts.is_empty() {
            consequence =
                format!("{consequence} · keys shift: {}", shifts.join(", "));
        }
    }
    Ok(UpdatePlan {
        before,
        after: after.clone(),
        changed: after != hub.contents,
        before_fingerprint,
        after_fingerprint,
        consequence,
        per_op,
        inverse_ops,
        shifted,
        free,
        categories,
    })
}

fn hotkey_map(
    config: &crate::native::bank::config::BankConfig,
) -> std::collections::BTreeMap<String, Option<char>> {
    config
        .categories
        .iter()
        .map(|category| (category.key.clone(), category.hotkey))
        .collect()
}

pub(crate) fn load_journal(vault: &std::path::Path) -> JournalSnapshot {
    journal::load(vault).unwrap_or_default()
}

pub(crate) fn commit_subject(plan: &UpdatePlan) -> String {
    if plan.per_op.len() > 1 {
        return format!("bank: config ({} edits)", plan.per_op.len());
    }
    match plan.per_op.first() {
        Some(result) if result.op_name == "category.add" => {
            format!(
                "bank: config (+{})",
                result.key.as_deref().unwrap_or("category")
            )
        }
        Some(result) if result.op_name == "category.remove" => {
            format!(
                "bank: config (-{})",
                result.key.as_deref().unwrap_or("category")
            )
        }
        Some(result) if result.op_name == "category.reset" => {
            format!(
                "bank: config (reset {})",
                result.key.as_deref().unwrap_or("category")
            )
        }
        Some(result) if result.op_name == "category.move" => {
            format!(
                "bank: config (move {})",
                result.key.as_deref().unwrap_or("category")
            )
        }
        Some(result) if result.op_name == "group.rename" => {
            "bank: config (rename group)".to_owned()
        }
        Some(result) if result.op_name == "group.move" => {
            "bank: config (move group)".to_owned()
        }
        Some(result) => format!(
            "bank: config ({})",
            result.key.as_deref().unwrap_or("edit")
        ),
        None => "bank: config".to_owned(),
    }
}
