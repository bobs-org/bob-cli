use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::NaiveDate;
use serde_json::json;

use super::ops::{self, InverseOp, UsageIndex};
use super::protocol::{Anchor, Restoration};
use super::{
    dry_run, parse_envelope, plan, print_human, run_standalone, Op,
    SCHEMA_VERSION,
};
use crate::native::bank::config::{self, ConfigParse};
use crate::native::bank::journal::{
    ImportCounts, ImportEvent, JournalSnapshot, MalformedLine,
};
use crate::native::bank::scan::{BankScan, HubFile, MonthFile};
use crate::native::bank::source::SourceKind;
use crate::native::env::TestEnvGuard;

fn live_flow() -> &'static str {
    concat!(
        "---\n",
        "parent: \"[[cash]]\"\n",
        "summary: Keep me\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      label: WF Checking\n",
        "      kind: checking\n",
        "      start: 2026-11-11\n",
        "  categories:\n",
        "    coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }\n",
        "    convenience: { label: 🏪 Convenience, hotkey: v, group: Everyday }\n",
        "    housing: { label: 🏠 Housing, hotkey: \"1\", group: Bills }\n",
        "    kids: { label: 🧸 Kids, hidden: true }\n",
        "  rules:\n",
        "    - { match: VENMO, cat: transfer }\n",
        "  stale_after: 10\n",
        "---\n",
        "\n",
        "# 🏦 Bank\n",
    )
}

fn block_shape() -> &'static str {
    concat!(
        "---\n",
        "parent: \"[[cash]]\"\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      label: WF Checking\n",
        "      kind: checking\n",
        "      start: 2026-11-12\n",
        "  categories:\n",
        "    coffee:\n",
        "      label: ☕ Coffee\n",
        "      hotkey: c\n",
        "      group: Everyday\n",
        "    housing:\n",
        "      label: 🏠 Housing\n",
        "      hotkey: \"1\"\n",
        "      group: Bills\n",
        "  rules:\n",
        "    - match: VENMO\n",
        "      cat: transfer\n",
        "---\n",
        "# Bank\n",
    )
}

fn scalar_docs() -> &'static str {
    concat!(
        "---\n",
        "parent: \"[[cash]]\"\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      label: WF Checking\n",
        "      kind: checking\n",
        "      start: 2026-11-12\n",
        "  categories:\n",
        "    coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }\n",
        "    kids: { label: 🧸 Kids, hidden: true }\n",
        "    groceries: 🛒 Groceries\n",
        "    dining: 🍽️ Dining\n",
        "  rules:\n",
        "    - match: VENMO\n",
        "      cat: transfer\n",
        "---\n",
        "\n",
        "# 🏦 Bank\n",
    )
}

fn mixed_shape() -> &'static str {
    concat!(
        "---\n",
        "parent: \"[[cash]]\"\n",
        "summary: Outside\n",
        "bank:\n",
        "  accounts:\n",
        "    checking: { label: WF Checking, kind: checking, start: 2026-11-11 }\n",
        "    savings:\n",
        "      kind: savings\n",
        "      start: 2026-12-01\n",
        "  categories:\n",
        "    # coffee note\n",
        "    coffee: { label: ☕ Coffee, hotkey: c } # keep\n",
        "    groceries: 🛒 Groceries\n",
        "    housing:\n",
        "      label: 🏠 Housing\n",
        "      hotkey: \"1\"\n",
        "  rules:\n",
        "    - { match: VENMO, cat: transfer }\n",
        "    - match: PAYPAL\n",
        "      payee: PayPal\n",
        "      cat: refund\n",
        "---\n",
    )
}

fn derived_shift() -> &'static str {
    concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      kind: checking\n",
        "      start: 2026-11-11\n",
        "  categories:\n",
        "    gifts: 🎁 Gifts\n",
        "    games: 🎮 Games\n",
        "---\n",
    )
}

fn coffee_in_use() -> &'static str {
    concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      kind: checking\n",
        "      start: 2026-11-01\n",
        "  categories:\n",
        "    coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }\n",
        "  rules:\n",
        "    - match: VENMO\n",
        "      cat: coffee\n",
        "---\n",
    )
}

fn parse_ok(markdown: &str) -> crate::native::bank::config::BankConfig {
    match config::parse_bank_config(markdown) {
        ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    }
}

fn fingerprint(markdown: &str) -> String {
    config::config_fingerprint(markdown).expect("fingerprint")
}

fn scan(hub: &str, month: Option<&str>) -> BankScan {
    BankScan {
        hub: Some(HubFile {
            path: "bank.md".to_owned(),
            contents: hub.to_owned(),
        }),
        bank_dir_exists: month.is_some(),
        months: month
            .map(|contents| {
                vec![MonthFile {
                    path: "bank/2026-11.md".to_owned(),
                    month: crate::native::bank::grammar::YearMonth {
                        year: 2026,
                        month: 11,
                    },
                    contents: contents.to_owned(),
                }]
            })
            .unwrap_or_default(),
        month_entries: Vec::new(),
        io_errors: Vec::new(),
    }
}

fn usage(markdown: &str, month: Option<&str>) -> UsageIndex {
    usage_with_journal(markdown, month, &JournalSnapshot::default())
}

fn usage_with_journal(
    markdown: &str,
    month: Option<&str>,
    journal: &JournalSnapshot,
) -> UsageIndex {
    UsageIndex::from_scan(&scan(markdown, month), &parse_ok(markdown), journal)
        .with_subscription_refs(Default::default())
}

fn import_journal(alias: &str) -> JournalSnapshot {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    JournalSnapshot {
        imports: vec![ImportEvent {
            ts: chrono::DateTime::parse_from_rfc3339(
                "2026-11-15T12:00:00-05:00",
            )
            .unwrap(),
            host: "test".to_owned(),
            path: "bank/2026-11.md".to_owned(),
            line: 1,
            account: alias.to_owned(),
            source: SourceKind::Csv,
            file_sha256: "sha256:test".to_owned(),
            start: date,
            end: date,
            balance_cents: None,
            balance_asof: None,
            counts: ImportCounts::default(),
        }],
        ..JournalSnapshot::default()
    }
}

