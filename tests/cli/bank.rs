//! End-to-end `bob bank` walkthrough against a temporary vault.

use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const NOW: &str = "2026-11-12T12:00:00";
const HOST: &str = "bank-e2e";

const HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-12\n    cash:\n      label: Pocket cash\n      kind: cash\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n  rules:\n    - match: VENMO\n      cat: transfer\n---\n\n# 🏦 Bank\n\n```bob-bank\nview: queue\n```\n";

const TWO_ACCOUNT_HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-12\n    card:\n      label: Credit card\n      kind: credit\n      start: 2026-11-12\n    cash:\n      label: Pocket cash\n      kind: cash\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n---\n\n# 🏦 Bank\n";

const EARLY_START_HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n---\n\n# 🏦 Bank\n";

struct BankVault {
    _temp: TempDir,
    path: PathBuf,
}

fn allow_journal(vault: &Path) {
    write_file(vault.join(".gitignore").as_path(), "!/.bob/bank/*.jsonl\n");
    write_file(
        vault.join(".gitattributes").as_path(),
        ".bob/bank/*.jsonl merge=union\n",
    );
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q"]);
}

fn syncable(hub: &str) -> BankVault {
    let temp = TempDir::new("bob-cli-bank-e2e");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    allow_journal(&vault);
    write_file(&vault.join("bank.md"), hub);
    commit_all(&vault, "bank hub");
    BankVault {
        _temp: temp,
        path: vault,
    }
}

fn bank_command(vault: &BankVault) -> Command {
    let home = vault._temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("home");
    let mut command = bob_command();
    command
        .arg("bank")
        .env("BOB_DIR", &vault.path)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("HOSTNAME", HOST)
        .env("HOME", home)
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            vault_sync_lock_file(&vault._temp),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            vault_sync_state_file(&vault._temp),
        )
        .env("XDG_CACHE_HOME", vault._temp.path().join("cache"));
    command
}

fn run(vault: &BankVault, args: &[&str]) -> Output {
    let mut command = bank_command(vault);
    command.args(args);
    command.output().expect("run bob bank")
}

fn run_at(vault: &BankVault, now: &str, args: &[&str]) -> Output {
    let mut command = bank_command(vault);
    command.env("BOB_NOW", now);
    command.args(args);
    command.output().expect("run bob bank")
}

fn json(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("expected JSON ({error}):\n{}", format_output(output))
    })
}

fn ofx(
    start: &str,
    end: &str,
    balance: &str,
    asof: &str,
    txns: &[(&str, &str, &str, &str, &str)],
) -> String {
    let mut body = String::new();
    for (trntype, posted, amount, fitid, name) in txns {
        body.push_str(&format!(
            "<STMTTRN>\n<TRNTYPE>{trntype}\n\
<DTPOSTED>{posted}040000.000[-8:PST]\n<TRNAMT>{amount}\n\
<FITID>{fitid}\n<NAME>{name}\n</STMTTRN>\n"
        ));
    }
    format!(
        "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\n\
ENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\n\
NEWFILEUID:NONE\n\n<OFX>\n<BANKMSGSRSV1>\n<STMTTRNRS>\n<STMTRS>\n\
<CURDEF>USD\n<BANKACCTFROM>\n<BANKID>121000248\n<ACCTID>1234567890\n\
<ACCTTYPE>CHECKING\n</BANKACCTFROM>\n<BANKTRANLIST>\n<DTSTART>{start}\n\
<DTEND>{end}\n{body}</BANKTRANLIST>\n<LEDGERBAL>\n<BALAMT>{balance}\n\
<DTASOF>{asof}\n</LEDGERBAL>\n</STMTRS>\n</STMTTRNRS>\n</BANKMSGSRSV1>\n</OFX>\n"
    )
}

fn write_qfx(vault: &BankVault, name: &str, body: &str) -> PathBuf {
    let path = vault._temp.path().join(name);
    fs::write(&path, body).expect("write synthetic QFX");
    path
}

fn month_note(vault: &BankVault) -> String {
    fs::read_to_string(vault.path.join("bank/2026-11.md")).unwrap_or_default()
}

fn append_month_rows(vault: &Path, rows: &str) {
    let path = vault.join("bank/2026-11.md");
    let mut body = fs::read_to_string(&path).expect("month note");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    if !body.ends_with("\n\n") {
        body.push('\n');
    }
    body.push_str(rows);
    if !body.ends_with('\n') {
        body.push('\n');
    }
    fs::write(path, body).expect("append rows");
}

fn append_rows(vault: &BankVault, rows: &str) {
    append_month_rows(&vault.path, rows);
}

fn findings(payload: &Value) -> Vec<String> {
    payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|finding| finding["code"].as_str().map(str::to_owned))
        .collect()
}

fn waiting_ids(payload: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for day in payload["days"].as_array().unwrap() {
        for row in day["rows"].as_array().unwrap() {
            if let Some(id) = row["id"].as_str() {
                ids.push(id.to_owned());
            }
        }
    }
    ids
}

fn ritual_rows() -> &'static str {
    "- [x] $13.71 Smashburger [cat:: dining]\n  - split with Kelly\n- [x] $4.25 7-Eleven ⚑ — coffee on the walk\n- [x] $12.00 Coffee cart [acct:: cash]\n- [ ] $61.20 ShopRite [cat:: groceries]\n- [x] +$1,250.00 Payroll [cat:: income]\n- [x] $\n"
}

