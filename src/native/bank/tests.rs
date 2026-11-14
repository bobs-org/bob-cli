//! Golden findings, teaching empty state, and task invisibility.

use std::{
    collections::{BTreeSet, HashMap},
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

#[test]
fn import_command_help_and_option_aliases_are_complete() {
    let mut command = super::build_cli_import();
    let help = command.render_help().to_string();
    for option in [
        "-a, --account <ALIAS>",
        "-b, --bob-dir <DIR>",
        "-d, --dry-run",
        "-F, --force",
        "-f, --format <FORMAT>",
        "-h, --help",
    ] {
        assert!(help.contains(option), "missing {option} in:\n{help}");
    }
    assert!(help.contains("[FILE]..."), "FILE is optional in:\n{help}");
    let parsed = super::build_cli_import()
        .try_get_matches_from([
            "bob bank import",
            "-a",
            "checking",
            "-d",
            "one.csv",
            "two.qfx",
        ])
        .expect("account, dry-run, and positional files parse together");
    assert_eq!(
        parsed.get_one::<std::ffi::OsString>("account"),
        Some(&std::ffi::OsString::from("checking"))
    );
    assert!(parsed.get_flag("dry-run"));
    assert_eq!(
        parsed
            .get_many::<std::ffi::OsString>("files")
            .unwrap()
            .count(),
        2
    );
    let empty = super::build_cli_import()
        .try_get_matches_from(["bob bank import"])
        .expect("empty source list reaches the runtime guard");
    assert!(empty.get_many::<std::ffi::OsString>("files").is_none());
    let restore = super::build_cli_import()
        .try_get_matches_from(["bob bank import", "-F"])
        .expect("force restore works without a source file");
    assert!(restore.get_flag("restore-missing"));
}

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
    lint(root, &scan, today())
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
fn hidden_category_stays_valid_in_check_and_totals() {
    let _guard = guard();
    let root = temp_vault("hidden-cat");
    write(
        root.as_path(),
        "bank.md",
        "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      kind: checking\n      start: 2026-11-12\n  categories:\n    kids: { label: 🧸 Kids, hidden: true }\n    groceries: 🛒 Groceries\n---\n# Bank\n",
    );
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("- [x] $10.00 Toy Store [cat:: kids]\n");
    write(root.as_path(), "bank/2026-11.md", &body);
    let report = report_for(&root);
    assert!(
        !has_code(&report, "bank_category_unknown"),
        "{:?}",
        report.findings
    );
    assert_eq!(report.exit_code(), 0);
    let kids = report
        .categories
        .iter()
        .find(|category| category.key == "kids")
        .unwrap();
    assert!(kids.hidden);
    assert!(kids.hotkey.is_none());
    let scan = scan_bank(&root);
    let config = match super::config::parse_bank_config(
        &scan.hub.as_ref().expect("hub").contents,
    ) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    let rows = super::model::collect_rows(&scan, Some(&config), None);
    let totals = super::model::compute_totals(&rows, Some(&config));
    assert_eq!(totals.out_cents, 1000);
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
fn journal_malformed_is_a_bounded_check_warning() {
    let _guard = guard();
    let root = temp_vault("journal-malformed");
    write(root.as_path(), "bank.md", valid_hub());
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("- [x] $13.71 Smashburger [cat:: dining]\n");
    write(root.as_path(), "bank/2026-11.md", &body);
    let journal = root.join(".bob/bank");
    fs::create_dir_all(&journal).unwrap();
    let mut lines = String::new();
    for index in 0..12 {
        lines.push_str(&format!("{{not json {index} secret-fitid-9999}}\n"));
    }
    fs::write(journal.join("athena.jsonl"), lines).unwrap();
    let report = report_for(&root);
    let malformed: Vec<&super::check::Finding> = report
        .findings
        .iter()
        .filter(|finding| finding.code == "bank_journal_malformed")
        .collect();
    assert_eq!(malformed.len(), 9);
    assert!(malformed.iter().all(|finding| {
        finding.severity == super::check::Severity::Warning
            && !finding.message.contains("secret-fitid-9999")
            && !finding.path.contains("secret-fitid")
    }));
    assert!(malformed.iter().any(|finding| {
        finding.path == ".bob/bank/athena.jsonl" && finding.line == Some(1)
    }));
    assert!(malformed.iter().any(|finding| {
        finding
            .message
            .contains("skipped 12 malformed bank journal lines")
    }));
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
fn bob_now_overrides_today_everywhere() {
    let _guard = guard();
    let now = crate::native::env::current_datetime();
    assert_eq!(now.date(), today());
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

fn cli_hub_with_cash() -> &'static str {
    "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n    pocket:\n      label: Pocket cash\n      kind: cash\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n---\n\n# 🏦 Bank\n"
}

fn totals_body() -> String {
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("- [x] +$2,000.00 Salary [cat:: income]\n");
    body.push_str("- [x] +$15.00 RefundThing [cat:: refund]\n");
    body.push_str("- [x] $100.00 Card payment [cat:: transfer]\n");
    body.push_str("- [x] +$100.00 From card [cat:: transfer]\n");
    body.push_str("- [x] $63.50 ATM withdrawal [cat:: cash]\n");
    body.push_str("- [x] $5.00 Pocket coffee [acct:: pocket]\n");
    body.push_str("- [x] $0.10 Dime\n");
    body.push_str("- [x] $0.20 Penny\n");
    body.push_str("- [x] $1,234.56 Big shop [cat:: groceries]\n");
    body.push_str("- [x] −$3.00 Minus sign [cat:: groceries]\n");
    body.push_str("- [x] +$9.99 Mystery inflow\n");
    body.push_str("- [ ] $61.20 ShopRite [cat:: groceries]\n");
    body
}

fn load_for_totals() -> (super::config::BankConfig, Vec<super::model::BankRow>)
{
    let _guard = guard();
    let root = temp_vault("totals-load");
    write(root.as_path(), "bank.md", cli_hub_with_cash());
    write(root.as_path(), "bank/2026-11.md", &totals_body());
    let scan = scan_bank(&root);
    let config = match super::config::parse_bank_config(
        &scan.hub.as_ref().expect("hub").contents,
    ) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    let rows = super::model::collect_rows(&scan, Some(&config), None);
    (config, rows)
}

#[test]
fn totals_keep_transfer_and_cash_apart() {
    let _guard = guard();
    let (config, rows) = load_for_totals();
    let totals = super::model::compute_totals(&rows, Some(&config));
    // Out: 63.50 cash + 0.10 + 0.20 + 1,234.56 + 3.00 + 61.20 waiting.
    assert_eq!(totals.out_cents, 63_50 + 10 + 20 + 123_456 + 300 + 6_120);
    // In: income + refund only; a credit alone is never income.
    assert_eq!(totals.in_cents, 200_000 + 1_500);
    assert_eq!(totals.net_cents, totals.in_cents - totals.out_cents);
    assert_eq!(totals.refund_cents, 1_500);
    assert_eq!(totals.transfer_out_cents, 10_000);
    assert_eq!(totals.transfer_in_cents, 10_000);
    assert_eq!(totals.cash_out_cents, 6_350);
    assert_eq!(totals.cash_traced_cents, 500);
    assert_eq!(totals.unclassified_out_cents, 10 + 20);
    assert_eq!(totals.unclassified_in_cents, 999);
}

#[test]
fn account_status_is_unverified_until_import() {
    let _guard = guard();
    let (config, _) = load_for_totals();
    let statuses = super::status::accounts(&config);
    assert_eq!(statuses[0].status, "unverified");
    assert_eq!(statuses[0].hint, "No import yet — bob bank import FILE");
    assert_eq!(statuses[1].status, "untracked");
}

#[test]
fn list_filters_waiting_all_and_month() {
    let _guard = guard();
    let (config, rows) = load_for_totals();
    let scan = {
        let root = temp_vault("list-scan");
        write(root.as_path(), "bank.md", cli_hub_with_cash());
        write(root.as_path(), "bank/2026-11.md", &totals_body());
        scan_bank(&root)
    };
    let headings = super::list::collect_heading_lines(&scan);
    let waiting = super::list::build(
        rows.clone(),
        today(),
        super::list::ListFilter {
            all: false,
            month: None,
        },
        config.categories.clone(),
        super::status::accounts(&config),
        headings.clone(),
    );
    assert_eq!(waiting.filter, "waiting");
    assert_eq!(waiting.waiting, 1);
    assert!(waiting.days.iter().all(|day| !day.rows.is_empty()));
    let all = super::list::build(
        rows.clone(),
        today(),
        super::list::ListFilter {
            all: true,
            month: None,
        },
        config.categories.clone(),
        super::status::accounts(&config),
        headings,
    );
    assert_eq!(all.filter, "all");
    assert!(all.rows_total > waiting.rows_total);
    let json = super::list::to_json(&waiting, "checking");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["filter"], "waiting");
    assert_eq!(json["counts"]["waiting"], 1);
    let groceries = json["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|category| category["key"] == "groceries")
        .unwrap();
    assert_eq!(groceries["hotkey"], "g");
    assert_eq!(groceries["hidden"], false);
    assert!(groceries["group"].is_null());
    let income = json["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|category| category["key"] == "income")
        .unwrap();
    assert_eq!(income["hotkey"], "i");
    assert_eq!(income["group"], "Money moves");
    assert!(json["days"][0]["rows"][0].get("suggestion").is_some());
}

#[test]
fn report_periods_and_breakdown() {
    let _guard = guard();
    let (config, rows) = load_for_totals();
    let scan = {
        let root = temp_vault("report-scan");
        write(root.as_path(), "bank.md", cli_hub_with_cash());
        write(root.as_path(), "bank/2026-11.md", &totals_body());
        scan_bank(&root)
    };
    let headings = super::list::collect_heading_lines(&scan);
    let period = super::report::resolve_period(
        None,
        Some("2026-11"),
        None,
        None,
        None,
        today(),
    )
    .expect("month");
    assert_eq!(period.key, "2026-11");
    assert!(period.so_far);
    let built = super::report::build(
        rows,
        today(),
        period,
        Some(&config),
        config.categories.clone(),
        super::status::accounts(&config),
        "checking".to_owned(),
        &headings,
        &super::journal::JournalSnapshot::default(),
        false,
    );
    // Breakdown is declared order, then cash, then unclassified.
    let keys: Vec<&str> = built
        .breakdown
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    assert_eq!(keys, vec!["groceries", "cash", "unclassified"]);
    // Every day of the month is present, including empty ones.
    assert_eq!(built.days.len(), 30);
    assert_eq!(built.waiting_all, 1);
    assert!(built.waiting_oldest.is_some());
    let json = super::report::to_json(&built);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["period"]["kind"], "month");
    assert_eq!(json["categories"][0]["hotkey"], "g");
    assert_eq!(json["categories"][0]["hidden"], false);
    assert!(json["days"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|day| day["rows"].as_array().unwrap())
        .all(|row| row.get("suggestion").is_some()));
    assert!(json["totals"]["out_cents"].as_i64().unwrap() > 0);
    // One day, one week, one year all resolve; at most one period.
    assert!(super::report::resolve_period(
        Some("2026-11-12"),
        None,
        None,
        None,
        None,
        today()
    )
    .is_ok());
    assert!(super::report::resolve_period(
        None,
        None,
        Some("2026-W46"),
        None,
        None,
        today()
    )
    .is_ok());
    assert!(super::report::resolve_period(
        None,
        None,
        None,
        Some("2026"),
        None,
        today()
    )
    .is_ok());
    assert!(super::report::resolve_period(
        None,
        Some("2026-11"),
        Some("2026-W46"),
        None,
        None,
        today()
    )
    .is_err());
}

fn hub_starting(start: &str) -> String {
    format!(
        "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: {start}\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n---\n\n# 🏦 Bank\n"
    )
}

fn month_note_body(month: &str, title: &str, sections: &str) -> String {
    format!(
        "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: {month}\ncreated: 2026-11-12T07:30:00-05:00\n---\n\n{title}\n\n```bob-bank\nview: month\n```\n\n{sections}"
    )
}

type VaultRows = (
    super::config::BankConfig,
    Vec<super::model::BankRow>,
    HashMap<(String, NaiveDate), (String, usize)>,
);

fn load_vault(root: &Path) -> VaultRows {
    let scan = scan_bank(root);
    let config = match super::config::parse_bank_config(
        &scan.hub.as_ref().expect("hub").contents,
    ) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    let rows = super::model::collect_rows(&scan, Some(&config), None);
    let headings = super::list::collect_heading_lines(&scan);
    (config, rows, headings)
}

fn place_report(
    config: &super::config::BankConfig,
    rows: Vec<super::model::BankRow>,
    period: super::report::Period,
    headings: &HashMap<(String, NaiveDate), (String, usize)>,
    journal: &super::journal::JournalSnapshot,
    summary: bool,
) -> super::report::BankReport {
    super::report::build(
        rows,
        today(),
        period,
        Some(config),
        config.categories.clone(),
        super::status::accounts(config),
        "checking".to_owned(),
        headings,
        journal,
        summary,
    )
}

fn resolve_flag(raw: &str) -> super::report::Period {
    match super::report::resolve_period(
        None,
        None,
        None,
        None,
        Some(raw),
        today(),
    ) {
        Ok(period) => period,
        Err(error) => panic!("-p {raw}: {error}"),
    }
}

fn import_event(
    account: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> super::journal::ImportEvent {
    super::journal::ImportEvent {
        ts: chrono::DateTime::parse_from_rfc3339("2026-10-21T00:00:00-05:00")
            .expect("ts"),
        host: "test".to_owned(),
        path: ".bob/bank/test.jsonl".to_owned(),
        line: 1,
        account: account.to_owned(),
        source: super::source::SourceKind::Ofx,
        file_sha256: "ab".repeat(32),
        start,
        end,
        balance_cents: None,
        balance_asof: None,
        counts: super::journal::ImportCounts {
            new: 1,
            matched: 0,
            seen: 0,
            ambiguous: 0,
            before_start: 0,
        },
    }
}

fn empty_journal() -> super::journal::JournalSnapshot {
    super::journal::JournalSnapshot::default()
}

#[test]
fn report_period_flag_resolves_stats_strip_spans() {
    use crate::native::periods::calendar::PeriodId;
    let _guard = guard();
    // Day forms go through resolve_day, identical to -D.
    let leap = resolve_flag("2028-02-29");
    assert_eq!(leap.kind, super::report::PeriodKind::Day);
    assert_eq!(leap.key, "2028-02-29");
    assert_eq!(
        leap.start,
        NaiveDate::from_ymd_opt(2028, 2, 29).expect("leap day")
    );
    assert_eq!(resolve_flag("today").start, today());
    // Week, quarter, and ISO year spans match the periods calendar.
    for raw in ["2026-W01", "2026-W53", "2026-Q4", "2026"] {
        let period = resolve_flag(raw);
        let id = PeriodId::resolve(raw, today()).expect("calendar");
        assert_eq!(period.start, id.start().expect("start"), "{raw}");
        assert_eq!(period.end, id.end().expect("end"), "{raw}");
    }
    let quarter = resolve_flag("2026-Q4");
    assert_eq!(quarter.kind, super::report::PeriodKind::Quarter);
    assert_eq!(quarter.key, "2026-Q4");
    assert_eq!(
        quarter.start,
        NaiveDate::from_ymd_opt(2026, 9, 28).expect("Q4 start")
    );
    assert_eq!(
        quarter.end,
        NaiveDate::from_ymd_opt(2027, 1, 3).expect("Q4 end")
    );
    assert_eq!(quarter.label, "2026-Q4 · Sep 28, 2026 – Jan 3, 2027");
    let iso = resolve_flag("2026");
    assert_eq!(iso.kind, super::report::PeriodKind::IsoYear);
    assert_eq!(iso.key, "2026");
    assert_eq!(
        iso.start,
        NaiveDate::from_ymd_opt(2025, 12, 29).expect("ISO start")
    );
    assert_eq!(
        iso.end,
        NaiveDate::from_ymd_opt(2027, 1, 3).expect("ISO end")
    );
    assert_eq!(iso.label, "2026 · Dec 29, 2025 – Jan 3, 2027");
    // Relative and note-path forms resolve too.
    assert_eq!(resolve_flag("last-quarter").key, "2026-Q3");
    let pathed = resolve_flag("2026/2026-W46.md");
    assert_eq!(pathed.kind, super::report::PeriodKind::Week);
    assert_eq!(pathed.key, "2026-W46");
    // A calendar month is not a stats-strip period.
    assert!(super::report::resolve_period(
        None,
        None,
        None,
        None,
        Some("2026-11"),
        today()
    )
    .is_err());
}

#[test]
fn report_period_flag_matches_flag_output_shapes() {
    let _guard = guard();
    let root = temp_vault("period-parity");
    write(root.as_path(), "bank.md", &hub_starting("2025-01-01"));
    write(
        root.as_path(),
        "bank/2025-12.md",
        &month_note_body(
            "2025-12",
            "# 🏦 December 2025",
            "## Tue Dec 30\n\n- [x] $10.00 Pre-year [cat:: groceries]\n",
        ),
    );
    write(
        root.as_path(),
        "bank/2026-06.md",
        &month_note_body(
            "2026-06",
            "# 🏦 June 2026",
            "## Mon Jun 1\n\n- [x] $20.00 Mid-year [cat:: groceries]\n",
        ),
    );
    write(
        root.as_path(),
        "bank/2027-01.md",
        &month_note_body(
            "2027-01",
            "# 🏦 January 2027",
            "## Sat Jan 2\n\n- [x] $30.00 Post-year [cat:: groceries]\n",
        ),
    );
    write(
        root.as_path(),
        "bank/2028-01.md",
        &month_note_body(
            "2028-01",
            "# 🏦 January 2028",
            "## Sat Jan 1\n\n- [x] $40.00 Next ISO year [cat:: groceries]\n",
        ),
    );
    write(
        root.as_path(),
        "bank/2026-11.md",
        &month_note_body(
            "2026-11",
            "# 🏦 November 2026",
            "## Thu Nov 12\n\n- [x] $13.71 Smashburger [cat:: dining]\n",
        ),
    );
    let (config, rows, headings) = load_vault(root.as_path());
    let out_of = |period: super::report::Period| {
        place_report(
            &config,
            rows.clone(),
            period,
            &headings,
            &empty_journal(),
            false,
        )
        .totals
        .out_cents
    };
    // -p 2026 is the ISO year: it counts the Dec 29–31 2025 and Jan 1–3
    // 2027 rows that -y 2026 leaves out (the Jan 1 2028 row is ISO 2027).
    assert_eq!(out_of(resolve_flag("2026")), 1000 + 2000 + 3000 + 1371);
    assert_eq!(
        out_of(
            super::report::resolve_period(
                None,
                None,
                None,
                Some("2026"),
                None,
                today()
            )
            .expect("calendar year")
        ),
        2000 + 1371
    );
    // And vice versa: -y 2027 counts Jan 1–3 2027 rows that -p 2027
    // leaves out, while -p 2027 reaches the Jan 1 2028 row.
    assert_eq!(out_of(resolve_flag("2027")), 4000);
    assert_eq!(
        out_of(
            super::report::resolve_period(
                None,
                None,
                None,
                Some("2027"),
                None,
                today()
            )
            .expect("calendar year")
        ),
        3000
    );
    // All time spans the bank's own history: the account start wins over
    // the earliest row, and the later of today and the latest row ends it.
    let provisional = resolve_flag("all");
    assert_eq!(provisional.kind, super::report::PeriodKind::AllTime);
    let all_time =
        super::report::all_time_period(Some(&config), &rows, today());
    assert_eq!(all_time.key, "all_time");
    assert_eq!(
        all_time.start,
        NaiveDate::from_ymd_opt(2025, 1, 1).expect("account start")
    );
    assert_eq!(
        all_time.end,
        NaiveDate::from_ymd_opt(2028, 1, 1).expect("latest row")
    );
    assert_eq!(all_time.label, "All time · since Jan 1, 2025");
    assert!(all_time.so_far);
    let everything = place_report(
        &config,
        rows.clone(),
        all_time,
        &headings,
        &empty_journal(),
        false,
    );
    assert_eq!(
        everything.totals.out_cents,
        1000 + 2000 + 3000 + 4000 + 1371
    );
    // Flag twins agree apart from what the flags themselves change.
    let week_p = place_report(
        &config,
        rows.clone(),
        resolve_flag("2026-W46"),
        &headings,
        &empty_journal(),
        false,
    );
    let week_w = place_report(
        &config,
        rows.clone(),
        super::report::resolve_period(
            None,
            None,
            Some("2026-W46"),
            None,
            None,
            today(),
        )
        .expect("week"),
        &headings,
        &empty_journal(),
        false,
    );
    assert_eq!(
        super::report::to_json(&week_p),
        super::report::to_json(&week_w)
    );
    let day_p = place_report(
        &config,
        rows.clone(),
        resolve_flag("2026-11-12"),
        &headings,
        &empty_journal(),
        false,
    );
    let day_d = place_report(
        &config,
        rows,
        super::report::resolve_period(
            Some("2026-11-12"),
            None,
            None,
            None,
            None,
            today(),
        )
        .expect("day"),
        &headings,
        &empty_journal(),
        false,
    );
    assert_eq!(
        super::report::to_json(&day_p),
        super::report::to_json(&day_d)
    );
}

#[test]
fn report_period_flag_rejects_extra_selectors() {
    let _guard = guard();
    let message =
        "pass at most one of -D/--day, -m/--month, -p/--period, -w/--week, -y/--year";
    for args in [
        (Some("2026-11-12"), None, None, None, Some("2026-Q4")),
        (None, None, Some("2026-W46"), None, Some("2026-Q4")),
        (None, Some("2026-11"), None, Some("2026"), Some("all")),
    ] {
        let error = super::report::resolve_period(
            args.0,
            args.1,
            args.2,
            args.3,
            args.4,
            today(),
        )
        .expect_err("two selectors must refuse");
        assert_eq!(error, message);
    }
    let invalid = super::report::resolve_period(
        None,
        None,
        None,
        None,
        Some("bogus"),
        today(),
    )
    .expect_err("bogus period must refuse");
    assert!(invalid.contains("invalid period"), "{invalid}");
}

#[test]
fn report_summary_omits_days_with_totals_intact() {
    let _guard = guard();
    let root = temp_vault("period-summary");
    write(root.as_path(), "bank.md", cli_hub_with_cash());
    write(root.as_path(), "bank/2026-11.md", &totals_body());
    let (config, rows, headings) = load_vault(root.as_path());
    let month = super::report::resolve_period(
        None,
        Some("2026-11"),
        None,
        None,
        None,
        today(),
    )
    .expect("month");
    let full = place_report(
        &config,
        rows.clone(),
        month.clone(),
        &headings,
        &empty_journal(),
        false,
    );
    let slim =
        place_report(&config, rows, month, &headings, &empty_journal(), true);
    assert_eq!(full.days.len(), 30);
    assert!(slim.days.is_empty());
    let full_json = super::report::to_json(&full);
    let slim_json = super::report::to_json(&slim);
    assert_eq!(slim_json["days"].as_array().expect("days").len(), 0);
    assert_eq!(slim_json["totals"], full_json["totals"]);
    assert_eq!(slim_json["breakdown"], full_json["breakdown"]);
    assert_eq!(slim_json["waiting"], full_json["waiting"]);
    assert_eq!(slim_json["coverage"], full_json["coverage"]);
    assert_eq!(slim_json["navigation"], full_json["navigation"]);
}

#[test]
fn report_day_totals_run_through_compute_totals() {
    let _guard = guard();
    let root = temp_vault("period-day-parity");
    write(root.as_path(), "bank.md", cli_hub_with_cash());
    write(
        root.as_path(),
        "bank/2026-11.md",
        &month_note_body(
            "2026-11",
            "# 🏦 November 2026",
            "## Thu Nov 12\n\n- [x] $10.00 Lunch [cat:: groceries]\n- [x] +$50.00 Mystery\n- [x] $100.00 Card payment [cat:: transfer]\n- [x] $5.00 Pocket coffee [acct:: pocket]\n- [x] $0.00 Zero [cat:: groceries]\n",
        ),
    );
    let (config, rows, headings) = load_vault(root.as_path());
    let day = super::report::resolve_period(
        Some("2026-11-12"),
        None,
        None,
        None,
        None,
        today(),
    )
    .expect("day");
    let built =
        place_report(&config, rows, day, &headings, &empty_journal(), false);
    let entry = built
        .days
        .iter()
        .find(|day| !day.rows.is_empty())
        .expect("day");
    // Only the groceries lunch counts: the mystery inflow is unclassified,
    // the transfer and cash-account rows are kept apart, and zero is zero.
    assert_eq!(entry.out_cents, 1000);
    assert_eq!(entry.in_cents, 0);
    assert_eq!(built.totals.unclassified_in_cents, 5000);
    assert_eq!(built.totals.transfer_out_cents, 10_000);
    assert_eq!(built.totals.cash_traced_cents, 500);
    for day in &built.days {
        let check = super::model::compute_totals(&day.rows, Some(&config));
        assert_eq!(day.out_cents, check.out_cents);
        assert_eq!(day.in_cents, check.in_cents);
        assert_eq!(day.waiting, check.waiting);
        assert_eq!(day.flagged, check.flagged);
    }
    assert_eq!(
        built.days.iter().map(|day| day.out_cents).sum::<i64>(),
        built.totals.out_cents
    );
    assert_eq!(
        built.days.iter().map(|day| day.in_cents).sum::<i64>(),
        built.totals.in_cents
    );
}

#[test]
fn report_coverage_tracks_history_and_imports() {
    let _guard = guard();
    let root = temp_vault("period-coverage");
    write(root.as_path(), "bank.md", &hub_starting("2026-11-10"));
    write(
        root.as_path(),
        "bank/2026-11.md",
        &month_note_body(
            "2026-11",
            "# 🏦 November 2026",
            "## Thu Nov 12\n\n- [x] $10.00 Lunch [cat:: groceries]\n",
        ),
    );
    let (config, rows, headings) = load_vault(root.as_path());
    // Tracking starts inside W46, and the account was never imported.
    let week = place_report(
        &config,
        rows.clone(),
        resolve_flag("2026-W46"),
        &headings,
        &empty_journal(),
        false,
    );
    assert!(!week.coverage.complete);
    assert_eq!(
        week.coverage.tracked_since,
        NaiveDate::from_ymd_opt(2026, 11, 10)
    );
    assert_eq!(week.coverage.start, NaiveDate::from_ymd_opt(2026, 11, 10));
    assert_eq!(
        week.coverage.imported_through,
        NaiveDate::from_ymd_opt(2026, 11, 9)
    );
    assert_eq!(
        week.coverage.through,
        NaiveDate::from_ymd_opt(2026, 11, 9).expect("through")
    );
    let json = super::report::to_json(&week);
    assert_eq!(json["coverage"]["tracked_since"], "2026-11-10");
    assert_eq!(json["coverage"]["start"], "2026-11-10");
    assert_eq!(json["coverage"]["imported_through"], "2026-11-09");
    assert_eq!(json["coverage"]["through"], "2026-11-09");
    assert_eq!(json["coverage"]["complete"], false);

    // A closed month imported short of its end stays incomplete; a full
    // window completes it.
    let october = temp_vault("period-coverage-oct");
    write(october.as_path(), "bank.md", &hub_starting("2026-10-01"));
    write(
        october.as_path(),
        "bank/2026-10.md",
        &month_note_body(
            "2026-10",
            "# 🏦 October 2026",
            "## Sat Oct 31\n\n- [x] $10.00 Lunch [cat:: groceries]\n",
        ),
    );
    let (oct_config, oct_rows, oct_headings) = load_vault(october.as_path());
    let october_period = super::report::resolve_period(
        None,
        Some("2026-10"),
        None,
        None,
        None,
        today(),
    )
    .expect("october");
    let short = super::journal::JournalSnapshot {
        imports: vec![import_event(
            "checking",
            NaiveDate::from_ymd_opt(2026, 10, 1).expect("start"),
            NaiveDate::from_ymd_opt(2026, 10, 20).expect("short end"),
        )],
        ..super::journal::JournalSnapshot::default()
    };
    let partial = place_report(
        &oct_config,
        oct_rows.clone(),
        october_period.clone(),
        &oct_headings,
        &short,
        false,
    );
    assert!(!partial.coverage.complete);
    assert_eq!(
        partial.coverage.tracked_since,
        NaiveDate::from_ymd_opt(2026, 10, 1)
    );
    assert_eq!(partial.coverage.start, NaiveDate::from_ymd_opt(2026, 10, 1));
    assert_eq!(
        partial.coverage.imported_through,
        NaiveDate::from_ymd_opt(2026, 10, 20)
    );
    assert_eq!(
        partial.coverage.through,
        NaiveDate::from_ymd_opt(2026, 10, 20).expect("through")
    );
    let covered = super::journal::JournalSnapshot {
        imports: vec![import_event(
            "checking",
            NaiveDate::from_ymd_opt(2026, 10, 1).expect("start"),
            NaiveDate::from_ymd_opt(2026, 10, 31).expect("end"),
        )],
        ..super::journal::JournalSnapshot::default()
    };
    let whole = place_report(
        &oct_config,
        oct_rows,
        october_period,
        &oct_headings,
        &covered,
        false,
    );
    assert!(whole.coverage.complete);
    assert_eq!(
        whole.coverage.through,
        NaiveDate::from_ymd_opt(2026, 10, 31).expect("end")
    );

    // With no importable accounts and no rows, coverage is nulls.
    let cash_only = temp_vault("period-coverage-cash");
    write(
        cash_only.as_path(),
        "bank.md",
        "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    pocket:\n      label: Pocket cash\n      kind: cash\n---\n\n# 🏦 Bank\n",
    );
    write(
        cash_only.as_path(),
        "bank/2026-11.md",
        &month_note_body("2026-11", "# 🏦 November 2026", "## Thu Nov 12\n"),
    );
    let (cash_config, cash_rows, cash_headings) =
        load_vault(cash_only.as_path());
    assert!(cash_rows.is_empty());
    let cash_month = super::report::resolve_period(
        None,
        Some("2026-11"),
        None,
        None,
        None,
        today(),
    )
    .expect("month");
    let cash = place_report(
        &cash_config,
        cash_rows,
        cash_month,
        &cash_headings,
        &empty_journal(),
        false,
    );
    assert!(cash.coverage.tracked_since.is_none());
    assert!(cash.coverage.imported_through.is_none());
    assert!(cash.coverage.start.is_none());
    assert_eq!(
        cash.coverage.through,
        NaiveDate::from_ymd_opt(2026, 11, 30).expect("month end")
    );
    assert!(cash.coverage.complete);
    let cash_json = super::report::to_json(&cash);
    assert!(cash_json["coverage"]["tracked_since"].is_null());
    assert!(cash_json["coverage"]["imported_through"].is_null());
    assert!(cash_json["coverage"]["start"].is_null());
}

#[test]
fn report_navigation_targets_waiting_and_first_rows() {
    let _guard = guard();
    let root = temp_vault("period-navigation");
    write(root.as_path(), "bank.md", &hub_starting("2026-10-01"));
    let october = month_note_body(
        "2026-10",
        "# 🏦 October 2026",
        "## Mon Oct 26\n\n- [x] $100.00 Card payment [cat:: transfer]\n\n## Tue Oct 27\n\n- [x] $9.00 Counted [cat:: groceries]\n\n## Fri Oct 30\n\n- [ ] $5.00 Waiting Oct [cat:: groceries]\n- [ ] $6.00 Waiting later [cat:: groceries]\n",
    );
    write(root.as_path(), "bank/2026-10.md", &october);
    let november = month_note_body(
        "2026-11",
        "# 🏦 November 2026",
        "## Sun Nov 1\n\n- [ ] $7.00 Waiting Nov [cat:: groceries]\n",
    );
    write(root.as_path(), "bank/2026-11.md", &november);
    let (config, rows, headings) = load_vault(root.as_path());
    // W44 spans Oct 26 – Nov 1, crossing the month boundary.
    let built = place_report(
        &config,
        rows,
        resolve_flag("2026-W44"),
        &headings,
        &empty_journal(),
        false,
    );
    let waiting = built.navigation.waiting.as_ref().expect("waiting");
    assert_eq!(
        waiting.date,
        NaiveDate::from_ymd_opt(2026, 10, 30).expect("date")
    );
    assert_eq!(waiting.path, "bank/2026-10.md");
    // Ties go to the earliest line on the earliest date.
    let october_line = |heading: &str| {
        october
            .lines()
            .position(|line| line == heading)
            .expect("heading")
            + 1
    };
    assert_eq!(waiting.line, october_line("## Fri Oct 30") + 2);
    // The transfer row is not counted money, so first lands on Oct 27.
    let first = built.navigation.first.as_ref().expect("first");
    assert_eq!(
        first.date,
        NaiveDate::from_ymd_opt(2026, 10, 27).expect("date")
    );
    assert_eq!(first.path, "bank/2026-10.md");
    assert_eq!(first.line, october_line("## Tue Oct 27"));
    assert_eq!(
        built.navigation.months,
        vec!["bank/2026-10.md".to_owned(), "bank/2026-11.md".to_owned()]
    );
    let json = super::report::to_json(&built);
    assert_eq!(json["navigation"]["waiting"]["date"], "2026-10-30");
    assert_eq!(json["navigation"]["waiting"]["path"], "bank/2026-10.md");
    assert_eq!(json["navigation"]["first"]["date"], "2026-10-27");
    assert_eq!(
        json["navigation"]["months"],
        serde_json::json!(["bank/2026-10.md", "bank/2026-11.md"])
    );
    // A period with no rows navigates nowhere.
    let empty = place_report(
        &config,
        Vec::new(),
        resolve_flag("2026-W40"),
        &headings,
        &empty_journal(),
        false,
    );
    assert!(empty.navigation.waiting.is_none());
    assert!(empty.navigation.first.is_none());
    assert!(empty.navigation.months.is_empty());
    let empty_json = super::report::to_json(&empty);
    assert!(empty_json["navigation"]["waiting"].is_null());
    assert!(empty_json["navigation"]["first"].is_null());
}

#[test]
fn ensure_plans_missing_note_heading_and_existing() {
    let _guard = guard();
    let root = temp_vault("ensure");
    write(root.as_path(), "bank.md", valid_hub());
    let config = match super::config::parse_bank_config(valid_hub()) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    // Missing note creates both.
    let plan =
        super::ensure::plan(root.as_path(), today(), valid_hub(), &config)
            .expect("plan");
    assert!(plan.created_note);
    assert!(!plan.created_heading);
    assert_eq!(plan.path, "bank/2026-11.md");
    // Write it, then a missing heading on another day is planned.
    if let Some(parent) = root.join(&plan.path).parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(
        root.join(&plan.path),
        plan.updated_contents.as_ref().expect("contents"),
    )
    .expect("write");
    let other = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
    let second =
        super::ensure::plan(root.as_path(), other, valid_hub(), &config)
            .expect("plan");
    assert!(!second.created_note);
    assert!(second.created_heading);
    fs::write(
        root.join(&second.path),
        second.updated_contents.as_ref().expect("contents"),
    )
    .expect("write");
    // Existing heading is idempotent and points under the heading.
    let third =
        super::ensure::plan(root.as_path(), other, valid_hub(), &config)
            .expect("plan");
    assert!(!third.created_note);
    assert!(!third.created_heading);
    assert!(third.updated_contents.is_none());
    let json = super::ensure::to_json(
        &third,
        today(),
        &config.categories,
        &super::status::accounts(&config),
    );
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["day"], "2026-11-13");
}

#[test]
fn ensure_line_lands_after_free_note_children() {
    let _guard = guard();
    let root = temp_vault("ensure-child");
    write(root.as_path(), "bank.md", valid_hub());
    let config = match super::config::parse_bank_config(valid_hub()) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("\n- [ ] $61.20 ShopRite [cat:: groceries]\n  free note stays with ShopRite\n");
    write(root.as_path(), "bank/2026-11.md", &body);
    let plan =
        super::ensure::plan(root.as_path(), today(), valid_hub(), &config)
            .expect("plan");
    assert!(!plan.created_note);
    assert!(!plan.created_heading);
    let child_line = body
        .lines()
        .position(|line| line.contains("free note stays with ShopRite"))
        .expect("child line")
        + 1;
    assert_eq!(plan.line, child_line + 1, "next row goes after the child");
}

#[test]
fn ensure_stamps_created_from_bob_now_in_the_configured_zone() {
    let root = temp_vault("ensure-created-zone");
    let config_file = root.join("bob-config.yml");
    fs::write(&config_file, "timezone: America/New_York\n").expect("config");
    let _guard = TestEnvGuard::set(&[
        ("BOB_CONFIG_FILE", Some(config_file.as_os_str())),
        ("BOB_NOW", Some(OsStr::new("2026-11-12T07:30:00"))),
        ("DATE", None),
    ]);
    write(root.as_path(), "bank.md", valid_hub());
    let config = match super::config::parse_bank_config(valid_hub()) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    let plan =
        super::ensure::plan(root.as_path(), today(), valid_hub(), &config)
            .expect("plan");
    assert!(plan.created_note);
    let contents = plan.updated_contents.expect("contents");
    assert_eq!(
        contents,
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today())
    );
    assert_eq!(
        super::import_run::now_stamp(
            NaiveDate::from_ymd_opt(2026, 11, 12)
                .unwrap()
                .and_hms_opt(7, 30, 0)
                .unwrap()
        ),
        "2026-11-12T07:30:00-05:00"
    );
}

#[test]
fn ensure_refuses_a_note_changed_after_planning() {
    let _guard = guard();
    let root = temp_vault("ensure-stale-preimage");
    write(root.as_path(), "bank.md", valid_hub());
    let config = match super::config::parse_bank_config(valid_hub()) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    let other_day = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
    let original =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", other_day);
    write(root.as_path(), "bank/2026-11.md", &original);

    let plan =
        super::ensure::plan(root.as_path(), today(), valid_hub(), &config)
            .expect("plan");
    assert_eq!(plan.original, original);
    let concurrent = format!("{original}\n- [ ] concurrent edit\n");
    write(root.as_path(), "bank/2026-11.md", &concurrent);
    let staged = crate::native::capture::StagedTextFile {
        target: plan.target,
        target_existed: plan.existed,
        original_target: plan.original,
        updated_target: plan.updated_contents.expect("planned contents"),
    };

    let error = crate::native::capture::write_staged_files_guarded(
        &[staged],
        || Ok(()),
    )
    .expect_err("stale planning snapshot must be refused");
    assert!(error.message.contains("changed on disk after planning"));
    assert_eq!(
        fs::read_to_string(root.join("bank/2026-11.md")).expect("note"),
        concurrent
    );
}

#[test]
fn ack_ticks_ids_day_and_reports_unknown() {
    let _guard = guard();
    let root = temp_vault("ack");
    write(root.as_path(), "bank.md", valid_hub());
    let mut body =
        canonical_template(month(), "2026-11-12T07:30:00-05:00", today());
    body.push_str("- [ ] $61.20 ShopRite [cat:: groceries] ^bank-2b91c0e4\n");
    body.push_str("- [ ] $4.25 Latte [cat:: dining]\n");
    body.push_str("- [x] $13.71 Smashburger [cat:: dining]\n");
    write(root.as_path(), "bank/2026-11.md", &body);
    let scan = scan_bank(&root);
    let config = match super::config::parse_bank_config(
        &scan.hub.as_ref().expect("hub").contents,
    ) {
        super::config::ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    };
    let rows = super::model::collect_rows(&scan, Some(&config), None);
    let mut contents = std::collections::HashMap::new();
    for month in &scan.months {
        contents.insert(month.path.clone(), month.contents.clone());
    }
    // By ID, with ^ prefix and bare hex both accepted.
    let by_id = super::ack::plan_ack(
        &rows,
        &super::ack::AckSelection {
            ids: vec!["^bank-2b91c0e4".to_owned()],
            day: None,
        },
        &contents,
    );
    assert_eq!(by_id.acknowledged.len(), 1);
    assert!(by_id.unknown.is_empty());
    // By day ticks both waiting rows.
    let by_day = super::ack::plan_ack(
        &rows,
        &super::ack::AckSelection {
            ids: Vec::new(),
            day: Some(today()),
        },
        &contents,
    );
    assert_eq!(by_day.acknowledged.len(), 2);
    // Unknown IDs refuse.
    let unknown = super::ack::plan_ack(
        &rows,
        &super::ack::AckSelection {
            ids: vec!["deadbeef".to_owned()],
            day: None,
        },
        &contents,
    );
    assert_eq!(unknown.unknown, vec!["deadbeef".to_owned()]);
    assert!(unknown.edits.is_empty() || !unknown.acknowledged.is_empty());
    // Already-ticked rows are no-ops.
    let already = super::ack::plan_ack(
        &rows,
        &super::ack::AckSelection {
            ids: vec!["^bank-2b91c0e4".to_owned()],
            day: None,
        },
        &contents,
    );
    let _ = already;
    let ticked_body = body.replace("- [ ] $61.20", "- [x] $61.20");
    write(root.as_path(), "bank/2026-11.md", &ticked_body);
    let rescan = scan_bank(&root);
    let rerows = super::model::collect_rows(&rescan, Some(&config), None);
    let mut recontents = std::collections::HashMap::new();
    for month in &rescan.months {
        recontents.insert(month.path.clone(), month.contents.clone());
    }
    let repeat = super::ack::plan_ack(
        &rerows,
        &super::ack::AckSelection {
            ids: vec!["2b91c0e4".to_owned()],
            day: None,
        },
        &recontents,
    );
    assert_eq!(repeat.acknowledged.len(), 0);
    assert_eq!(repeat.already.len(), 1);
    assert!(repeat.unknown.is_empty());
}