fn empty_usage(markdown: &str) -> UsageIndex {
    usage(markdown, None)
}

fn restorations_none(ops: &[Op]) -> Vec<Option<Restoration>> {
    vec![None; ops.len()]
}

fn apply(
    markdown: &str,
    ops: &[Op],
) -> (String, Vec<ops::OpResult>, Vec<InverseOp>) {
    ops::apply_ops(
        markdown,
        ops,
        &restorations_none(ops),
        &empty_usage(markdown),
        0,
    )
    .expect("ops apply")
}

fn apply_with_usage(
    markdown: &str,
    ops: &[Op],
    usage: &UsageIndex,
) -> Result<(String, Vec<ops::OpResult>, Vec<InverseOp>), ops::OpError> {
    ops::apply_ops(markdown, ops, &restorations_none(ops), usage, 0)
}

fn apply_inverses(
    markdown: &str,
    inverses: &[InverseOp],
    usage: &UsageIndex,
) -> Result<(String, Vec<ops::OpResult>, Vec<InverseOp>), ops::OpError> {
    let ops = inverses
        .iter()
        .map(|item| item.op.clone())
        .collect::<Vec<_>>();
    let restorations = inverses
        .iter()
        .map(|item| item.restore.clone())
        .collect::<Vec<_>>();
    ops::apply_ops(markdown, &ops, &restorations, usage, 0)
}

fn envelope_json(expect: &str, ops: serde_json::Value) -> String {
    json!({
        "schema_version": SCHEMA_VERSION,
        "expect": expect,
        "ops": ops,
    })
    .to_string()
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()
}

fn plan_ok(markdown: &str, ops: serde_json::Value) -> plan::UpdatePlan {
    let envelope = parse_envelope(&envelope_json(&fingerprint(markdown), ops))
        .expect("envelope");
    plan::plan_update(
        &scan(markdown, None),
        &JournalSnapshot::default(),
        &plan::SubscriptionRefs::new(),
        &envelope,
        today(),
    )
    .expect("plan")
}

fn siblings_survive(original: &str, after: &str) {
    for marker in ["  accounts:", "parent:", "  rules:"] {
        if original.contains(marker) {
            assert!(after.contains(marker), "lost {marker}");
        }
    }
    if original.contains("stale_after:") {
        assert!(after.contains("stale_after:"));
    }
}

#[test]
fn protocol_rejection_matrix() {
    let empty = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:abc","ops":[]}"#,
    )
    .expect("empty ops");
    assert!(empty.ops.is_empty());
    assert_eq!(empty.expect, "sha256:abc");

    let duplicate = parse_envelope(
        r#"{"schema_version":1,"schema_version":1,"expect":"sha256:x","ops":[]}"#,
    )
    .expect_err("duplicate top-level keys");
    assert_eq!(duplicate.code, "envelope_duplicate_key");

    let nested = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"category.remove","key":"a","key":"b"}]}"#,
    )
    .expect_err("duplicate op keys");
    assert_eq!(nested.code, "envelope_duplicate_key");

    let version =
        parse_envelope(r#"{"schema_version":2,"expect":"sha256:x","ops":[]}"#)
            .expect_err("unknown version");
    assert_eq!(version.code, "envelope_unknown_version");

    let op = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"not.an.op"}]}"#,
    )
    .expect_err("unknown op");
    assert_eq!(op.code, "envelope_unknown_op");
    assert_eq!(op.op_index, Some(0));

    let key = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"category.remove","key":"coffee","extra":1}]}"#,
    )
    .expect_err("unknown key");
    assert_eq!(key.code, "envelope_unknown_key");

    let wrong = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"category.remove","key":1}]}"#,
    )
    .expect_err("wrong type");
    assert_eq!(wrong.code, "envelope_wrong_type");

    let malformed =
        parse_envelope(r#"{"schema_version":1,"#).expect_err("truncated");
    assert_eq!(malformed.code, "envelope_malformed");

    let reserved = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"category.add","label":"X","hotkey":"q"}]}"#,
    )
    .expect_err("q reserved");
    assert_eq!(reserved.code, "hotkey_reserved");

    let invalid = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"category.add","label":"X","hotkey":"Q"}]}"#,
    )
    .expect_err("invalid hotkey");
    assert_eq!(invalid.code, "hotkey_invalid");

    let both = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"category.move","key":"coffee","before":"a","after":"b"}]}"#,
    )
    .expect_err("before and after");
    assert_eq!(both.code, "envelope_wrong_type");

    let empty_set = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"category.set","key":"coffee"}]}"#,
    )
    .expect_err("empty set");
    assert_eq!(empty_set.code, "envelope_missing_key");
}

#[test]
fn protocol_parses_each_category_and_group_op() {
    let envelope = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[
            {"op":"category.add","label":"🍔 Fast food","group":"Everyday","hotkey":"b"},
            {"op":"category.set","key":"coffee","set":{"label":"☕ Café","hidden":true},"unset":["group"]},
            {"op":"category.reset","key":"income"},
            {"op":"category.move","key":"coffee","after":"housing"},
            {"op":"category.remove","key":"kids"},
            {"op":"group.rename","group":"Everyday","to":"Daily"},
            {"op":"group.move","group":"Daily","before":"Bills"}
        ]}"#,
    )
    .expect("ops parse");
    assert_eq!(envelope.ops.len(), 7);
    assert_eq!(envelope.ops[0].name(), "category.add");
    assert_eq!(envelope.ops[6].name(), "group.move");
}