#[test]
fn walk_ensure_type_list_report_ack_and_import_twice() {
    let vault = syncable(HUB);

    let ensured = run(&vault, &["ensure", "-f", "json"]);
    assert_success(&ensured);
    let payload = json(&ensured);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["path"], "bank/2026-11.md");
    assert_eq!(payload["day"], "2026-11-12");
    assert_eq!(payload["created_note"], true);
    assert!(month_note(&vault).contains("## Thu Nov 12"));
    assert!(month_note(&vault).contains("view: month"));

    append_rows(&vault, ritual_rows());
    let checked = run(&vault, &["check", "-f", "json"]);
    assert_success(&checked);
    let payload = json(&checked);
    assert_eq!(payload["ok"], true);
    let codes = findings(&payload);
    assert!(
        codes.contains(&"bank_flagged".to_owned()),
        "flagged row should surface: {codes:?}"
    );
    assert!(
        !codes.iter().any(|code| code == "bank_row_unparsed"
            || code == "bank_row_payee_missing"),
        "empty stub must be ignored: {codes:?}"
    );

    let listed = run(&vault, &["list", "-f", "json"]);
    assert_success(&listed);
    let payload = json(&listed);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["filter"], "waiting");
    assert_eq!(payload["counts"]["waiting"], 1);
    let human = run(&vault, &["list"]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("ShopRite"), "{text}");
    assert!(text.contains("1 to acknowledge"), "{text}");

    for args in [
        ["report", "-m", "2026-11", "-f", "json"].as_slice(),
        ["report", "-w", "2026-W46", "-f", "json"].as_slice(),
        ["report", "-D", "2026-11-12", "-f", "json"].as_slice(),
    ] {
        let output = run(&vault, args);
        assert_success(&output);
        let payload = json(&output);
        assert_eq!(payload["schema_version"], 1);
        assert!(
            payload["totals"]["out_cents"].as_i64().unwrap() > 0,
            "{}",
            format_output(&output)
        );
    }
    let month = json(&run(&vault, &["report", "-m", "2026-11", "-f", "json"]));
    // Smashburger + 7-Eleven + waiting ShopRite. Cash-account coffee is traced, not Out.
    assert_eq!(month["totals"]["out_cents"], 1371 + 425 + 6120);
    assert_eq!(month["totals"]["in_cents"], 125_000);
    assert_eq!(month["totals"]["cash_traced_cents"], 1200);
    assert_eq!(month["accounts"][0]["status"], "unverified");
    assert_eq!(month["accounts"][1]["status"], "untracked");

    let acked = run(&vault, &["ack", "-D", "2026-11-12", "-f", "json"]);
    assert_success(&acked);
    let payload = json(&acked);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["acknowledged"].as_array().unwrap().len(), 1);
    assert!(month_note(&vault).contains("- [x] $61.20 ShopRite"));

    commit_all(&vault.path, "typed and acked rows");

    let qfx = write_qfx(
        &vault,
        "checking.qfx",
        &ofx(
            "20261112",
            "20261112",
            "1000.00",
            "20261112",
            &[
                ("DEBIT", "20261112", "-13.71", "fit-smash-1", "SMASHBURGER"),
                ("DEBIT", "20261112", "-61.20", "fit-shop-1", "SHOPRITE"),
                ("CREDIT", "20261112", "1250.00", "fit-pay-1", "PAYROLL"),
                ("DEBIT", "20261112", "-63.50", "fit-atm-1", "ATM WITHDRAWAL"),
            ],
        ),
    );
    let imported = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&qfx)],
    );
    assert_success(&imported);
    let payload = json(&imported);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["files"][0]["new"], 1);
    assert_eq!(payload["files"][0]["matched"], 3);
    assert_eq!(payload["files"][0]["seen"], 0);
    assert_eq!(
        payload["commit"]["subject"],
        "bank: import 1 rows (checking)"
    );
    let note = month_note(&vault);
    assert!(note.contains("^bank-"), "{note}");
    assert!(note.contains("ATM withdrawal"), "{note}");
    assert!(note.contains("[cat:: cash]"), "{note}");
    assert!(note.contains("- [x] $13.71 Smashburger"), "{note}");

    let waiting = json(&run(&vault, &["list", "-f", "json"]));
    let ids = waiting_ids(&waiting);
    assert_eq!(ids.len(), 1, "{waiting}");
    let acked = run(&vault, &["ack", "-f", "json", &ids[0]]);
    assert_success(&acked);
    assert_eq!(json(&acked)["acknowledged"].as_array().unwrap().len(), 1);

    let replay = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&qfx)],
    );
    assert_success(&replay);
    let payload = json(&replay);
    assert_eq!(payload["files"][0]["new"], 0);
    assert_eq!(payload["files"][0]["seen"], 4);
    assert!(payload["commit"].is_null(), "{payload}");
    let status = json(&run(&vault, &["report", "-f", "json"]));
    assert_eq!(status["accounts"][0]["status"], "balanced");
}

#[test]
fn typed_twin_amount_mismatch_and_ambiguous_tie() {
    let vault = syncable(EARLY_START_HUB);
    let ensured = run(&vault, &["ensure", "-D", "2026-11-12", "-f", "json"]);
    assert_success(&ensured);
    append_rows(&vault, "- [x] $20.00 Tab\n");
    let _ = run(&vault, &["ensure", "-D", "2026-11-11", "-f", "json"]);
    let _ = run(&vault, &["ensure", "-D", "2026-11-13", "-f", "json"]);
    let mut body = month_note(&vault);
    body = body.replacen(
        "## Wed Nov 11\n",
        "## Wed Nov 11\n\n- [x] $5.00 Coffee\n",
        1,
    );
    body = body.replacen(
        "## Fri Nov 13\n",
        "## Fri Nov 13\n\n- [x] $5.00 Coffee\n",
        1,
    );
    fs::write(vault.path.join("bank/2026-11.md"), body).unwrap();
    commit_all(&vault.path, "typed twins and coffee");

    let twins = write_qfx(
        &vault,
        "twins.qfx",
        &ofx(
            "20261111",
            "20261113",
            "900.00",
            "20261113",
            &[
                ("DEBIT", "20261112", "-24.00", "fit-tab-24", "TAB"),
                ("DEBIT", "20261112", "-5.00", "fit-coffee-1", "COFFEE"),
            ],
        ),
    );
    let imported = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&twins)],
    );
    assert_success(&imported);
    let payload = json(&imported);
    assert_eq!(payload["files"][0]["new"], 2, "{payload}");
    assert!(
        payload["files"][0]["ambiguous"].as_u64().unwrap() >= 1,
        "equidistant $5 Coffee rows should not be guessed: {payload}"
    );
    let note = month_note(&vault);
    assert!(note.contains("$20.00 Tab"), "{note}");
    assert!(note.contains("$24.00"), "{note}");
    assert!(
        note.matches("$5.00 Coffee").count() >= 3,
        "typed pair plus the imported ambiguous row:\n{note}"
    );

    let checked =
        run_at(&vault, "2026-11-13T12:00:00", &["check", "-f", "json"]);
    let codes = findings(&json(&checked));
    assert!(
        codes.contains(&"bank_twins".to_owned()),
        "typed $20 vs posted $24 must list twins: {codes:?}"
    );
}

