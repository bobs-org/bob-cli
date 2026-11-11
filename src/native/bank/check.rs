//! Grammar-phase findings for `bob bank check`.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    config::{
        account_statuses, parse_bank_config, suggest_name, AccountStatus,
        BankConfig, Category, ConfigDiagnostic, ConfigParse,
    },
    grammar::{looks_like_day_heading, render_day_heading, ParsedRow},
    month::{parse_month_note, LocatedRow, LocatedRowKind, MonthNote},
    scan::{is_teaching_empty, BankScan},
};

const SCHEMA_VERSION: u32 = 1;
const SPENDING_EXEMPT: &[&str] = &["income", "refund", "transfer"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) code: &'static str,
    pub(crate) severity: Severity,
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) day: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueCounts {
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckReport {
    pub(crate) teaching_empty: bool,
    pub(crate) today: NaiveDate,
    pub(crate) findings: Vec<Finding>,
    pub(crate) accounts: Vec<AccountStatus>,
    pub(crate) categories: Vec<Category>,
    pub(crate) issues: IssueCounts,
    pub(crate) io_errors: Vec<String>,
    pub(crate) note_count: usize,
}

impl CheckReport {
    pub(crate) fn ok(&self) -> bool {
        self.issues.errors == 0 && self.io_errors.is_empty()
    }

    pub(crate) fn exit_code(&self) -> i32 {
        if self.ok() {
            0
        } else {
            1
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "schema_version": SCHEMA_VERSION,
            "today": self.today.format("%Y-%m-%d").to_string(),
            "categories": self.categories.iter().map(category_json).collect::<Vec<_>>(),
            "accounts": self.accounts.iter().map(account_json).collect::<Vec<_>>(),
            "issues": {
                "errors": self.issues.errors,
                "warnings": self.issues.warnings,
            },
            "findings": self.findings.iter().map(finding_json).collect::<Vec<_>>(),
        })
    }
}

pub(crate) fn lint(scan: &BankScan, today: NaiveDate) -> CheckReport {
    if is_teaching_empty(scan) {
        return CheckReport {
            teaching_empty: true,
            today,
            findings: Vec::new(),
            accounts: Vec::new(),
            categories: Vec::new(),
            issues: IssueCounts {
                errors: 0,
                warnings: 0,
            },
            io_errors: Vec::new(),
            note_count: 0,
        };
    }

    let mut findings = Vec::new();
    let mut config = None;
    if let Some(hub) = &scan.hub {
        match parse_bank_config(&hub.contents) {
            ConfigParse::MissingMapping => {
                findings.push(finding(
                    "bank_hub_missing",
                    Severity::Error,
                    "bank.md",
                    None,
                    "bank.md has no bank: mapping",
                    Some("Add a bank: mapping. See docs/bank.md.".to_owned()),
                    None,
                ));
            }
            ConfigParse::Invalid {
                diagnostics,
                config: parsed,
            } => {
                for diagnostic in diagnostics {
                    findings.push(config_finding(diagnostic));
                }
                config = parsed;
            }
            ConfigParse::Ok(parsed) => config = Some(parsed),
        }
    } else if scan.bank_dir_exists {
        findings.push(finding(
            "bank_hub_missing",
            Severity::Error,
            "bank.md",
            None,
            "bank/ exists without bank.md",
            Some(
                "Create bank.md at the vault root with a bank: mapping."
                    .to_owned(),
            ),
            None,
        ));
    }

    let mut id_seen: HashMap<String, (String, usize)> = HashMap::new();
    for month in &scan.months {
        let note = parse_month_note(&month.path, &month.contents, month.month);
        lint_month(&note, config.as_ref(), &mut findings, &mut id_seen);
    }

    findings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
            .then(left.code.cmp(right.code))
    });

    let errors = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .count();
    let warnings = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Warning)
        .count();
    let (accounts, categories) = match &config {
        Some(config) => (account_statuses(config), config.categories.clone()),
        None => (Vec::new(), Vec::new()),
    };
    let note_count = usize::from(scan.hub.is_some()) + scan.months.len();
    CheckReport {
        teaching_empty: false,
        today,
        findings,
        accounts,
        categories,
        issues: IssueCounts { errors, warnings },
        io_errors: scan.io_errors.clone(),
        note_count,
    }
}