#[test]
fn protocol_parses_account_rule_and_setting_ops() {
    let envelope = parse_envelope(
        r#"{"schema_version":1,"expect":"sha256:x","ops":[
            {"op":"account.add","label":"Pocket cash","kind":"cash"},
            {"op":"account.set","alias":"checking","set":{"label":"Primary","kind":"savings","start":"2026-11-12"}},
            {"op":"account.rename","alias":"checking","to":"primary"},
            {"op":"account.default","alias":"checking"},
            {"op":"account.move","alias":"savings","after":"checking"},
            {"op":"account.remove","alias":"savings"},
            {"op":"rule.add","match":"VENMO","cat":"coffee","position":0},
            {"op":"rule.set","index":0,"match_was":"VENMO","set":{"payee":"Venmo"},"unset":["link"]},
            {"op":"rule.move","index":0,"match_was":"VENMO","position":1},
            {"op":"rule.remove","index":0,"match_was":"VENMO"},
            {"op":"bank.set","set":{"stale_after":0}},
            {"op":"bank.set","unset":["stale_after"]}
        ]}"#,
    )
    .expect("writer ops parse");
    assert_eq!(envelope.ops.len(), 12);
    assert_eq!(envelope.ops[0].name(), "account.add");
    assert_eq!(envelope.ops[6].name(), "rule.add");
    assert_eq!(envelope.ops[10].name(), "bank.set");
}

#[test]
fn account_add_set_remove_and_inverse_preserve_authored_style() {
    let add = [Op::AccountAdd {
        alias: None,
        label: "Savings account".to_owned(),
        kind: crate::native::bank::config::AccountKind::Savings,
        start: Some("2026-11-12".to_owned()),
    }];
    let (added, _, inverses) = apply(mixed_shape(), &add);
    assert!(added.contains("    savings_account:\n"));
    assert_eq!(parse_ok(&added).accounts[2].alias, "savings_account");
    assert_eq!(inverses[0].op.name(), "account.remove");
    let restored =
        apply_inverses(&added, &inverses, &empty_usage(&added)).unwrap();
    assert_eq!(restored.0, mixed_shape());

    let rename = [Op::AccountSet {
        alias: "checking".to_owned(),
        label: Some("Primary".to_owned()),
        rename_to: None,
        kind: None,
        start: None,
        unset: Vec::new(),
    }];
    let (renamed, _, inverses) = apply(mixed_shape(), &rename);
    assert!(renamed.contains("checking: {"));
    let checking = &parse_ok(&renamed).accounts[0];
    assert_eq!(checking.label, "Primary");
    assert_eq!(
        checking.kind,
        crate::native::bank::config::AccountKind::Checking
    );
    assert_eq!(checking.start, NaiveDate::from_ymd_opt(2026, 11, 11));
    siblings_survive(mixed_shape(), &renamed);
    let restored =
        apply_inverses(&renamed, &inverses, &empty_usage(&renamed)).unwrap();
    assert_eq!(restored.0, mixed_shape());

    let remove = [Op::AccountRemove {
        alias: "savings".to_owned(),
    }];
    let (removed, _, inverses) = apply(mixed_shape(), &remove);
    assert!(!removed.contains("    savings:\n"));
    let restored =
        apply_inverses(&removed, &inverses, &empty_usage(&removed)).unwrap();
    assert_eq!(restored.0, mixed_shape());
}

#[test]
fn account_migration_refusals_report_complete_usage_counts() {
    let month = "---\nmonth: 2026-11\n---\n\n# November 2026\n\n## Sun Nov 15\n\n- [ ] $5.00 Store [acct:: savings] [cat:: groceries]\n";
    let journal = import_journal("savings");
    let usage = usage_with_journal(mixed_shape(), Some(month), &journal);

    let remove = apply_with_usage(
        mixed_shape(),
        &[Op::AccountRemove {
            alias: "savings".to_owned(),
        }],
        &usage,
    )
    .expect_err("used account remains");
    assert_eq!(remove.code, "account_in_use");
    assert_eq!(remove.affected, Some(2));
    assert!(remove.message.contains("1 rows and 1 journal events"));

    let rename = apply_with_usage(
        mixed_shape(),
        &[Op::AccountRename {
            alias: "savings".to_owned(),
            to: "joint".to_owned(),
        }],
        &usage,
    )
    .expect_err("alias migration refused");
    assert_eq!(rename.code, "account_alias_immutable");
    assert!(rename.message.contains("1 rows and 1 journal events"));

    let kind = apply_with_usage(
        mixed_shape(),
        &[Op::AccountSet {
            alias: "savings".to_owned(),
            label: None,
            rename_to: None,
            kind: Some(crate::native::bank::config::AccountKind::Credit),
            start: None,
            unset: Vec::new(),
        }],
        &usage,
    )
    .expect_err("kind migration refused");
    assert_eq!(kind.code, "account_migration");
    assert_eq!(kind.affected, Some(2));

    let default = apply_with_usage(
        mixed_shape(),
        &[Op::AccountDefault {
            alias: "savings".to_owned(),
        }],
        &usage,
    )
    .expect_err("default change refused");
    assert_eq!(default.code, "account_default");

    let reordered = apply_with_usage(
        mixed_shape(),
        &[Op::AccountMove {
            alias: "savings".to_owned(),
            place: Anchor::Before("checking".to_owned()),
        }],
        &usage,
    )
    .expect_err("account reordering refused");
    assert_eq!(reordered.code, "account_default");

    let default_remove = apply_with_usage(
        mixed_shape(),
        &[Op::AccountRemove {
            alias: "checking".to_owned(),
        }],
        &usage,
    )
    .expect_err("default account stays");
    assert_eq!(default_remove.code, "account_default");
}

#[test]
fn account_identity_refusals_hold_when_journal_scan_is_incomplete() {
    let journal = JournalSnapshot {
        malformed: vec![MalformedLine {
            path: ".bob/bank/test.jsonl".to_owned(),
            line: 1,
        }],
        ..JournalSnapshot::default()
    };
    let envelope = parse_envelope(&envelope_json(
        &fingerprint(mixed_shape()),
        json!([{"op":"account.remove","alias":"savings"}]),
    ))
    .unwrap();
    let error = plan::plan_update(
        &scan(mixed_shape(), None),
        &journal,
        &plan::SubscriptionRefs::new(),
        &envelope,
        today(),
    )
    .expect_err("incomplete history blocks identity edit");
    assert_eq!(error.code, "journal_incomplete");
}

