//! Retired close spellings: whole-item `=x…`/`=X…`/`=*…`/`=!…` and the link
//! suffix `=*…`/`=!…` aliases fail with a teaching error and write nothing.

use super::pomodoro_close::{close_worked_vault, run_close_expect_error};
use crate::support::*;
use std::fs;

const NOW: &str = "2026-09-28 09:37:00";

fn unchanged_after_error(name: &str, args: &[&str], expected: &str) {
    let (_temp, vault, day_file) = close_worked_vault(name);
    let day_before = fs::read_to_string(&day_file).expect("read day");
    let bob_before = fs::read_to_string(vault.join("bob.md")).expect("bob");
    let error = run_close_expect_error(&vault, &day_file, NOW, args);
    assert!(error.contains(expected), "{args:?}: {error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), day_before);
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("bob"),
        bob_before
    );
}

#[test]
fn retired_whole_item_closes_name_the_bare_rewrite_and_write_nothing() {
    for (index, (raw, token, rewrite)) in [
        ("=x", "=x", "x"),
        ("=X", "=X", "x"),
        ("=x2!1", "=x2!1", "x2!1"),
        ("=*", "=*", "x*"),
        ("=!2", "=!2", "x!2"),
        ("=*!2", "=*!2", "x*!2"),
        ("=x wired the lexer", "=x", "x wired the lexer"),
        ("=x#bugs", "=x#bugs", "x =#bugs"),
    ]
    .into_iter()
    .enumerate()
    {
        let expected = format!(
            "`{token}` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `{rewrite}`"
        );
        unchanged_after_error(
            &format!("bob-cli-retired-close-{index}"),
            &[raw],
            &expected,
        );
    }
}

#[test]
fn retired_close_in_a_chain_rolls_the_whole_batch_back() {
    unchanged_after_error(
        "bob-cli-retired-close-chain",
        &["=x ="],
        "`=x` is retired",
    );
    unchanged_after_error(
        "bob-cli-retired-close-adjust",
        &["-2\n\n=x"],
        "`=x` is retired",
    );
}

#[test]
fn double_equals_close_teaches_the_bare_spelling() {
    unchanged_after_error(
        "bob-cli-retired-double-close",
        &["==x"],
        "`==x` is not a close: `==` restarts or swaps the running Pomodoro and never closes it; close it with `x`",
    );
}

#[test]
fn retired_link_aliases_name_the_x_spelling_and_write_nothing() {
    for (index, (raw, token, rewrite)) in [
        ("^bob:capture-stop=*", "=*", "=x*"),
        ("^bob:capture-stop=!", "=!", "=x!"),
        ("@bob:capture-stop=!2", "=!2", "=x!2"),
        ("Draft docs @bob:draft-docs=*", "=*", "=x*"),
    ]
    .into_iter()
    .enumerate()
    {
        let expected = format!(
            "`{token}` is retired: a link or task close spells the `x`; write `{rewrite}`"
        );
        unchanged_after_error(
            &format!("bob-cli-retired-link-alias-{index}"),
            &[raw],
            &expected,
        );
    }
}

#[test]
fn capture_parse_reports_retired_closes_as_close_diagnostics() {
    let parse = |text: &str| {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run capture-parse");
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture-parse JSON")
    };
    let retired = parse("=x2!1");
    assert_eq!(retired["mode"], "pomodoro_close");
    assert!(retired.get("pomodoro_close").is_none(), "{retired}");
    assert_eq!(retired["diagnostics"][0]["code"], "invalid_pomodoro_close");
    assert_eq!(
        retired["diagnostics"][0]["range"],
        serde_json::json!([0, 5])
    );
    assert_eq!(
        retired["diagnostics"][0]["message"],
        "`=x2!1` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x2!1`"
    );
    assert_eq!(
        retired["spans"],
        serde_json::json!([{ "start": 0, "end": 5, "kind": "pomodoro_close" }])
    );

    let alias = parse("^bob:stop=*1");
    assert!(alias.get("pomodoro_close").is_none(), "{alias}");
    assert_eq!(alias["diagnostics"][0]["code"], "invalid_pomodoro_close");
    assert_eq!(
        alias["diagnostics"][0]["message"],
        "`=*1` is retired: a link or task close spells the `x`; write `=x*1`"
    );
    assert_eq!(alias["diagnostics"][0]["range"], serde_json::json!([9, 12]));

    let chain = parse("=x =");
    assert_eq!(chain["items"][0]["mode"], "pomodoro_close");
    assert_eq!(chain["diagnostics"][0]["code"], "invalid_pomodoro_close");
    assert_eq!(chain["diagnostics"][0]["range"], serde_json::json!([0, 2]));
    assert_eq!(chain["items"][1]["mode"], "pomodoro_start");
}
