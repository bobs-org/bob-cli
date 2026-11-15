use super::*;
use crate::native::bank::config::{parse_bank_config, ConfigParse};

fn live_flow() -> &'static str {
    concat!(
        "---\n",
        "parent: \"[[cash]]\"\n",
        "type: \"[[area]]\"\n",
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
        "type: \"[[area]]\"\n",
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
        "type: \"[[area]]\"\n",
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

fn parse_ok(text: &str) -> crate::native::bank::config::BankConfig {
    match parse_bank_config(text) {
        ConfigParse::Ok(config) => config,
        other => panic!("expected ok, got {other:?}"),
    }
}

fn assert_restore(original: &str, edit: &BankSpanEdit) {
    let restored = restore(&edit.contents, &edit.spans).expect("restore");
    assert_eq!(restored, original);
}

fn assert_outside_untouched(original: &str, edit: &BankSpanEdit) {
    for span in &edit.spans {
        if let Some(after) = &span.after {
            assert!(
                edit.contents.contains(after),
                "edited text missing after span {}",
                span.key_path
            );
        }
        if let Some(before) = &span.before {
            assert!(
                original.contains(before),
                "original text missing before span {}",
                span.key_path
            );
        }
    }
    assert_restore(original, edit);
}

fn coffee_relabel() -> CategoryDraft {
    CategoryDraft {
        key: "coffee".to_owned(),
        label: "☕ Café".to_owned(),
        hotkey: Some('c'),
        group: Some("Everyday".to_owned()),
        hidden: None,
    }
}

#[test]
fn live_flow_replace_keeps_flow_and_quotes_digit_hotkeys() {
    let original = live_flow();
    let edit = replace_category(original, &coffee_relabel()).unwrap();
    assert!(edit
        .contents
        .contains("coffee: { label: ☕ Café, hotkey: c, group: Everyday }"));
    assert!(edit.contents.contains("hotkey: \"1\""));
    assert!(edit.contents.contains("summary: Keep me"));
    assert!(edit.contents.contains("stale_after: 10"));
    assert!(edit.contents.contains("- { match: VENMO, cat: transfer }"));
    let config = parse_ok(&edit.contents);
    assert_eq!(config.category("coffee").unwrap().label, "Café");
    assert_eq!(config.category("housing").unwrap().hotkey, Some('1'));
    assert_outside_untouched(original, &edit);
}

#[test]
fn live_flow_insert_copies_flow_style_and_survives_siblings() {
    let original = live_flow();
    let draft = CategoryDraft {
        key: "fast_food".to_owned(),
        label: "🍔 Fast food".to_owned(),
        hotkey: Some('b'),
        group: Some("Everyday".to_owned()),
        hidden: None,
    };
    let edit =
        insert_category(original, &draft, Place::After("coffee")).unwrap();
    assert!(edit.contents.contains(
        "fast_food: { label: 🍔 Fast food, hotkey: b, group: Everyday }"
    ));
    let coffee = edit.contents.find("coffee:").unwrap();
    let fast = edit.contents.find("fast_food:").unwrap();
    let convenience = edit.contents.find("convenience:").unwrap();
    assert!(coffee < fast && fast < convenience);
    assert!(edit.contents.contains("rules:"));
    assert!(edit.contents.contains("stale_after: 10"));
    assert!(edit.contents.contains("summary: Keep me"));
    let config = parse_ok(&edit.contents);
    assert_eq!(config.category("fast_food").unwrap().hotkey, Some('b'));
    assert!(config.category("coffee").is_some());
    assert_outside_untouched(original, &edit);
}

#[test]
fn live_flow_remove_and_move_round_trip() {
    let original = live_flow();
    let removed = remove_category(original, "kids").unwrap();
    assert!(!removed.contents.contains("kids:"));
    assert!(removed.contents.contains("coffee:"));
    assert!(removed.contents.contains("stale_after: 10"));
    parse_ok(&removed.contents);
    assert_outside_untouched(original, &removed);

    let moved =
        move_category(original, "housing", Place::Before("coffee")).unwrap();
    let housing = moved.contents.find("housing:").unwrap();
    let coffee = moved.contents.find("coffee:").unwrap();
    assert!(housing < coffee);
    parse_ok(&moved.contents);
    assert_outside_untouched(original, &moved);
}

#[test]
fn block_shape_replace_stays_block() {
    let original = block_shape();
    let edit = replace_category(original, &coffee_relabel()).unwrap();
    assert!(edit.contents.contains("    coffee:\n      label: ☕ Café"));
    assert!(edit.contents.contains("      hotkey: c"));
    assert!(!edit.contents.contains("coffee: { label: ☕ Café"));
    parse_ok(&edit.contents);
    assert_outside_untouched(original, &edit);
}

#[test]
fn scalar_category_stays_scalar_on_label_only_replace() {
    let original = scalar_docs();
    let draft = CategoryDraft {
        key: "groceries".to_owned(),
        label: "🛒 Food".to_owned(),
        hotkey: None,
        group: None,
        hidden: None,
    };
    let edit = replace_category(original, &draft).unwrap();
    assert!(edit.contents.contains("groceries: 🛒 Food\n"));
    assert!(!edit.contents.contains("groceries: {"));
    parse_ok(&edit.contents);
    assert_outside_untouched(original, &edit);
}

#[test]
fn scalar_category_promotes_to_flow_when_siblings_are_flow() {
    let original = scalar_docs();
    let draft = CategoryDraft {
        key: "groceries".to_owned(),
        label: "🛒 Groceries".to_owned(),
        hotkey: Some('g'),
        group: None,
        hidden: None,
    };
    let edit = replace_category(original, &draft).unwrap();
    assert!(edit
        .contents
        .contains("groceries: { label: 🛒 Groceries, hotkey: g }"));
    parse_ok(&edit.contents);
    assert_outside_untouched(original, &edit);
}

#[test]
fn mixed_shape_preserves_comments_and_styles() {
    let original = mixed_shape();
    let edit = replace_category(original, &coffee_relabel()).unwrap();
    assert!(edit.contents.contains("# coffee note"));
    assert!(edit.contents.contains(
        "coffee: { label: ☕ Café, hotkey: c, group: Everyday } # keep"
    ));
    assert!(edit.contents.contains("groceries: 🛒 Groceries"));
    assert!(edit
        .contents
        .contains("    housing:\n      label: 🏠 Housing"));
    assert!(edit.contents.contains("- { match: VENMO, cat: transfer }"));
    assert!(edit.contents.contains("summary: Outside"));
    parse_ok(&edit.contents);
    assert_outside_untouched(original, &edit);
}

#[test]
fn no_op_replace_is_byte_identical() {
    let original = live_flow();
    let draft = CategoryDraft {
        key: "coffee".to_owned(),
        label: "☕ Coffee".to_owned(),
        hotkey: Some('c'),
        group: Some("Everyday".to_owned()),
        hidden: None,
    };
    let edit = replace_category(original, &draft).unwrap();
    assert_eq!(edit.contents, original);
    assert!(edit.spans.is_empty());
}

#[test]
fn sibling_keys_survive_category_edits() {
    let original = live_flow();
    let draft = CategoryDraft {
        key: "transit".to_owned(),
        label: "🚆 Transit".to_owned(),
        hotkey: Some('n'),
        group: Some("Everyday".to_owned()),
        hidden: None,
    };
    let edit = insert_category(original, &draft, Place::End).unwrap();
    assert!(edit.contents.contains("  accounts:"));
    assert!(edit.contents.contains("  rules:"));
    assert!(edit.contents.contains("  stale_after: 10"));
    assert!(edit.contents.contains("parent: \"[[cash]]\""));
    assert!(edit.contents.contains("# 🏦 Bank"));
    let removed = remove_category(&edit.contents, "transit").unwrap();
    assert_eq!(
        restore(&removed.contents, &removed.spans).unwrap(),
        edit.contents
    );
}

#[test]
fn insert_account_defaults_to_block_without_siblings_of_other_style() {
    let original = concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      kind: checking\n",
        "      start: 2026-11-12\n",
        "  categories:\n",
        "    groceries: 🛒 Groceries\n",
        "---\n",
    );
    let draft = AccountDraft {
        alias: "savings".to_owned(),
        label: Some("WF Savings".to_owned()),
        kind: "savings".to_owned(),
        start: Some("2026-12-01".to_owned()),
    };
    let edit = insert_account(original, &draft, Place::End).unwrap();
    assert!(edit
        .contents
        .contains("    savings:\n      label: WF Savings"));
    assert!(edit.contents.contains("      kind: savings"));
    parse_ok(&edit.contents);
    assert_outside_untouched(original, &edit);
}

