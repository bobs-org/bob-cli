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
