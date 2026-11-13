//! `bob bank ensure` — create the month note and day heading.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    config::BankConfig,
    dates::resolve_day,
    month::{canonical_template, now_created_stamp, parse_month_note},
};

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct EnsurePlan {
    pub(crate) day: NaiveDate,
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) created_note: bool,
    pub(crate) created_heading: bool,
    pub(crate) updated_contents: Option<String>,
    pub(crate) target: PathBuf,
    pub(crate) existed: bool,
    pub(crate) original: String,
}

pub(crate) fn plan(
    bob_dir: &Path,
    day: NaiveDate,
    hub_contents: &str,
    config: &BankConfig,
) -> Result<EnsurePlan, String> {
    let _ = (hub_contents, config);
    let month = super::model::month_of(day);
    let path = super::model::month_path(month);
    let target = bob_dir.join(&path);
    let existing = fs::read_to_string(&target).ok();
    let original = existing.clone().unwrap_or_default();
    let existed = existing.is_some();
    let (mut contents, created_note) = match existing {
        Some(text) => (text, false),
        None => (canonical_template(month, &now_created_stamp(), day), true),
    };
    let mut created_heading = false;
    let note = parse_month_note(&path, &contents, month);
    if !note.days.iter().any(|section| section.date == day) {
        let Some(edit) = super::month::plan_insert_day(&contents, month, day)
        else {
            return Err("could not plan the day heading".to_owned());
        };
        contents = edit.apply(&contents);
        created_heading = true;
    }
    let note = parse_month_note(&path, &contents, month);
    let line = match note.days.iter().find(|section| section.date == day) {
        Some(section) => super::month::next_row_line(&contents, section),
        None => 1,
    };
    let updated = if created_note || created_heading {
        Some(contents.clone())
    } else {
        None
    };
    Ok(EnsurePlan {
        day,
        path,
        line,
        created_note,
        created_heading,
        updated_contents: updated,
        target,
        existed,
        original,
    })
}

pub(crate) fn resolve_ensure_day(
    raw: Option<&str>,
    today: NaiveDate,
) -> Result<NaiveDate, String> {
    match raw {
        None => Ok(today),
        Some(value) => resolve_day(value, today)
            .map_err(|message| format!("invalid day {value:?}: {message}")),
    }
}

pub(crate) fn to_json(
    plan: &EnsurePlan,
    today: NaiveDate,
    categories: &[super::config::Category],
    accounts: &[super::config::AccountStatus],
) -> Value {
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "today": today.format("%Y-%m-%d").to_string(),
        "categories": categories.iter().map(super::config::Category::to_json).collect::<Vec<_>>(),
        "accounts": accounts.iter().map(|account| account.to_json()).collect::<Vec<_>>(),
        "issues": {"errors": 0, "warnings": 0},
        "path": plan.path,
        "line": plan.line,
        "day": plan.day.format("%Y-%m-%d").to_string(),
        "created_note": plan.created_note,
        "created_heading": plan.created_heading,
    })
}

pub(crate) fn open_in_obsidian(
    vault: &Path,
    relative: &Path,
) -> Result<(), String> {
    let vault_name = vault
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!("cannot determine vault name from {}", vault.display())
        })?;
    let vault_q: String =
        url::form_urlencoded::byte_serialize(vault_name.as_bytes()).collect();
    let file_q: String = url::form_urlencoded::byte_serialize(
        relative.to_string_lossy().as_bytes(),
    )
    .collect();
    let uri = format!("obsidian://open?vault={vault_q}&file={file_q}");
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(uri)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!("could not open the note in Obsidian: {error}")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::bank::config::BankConfig;

    #[test]
    fn ensure_places_missing_day_after_the_complete_prior_section() {
        let temp = tempfile::tempdir().expect("temporary vault");
        fs::create_dir_all(temp.path().join("bank")).expect("bank directory");
        let original = concat!(
            "---\nparent: \"[[bank]]\"\n---\n\n# November 2026\n\n",
            "## Wed Nov 11\n\n- [x] $5.00 Prior purchase\n",
            "  - original child\nKeep this prose.\n\n",
            "An isolated note.\n- [ ] call the bank\n- [ ] $\n\n",
            "## Sat Nov 14\n\n- [ ] $8.00 Later day\n",
        );
        fs::write(temp.path().join("bank/2026-11.md"), original)
            .expect("month note");
        let config = BankConfig {
            accounts: Vec::new(),
            categories: Vec::new(),
            rules: Vec::new(),
            stale_after: 10,
        };
        let date = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        let plan = plan(temp.path(), date, "", &config).expect("ensure plan");
        let updated = plan.updated_contents.expect("heading insertion");
        let inserted = updated.find("## Thu Nov 12\n").unwrap();
        for content in [
            "  - original child",
            "Keep this prose.",
            "An isolated note.",
            "- [ ] call the bank",
            "- [ ] $",
        ] {
            assert!(updated.find(content).unwrap() < inserted, "{content}");
        }
        assert!(inserted < updated.find("## Sat Nov 14\n").unwrap());
    }
}