#[test]
fn rule_ops_preserve_siblings_apply_guards_and_round_trip() {
    let add = [Op::RuleAdd {
        match_text: "CARD 1234".to_owned(),
        payee: Some("Card payment".to_owned()),
        cat: Some("coffee".to_owned()),
        link: Some("[[bank/notes]]".to_owned()),
        position: Some(0),
    }];
    let (added, _, inverses) = apply(live_flow(), &add);
    let rules = &parse_ok(&added).rules;
    assert_eq!(rules[0].match_text, "CARD 1234");
    assert_eq!(rules[0].cat.as_deref(), Some("coffee"));
    siblings_survive(live_flow(), &added);
    let restored =
        apply_inverses(&added, &inverses, &empty_usage(&added)).unwrap();
    assert_eq!(restored.0, live_flow());

    let update = [Op::RuleSet {
        index: 0,
        match_was: "VENMO".to_owned(),
        match_text: None,
        payee: Some("Venmo".to_owned()),
        cat: None,
        link: None,
        unset: Vec::new(),
    }];
    let (updated, _, inverses) = apply(live_flow(), &update);
    assert!(updated.contains("- { match: VENMO"));
    assert!(!updated.contains("- match: VENMO"));
    assert_eq!(parse_ok(&updated).rules[0].payee.as_deref(), Some("Venmo"));
    let restored =
        apply_inverses(&updated, &inverses, &empty_usage(&updated)).unwrap();
    assert_eq!(restored.0, live_flow());

    let two_rules = live_flow().replace(
        "  rules:\n    - { match: VENMO, cat: transfer }\n",
        "  rules:\n    - { match: VENMO, cat: transfer }\n    - { match: PAYPAL, cat: refund }\n",
    );
    let rule_move_op = [Op::RuleMove {
        index: 0,
        match_was: "VENMO".to_owned(),
        position: 1,
    }];
    let (moved, _, inverses) = apply(&two_rules, &rule_move_op);
    let moved_rules = parse_ok(&moved).rules;
    assert_eq!(moved_rules[0].match_text, "PAYPAL");
    let restored =
        apply_inverses(&moved, &inverses, &empty_usage(&moved)).unwrap();
    assert_eq!(restored.0, two_rules);

    let remove = [Op::RuleRemove {
        index: 0,
        match_was: "VENMO".to_owned(),
    }];
    let (removed, _, inverses) = apply(live_flow(), &remove);
    assert!(parse_ok(&removed).rules.is_empty());
    let restored =
        apply_inverses(&removed, &inverses, &empty_usage(&removed)).unwrap();
    assert_eq!(restored.0, live_flow());

    let mismatch = apply_with_usage(
        live_flow(),
        &[Op::RuleRemove {
            index: 0,
            match_was: "PAYPAL".to_owned(),
        }],
        &empty_usage(live_flow()),
    )
    .expect_err("rule guard mismatch");
    assert_eq!(mismatch.code, "rule_mismatch");
}

#[test]
fn rules_and_stale_after_create_missing_containers_and_report_warnings() {
    let no_rules = live_flow()
        .replace("  rules:\n    - { match: VENMO, cat: transfer }\n", "");
    let no_rules = no_rules.replace("  stale_after: 10\n", "");
    let ops_json = json!([
        {"op":"rule.add","match":"CARD 1234","cat":"coffee"},
        {"op":"bank.set","set":{"stale_after":0}}
    ]);
    let plan = plan_ok(&no_rules, ops_json);
    assert!(plan.after.contains("  rules:\n    - match: CARD 1234"));
    assert!(plan.after.contains("  stale_after: 0\n"));
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.contains("bank_rule_read_time_dead")));
    siblings_survive(&no_rules, &plan.after);
    let inverses = plan.inverse_ops.clone();
    let restored =
        apply_inverses(&plan.after, &inverses, &empty_usage(&plan.after))
            .unwrap();
    assert_eq!(restored.0, no_rules);

    let unset = [Op::BankSet {
        stale_after: None,
        unset_stale_after: true,
    }];
    let (without_setting, _, inverses) = apply(live_flow(), &unset);
    assert!(!without_setting.contains("stale_after:"));
    let restored = apply_inverses(
        &without_setting,
        &inverses,
        &empty_usage(&without_setting),
    )
    .unwrap();
    assert_eq!(restored.0, live_flow());

    let unknown_category = apply_with_usage(
        live_flow(),
        &[Op::RuleAdd {
            match_text: "SHOP".to_owned(),
            payee: None,
            cat: Some("not_a_category".to_owned()),
            link: None,
            position: None,
        }],
        &empty_usage(live_flow()),
    )
    .expect_err("rule category must exist");
    assert_eq!(unknown_category.code, "unknown_category");
}

fn assert_add_on(fixture: &str) {
    let ops = [Op::CategoryAdd {
        label: "🍔 Fast food".to_owned(),
        group: Some("Everyday".to_owned()),
        hotkey: None,
        hidden: None,
        place: None,
        restore_key: None,
    }];
    let (after, results, inverses) = apply(fixture, &ops);
    let config = parse_ok(&after);
    let added = config.category("fast_food").expect("added");
    assert!(added.declared);
    assert_eq!(added.hotkey, Some('f'));
    assert_eq!(added.group.as_deref(), Some("Everyday"));
    assert!(after.contains("fast_food"));
    assert_eq!(results[0].key.as_deref(), Some("fast_food"));
    assert_eq!(results[0].hotkey, Some('f'));
    assert!(results[0].summary.contains("stored as fast_food"));
    assert!(matches!(inverses[0].op, Op::CategoryRemove { .. }));
    siblings_survive(fixture, &after);
    let restored = apply_inverses(&after, &inverses, &empty_usage(&after))
        .expect("inverse");
    assert_eq!(restored.0, fixture);
}