fn lint_month(
    note: &MonthNote,
    config: Option<&BankConfig>,
    findings: &mut Vec<Finding>,
    id_seen: &mut HashMap<String, (String, usize)>,
) {
    if !note.type_is_bank_month {
        findings.push(finding(
            "bank_month_mismatch",
            Severity::Warning,
            &note.path,
            None,
            "type: should be [[bank_month]]",
            None,
            None,
        ));
    }
    if let Some(frontmatter_month) = note.frontmatter_month
        && frontmatter_month != note.month
    {
        findings.push(finding(
            "bank_month_mismatch",
            Severity::Warning,
            &note.path,
            None,
            format!(
                "month: {} does not match filename {}",
                frontmatter_month.as_str(),
                note.month.as_str()
            ),
            None,
            None,
        ));
    } else if note.frontmatter_month.is_none() {
        findings.push(finding(
            "bank_month_mismatch",
            Severity::Warning,
            &note.path,
            None,
            format!("month: is missing; filename is {}", note.month.as_str()),
            None,
            None,
        ));
    }

    let mut seen_dates: HashMap<NaiveDate, usize> = HashMap::new();
    let mut previous_date: Option<NaiveDate> = None;
    for day in &note.days {
        if !note.month.contains(day.date) {
            findings.push(finding(
                "bank_day_invalid",
                Severity::Error,
                &note.path,
                Some(day.heading_line),
                format!(
                    "day heading {} is outside month {}",
                    day.date,
                    note.month.as_str()
                ),
                None,
                Some(day.date),
            ));
        }
        if let Some(first_line) = seen_dates.get(&day.date) {
            findings.push(finding(
                "bank_day_duplicate",
                Severity::Warning,
                &note.path,
                Some(day.heading_line),
                format!(
                    "duplicate day heading for {} (first at line {first_line})",
                    render_day_heading(day.date).trim_start_matches('#').trim()
                ),
                None,
                Some(day.date),
            ));
        } else {
            seen_dates.insert(day.date, day.heading_line);
        }
        if let Some(previous) = previous_date
            && day.date < previous
        {
            findings.push(finding(
                "bank_day_order",
                Severity::Warning,
                &note.path,
                Some(day.heading_line),
                format!(
                    "day heading {} is out of order",
                    render_day_heading(day.date).trim_start_matches('#').trim()
                ),
                Some("Day headings should be oldest first.".to_owned()),
                Some(day.date),
            ));
        }
        previous_date = Some(day.date);
        if day.heading.weekday_mismatch {
            findings.push(finding(
                "bank_day_weekday",
                Severity::Warning,
                &note.path,
                Some(day.heading_line),
                format!("weekday does not match {}", day.date),
                None,
                Some(day.date),
            ));
        }
        for row in &day.rows {
            lint_row(
                &note.path,
                Some(day.date),
                row,
                config,
                findings,
                id_seen,
            );
        }
    }

    for (line, title) in &note.other_headings {
        if looks_like_day_heading(title) {
            findings.push(finding(
                "bank_day_invalid",
                Severity::Error,
                &note.path,
                Some(*line),
                format!("day heading {title:?} is not a valid date"),
                None,
                None,
            ));
        }
    }

    for row in &note.outside_rows {
        lint_row(&note.path, None, row, config, findings, id_seen);
        findings.push(finding(
            "bank_row_outside_day",
            Severity::Error,
            &note.path,
            Some(row.line),
            "amount-led row is not under a day heading",
            None,
            None,
        ));
    }
}

fn lint_row(
    path: &str,
    day: Option<NaiveDate>,
    row: &LocatedRow,
    config: Option<&BankConfig>,
    findings: &mut Vec<Finding>,
    id_seen: &mut HashMap<String, (String, usize)>,
) {
    match &row.kind {
        LocatedRowKind::NoCheckbox => {
            findings.push(finding(
                "bank_row_no_checkbox",
                Severity::Warning,
                path,
                Some(row.line),
                "amount-led list item has no checkbox",
                Some("Write `- [x] $…` or `- [ ] $…`.".to_owned()),
                day,
            ));
        }
        LocatedRowKind::Unparsed => {
            findings.push(finding(
                "bank_row_unparsed",
                Severity::Error,
                path,
                Some(row.line),
                "amount-led row does not parse",
                Some(
                    "Use $ plus digits, optional commas, and .d or .dd."
                        .to_owned(),
                ),
                day,
            ));
        }
        LocatedRowKind::Parsed(parsed) => {
            lint_parsed(path, day, row.line, parsed, config, findings);
            if let Some(id) = &parsed.id {
                if let Some((first_path, first_line)) = id_seen.get(id) {
                    findings.push(finding(
                        "bank_id_duplicate",
                        Severity::Error,
                        path,
                        Some(row.line),
                        format!(
                            "duplicate ^bank-{id} (first at {first_path}:{first_line})"
                        ),
                        None,
                        day,
                    ));
                } else {
                    id_seen.insert(id.clone(), (path.to_owned(), row.line));
                }
            }
        }
    }
}