#[test]
fn set_account_label_rewrites_only_the_label_child() {
    let original = live_flow();
    let edit =
        set_account_label(original, "checking", Some("Wells Fargo")).unwrap();
    assert!(edit.contents.contains("      label: Wells Fargo\n"));
    assert!(edit.contents.contains("      kind: checking\n"));
    assert!(edit.contents.contains("      start: 2026-11-11\n"));
    let config = parse_ok(&edit.contents);
    assert_eq!(config.accounts[0].label, "Wells Fargo");
    assert_outside_untouched(original, &edit);
}

#[test]
fn set_and_clear_stale_after() {
    let original = live_flow();
    let cleared = set_stale_after(original, None).unwrap();
    assert!(!cleared.contents.contains("stale_after"));
    assert!(cleared.contents.contains("  rules:"));
    parse_ok(&cleared.contents);
    assert_outside_untouched(original, &cleared);

    let restored = set_stale_after(&cleared.contents, Some(14)).unwrap();
    assert!(restored.contents.contains("  stale_after: 14\n"));
    let config = parse_ok(&restored.contents);
    assert_eq!(config.stale_after, 14);
}

#[test]
fn ensure_container_inserts_in_canonical_order() {
    let original = concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      kind: checking\n",
        "      start: 2026-11-12\n",
        "  stale_after: 7\n",
        "---\n",
    );
    let categories =
        ensure_container(original, BankContainer::Categories).unwrap();
    let accounts = categories.contents.find("  accounts:").unwrap();
    let cats = categories.contents.find("  categories:").unwrap();
    let stale = categories.contents.find("  stale_after:").unwrap();
    assert!(accounts < cats && cats < stale);
    let rules =
        ensure_container(&categories.contents, BankContainer::Rules).unwrap();
    let rules_at = rules.contents.find("  rules:").unwrap();
    let cats = rules.contents.find("  categories:").unwrap();
    let stale = rules.contents.find("  stale_after:").unwrap();
    assert!(cats < rules_at && rules_at < stale);
}