#[test]
fn deleted_row_shows_delta_then_force_restores() {
    let vault = syncable(HUB);
    assert_success(&run(&vault, &["ensure", "-f", "json"]));
    commit_all(&vault.path, "month note");
    let qfx = write_qfx(
        &vault,
        "one.qfx",
        &ofx(
            "20261112",
            "20261112",
            "1000.00",
            "20261112",
            &[("DEBIT", "20261112", "-13.71", "fit-del-1", "SMASHBURGER")],
        ),
    );
    let imported = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&qfx)],
    );
    assert_success(&imported);
    assert_eq!(json(&imported)["files"][0]["new"], 1);

    let mut body = month_note(&vault);
    let before = body.clone();
    body = body
        .lines()
        .filter(|line| {
            !line.contains("Smashburger") && !line.contains("^bank-")
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    assert_ne!(body, before, "the imported row must actually be removed");
    fs::write(vault.path.join("bank/2026-11.md"), body).unwrap();
    commit_all(&vault.path, "delete imported row");

    let checked = run(&vault, &["check", "-f", "json"]);
    assert_eq!(
        checked.status.code(),
        Some(1),
        "{}",
        format_output(&checked)
    );
    let payload = json(&checked);
    let codes = findings(&payload);
    assert!(codes.contains(&"bank_row_missing".to_owned()), "{codes:?}");
    assert!(
        codes.contains(&"bank_balance_delta".to_owned()),
        "{codes:?}"
    );
    let report = json(&run(&vault, &["report", "-f", "json"]));
    assert_eq!(report["accounts"][0]["status"], "delta");

    let restored = run(&vault, &["import", "-F", "-f", "json"]);
    assert_success(&restored);
    let payload = json(&restored);
    assert_eq!(payload["restored"], 1, "{payload}");
    assert!(month_note(&vault).contains("Smashburger"));
}

#[test]
fn skipped_week_is_a_named_gap() {
    let vault = syncable(EARLY_START_HUB);
    assert_success(&run(&vault, &["ensure", "-D", "2026-11-01", "-f", "json"]));
    commit_all(&vault.path, "month note");
    let first = write_qfx(
        &vault,
        "week1.qfx",
        &ofx(
            "20261101",
            "20261107",
            "1000.00",
            "20261107",
            &[("DEBIT", "20261103", "-10.00", "fit-w1", "SHOP ONE")],
        ),
    );
    assert_success(&run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&first)],
    ));
    let second = write_qfx(
        &vault,
        "week3.qfx",
        &ofx(
            "20261115",
            "20261121",
            "980.00",
            "20261121",
            &[("DEBIT", "20261116", "-10.00", "fit-w3", "SHOP TWO")],
        ),
    );
    let imported = run_at(
        &vault,
        "2026-11-22T12:00:00",
        &["import", "-a", "checking", "-f", "json", path_str(&second)],
    );
    assert_success(&imported);
    let report = json(&run_at(
        &vault,
        "2026-11-22T12:00:00",
        &["report", "-f", "json"],
    ));
    assert_eq!(report["accounts"][0]["status"], "gap", "{report}");
    let human = run_at(&vault, "2026-11-22T12:00:00", &["list"]);
    let text = stdout(&human);
    assert!(
        text.contains("coverage gap") || text.contains("gap"),
        "empty waiting must still name the gap:\n{text}"
    );
    let checked = json(&run_at(
        &vault,
        "2026-11-22T12:00:00",
        &["check", "-f", "json"],
    ));
    assert!(
        findings(&checked).contains(&"bank_coverage_gap".to_owned()),
        "{checked}"
    );
}

#[test]
fn card_payment_nets_out_and_a_credit_is_never_income() {
    let temp = TempDir::new("bob-cli-bank-totals");
    let vault_path = temp.path().join("vault");
    fs::create_dir_all(&vault_path).unwrap();
    write_file(&vault_path.join("bank.md"), TWO_ACCOUNT_HUB);
    let mut command = bob_command();
    command
        .arg("bank")
        .env("BOB_DIR", &vault_path)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1");
    let ensured = command
        .args(["ensure", "-f", "json"])
        .output()
        .expect("ensure");
    assert_success(&ensured);
    append_month_rows(
        &vault_path,
        "- [x] +$2,000.00 Salary [cat:: income]\n- [x] +$15.00 RefundThing [cat:: refund]\n- [x] $100.00 Card payment [cat:: transfer]\n- [x] +$100.00 From checking [cat:: transfer] [acct:: card]\n- [x] +$9.99 Mystery inflow\n- [x] $13.71 Smashburger [cat:: dining]\n",
    );
    let mut report = bob_command();
    let output = report
        .arg("bank")
        .args(["report", "-m", "2026-11", "-f", "json"])
        .env("BOB_DIR", &vault_path)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .output()
        .expect("report");
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["totals"]["out_cents"], 1371);
    assert_eq!(payload["totals"]["in_cents"], 200_000 + 1_500);
    assert_eq!(payload["totals"]["refund_cents"], 1_500);
    assert_eq!(payload["totals"]["transfer_out_cents"], 10_000);
    assert_eq!(payload["totals"]["transfer_in_cents"], 10_000);
    assert_eq!(payload["totals"]["unclassified_in_cents"], 999);
}