#[test]
fn category_add_on_flow_block_scalar_and_mixed() {
    assert_add_on(live_flow());
    assert_add_on(block_shape());
    assert_add_on(scalar_docs());
    assert_add_on(mixed_shape());
}

#[test]
fn key_derivation_collides_and_suffixes() {
    let ops = [Op::CategoryAdd {
        label: "☕ Coffee".to_owned(),
        group: None,
        hotkey: None,
        hidden: None,
        place: None,
        restore_key: None,
    }];
    let error = apply_with_usage(live_flow(), &ops, &empty_usage(live_flow()))
        .expect_err("same label");
    assert_eq!(error.code, "category_exists");
    assert!(error.hint.as_deref().unwrap().contains("edit the existing"));

    let hidden = [Op::CategoryAdd {
        label: "🧸 Kids".to_owned(),
        group: None,
        hotkey: None,
        hidden: None,
        place: None,
        restore_key: None,
    }];
    let error =
        apply_with_usage(live_flow(), &hidden, &empty_usage(live_flow()))
            .expect_err("hidden same label");
    assert_eq!(error.code, "category_exists");
    assert!(error.hint.as_deref().unwrap().contains("unhide"));

    let suffix = [Op::CategoryAdd {
        label: "Coffee???".to_owned(),
        group: None,
        hotkey: None,
        hidden: None,
        place: None,
        restore_key: None,
    }];
    let (after, results, _) = apply(live_flow(), &suffix);
    assert_eq!(results[0].key.as_deref(), Some("coffee_2"));
    assert!(parse_ok(&after).category("coffee_2").is_some());
}

#[test]
fn pinned_hotkey_q_and_collision_refusals() {
    let taken = [Op::CategoryAdd {
        label: "🚆 Transit".to_owned(),
        group: None,
        hotkey: Some('c'),
        hidden: None,
        place: None,
        restore_key: None,
    }];
    let error =
        apply_with_usage(live_flow(), &taken, &empty_usage(live_flow()))
            .expect_err("c taken");
    assert_eq!(error.code, "hotkey_taken");
    assert!(error.message.contains("coffee"));

    let set_taken = [Op::CategorySet {
        key: "housing".to_owned(),
        label: None,
        group: None,
        hotkey: Some('c'),
        hidden: None,
        unset: Vec::new(),
    }];
    let error =
        apply_with_usage(live_flow(), &set_taken, &empty_usage(live_flow()))
            .expect_err("set taken");
    assert_eq!(error.code, "hotkey_taken");
}