fn lint_parsed(
    path: &str,
    day: Option<NaiveDate>,
    line: usize,
    row: &ParsedRow,
    config: Option<&BankConfig>,
    findings: &mut Vec<Finding>,
) {
    if !matches!(row.status, ' ' | 'x' | 'X') {
        findings.push(finding(
            "bank_row_status",
            Severity::Error,
            path,
            Some(line),
            format!("checkbox status {:?} is not [ ], [x], or [X]", row.status),
            None,
            day,
        ));
    }
    if row.payee.is_empty() {
        findings.push(finding(
            "bank_row_payee_missing",
            Severity::Error,
            path,
            Some(line),
            "row has no payee",
            None,
            day,
        ));
    }
    if row.has_task_tag {
        findings.push(finding(
            "bank_row_task",
            Severity::Error,
            path,
            Some(line),
            "#task on a bank row",
            Some("Bank rows are never tasks; drop #task.".to_owned()),
            day,
        ));
    }
    if row.cents == 0 {
        findings.push(finding(
            "bank_row_zero",
            Severity::Warning,
            path,
            Some(line),
            "$0.00 row",
            Some(
                "A day with nothing posted needs no row; completing the chore is the record."
                    .to_owned(),
            ),
            day,
        ));
    }
    for (key, _) in &row.extra_fields {
        findings.push(finding(
            "bank_field_unknown",
            Severity::Warning,
            path,
            Some(line),
            format!("unknown inline field {key:?}"),
            Some("did you mean cat, acct?".to_owned()),
            day,
        ));
    }
    if row.flagged {
        findings.push(finding(
            "bank_flagged",
            Severity::Info,
            path,
            Some(line),
            format!("flagged row {}", row.payee),
            None,
            day,
        ));
    }
    if let Some(category) = &row.category
        && let Some(config) = config
    {
        if config.category(category).is_none() {
            let keys = config
                .categories
                .iter()
                .map(|category| category.key.clone())
                .collect::<Vec<_>>();
            findings.push(finding(
                "bank_category_unknown",
                Severity::Error,
                path,
                Some(line),
                format!("unknown category {category:?}"),
                suggest_name(category, &keys),
                day,
            ));
        } else if row.cents > 0 && !SPENDING_EXEMPT.contains(&category.as_str())
        {
            findings.push(finding(
                "bank_inflow_category",
                Severity::Info,
                path,
                Some(line),
                format!("inflow categorized as {category:?}"),
                Some("Income and refunds use income or refund.".to_owned()),
                day,
            ));
        }
    }
    if let Some(account) = &row.account
        && let Some(config) = config
        && config.account(account).is_none()
    {
        let aliases = config
            .accounts
            .iter()
            .map(|account| account.alias.clone())
            .collect::<Vec<_>>();
        findings.push(finding(
            "bank_account_unknown",
            Severity::Error,
            path,
            Some(line),
            format!("unknown account {account:?}"),
            suggest_name(account, &aliases),
            day,
        ));
    }
}

fn config_finding(diagnostic: ConfigDiagnostic) -> Finding {
    finding(
        "bank_config_invalid",
        Severity::Error,
        "bank.md",
        diagnostic.line,
        format!("{}: {}", diagnostic.key_path, diagnostic.message),
        diagnostic.hint,
        None,
    )
}

fn finding(
    code: &'static str,
    severity: Severity,
    path: impl Into<String>,
    line: Option<usize>,
    message: impl Into<String>,
    hint: Option<String>,
    day: Option<NaiveDate>,
) -> Finding {
    Finding {
        code,
        severity,
        path: path.into(),
        line,
        message: message.into(),
        hint,
        day,
    }
}

fn finding_json(finding: &Finding) -> Value {
    json!({
        "code": finding.code,
        "severity": finding.severity.as_str(),
        "path": finding.path,
        "line": finding.line,
        "message": finding.message,
        "hint": finding.hint,
    })
}

fn category_json(category: &Category) -> Value {
    json!({
        "key": category.key,
        "emoji": category.emoji,
        "label": category.label,
        "builtin": category.builtin,
    })
}

fn account_json(account: &AccountStatus) -> Value {
    json!({
        "alias": account.alias,
        "label": account.label,
        "kind": account.kind.as_str(),
        "default": account.default,
        "start": account.start.map(|date| date.format("%Y-%m-%d").to_string()),
        "status": account.status,
        "last_import": Value::Null,
        "balance_cents": Value::Null,
        "balance_asof": Value::Null,
        "delta_cents": Value::Null,
        "hint": account.hint,
    })
}
