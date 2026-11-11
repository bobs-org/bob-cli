//! Golden findings, teaching empty state, and task invisibility.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::{
    check::{lint, CheckReport},
    month::canonical_template,
    scan::scan_bank,
};
use crate::native::{env::TestEnvGuard, note_tasks};

fn month() -> super::grammar::YearMonth {
    super::grammar::YearMonth {
        year: 2026,
        month: 11,
    }
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 12).unwrap()
}

fn guard() -> TestEnvGuard {
    TestEnvGuard::set(&[
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent-bob-cli-test-config.yml")),
        ),
        ("BOB_NOW", Some(OsStr::new("2026-11-12T12:00:00"))),
    ])
}

fn temp_vault(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bob-bank-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("vault");
    root
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, body).expect("write");
}

fn valid_hub() -> &'static str {
    "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-12\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n---\n\n# 🏦 Bank\n"
}

fn report_for(root: &Path) -> CheckReport {
    let scan = scan_bank(root);
    lint(&scan, today())
}

fn codes(report: &CheckReport) -> BTreeSet<&str> {
    report.findings.iter().map(|finding| finding.code).collect()
}

fn has_code(report: &CheckReport, code: &str) -> bool {
    report.findings.iter().any(|finding| finding.code == code)
}

#[test]
fn teaching_empty_state_is_ok() {
    let _guard = guard();
    let root = temp_vault("empty");
    let report = report_for(&root);
    assert!(report.teaching_empty);
    assert!(report.ok());
    assert_eq!(report.exit_code(), 0);
    let json = report.to_json();
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["today"], "2026-11-12");
    assert_eq!(json["findings"].as_array().unwrap().len(), 0);
}

#[test]
fn hub_missing_when_bank_dir_exists() {
    let _guard = guard();
    let root = temp_vault("dir-only");
    fs::create_dir_all(root.join("bank")).unwrap();
    let report = report_for(&root);
    assert!(!report.teaching_empty);
    assert!(has_code(&report, "bank_hub_missing"));
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn hub_missing_when_bank_mapping_absent() {
    let _guard = guard();
    let root = temp_vault("no-mapping");
    write(
        root.as_path(),
        "bank.md",
        "---\nparent: \"[[cash]]\"\n---\n",
    );
    let report = report_for(&root);
    assert!(has_code(&report, "bank_hub_missing"));
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn config_invalid_names_key_path() {
    let _guard = guard();
    let root = temp_vault("bad-kind");
    write(
        root.as_path(),
        "bank.md",
        "---\nbank:\n  accounts:\n    checking:\n      kind: debit\n      start: 2026-11-12\n---\n",
    );
    let report = report_for(&root);
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.code == "bank_config_invalid")
        .unwrap();
    assert_eq!(finding.line, Some(5));
    assert!(finding.message.contains("bank.accounts.checking.kind"));
}

#[test]
fn grammar_findings_cover_the_table() {
    let _guard = guard();
    let root = temp_vault("findings");
    write(root.as_path(), "bank.md", valid_hub());
    write(
        root.as_path(),
        "bank/2026-11.md",
        "---\nparent: \"[[bank]]\"\ntype: \"[[area]]\"\nmonth: 2026-10\n---\n\n# 🏦 November 2026\n\n## Notes\n\n- [ ] $12.00 Outside [cat:: groceries]\n\n## Thu Nov 12\n\n- [ ] $61.20 ShopRite [cat:: groceries]\n- [ ] $1.234 BadDecimals Payee\n- [ ] $5.00\n- [>] $3.00 WeirdStatus Foo\n- [ ] $4.00 Tagged #task [cat:: groceries]\n- $9.00 NoBox Payee\n- [ ] $0.00 ZeroRow [cat:: groceries]\n- [ ] $8.00 Extra [cta:: oops] [cat:: groceries]\n- [x] $13.71 Smash ⚑ look [cat:: dining]\n- [x] +$20.00 RefundDining [cat:: dining]\n- [ ] $7.00 UnknownCat [cat:: snacks]\n- [ ] $7.00 UnknownAcct [acct:: card] [cat:: groceries]\n- [x] $1.00 DupA [cat:: groceries] ^bank-aaaa\n- [x] $1.00 DupB [cat:: groceries] ^bank-aaaa\n- [x] $\n\n## Fri Nov 13\n\n## Thu Nov 12\n\n## Mon Nov 10\n\n## 2026-11-12 Wed\n\n## Thu Oct 12\n\n## Thu Nov 31\n",
    );
    let report = report_for(&root);
    let found = codes(&report);
    for code in [
        "bank_month_mismatch",
        "bank_day_invalid",
        "bank_row_unparsed",
        "bank_row_payee_missing",
        "bank_row_status",
        "bank_row_task",
        "bank_row_outside_day",
        "bank_category_unknown",
        "bank_account_unknown",
        "bank_id_duplicate",
        "bank_day_duplicate",
        "bank_day_order",
        "bank_day_weekday",
        "bank_row_no_checkbox",
        "bank_row_zero",
        "bank_field_unknown",
        "bank_flagged",
        "bank_inflow_category",
    ] {
        assert!(found.contains(code), "missing {code} in {found:?}");
    }
    assert!(!found.contains("bank_hub_missing"));
    assert!(!report.findings.iter().any(|finding| {
        finding.message.contains("empty stub")
            || finding.message.contains("- [x] $")
    }));
    assert_eq!(report.exit_code(), 1);
    let json = report.to_json();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], false);
    assert!(json["issues"]["errors"].as_u64().unwrap() > 0);
    assert_eq!(json["accounts"][0]["status"], "unverified");
    assert_eq!(json["accounts"][0]["kind"], "checking");
}

#[test]
fn clean_month_is_ok() {
    let _guard = guard();
    let root = temp_vault("clean");
    write(root.as_path(), "bank.md", valid_hub());
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("- [x] $13.71 Smashburger [cat:: dining]\n");
    body.push_str("- [ ] $61.20 ShopRite [cat:: groceries]\n");
    write(root.as_path(), "bank/2026-11.md", &body);
    let report = report_for(&root);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn flagged_info_does_not_fail_check() {
    let _guard = guard();
    let root = temp_vault("flag");
    write(root.as_path(), "bank.md", valid_hub());
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("- [x] $13.71 Smashburger [cat:: dining] ⚑ look\n");
    write(root.as_path(), "bank/2026-11.md", &body);
    let report = report_for(&root);
    assert!(has_code(&report, "bank_flagged"));
    assert_eq!(report.issues.errors, 0);
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn bank_rows_are_invisible_to_task_scanners() {
    let _guard = guard();
    let root = temp_vault("invisible");
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("- [x] $13.71 Smashburger [cat:: dining]\n");
    body.push_str("- [ ] $61.20 ShopRite [cat:: groceries]\n");
    body.push_str("- [ ] #task Keep going ^keep\n");
    write(root.as_path(), "bank/2026-11.md", &body);
    let settings = note_tasks::read_settings(&root);
    assert_eq!(settings.global_filter, "#task");
    let scan = note_tasks::scan(&body, &settings);
    // plan, ready, task archive, and task reconcile all scan tasks
    // through note_tasks with the #task global filter. Bank rows omit
    // #task, so they never become tasks. Archive dry-run is `-n`.
    let descriptions: Vec<&str> = scan
        .tasks()
        .iter()
        .map(|task| task.description.as_str())
        .collect();
    assert_eq!(descriptions, vec!["Keep going"]);
    assert!(scan.tasks().iter().all(|task| {
        !task.description.contains("Smashburger")
            && !task.description.contains("ShopRite")
    }));
}