#[test]
fn malformed_file_refuses_the_batch() {
    let vault = syncable(HUB);
    assert_success(&run(&vault, &["ensure", "-f", "json"]));
    commit_all(&vault.path, "month note");
    let head = stdout(&git_in(&vault.path, ["rev-parse", "HEAD"]));
    let note_before = month_note(&vault);
    let bad = write_qfx(
        &vault,
        "truncated.qfx",
        "<OFX><STMTRS><BANKTRANLIST><DTSTART>20261101</DTSTART>\
<DTEND>20261112</DTEND><STMTTRN><TRNAMT>-1.00</TRNAMT><FITID>x</FITID>",
    );
    let refused = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&bad)],
    );
    assert_eq!(
        refused.status.code(),
        Some(2),
        "{}",
        format_output(&refused)
    );
    assert_eq!(month_note(&vault), note_before);
    assert!(!vault.path.join(".bob/bank").exists());
    assert_eq!(stdout(&git_in(&vault.path, ["rev-parse", "HEAD"])), head);
}

const MAPPING_HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n  categories:\n    coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }\n    kids: { label: 🧸 Kids, hidden: true }\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n---\n\n# 🏦 Bank\n";

#[test]
fn mapping_categories_report_hotkeys_and_hidden_stays_valid() {
    let vault = syncable(MAPPING_HUB);
    assert_success(&run(&vault, &["ensure", "-f", "json"]));
    append_rows(
        &vault,
        "- [x] $10.00 Toy Store [cat:: kids]\n- [x] $4.00 Cafe Nero [cat:: coffee]\n- [ ] $5.00 Cafe Nero\n",
    );

    let checked = run(&vault, &["check", "-f", "json"]);
    assert_success(&checked);
    let payload = json(&checked);
    assert_eq!(payload["ok"], true);
    assert!(
        !findings(&payload)
            .iter()
            .any(|code| code == "bank_category_unknown"),
        "{payload}"
    );
    let kids = payload["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|category| category["key"] == "kids")
        .unwrap();
    assert_eq!(kids["hidden"], true);
    assert!(kids["hotkey"].is_null());
    let coffee = payload["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|category| category["key"] == "coffee")
        .unwrap();
    assert_eq!(coffee["hotkey"], "c");
    assert_eq!(coffee["group"], "Everyday");

    let listed = json(&run(&vault, &["list", "-a", "-f", "json"]));
    assert_eq!(listed["categories"][0]["key"], "coffee");
    assert_eq!(listed["categories"][0]["hotkey"], "c");
    let waiting = listed["days"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|day| day["rows"].as_array().unwrap())
        .find(|row| row["acknowledged"] == false)
        .unwrap();
    assert_eq!(waiting["payee"], "Cafe Nero");
    assert_eq!(waiting["suggestion"]["category"], "coffee");
    assert_eq!(waiting["suggestion"]["source"], "learned");

    let report = json(&run(&vault, &["report", "-m", "2026-11", "-f", "json"]));
    assert_eq!(report["totals"]["out_cents"], 1000 + 400 + 500);
}

#[test]
fn config_show_reports_effective_setup_usage_and_fingerprint() {
    let vault = syncable(HUB);
    fs::create_dir_all(vault.path.join("bank")).expect("bank folder");
    write_file(
        &vault.path.join("bank/2026-11.md"),
        "---\nmonth: 2026-11\n---\n\n# November 2026\n\n## Thu Nov 12\n\n- [ ] $10.00 Groceries [cat:: groceries]\n- [ ] $1.00 Pocket cash [acct:: cash] [cat:: dining]\n",
    );

    let shown = run(&vault, &["config", "show", "-f", "json"]);
    assert_success(&shown);
    let payload = json(&shown);
    assert_eq!(payload["status"], "valid");
    assert_eq!(payload["teaching_empty"], false);
    assert!(payload["config_fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(payload["categories"].as_array().unwrap().len() >= 6);
    assert_eq!(payload["categories"][0]["usage"]["rows"], 1);
    assert_eq!(payload["categories"][1]["usage"]["rows"], 1);
    assert_eq!(payload["accounts"][0]["usage"]["default_rows"], 1);
    assert_eq!(payload["accounts"][1]["usage"]["explicit_rows"], 1);
    assert_eq!(payload["rules"].as_array().unwrap().len(), 1);
    assert_eq!(payload["stale_after"]["days"], 10);
    assert_eq!(payload["stale_after"]["explicit"], false);
    assert!(payload["free_hotkeys"]
        .as_array()
        .unwrap()
        .iter()
        .all(|key| key != "q"));

    let listed = json(&run(&vault, &["list", "-f", "json"]));
    let reported = json(&run(&vault, &["report", "-f", "json"]));
    assert_eq!(listed["config_fingerprint"], payload["config_fingerprint"]);
    assert_eq!(
        reported["config_fingerprint"],
        payload["config_fingerprint"]
    );

    let human = run(&vault, &["config"]);
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    assert_text_order(
        &stdout(&human),
        &[
            "Bank setup · valid",
            "Accounts",
            "Categories",
            "Free keys",
            "Rules",
            "Stale after",
        ],
    );
}

#[test]
fn invalid_rule_diagnostics_keep_all_lines_in_read_envelopes() {
    let hub = concat!(
        "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n",
        "  accounts:\n    checking:\n      label: Checking\n",
        "      kind: checking\n      start: 2026-11-01\n",
        "  categories:\n    coffee: ☕ Coffee\n  rules:\n",
        "    - match: FIRST\n      cat: missing_one\n",
        "    - { match: SECOND, cat: missing_two }\n---\n# Bank\n",
    );
    let vault = syncable(hub);

    for args in [
        ["list", "-f", "json"].as_slice(),
        ["report", "-f", "json"].as_slice(),
    ] {
        let output = run(&vault, args);
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        let payload = json(&output);
        assert_eq!(
            payload["error"],
            "bank.rules.0.cat: unknown category \"missing_one\""
        );
        assert_eq!(payload["findings"].as_array().unwrap().len(), 2);
        assert_eq!(payload["findings"][0]["key_path"], "bank.rules.0.cat");
        assert_eq!(payload["findings"][0]["line"], 14);
        assert_eq!(payload["findings"][1]["key_path"], "bank.rules.1.cat");
        assert_eq!(payload["findings"][1]["line"], 15);
        assert!(payload["config_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    let checked = run(&vault, &["check", "-f", "json"]);
    assert_eq!(
        checked.status.code(),
        Some(1),
        "{}",
        format_output(&checked)
    );
    let payload = json(&checked);
    let config_findings = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["code"] == "bank_config_invalid")
        .collect::<Vec<_>>();
    assert_eq!(config_findings.len(), 2);
    assert_eq!(config_findings[1]["key_path"], "bank.rules.1.cat");
    assert_eq!(config_findings[1]["line"], 15);

    let ensured = run(&vault, &["ensure", "-f", "json"]);
    assert_eq!(
        ensured.status.code(),
        Some(2),
        "{}",
        format_output(&ensured)
    );
    assert_eq!(json(&ensured)["findings"].as_array().unwrap().len(), 2);
}

#[test]
fn rule_with_masked_digits_is_a_warning_in_check_and_config_report() {
    let hub = HUB.replace(
        "- match: VENMO\n      cat: transfer",
        "- match: VENMO 1234\n      cat: transfer",
    );
    let vault = syncable(&hub);
    let checked = run(&vault, &["check", "-f", "json"]);
    assert_success(&checked);
    let checked_json = json(&checked);
    let warning = checked_json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["code"] == "bank_rule_read_time_dead")
        .expect("read-time-dead rule warning");
    assert_eq!(warning["severity"], "warning");
    assert_eq!(warning["key_path"], "bank.rules.0.match");
    let config = json(&run(&vault, &["config", "show", "-f", "json"]));
    assert!(config["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "bank_rule_read_time_dead"));
}

#[test]
fn import_writes_shared_suggestion_and_points_next_at_the_walk() {
    let vault = syncable(MAPPING_HUB);
    assert_success(&run(&vault, &["ensure", "-f", "json"]));
    commit_all(&vault.path, "month note");
    let qfx = write_qfx(
        &vault,
        "checking.qfx",
        &ofx(
            "20261112",
            "20261112",
            "1000.00",
            "20261112",
            &[
                (
                    "ATM",
                    "20261112",
                    "-63.50",
                    "fit-atm-walk-1",
                    "ATM WITHDRAWAL",
                ),
                ("DEBIT", "20261112", "-4.00", "fit-cafe-walk-1", "CAFE NERO"),
            ],
        ),
    );
    let imported = run(&vault, &["import", "-a", "checking", path_str(&qfx)]);
    assert_success(&imported);
    let text = stdout(&imported);
    assert!(
        text.contains(
            "Next: file the 2 waiting rows in bank/2026-11.md — ]b walks them oldest first, Ctrl+Enter files each (or bob bank to list them)."
        ),
        "{text}"
    );

    let listed = json(&run(&vault, &["list", "-a", "-f", "json"]));
    let atm = listed["days"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|day| day["rows"].as_array().unwrap())
        .find(|row| row["payee"] == "ATM withdrawal")
        .unwrap();
    assert_eq!(atm["category"], "cash");
    assert_eq!(atm["suggestion"]["category"], "cash");
    assert_eq!(atm["suggestion"]["source"], "bank");
    assert_eq!(atm["category"], atm["suggestion"]["category"]);
}

const CHARTS_HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n  categories:\n    groceries: 🛒 Groceries\n    dining: 🍽️ Dining\n---\n\n# 🏦 Bank\n";

const CHARTS_MONTH: &str = "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: 2026-11\ncreated: 2026-11-12T07:30:00-05:00\n---\n\n# 🏦 November 2026\n\n```bob-bank\nview: month\n```\n\n## Wed Nov 11\n\n- [x] $13.71 Smashburger [cat:: dining]\n- [x] +$15.00 Refund [cat:: refund]\n- [x] $100.00 Card payment [cat:: transfer]\n\n## Thu Nov 12\n\n- [ ] $61.20 ShopRite [cat:: groceries]\n- [x] +$1,250.00 Payroll [cat:: income]\n";

fn charts_vault() -> (TempDir, PathBuf) {
    let temp = TempDir::new("bob-cli-bank-charts");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("bank")).expect("bank folder");
    write_file(&vault.join("bank.md"), CHARTS_HUB);
    write_file(&vault.join("bank/2026-11.md"), CHARTS_MONTH);
    (temp, vault)
}

fn run_charts(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("bank")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob bank report")
}

#[test]
fn report_charts_aggregate_rides_the_strip_axis() {
    let (_temp, vault) = charts_vault();
    let charted = run_charts(
        &vault,
        &["report", "-p", "2026-W46", "-s", "-c", "-f", "json"],
    );
    assert_success(&charted);
    let mut payload = json(&charted);
    assert_eq!(payload["days"], serde_json::json!([]));
    let charts = payload.as_object_mut().unwrap().remove("charts").unwrap();
    assert_eq!(charts["version"], 1);
    assert_eq!(charts["granularity"], "day");
    let buckets = charts["buckets"].as_array().unwrap();
    let ids: Vec<&str> = buckets
        .iter()
        .map(|bucket| bucket["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "2026-11-09",
            "2026-11-10",
            "2026-11-11",
            "2026-11-12",
            "2026-11-13",
            "2026-11-14",
            "2026-11-15"
        ]
    );
    assert_eq!(buckets[2]["label"], "Wed 11");
    assert_eq!(buckets[2]["out_cents"], 1371);
    assert_eq!(buckets[2]["in_cents"], 1500);
    assert_eq!(buckets[2]["rows"], 3);
    assert_eq!(buckets[3]["out_cents"], 6120);
    assert_eq!(buckets[3]["in_cents"], 125_000);
    assert_eq!(buckets[3]["waiting"], 1);
    assert_eq!(buckets[3]["so_far"], true);
    // Never imported: every elapsed day is awaiting import, later days are
    // future, and the money stays.
    assert_eq!(buckets[3]["state"], "awaiting_import");
    assert_eq!(buckets[4]["state"], "future");
    // Day targets are the month note's day headings, or null.
    let heading_line = CHARTS_MONTH
        .lines()
        .position(|line| line == "## Wed Nov 11")
        .unwrap()
        + 1;
    assert_eq!(buckets[2]["target"]["path"], "bank/2026-11.md");
    assert_eq!(buckets[2]["target"]["line"], heading_line);
    assert!(buckets[0]["target"].is_null());
    let out: i64 = buckets
        .iter()
        .map(|bucket| bucket["out_cents"].as_i64().unwrap())
        .sum();
    assert_eq!(out, payload["totals"]["out_cents"].as_i64().unwrap());
    assert_eq!(charts["categories"]["series"][0]["key"], "groceries");
    assert!(charts["categories"]["rest"].is_null());

    // Without -c the payload is the same minus the charts key, and -w
    // charts the same week as -p.
    let plain = json(&run_charts(
        &vault,
        &["report", "-p", "2026-W46", "-s", "-f", "json"],
    ));
    assert!(plain.get("charts").is_none());
    assert_eq!(payload, plain);
    let week = json(&run_charts(
        &vault,
        &["report", "-w", "2026-W46", "-c", "-f", "json"],
    ));
    assert_eq!(week["charts"]["buckets"], charts["buckets"]);
    assert_eq!(week["days"].as_array().unwrap().len(), 7);

    let quarter = json(&run_charts(
        &vault,
        &["report", "-p", "2026-Q4", "-s", "-c", "-f", "json"],
    ));
    assert_eq!(quarter["charts"]["granularity"], "week");
    assert_eq!(quarter["charts"]["buckets"].as_array().unwrap().len(), 14);
    assert_eq!(quarter["charts"]["buckets"][6]["id"], "2026-W46");
    assert_eq!(
        quarter["charts"]["buckets"][6]["target"],
        serde_json::json!({"path": "2026/2026-W46.md", "line": 1})
    );
    let year = json(&run_charts(
        &vault,
        &["report", "-p", "2026", "-s", "-c", "-f", "json"],
    ));
    assert_eq!(year["charts"]["buckets"].as_array().unwrap().len(), 53);
    assert_eq!(year["charts"]["groups"].as_array().unwrap().len(), 4);
    let all = json(&run_charts(
        &vault,
        &["report", "-p", "all", "-s", "-c", "-f", "json"],
    ));
    assert_eq!(all["charts"]["granularity"], "quarter");
    assert_eq!(all["charts"]["buckets"][0]["id"], "2026-Q4");
    assert_eq!(all["charts"]["buckets"][0]["start"], "2026-11-01");

    let human = run_charts(&vault, &["report", "-w", "2026-W46", "-c"]);
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    let text = stdout(&human);
    assert_text_order(
        &text,
        &["By day", "Mon 9", "Wed 11", "Thu 12", "Sun 15"],
    );
    let today = text
        .lines()
        .find(|line| line.starts_with("Thu 12"))
        .unwrap();
    assert!(today.contains("+$1,250.00"), "{text}");
    assert!(today.contains("$61.20"), "{text}");
    assert!(today.contains("1 waiting"), "{text}");
    assert!(today.contains("so far · not imported yet"), "{text}");
    let friday = text
        .lines()
        .find(|line| line.starts_with("Fri 13"))
        .unwrap();
    assert!(friday.ends_with("future"), "{text}");
}

#[test]
fn report_charts_refuses_periods_without_children() {
    let (_temp, vault) = charts_vault();
    let message =
        "-c/--charts needs a week, quarter, ISO year, or all-time period (-w or -p)";
    for args in [
        ["report", "-c"].as_slice(),
        ["report", "-c", "-m", "2026-11"].as_slice(),
        ["report", "-c", "-D", "today"].as_slice(),
        ["report", "-c", "-y", "2026"].as_slice(),
        ["report", "-c", "-p", "2026-11-12"].as_slice(),
    ] {
        let human = run_charts(&vault, args);
        assert_eq!(human.status.code(), Some(2), "{}", format_output(&human));
        assert!(stdout(&human).is_empty(), "{}", format_output(&human));
        assert!(
            stderr(&human).contains(&format!("bob bank report: {message}")),
            "{}",
            format_output(&human)
        );
        let mut json_args = args.to_vec();
        json_args.extend(["-f", "json"]);
        let refused = run_charts(&vault, &json_args);
        assert_eq!(
            refused.status.code(),
            Some(2),
            "{}",
            format_output(&refused)
        );
        let payload = json(&refused);
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["error"], message);
    }
}

const SUB_HUB: &str = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n    cash:\n      label: Pocket cash\n      kind: cash\n  categories:\n    streaming: Streaming\n    dining: 🍽️ Dining\n    kids: { label: 🧸 Kids, hidden: true }\n  rules:\n    - match: TUNELY\n      cat: dining\n---\n\n# 🏦 Bank\n";

fn subscription_album(stem: &str, parent: &str, title: &str) -> String {
    format!(
        "---\ntype: [[album]]\nparent: \"[[{parent}]]\"\nemoji: 🔁\nrecord:\n  noun: subscription\n  label: Subscription\n  fields:\n    status:\n      required: true\n      values:\n        to_cancel: To cancel\n        to_decide: To decide\n        active: Active\n        cancelled: Cancelled\n    amount:\n      required: true\n      values: number\n      min: 0\n    cadence:\n      required: true\n      values:\n        monthly: Monthly\n        four_weekly: Every four weeks\n        quarterly: Quarterly\n        yearly: Yearly\n    pricing:\n      default: fixed\n      values:\n        fixed: Fixed\n        variable: Variable\n    match: {{list: true}}\n    category: {{suggest: true}}\n    url: {{values: url}}\n    renews: {{values: date}}\n    ended: {{values: date}}\n---\n# {title}\n"
    )
    .replace("billing", stem)
}

fn json_rows(payload: &Value) -> Vec<&Value> {
    if let Some(rows) = payload["rows"].as_array() {
        return rows.iter().collect();
    }
    let mut rows = Vec::new();
    if let Some(days) = payload["days"].as_array() {
        for day in days {
            if let Some(day_rows) = day["rows"].as_array() {
                rows.extend(day_rows.iter());
            }
        }
    }
    rows
}

fn row_with_payee<'a>(rows: &'a [&Value], needle: &str) -> &'a Value {
    rows.iter()
        .copied()
        .find(|row| {
            row["payee"]
                .as_str()
                .is_some_and(|payee| payee.contains(needle))
        })
        .unwrap_or_else(|| panic!("missing payee {needle} in {rows:?}"))
}

fn write_renamed_subscription_album(vault: &BankVault) {
    write_file(
        &vault.path.join("media.md"),
        &subscription_album("media", "bank", "Media"),
    );
    fs::create_dir_all(vault.path.join("media")).unwrap();
    write_file(
        &vault.path.join("media/tunely.md"),
        "---\ntitle: Tunely\nstatus: active\namount: 9.99\ncadence: monthly\nmatch: [TUNELY]\ncategory: streaming\n---\n# Tunely\n",
    );
    write_file(
        &vault.path.join("media/cloudbox.md"),
        "---\ntitle: Cloudbox\nstatus: active\namount: 19.99\ncadence: monthly\nmatch: [CLOUDBOX]\ncategory: kids\n---\n# Cloudbox\n",
    );
    write_file(
        &vault.path.join("restaurants.md"),
        "---\nparent: \"[[bank]]\"\ntype: [[album]]\nsummary: Restaurants.\nemoji: \"🍽️\"\nrecord:\n  noun: eat\n  label: Restaurant\n  fields: {}\n---\n\n# Restaurants\n",
    );
    fs::create_dir_all(vault.path.join("restaurants")).unwrap();
    write_file(
        &vault.path.join("restaurants/smashburger.md"),
        "---\ntitle: Smashburger\n---\n# Smashburger\n",
    );
}

fn tunely_ofx() -> String {
    ofx(
        "20261010",
        "20261112",
        "1000.00",
        "20261112",
        &[
            ("DEBIT", "20261010", "-10.99", "fit-tunely-oct", "TUNELY"),
            (
                "DEBIT",
                "20261112",
                "-10.99",
                "fit-tunely-nov",
                "TUNELY PREMIUM 1234 RECURRING PAYMENT",
            ),
            ("DEBIT", "20261112", "-19.99", "fit-cloudbox", "CLOUDBOX"),
            ("DEBIT", "20261112", "-13.71", "fit-smash", "SMASHBURGER"),
        ],
    )
}

fn assert_tunely_after_state(row: &Value) {
    let sub = &row["subscription"];
    assert_eq!(sub["stem"], "tunely", "{row}");
    assert_eq!(sub["path"], "media/tunely.md", "{row}");
    assert_eq!(sub["title"], "Tunely", "{row}");
    assert_eq!(sub["source"], "match", "{row}");
    assert_eq!(sub["needle"], "TUNELY", "{row}");
    assert_eq!(sub["amount_cents"], 999, "{row}");
    assert_eq!(sub["drift_cents"], 100, "{row}");
    assert_eq!(sub["price_change"]["from_cents"], 999, "{row}");
    assert_eq!(sub["price_change"]["to_cents"], 1099, "{row}");
    assert_eq!(sub["price_change"]["since"], "2026-11-12", "{row}");
    assert_eq!(row["bank_recurring"], true, "{row}");
    assert_eq!(row["subscription_candidate"], false, "{row}");
}

#[test]
fn import_preview_apply_list_and_report_agree_on_subscription_after_state() {
    let vault = syncable(SUB_HUB);
    write_renamed_subscription_album(&vault);
    assert_success(&run(&vault, &["ensure", "-D", "2026-11-12", "-f", "json"]));
    append_rows(&vault, "- [ ] $10.99 Tunely\n");
    commit_all(&vault.path, "subscription album and typed tunely");

    let qfx = write_qfx(&vault, "tunely.qfx", &tunely_ofx());
    let before_preview = month_note(&vault);
    let preview = run(
        &vault,
        &[
            "import",
            "-a",
            "checking",
            "-d",
            "-f",
            "json",
            path_str(&qfx),
        ],
    );
    assert_success(&preview);
    let preview_json = json(&preview);
    assert_eq!(preview_json["ok"], true);
    assert_eq!(preview_json["dry_run"], true);
    assert_eq!(preview_json["files"][0]["new"], 2, "{preview_json}");
    assert_eq!(preview_json["files"][0]["matched"], 1, "{preview_json}");
    assert_eq!(
        preview_json["files"][0]["before_start"], 1,
        "{preview_json}"
    );
    assert_eq!(month_note(&vault), before_preview, "dry-run is write-free");

    let preview_rows = json_rows(&preview_json);
    let tunely = row_with_payee(&preview_rows, "Tunely");
    assert_eq!(tunely["payee"], "Tunely");
    assert_eq!(tunely["origin"], "matched");
    assert_eq!(tunely["category"], Value::Null);
    assert_eq!(tunely["suggestion"]["source"], "rule");
    assert_eq!(tunely["suggestion"]["category"], "dining");
    assert_tunely_after_state(tunely);
    let cloudbox = row_with_payee(&preview_rows, "Cloudbox");
    assert_eq!(cloudbox["subscription"]["path"], "media/cloudbox.md");
    assert_eq!(cloudbox["suggestion"], Value::Null, "{cloudbox}");
    let smash = row_with_payee(&preview_rows, "Smashburger");
    assert_eq!(smash["subscription"], Value::Null);
    assert!(
        smash["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link.as_str() == Some("[[restaurants/smashburger|🍽️]]")),
        "{smash}"
    );

    let applied = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&qfx)],
    );
    assert_success(&applied);
    let applied_json = json(&applied);
    assert_eq!(applied_json["ok"], true);
    assert_eq!(applied_json["dry_run"], false);
    assert_eq!(applied_json["files"][0]["new"], 2, "{applied_json}");
    assert_eq!(applied_json["files"][0]["matched"], 1, "{applied_json}");
    let applied_rows = json_rows(&applied_json);
    let tunely = row_with_payee(&applied_rows, "Tunely");
    assert_eq!(tunely["payee"], "Tunely", "typed payee stays");
    assert_tunely_after_state(tunely);
    assert_eq!(tunely["suggestion"]["source"], "rule");
    let note = month_note(&vault);
    assert!(note.contains("- [ ] $10.99 Tunely"), "{note}");
    assert!(note.contains("[[restaurants/smashburger|🍽️]]"), "{note}");
    assert!(note.contains("^bank-"), "{note}");

    let listed = json(&run(&vault, &["list", "-a", "-f", "json"]));
    let reported =
        json(&run(&vault, &["report", "-m", "2026-11", "-f", "json"]));
    let list_rows = json_rows(&listed);
    let report_rows = json_rows(&reported);
    let list_tunely = row_with_payee(&list_rows, "Tunely");
    let report_tunely = row_with_payee(&report_rows, "Tunely");
    assert_tunely_after_state(list_tunely);
    assert_tunely_after_state(report_tunely);
    assert_eq!(list_tunely["suggestion"]["source"], "rule");
    assert_eq!(report_tunely["suggestion"]["source"], "rule");
    assert_eq!(
        row_with_payee(&list_rows, "Cloudbox")["suggestion"],
        Value::Null
    );
    let note_after_reads = month_note(&vault);
    assert_eq!(note_after_reads, note, "list and report are read-only");

    let replay = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&qfx)],
    );
    assert_success(&replay);
    let replay_json = json(&replay);
    assert_eq!(replay_json["files"][0]["new"], 0, "{replay_json}");
    assert_eq!(replay_json["files"][0]["seen"], 4, "{replay_json}");
    assert!(replay_json["commit"].is_null(), "{replay_json}");
    assert_eq!(month_note(&vault), note, "replay is byte-idempotent");

    let mut body = month_note(&vault);
    body = body
        .lines()
        .filter(|line| !line.contains("Tunely"))
        .collect::<Vec<_>>()
        .join("\n");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    fs::write(vault.path.join("bank/2026-11.md"), body).unwrap();
    commit_all(&vault.path, "delete tunely row");
    let restored = run(&vault, &["import", "-F", "-f", "json"]);
    assert_success(&restored);
    let restored_json = json(&restored);
    assert_eq!(restored_json["restored"], 1, "{restored_json}");
    let restored_row = restored_json["restored_rows"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(json_rows(&restored_json))
        .find(|row| {
            row["payee"]
                .as_str()
                .is_some_and(|payee| payee.contains("Tunely"))
                || row["subscription"]["path"] == "media/tunely.md"
        })
        .cloned()
        .unwrap_or_else(|| {
            let listed = json(&run(&vault, &["list", "-a", "-f", "json"]));
            row_with_payee(&json_rows(&listed), "Tunely").clone()
        });
    assert_eq!(restored_row["subscription"]["path"], "media/tunely.md");
    assert_eq!(
        restored_row["subscription"]["price_change"]["to_cents"],
        1099
    );
}