#[test]
fn insert_rule_copies_flow_neighbour_and_block_default() {
    let original = live_flow();
    let draft = RuleDraft {
        match_text: "PAYPAL".to_owned(),
        payee: Some("PayPal".to_owned()),
        cat: Some("refund".to_owned()),
        link: None,
    };
    let edit = insert_rule(original, &draft, SeqPlace::End).unwrap();
    assert!(edit
        .contents
        .contains("- { match: PAYPAL, payee: PayPal, cat: refund }"));
    parse_ok(&edit.contents);
    assert_outside_untouched(original, &edit);

    let block = block_shape();
    let edit = insert_rule(block, &draft, SeqPlace::End).unwrap();
    assert!(edit.contents.contains("    - match: PAYPAL"));
    assert!(edit.contents.contains("      payee: PayPal"));
    parse_ok(&edit.contents);
    assert_outside_untouched(block, &edit);
}

#[test]
fn new_category_without_siblings_is_flow() {
    let original = concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      kind: checking\n",
        "      start: 2026-11-12\n",
        "---\n",
    );
    let ensured =
        ensure_container(original, BankContainer::Categories).unwrap();
    let draft = CategoryDraft {
        key: "coffee".to_owned(),
        label: "☕ Coffee".to_owned(),
        hotkey: Some('c'),
        group: None,
        hidden: None,
    };
    let edit = insert_category(&ensured.contents, &draft, Place::End).unwrap();
    assert!(edit
        .contents
        .contains("coffee: { label: ☕ Coffee, hotkey: c }"));
    parse_ok(&edit.contents);
}

#[test]
fn flow_spacing_is_copied_from_the_neighbour() {
    let original = concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      kind: checking\n",
        "      start: 2026-11-12\n",
        "  categories:\n",
        "    coffee: {label: ☕ Coffee,hotkey: c}\n",
        "---\n",
    );
    let draft = CategoryDraft {
        key: "kids".to_owned(),
        label: "🧸 Kids".to_owned(),
        hotkey: None,
        group: None,
        hidden: Some(true),
    };
    let edit = insert_category(original, &draft, Place::End).unwrap();
    assert!(edit
        .contents
        .contains("kids: {label: 🧸 Kids,hidden: true}"));
    parse_ok(&edit.contents);
}

