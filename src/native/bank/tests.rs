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

#[test]
fn import_command_help_and_option_aliases_are_complete() {
    let mut command = super::build_cli_import();
    let help = command.render_help().to_string();
    for option in [
        "-a, --account <ALIAS>",
        "-b, --bob-dir <DIR>",
        "-d, --dry-run",
        "-f, --format <FORMAT>",
        "-h, --help",
    ] {
        assert!(help.contains(option), "missing {option} in:\n{help}");
    }
    assert!(help.contains("<FILE>..."), "FILE is required in:\n{help}");
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
    assert!(
        super::build_cli_import()
            .try_get_matches_from(["bob bank import"])
            .is_err(),
        "omitting FILE is a clap usage error"
    );
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
    assert!(json["totals"]["out_cents"].as_i64().unwrap() > 0);
    // One day, one week, one year all resolve; at most one period.
    assert!(super::report::resolve_period(
        Some("2026-11-12"),
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
        today()
    )
    .is_ok());
    assert!(super::report::resolve_period(
        None,
        None,
        None,
        Some("2026"),
        today()
    )
    .is_ok());
    assert!(super::report::resolve_period(
        None,
        Some("2026-11"),
        Some("2026-W46"),
        None,
        today()
    )
    .is_err());
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