#[test]
fn missing_subscription_album_nulls_projection() {
    let vault = syncable(SUB_HUB);
    assert_success(&run(&vault, &["ensure", "-D", "2026-11-12", "-f", "json"]));
    commit_all(&vault.path, "month note");
    let qfx = write_qfx(
        &vault,
        "tunely.qfx",
        &ofx(
            "20261112",
            "20261112",
            "1000.00",
            "20261112",
            &[(
                "DEBIT",
                "20261112",
                "-10.99",
                "fit-tunely",
                "TUNELY PREMIUM 1234 RECURRING PAYMENT",
            )],
        ),
    );
    let imported = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&qfx)],
    );
    assert_success(&imported);
    let payload = json(&imported);
    let rows = json_rows(&payload);
    assert_eq!(rows[0]["subscription"], Value::Null, "{payload}");
    assert_eq!(rows[0]["bank_recurring"], false, "{payload}");
    assert_eq!(rows[0]["subscription_candidate"], false, "{payload}");
    let listed = json(&run(&vault, &["list", "-a", "-f", "json"]));
    assert_eq!(listed["warnings"].as_array().unwrap().len(), 0, "{listed}");
    let list_rows = json_rows(&listed);
    assert_eq!(list_rows[0]["subscription"], Value::Null);
}