#[test]
fn hide_unhide_and_move_report_hotkey_shifts() {
    let hide = plan_ok(
        scalar_docs(),
        json!([{"op":"category.set","key":"groceries","set":{"hidden":true}}]),
    );
    assert!(hide.shifted.iter().any(|shift| {
        shift.key == "groceries"
            && shift.from == Some('g')
            && shift.to.is_none()
    }));
    assert!(hide.consequence.contains("hides"));

    let unhide = plan_ok(
        live_flow(),
        json!([{"op":"category.set","key":"kids","set":{"hidden":false}}]),
    );
    assert!(unhide.consequence.contains("unhides"));

    let moved = plan_ok(
        derived_shift(),
        json!([{"op":"category.move","key":"games","before":"gifts"}]),
    );
    assert!(
        moved.shifted.iter().any(|shift| shift.key == "games")
            || moved.shifted.iter().any(|shift| shift.key == "gifts"),
        "moving derived keys reports a shift: {:?}",
        moved
            .shifted
            .iter()
            .map(|shift| format!(
                "{} {:?}→{:?}",
                shift.key, shift.from, shift.to
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn set_reset_move_remove_and_group_ops_emit_bytes() {
    let set = [Op::CategorySet {
        key: "coffee".to_owned(),
        label: Some("☕ Café".to_owned()),
        group: None,
        hotkey: None,
        hidden: None,
        unset: Vec::new(),
    }];
    let (after, results, inverses) = apply(live_flow(), &set);
    assert!(after.contains("☕ Café"));
    assert!(after.contains("hotkey: c"));
    assert_eq!(parse_ok(&after).category("coffee").unwrap().key, "coffee");
    assert!(results[0].summary.contains("renames"));
    siblings_survive(live_flow(), &after);
    let restored =
        apply_inverses(&after, &inverses, &empty_usage(&after)).unwrap();
    assert_eq!(restored.0, live_flow());

    let reset_ops = [Op::CategorySet {
        key: "income".to_owned(),
        label: Some("💵 Pay".to_owned()),
        group: None,
        hotkey: None,
        hidden: None,
        unset: Vec::new(),
    }];
    let (declared, _, _) = apply(live_flow(), &reset_ops);
    assert!(declared.contains("income:"));
    let reset = [Op::CategoryReset {
        key: "income".to_owned(),
    }];
    let (after, results, inverses) = apply(&declared, &reset);
    assert!(!after.contains("💵 Pay"));
    assert!(results[0].summary.contains("default appearance"));
    let restored =
        apply_inverses(&after, &inverses, &empty_usage(&after)).unwrap();
    assert_eq!(restored.0, declared);

    let not_builtin = [Op::CategoryReset {
        key: "coffee".to_owned(),
    }];
    let error =
        apply_with_usage(live_flow(), &not_builtin, &empty_usage(live_flow()))
            .expect_err("not builtin");
    assert_eq!(error.code, "not_builtin");

    let moved = [Op::CategoryMove {
        key: "coffee".to_owned(),
        place: Anchor::After("housing".to_owned()),
    }];
    let (after, _, inverses) = apply(block_shape(), &moved);
    let keys = authored_keys(&after);
    assert!(
        keys.iter().position(|key| key == "housing")
            < keys.iter().position(|key| key == "coffee")
    );
    let restored =
        apply_inverses(&after, &inverses, &empty_usage(&after)).unwrap();
    assert_eq!(restored.0, block_shape());

    let remove = [Op::CategoryRemove {
        key: "kids".to_owned(),
    }];
    let (after, results, inverses) = apply(live_flow(), &remove);
    assert!(!after.contains("kids:"));
    assert!(results[0].summary.contains("removes"));
    let restored =
        apply_inverses(&after, &inverses, &empty_usage(&after)).unwrap();
    assert_eq!(restored.0, live_flow());

    let rename = [Op::GroupRename {
        group: "Everyday".to_owned(),
        to: "Daily".to_owned(),
    }];
    let (after, _, inverses) = apply(live_flow(), &rename);
    assert!(after.contains("group: Daily"));
    assert!(!after.contains("group: Everyday"));
    let restored =
        apply_inverses(&after, &inverses, &empty_usage(&after)).unwrap();
    assert_eq!(restored.0, live_flow());

    let group_move = [Op::GroupMove {
        group: "Everyday".to_owned(),
        place: Anchor::After("Bills".to_owned()),
    }];
    let (after, _, inverses) = apply(live_flow(), &group_move);
    let keys = authored_keys(&after);
    let housing = keys.iter().position(|key| key == "housing").unwrap();
    let coffee = keys.iter().position(|key| key == "coffee").unwrap();
    assert!(housing < coffee);
    let restored =
        apply_inverses(&after, &inverses, &empty_usage(&after)).unwrap();
    assert_eq!(restored.0, live_flow());
}

fn authored_keys(markdown: &str) -> Vec<String> {
    parse_ok(markdown)
        .categories
        .iter()
        .filter(|category| category.declared)
        .map(|category| category.key.clone())
        .collect()
}

#[test]
fn remove_and_reset_refusal_matrix_with_counts() {
    let month = "---\nmonth: 2026-11\n---\n\n# November 2026\n\n## Sat Nov 15\n\n- [ ] $5.00 Cafe [cat:: coffee]\n";
    let error = apply_with_usage(
        coffee_in_use(),
        &[Op::CategoryRemove {
            key: "coffee".to_owned(),
        }],
        &usage(coffee_in_use(), Some(month)),
    )
    .expect_err("in use");
    assert_eq!(error.code, "category_in_use");
    assert_eq!(error.affected, Some(2));
    assert!(error.message.contains("row"));
    assert!(error.message.contains("rule"));
    assert!(error.message.contains("hide it instead"));

    let builtin = apply_with_usage(
        live_flow(),
        &[Op::CategoryRemove {
            key: "income".to_owned(),
        }],
        &empty_usage(live_flow()),
    )
    .expect_err("builtin");
    assert_eq!(builtin.code, "builtin_category");
    assert!(builtin.hint.as_deref().unwrap().contains("reset or hide"));

    let builtin_group = apply_with_usage(
        live_flow(),
        &[Op::GroupRename {
            group: "Money moves".to_owned(),
            to: "Cashflow".to_owned(),
        }],
        &empty_usage(live_flow()),
    )
    .expect_err("builtin group");
    assert_eq!(builtin_group.code, "group_builtin");
}

#[test]
fn category_removal_counts_subscription_records_and_ignores_missing_album() {
    let markdown = live_flow();
    let remove = || {
        parse_envelope(&envelope_json(
            &fingerprint(markdown),
            json!([{"op":"category.remove","key":"convenience"}]),
        ))
        .expect("envelope")
    };

    let missing_album = tempfile::tempdir().unwrap();
    let allowed = plan::plan_update(
        &scan(markdown, None),
        &JournalSnapshot::default(),
        &plan::load_subscription_refs(missing_album.path()),
        &remove(),
        today(),
    )
    .expect("missing album has no subscription users");
    assert!(allowed.changed);

    let vault = tempfile::tempdir().unwrap();
    fs::write(
        vault.path().join("area.md"),
        "---\ntype: [[area]]\n---\n# Work\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("billing.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[area]]\"\n",
            "emoji: 🔁\n",
            "record:\n",
            "  noun: subscription\n",
            "  label: Subscription\n",
            "  fields:\n",
            "    status:\n",
            "      required: true\n",
            "      values: { active: Active, cancelled: Cancelled, to_cancel: To cancel, to_decide: To decide }\n",
            "    amount: { required: true, values: number, min: 0 }\n",
            "    cadence: { required: true, values: { monthly: Monthly, four_weekly: Four weeks, quarterly: Quarterly, yearly: Yearly } }\n",
            "    pricing: { default: fixed, values: { fixed: Fixed, variable: Variable } }\n",
            "    match: { list: true }\n",
            "    category: { suggest: true }\n",
            "    url: { values: url }\n",
            "    renews: { values: date }\n",
            "    ended: { values: date }\n",
            "---\n# Subscriptions\n",
        ),
    )
    .unwrap();
    fs::create_dir(vault.path().join("billing")).unwrap();
    fs::write(
        vault.path().join("billing/tunely.md"),
        "---\ntitle: Tunely\nstatus: active\namount: 9.99\ncadence: monthly\ncategory: convenience\nmatch: [TUNELY]\n---\n# Tunely\n",
    )
    .unwrap();

    let refused = plan::plan_update(
        &scan(markdown, None),
        &JournalSnapshot::default(),
        &plan::load_subscription_refs(vault.path()),
        &remove(),
        today(),
    )
    .expect_err("subscription category use refuses removal");
    assert_eq!(refused.code, "category_in_use");
    assert_eq!(refused.affected, Some(1));
    assert!(refused.message.contains("1 subscription use"));
    assert!(refused.message.contains("Convenience"));
}

#[test]
fn stale_fingerprint_and_invalid_pre_image_refuse() {
    let envelope =
        parse_envelope(&envelope_json("sha256:deadbeef", json!([]))).unwrap();
    let error = plan::plan_update(
        &scan(live_flow(), None),
        &JournalSnapshot::default(),
        &plan::SubscriptionRefs::new(),
        &envelope,
        today(),
    )
    .expect_err("stale");
    assert_eq!(error.code, "stale_config");

    let missing = "---\nparent: cash\n---\n";
    let envelope =
        parse_envelope(&envelope_json(&fingerprint(live_flow()), json!([])))
            .unwrap();
    let error = plan::plan_update(
        &scan(missing, None),
        &JournalSnapshot::default(),
        &plan::SubscriptionRefs::new(),
        &envelope,
        today(),
    )
    .expect_err("invalid");
    assert_eq!(error.code, "invalid_config");
    assert!(error.message.contains("fix bank:"));
}

#[test]
fn empty_ops_are_byte_identical_no_commit() {
    let plan = plan_ok(live_flow(), json!([]));
    assert!(!plan.changed);
    assert_eq!(plan.before, plan.after);
    assert_eq!(plan.before_fingerprint, plan.after_fingerprint);
    assert_eq!(plan::commit_subject(&plan), "bank: config");
}

#[test]
fn inverse_round_trip_including_flow_entry() {
    let ops = [Op::CategoryAdd {
        label: "🚆 Transit".to_owned(),
        group: Some("Everyday".to_owned()),
        hotkey: Some('n'),
        hidden: None,
        place: Some(Anchor::After("coffee".to_owned())),
        restore_key: None,
    }];
    let (after, _, inverses) = apply(live_flow(), &ops);
    assert!(after.contains("transit: {"));
    let restored =
        apply_inverses(&after, &inverses, &empty_usage(&after)).unwrap();
    assert_eq!(restored.0, live_flow());
}

#[test]
fn undo_blocked_when_rows_now_depend_on_the_category() {
    let ops = [Op::CategoryAdd {
        label: "🚆 Transit".to_owned(),
        group: None,
        hotkey: Some('n'),
        hidden: None,
        place: None,
        restore_key: None,
    }];
    let (after, _, inverses) = apply(live_flow(), &ops);
    let month = "---\nmonth: 2026-11\n---\n\n# November 2026\n\n## Sat Nov 15\n\n- [ ] $4.00 Bus [cat:: transit]\n";
    let error = apply_inverses(&after, &inverses, &usage(&after, Some(month)))
        .expect_err("undo blocked");
    assert_eq!(error.code, "undo_blocked");
    assert_eq!(error.affected, Some(1));
}

#[test]
fn subscription_records_block_category_remove_and_its_undo() {
    let refs =
        |key: &str| std::collections::HashMap::from([(key.to_owned(), 2usize)]);
    let error = apply_with_usage(
        live_flow(),
        &[Op::CategoryRemove {
            key: "coffee".to_owned(),
        }],
        &empty_usage(live_flow()).with_subscription_refs(refs("coffee")),
    )
    .expect_err("subscriptions use it");
    assert_eq!(error.code, "category_in_use");
    assert_eq!(error.affected, Some(2));
    assert!(
        error.message.contains("2 subscriptions"),
        "{}",
        error.message
    );
    assert!(!error.message.contains("row"), "{}", error.message);

    let ops = [Op::CategoryAdd {
        label: "🚆 Transit".to_owned(),
        group: None,
        hotkey: Some('n'),
        hidden: None,
        place: None,
        restore_key: None,
    }];
    let (after, _, inverses) = apply(live_flow(), &ops);
    let error = apply_inverses(
        &after,
        &inverses,
        &empty_usage(&after).with_subscription_refs(refs("transit")),
    )
    .expect_err("undo blocked by subscriptions");
    assert_eq!(error.code, "undo_blocked");
    assert_eq!(error.affected, Some(2));
    assert!(
        error.message.contains("2 subscriptions"),
        "{}",
        error.message
    );
}

#[test]
fn post_image_mismatch_refuses_forged_restore() {
    let markdown = live_flow();
    let envelope = parse_envelope(
        &json!({
            "schema_version": 1,
            "expect": fingerprint(markdown),
            "ops": [{
                "op": "category.set",
                "key": "coffee",
                "set": {"hidden": true},
                "restore": {
                    "spans": [{
                        "key_path": "bank.accounts.checking",
                        "expected": "      kind: checking\n",
                        "prior": "      kind: nope\n"
                    }]
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let error = plan::plan_update(
        &scan(markdown, None),
        &JournalSnapshot::default(),
        &plan::SubscriptionRefs::new(),
        &envelope,
        today(),
    )
    .expect_err("post image");
    assert_eq!(error.code, "post_image_mismatch");
}

#[test]
fn unsupported_yaml_refuses_anchors() {
    let markdown = concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking: &acct\n",
        "      kind: checking\n",
        "      start: 2026-11-11\n",
        "  categories:\n",
        "    coffee: { label: ☕ Coffee, hotkey: c }\n",
        "---\n",
    );
    let error = apply_with_usage(
        markdown,
        &[Op::CategorySet {
            key: "coffee".to_owned(),
            label: Some("☕ Café".to_owned()),
            group: None,
            hotkey: None,
            hidden: None,
            unset: Vec::new(),
        }],
        &empty_usage(markdown),
    )
    .expect_err("anchor");
    assert_eq!(error.code, "unsupported_yaml");
}

#[test]
fn human_and_json_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("bank.md"), live_flow()).unwrap();
    let _guard = TestEnvGuard::set(&[
        ("BOB_DIR", Some(vault.as_os_str())),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent-bob-bank-config-update-test.yml")),
        ),
        ("BOB_NOW", Some(OsStr::new("2026-11-15T12:00:00"))),
        ("NO_COLOR", Some(OsStr::new("1"))),
    ]);
    let envelope = envelope_json(
        &fingerprint(live_flow()),
        json!([{"op":"category.add","label":"🍔 Fast food","group":"Everyday"}]),
    );
    let report = dry_run(&vault, &envelope);
    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert!(report.consequence.contains("adds 🍔 Fast food"));
    assert!(report.consequence.contains("stored as fast_food"));
    let json = report.to_json();
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "update");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["changed"], true);
    assert!(json["before_fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_ne!(json["before_fingerprint"], json["after_fingerprint"]);
    assert_eq!(json["per_op"][0]["op"], "category.add");
    assert_eq!(json["per_op"][0]["key"], "fast_food");
    assert_eq!(json["inverse_ops"][0]["op"], "category.remove");
    assert!(json["categories"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| { row["key"] == "fast_food" }));
    assert!(json["hotkeys"]["free"].as_array().is_some());
    assert_eq!(json["sync"]["pre"], "skipped");
    print_human(&report);
}

struct TestVault {
    _temp: tempfile::TempDir,
    path: PathBuf,
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn vault_with(hub: &str) -> TestVault {
    let temp = tempfile::tempdir().expect("temporary vault");
    let path = temp.path().join("vault");
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q"]);
    git(&path, &["config", "user.email", "test@example.invalid"]);
    git(&path, &["config", "user.name", "Bank Config Test"]);
    fs::write(path.join("bank.md"), hub).unwrap();
    git(&path, &["add", "bank.md"]);
    git(&path, &["commit", "-qm", "fixture"]);
    TestVault { _temp: temp, path }
}

fn syncable_vault(hub: &str) -> TestVault {
    let vault = vault_with(hub);
    let remote = vault._temp.path().join("remote.git");
    let output = Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&remote)
        .output()
        .expect("initialize local sync remote");
    assert!(output.status.success());
    let remote = remote.to_string_lossy().into_owned();
    let branch = git(&vault.path, &["branch", "--show-current"]);
    git(&vault.path, &["remote", "add", "origin", &remote]);
    git(
        &vault.path,
        &["push", "--quiet", "--set-upstream", "origin", &branch],
    );
    git(&vault.path, &["remote", "set-head", "origin", &branch]);
    vault
}

fn sync_environment(vault: &TestVault) -> TestEnvGuard {
    let lock = vault._temp.path().join("sync.lock");
    let state = vault._temp.path().join("sync-state.json");
    TestEnvGuard::set(&[
        ("BOB_DIR", Some(vault.path.as_os_str())),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent-bob-bank-config-update-test.yml")),
        ),
        ("BOB_NOW", Some(OsStr::new("2026-11-15T12:00:00"))),
        ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
        ("BOB_VAULT_SYNC_STATE_FILE", Some(state.as_os_str())),
        ("BOB_VAULT_SYNC_NOTIFY", Some(OsStr::new("0"))),
        ("NO_COLOR", Some(OsStr::new("1"))),
    ])
}

#[test]
fn dry_run_takes_no_lock_and_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("bank.md"), live_flow()).unwrap();
    let _guard = TestEnvGuard::set(&[
        ("BOB_DIR", Some(vault.as_os_str())),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent-bob-bank-config-update-test.yml")),
        ),
        ("BOB_NOW", Some(OsStr::new("2026-11-15T12:00:00"))),
        ("NO_COLOR", Some(OsStr::new("1"))),
    ]);
    let before = fs::read_to_string(vault.join("bank.md")).unwrap();
    let envelope = envelope_json(
        &fingerprint(&before),
        json!([{"op":"category.add","label":"🍔 Fast food","group":"Everyday"}]),
    );
    let report = dry_run(&vault, &envelope);
    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert!(report.dry_run);
    assert!(report.changed);
    assert_eq!(fs::read_to_string(vault.join("bank.md")).unwrap(), before);
    assert!(!vault.join(".git").exists());
    assert!(!vault.join(".bob").exists());
}