#[test]
fn refuses_anchors_aliases_tags_duplicates_comments_tabs_and_flow_containers() {
    let cases = [
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "  categories:\n",
                "    coffee: { label: *summary }\n",
                "---\n",
            ),
            "coffee",
        ),
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "  categories:\n",
                "    coffee: &id { label: Coffee }\n",
                "---\n",
            ),
            "coffee",
        ),
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "  categories:\n",
                "    coffee: !tag Coffee\n",
                "---\n",
            ),
            "coffee",
        ),
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "  categories:\n",
                "    coffee: { label: Coffee, label: Cafe }\n",
                "---\n",
            ),
            "coffee",
        ),
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "  categories:\n",
                "    coffee: { label: Coffee # nope }\n",
                "---\n",
            ),
            "coffee",
        ),
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "  categories:\n",
                "    coffee: { label: Coffee,\n",
                "      hotkey: c }\n",
                "---\n",
            ),
            "coffee",
        ),
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "\tcategories:\n",
                "\t  coffee: ☕ Coffee\n",
                "---\n",
            ),
            "coffee",
        ),
        (
            concat!(
                "---\n",
                "bank:\n",
                "  accounts:\n",
                "    checking:\n",
                "      kind: checking\n",
                "      start: 2026-11-12\n",
                "  categories: { coffee: { label: Coffee } }\n",
                "---\n",
            ),
            "coffee",
        ),
    ];
    for (markdown, key) in cases {
        let error = parse_bank_document(markdown)
            .err()
            .or_else(|| {
                replace_category(
                    markdown,
                    &CategoryDraft {
                        key: key.to_owned(),
                        label: "X".to_owned(),
                        hotkey: None,
                        group: None,
                        hidden: None,
                    },
                )
                .err()
            })
            .expect("refusal");
        assert_eq!(error.code, "unsupported_yaml", "{markdown}");
        assert!(error.line.is_some() || error.key_path.is_some(), "{error}");
    }
}

#[test]
fn account_and_rule_primitives_round_trip() {
    let original = mixed_shape();
    let removed = remove_account(original, "savings").unwrap();
    assert!(!removed.contents.contains("savings:"));
    assert!(removed.contents.contains("checking:"));
    parse_ok(&removed.contents);
    assert_outside_untouched(original, &removed);

    let moved =
        move_account(original, "savings", Place::Before("checking")).unwrap();
    let savings = moved.contents.find("savings:").unwrap();
    let checking = moved.contents.find("checking:").unwrap();
    assert!(savings < checking);
    parse_ok(&moved.contents);
    assert_outside_untouched(original, &moved);

    let draft = RuleDraft {
        match_text: "STRIPE".to_owned(),
        payee: Some("Stripe".to_owned()),
        cat: Some("income".to_owned()),
        link: None,
    };
    let replaced = replace_rule(original, 0, &draft).unwrap();
    assert!(replaced
        .contents
        .contains("- { match: STRIPE, cat: income, payee: Stripe }"));
    parse_ok(&replaced.contents);
    assert_outside_untouched(original, &replaced);

    let removed_rule = remove_rule(original, 1).unwrap();
    assert!(!removed_rule.contents.contains("PAYPAL"));
    parse_ok(&removed_rule.contents);
    assert_outside_untouched(original, &removed_rule);

    let moved_rule = move_rule(original, 1, SeqPlace::Before(0)).unwrap();
    let paypal = moved_rule.contents.find("PAYPAL").unwrap();
    let venmo = moved_rule.contents.find("VENMO").unwrap();
    assert!(paypal < venmo);
    parse_ok(&moved_rule.contents);
    assert_outside_untouched(original, &moved_rule);

    let inserted = insert_rule(original, &draft, SeqPlace::After(0)).unwrap();
    let venmo = inserted.contents.find("VENMO").unwrap();
    let stripe = inserted.contents.find("STRIPE").unwrap();
    let paypal = inserted.contents.find("PAYPAL").unwrap();
    assert!(venmo < stripe && stripe < paypal);
    parse_ok(&inserted.contents);
    assert_outside_untouched(original, &inserted);

    let accounts = ensure_container(original, BankContainer::Accounts).unwrap();
    assert_eq!(accounts.contents, original);
}

#[test]
fn duplicate_block_keys_refuse() {
    let markdown = concat!(
        "---\n",
        "bank:\n",
        "  accounts:\n",
        "    checking:\n",
        "      kind: checking\n",
        "      start: 2026-11-12\n",
        "  categories:\n",
        "    coffee: ☕ Coffee\n",
        "    coffee: ☕ Cafe\n",
        "---\n",
    );
    let error = parse_bank_document(markdown).unwrap_err();
    assert_eq!(error.code, "unsupported_yaml");
    assert_eq!(error.line, Some(9));
}