#[test]
fn invalid_subscription_album_warns_once_and_nulls_projection() {
    let vault = syncable(SUB_HUB);
    write_file(
        &vault.path.join("media.md"),
        "---\ntype: [[album]]\nparent: \"[[bank]]\"\nrecord: {noun: subscription, fields: {amount: {values: [invalid]}}}\n---\n# Media\n",
    );
    assert_success(&run(&vault, &["ensure", "-D", "2026-11-12", "-f", "json"]));
    commit_all(&vault.path, "invalid subscription album");
    let qfx = write_qfx(
        &vault,
        "tunely.qfx",
        &ofx(
            "20261112",
            "20261112",
            "1000.00",
            "20261112",
            &[(
                "DEBIT",
                "20261112",
                "-10.99",
                "fit-tunely",
                "TUNELY PREMIUM 1234 RECURRING PAYMENT",
            )],
        ),
    );
    let imported = run(
        &vault,
        &["import", "-a", "checking", "-f", "json", path_str(&qfx)],
    );
    assert_success(&imported);
    let payload = json(&imported);
    assert_eq!(payload["ok"], true, "{payload}");
    let import_codes = findings(&payload);
    assert_eq!(
        import_codes
            .iter()
            .filter(|code| *code == "bank_import_subscriptions_unavailable")
            .count(),
        1,
        "{import_codes:?}"
    );
    let rows = json_rows(&payload);
    assert_eq!(rows[0]["subscription"], Value::Null, "{payload}");
    assert_eq!(rows[0]["bank_recurring"], false, "{payload}");

    let listed = json(&run(&vault, &["list", "-a", "-f", "json"]));
    assert_eq!(listed["warnings"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(
        listed["findings"][0]["code"],
        "bank_subscription_album_invalid"
    );
    assert_eq!(json_rows(&listed)[0]["subscription"], Value::Null);
    let reported =
        json(&run(&vault, &["report", "-m", "2026-11", "-f", "json"]));
    assert_eq!(
        reported["findings"][0]["code"],
        "bank_subscription_album_invalid"
    );
    let checked = json(&run(&vault, &["check", "-f", "json"]));
    assert_eq!(
        findings(&checked)
            .iter()
            .filter(|code| *code == "bank_subscription_album_invalid")
            .count(),
        1
    );
}