#[test]
fn scratch_git_apply_makes_exactly_one_config_commit() {
    let vault = syncable_vault(live_flow());
    let _guard = sync_environment(&vault);
    let before = fs::read_to_string(vault.path.join("bank.md")).unwrap();
    let head = git(&vault.path, &["rev-parse", "HEAD"]);
    let envelope = envelope_json(
        &fingerprint(&before),
        json!([{"op":"category.add","label":"🍔 Fast food","group":"Everyday"}]),
    );
    let report = run_standalone(&vault.path, &envelope, true);
    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert!(report.changed);
    let after = fs::read_to_string(vault.path.join("bank.md")).unwrap();
    assert_ne!(after, before);
    assert!(after.contains("fast_food"));
    let log = git(&vault.path, &["log", "--pretty=%s"]);
    let subjects = log.lines().collect::<Vec<_>>();
    assert_eq!(subjects[0], "bank: config (+fast_food)");
    assert_eq!(subjects.len(), 2);
    let new_head = git(&vault.path, &["rev-parse", "HEAD"]);
    assert_ne!(new_head, head);
    let commit = report.commit.expect("commit");
    assert_eq!(commit.subject, "bank: config (+fast_food)");
    assert_eq!(report.sync_pre, Some("ok"));
}

#[test]
fn update_help_lists_flags_and_stays_the_default_show_group() {
    let mut command = crate::native::bank::build_cli_config();
    let parent = command.render_long_help().to_string();
    assert!(parent.contains("show"));
    assert!(parent.contains("update"));
    assert!(parent.contains("Bare `bob bank config` runs the read-only `show`"));
    let update = command
        .find_subcommand_mut("update")
        .expect("update")
        .render_long_help()
        .to_string();
    for option in [
        "-b, --bob-dir <DIR>",
        "-d, --dry-run",
        "-f, --format <FORMAT>",
        "-h, --help",
        "-i, --input <FILE>",
    ] {
        assert!(update.contains(option), "missing {option} in:\n{update}");
    }
    let b = update.find("-b, --bob-dir").unwrap();
    let d = update.find("-d, --dry-run").unwrap();
    let f = update.find("-f, --format").unwrap();
    let h = update.find("-h, --help").unwrap();
    let i = update.find("-i, --input").unwrap();
    assert!(b < d && d < f && f < h && h < i, "{update}");
}
