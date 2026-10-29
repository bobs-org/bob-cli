//! Relation capture grammar contract (`~` / `~~`): lexer, guards,
//! labels, ownership, spans, and additive JSON (plan:202610/task_relations.md
//! capture-parse phase, LP1-LP18).

use crate::support::*;

fn parse_json(draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-parse json");
    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

// LP1: complete relations in both directions, one with a label, and owner reuse.
#[test]
fn relation_complete_to_from_label_and_owner_reuse() {
    let to = parse_json("Fix bug ~work:foo");
    assert_eq!(to["body"], "Fix bug");
    assert_eq!(to["mode"], "task");
    assert_eq!(
        to["relations"],
        serde_json::json!([{
            "raw": "~work:foo",
            "direction": "to",
            "note": "work",
            "block_id": "foo",
            "quoted": false,
            "range": { "start": 8, "end": 17 },
        }])
    );
    assert_eq!(
        to["dependency_target"],
        serde_json::json!({ "kind": "new_task" })
    );

    let from = parse_json("Fix bug ~~work:foo");
    assert_eq!(from["relations"][0]["direction"], serde_json::json!("from"));

    let labelled = parse_json("Fix bug ~work:foo|see");
    assert_eq!(
        labelled["relations"],
        serde_json::json!([{
            "raw": "~work:foo|see",
            "direction": "to",
            "note": "work",
            "block_id": "foo",
            "quoted": false,
            "label": "see",
            "label_range": { "start": 18, "end": 21 },
            "range": { "start": 8, "end": 21 },
        }])
    );

    // A second `~` on the same item reuses the same dependent.
    let two = parse_json("Fix bug ~work:foo ~~work:bar");
    assert_eq!(two["body"], "Fix bug");
    assert_eq!(two["relations"].as_array().unwrap().len(), 2);
    assert_eq!(
        two["dependency_target"],
        serde_json::json!({ "kind": "new_task" })
    );
}

// LP2: `~` interleaves with `&` / `*` in either order.
#[test]
fn relation_interleaves_with_dependencies_in_either_order() {
    let first = parse_json("Fix ~work:foo &job:bar");
    assert_eq!(first["body"], "Fix");
    assert_eq!(first["dependencies"].as_array().unwrap().len(), 1);
    assert_eq!(first["relations"].as_array().unwrap().len(), 1);

    let second = parse_json("Fix &job:bar ~work:foo");
    assert_eq!(second["body"], "Fix");
    assert_eq!(second["dependencies"].as_array().unwrap().len(), 1);
    assert_eq!(second["relations"].as_array().unwrap().len(), 1);

    let blocked = parse_json("Fix ~work:foo *job:cleanup");
    assert_eq!(blocked["blocked_tasks"].as_array().unwrap().len(), 1);
    assert_eq!(blocked["relations"].as_array().unwrap().len(), 1);
}

// LP3: mid-line `~` stays trailing prose.
#[test]
fn relation_mid_line_stays_prose() {
    let json = parse_json("Fix it ~work:foo now");
    assert_eq!(json["body"], "Fix it ~work:foo now");
    assert!(json.get("relations").is_none());
    assert_eq!(json["relation_grammar"], 1);
}

// LP4: digit and slash guards stay literal.
#[test]
fn relation_digit_and_slash_guards_stay_literal() {
    for draft in [
        "Plan trip ~5m",
        "Plan trip ~2h",
        "Plan trip ~5pm",
        "Clean ~/Downloads",
    ] {
        let json = parse_json(draft);
        assert_eq!(json["body"], draft);
        assert!(json.get("relations").is_none(), "{draft}");
        assert_eq!(json["relation_grammar"], 1, "{draft}");
    }
}

// LP5: strike guard stays body text.
#[test]
fn relation_strike_guard_stays_literal() {
    for draft in ["Read ~~old~~", "Read ~~old~~ notes"] {
        let json = parse_json(draft);
        assert_eq!(json["body"], draft);
        assert!(json.get("relations").is_none(), "{draft}");
    }
}

// LP6: bare and partial targets need `relation_target`.
#[test]
fn relation_bare_and_partial_targets_need_relation_target() {
    for draft in ["Fix it ~", "Fix it ~~", "Fix it ~wo", "Fix it ~work:"] {
        let json = parse_json(draft);
        assert_eq!(
            json["needs"],
            serde_json::json!(["relation_target"]),
            "{draft}"
        );
    }
}

// LP7: labels after `|` add spans and a label.
#[test]
fn relation_labels_after_pipe() {
    let plain = parse_json("Fix it ~work:foo|see");
    assert_eq!(plain["relations"][0]["label"], serde_json::json!("see"));
    assert_eq!(
        plain["spans"],
        serde_json::json!([
            { "start": 7, "end": 8, "kind": "relation_sigil" },
            { "start": 8, "end": 12, "kind": "relation_note" },
            { "start": 13, "end": 16, "kind": "relation_block_id" },
            { "start": 16, "end": 17, "kind": "relation_label_separator" },
            { "start": 17, "end": 20, "kind": "relation_label" },
        ])
    );

    let quoted = parse_json("Fix it ~work:foo|\"follow up of\"");
    assert_eq!(
        quoted["relations"][0]["label"],
        serde_json::json!("follow up of")
    );
}

// LP8: dangling `|` and open quotes need `relation_label`.
#[test]
fn relation_partial_labels_need_relation_label() {
    for draft in ["Fix it ~work:foo|", "Fix it ~work:foo|\"follow up"] {
        let json = parse_json(draft);
        assert_eq!(
            json["needs"],
            serde_json::json!(["relation_label"]),
            "{draft}"
        );
    }
}

// LP9: an invalid label is refused with a teaching message.
#[test]
fn relation_invalid_label_is_refused() {
    let json = parse_json("Fix it ~work:foo|\"a (b)\"");
    let diagnostics = json["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "invalid_relation"
                && diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("forbidden")
        }),
        "{}",
        serde_json::to_string_pretty(&json).unwrap()
    );
}

// LP10: an escape leaves a visible tilde.
#[test]
fn relation_escape_leaves_visible_tilde() {
    let json = parse_json("Fix it \\~work:foo");
    assert_eq!(json["body"], "Fix it ~work:foo");
    assert!(json.get("relations").is_none());
}

// LP11: session `~` drop lists are unchanged; `~~` doubles error.
#[test]
fn relation_session_token_drops_are_unchanged() {
    let close = parse_json("x~1");
    assert!(close.get("relations").is_none());
    let start = parse_json("=~2");
    assert!(start.get("relations").is_none());

    for draft in ["x~~1", "=~~2"] {
        let json = parse_json(draft);
        let messages = json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|diagnostic| {
                diagnostic["message"].as_str().unwrap().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(messages.contains("use one `~` list"), "{draft}: {messages}");
        assert!(json.get("relations").is_none(), "{draft}");
    }
}

// LP12: an ownerless relation reuses `dependency_target`.
#[test]
fn relation_ownerless_reuses_dependency_target() {
    let json = parse_json("~work:foo");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["dependency_target"]));
    assert_eq!(json["relations"][0]["direction"], serde_json::json!("to"));
}

// LP13: a relation after `x` is refused on the operator.
#[test]
fn relation_after_close_is_refused() {
    let json = parse_json("x ~work:foo");
    assert!(json.get("relations").is_some());
    let codes = json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"dependency_unsupported_target".to_string()),
        "{}",
        serde_json::to_string_pretty(&json).unwrap()
    );
}

// LP14: `~~~` is invalid.
#[test]
fn relation_triple_tilde_is_invalid() {
    let json = parse_json("Fix ~~~work:foo");
    let diagnostics = json["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic["code"] == "invalid_relation" }),
        "{}",
        serde_json::to_string_pretty(&json).unwrap()
    );
}

// LP15: code spans and wikilinks are never scanned.
#[test]
fn relation_code_and_wikilinks_stay_literal() {
    let code = parse_json("Fix `~work:foo`");
    assert!(code.get("relations").is_none());
    let wiki = parse_json("Fix [[~work:foo]]");
    assert!(wiki.get("relations").is_none());
}

// LP16: a clock-time lookalike still opens the picker.
#[test]
fn relation_clock_lookalike_needs_relation_target() {
    let json = parse_json("Meet ~noon");
    assert_eq!(json["needs"], serde_json::json!(["relation_target"]));
}

// LP17: a relationship-only existing owner reports `task_dependency`.
#[test]
fn relation_only_existing_owner_reports_task_dependency() {
    let json = parse_json("@bob+fix ~sase:retry");
    assert_eq!(json["mode"], "task_dependency");
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": "bob",
            "block_id": "fix",
        })
    );
    assert_eq!(
        json["relations"][0]["raw"],
        serde_json::json!("~sase:retry")
    );
}

// LP18: drafts without `~` stay byte-identical (no `relation_grammar`).
#[test]
fn relation_grammar_absent_without_tilde() {
    let json = parse_json("Fix bug");
    assert!(json.get("relations").is_none());
    assert!(json.get("relation_grammar").is_none());
}

// LD1: `||` marks the label hidden, keeps the plain spelling, and spans
// both pipes.
#[test]
fn relation_double_pipe_marks_label_hidden() {
    let json = parse_json("Fix bug ~work:foo||see");
    assert_eq!(
        json["relations"],
        serde_json::json!([{
            "raw": "~work:foo||see",
            "direction": "to",
            "note": "work",
            "block_id": "foo",
            "quoted": false,
            "hidden": true,
            "label": "see",
            "label_range": { "start": 19, "end": 22 },
            "range": { "start": 8, "end": 22 },
        }])
    );
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 8, "end": 9, "kind": "relation_sigil" },
            { "start": 9, "end": 13, "kind": "relation_note" },
            { "start": 14, "end": 17, "kind": "relation_block_id" },
            { "start": 17, "end": 19, "kind": "relation_label_separator" },
            { "start": 19, "end": 22, "kind": "relation_label" },
        ])
    );

    let from = parse_json("Fix bug ~~work:foo||see");
    assert_eq!(from["relations"][0]["direction"], "from");
    assert_eq!(from["relations"][0]["hidden"], true);

    let quoted = parse_json("Fix bug ~work:foo||\"follow up of\"");
    assert_eq!(quoted["relations"][0]["hidden"], true);
    assert_eq!(quoted["relations"][0]["label"], "follow up of");
}

// LD2: an empty `||` still needs `relation_label`.
#[test]
fn relation_empty_double_pipe_needs_relation_label() {
    for draft in ["Fix it ~work:foo||", "Fix it ~~work:foo||"] {
        let json = parse_json(draft);
        assert_eq!(
            json["needs"],
            serde_json::json!(["relation_label"]),
            "{draft}"
        );
        assert!(json.get("relations").is_none(), "{draft}");
    }
}

// LD3: a third pipe is a teaching error, not a label.
#[test]
fn relation_triple_pipe_is_teaching_error() {
    for draft in ["Fix it ~work:foo|||see", "Fix it ~work:foo|||"] {
        let json = parse_json(draft);
        assert!(json.get("relations").is_none(), "{draft}");
        let diagnostics = json["diagnostics"].as_array().unwrap();
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic["code"] == "invalid_relation"
                    && diagnostic["message"]
                        == "use || to hide a label, | to name one"
            }),
            "{}",
            serde_json::to_string_pretty(&json).unwrap()
        );
    }
}

// LD4: a single `|` never reports `hidden`, so older JSON stays
// byte-identical.
#[test]
fn relation_single_pipe_omits_hidden() {
    let json = parse_json("Fix bug ~work:foo|see");
    assert!(json["relations"][0].get("hidden").is_none());
    let partial = parse_json("Fix bug ~work:foo|");
    assert_eq!(partial["needs"], serde_json::json!(["relation_label"]));
}

// Cancel-label spans (plan:202610/relation_cancel_labels.md cancel-surfaces):
// a configured cancel label flags its `relation_label` span.
#[test]
fn relation_label_spans_flag_configured_cancel_labels() {
    let temp = TempDir::new("bob-cli-capture-parse-cancel-spans");
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        "relations:\n  cancel_labels:\n    - duplicate of\n",
    );
    let parsed = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix trip ~work:foo|\"duplicate of\" ~work:bar|see")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run capture-parse cancel spans");
    assert_success(&parsed);
    assert!(
        stderr(&parsed).is_empty(),
        "clean config warns nothing:\n{}",
        format_output(&parsed)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&parsed).trim()).expect("parse JSON");
    let spans = json["spans"].as_array().expect("spans");
    let cancel = spans
        .iter()
        .find(|span| {
            span["kind"] == "relation_label"
                && json["input"].as_str().expect("input").get(
                    span["start"].as_u64().expect("start") as usize
                        ..span["end"].as_u64().expect("end") as usize,
                ) == Some("duplicate of")
        })
        .expect("cancel label span");
    assert_eq!(cancel["cancels_source"], true);
    let plain = spans
        .iter()
        .find(|span| {
            span["kind"] == "relation_label"
                && json["input"].as_str().expect("input").get(
                    span["start"].as_u64().expect("start") as usize
                        ..span["end"].as_u64().expect("end") as usize,
                ) == Some("see")
        })
        .expect("plain label span");
    assert!(plain.get("cancels_source").is_none());

    // Without config every span stays byte-identical (no flag key).
    let bare = parse_json("Fix trip ~work:foo|\"duplicate of\"");
    assert!(
        bare["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .all(|span| span.get("cancels_source").is_none()),
        "{}",
        bare["spans"]
    );
}

#[test]
fn relation_label_spans_flag_configured_complete_labels() {
    let temp = TempDir::new("bob-cli-capture-parse-complete-spans");
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        "relations:\n  complete_labels:\n    - implemented by\n",
    );
    let parsed = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix trip ~work:foo|\"implemented by\" ~work:bar|see")
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run capture-parse complete spans");
    assert_success(&parsed);
    assert!(stderr(&parsed).is_empty());
    let json: serde_json::Value =
        serde_json::from_str(stdout(&parsed).trim()).expect("parse JSON");
    let spans = json["spans"].as_array().expect("spans");
    let complete = spans
        .iter()
        .find(|span| {
            span["kind"] == "relation_label"
                && json["input"].as_str().unwrap().get(
                    span["start"].as_u64().unwrap() as usize
                        ..span["end"].as_u64().unwrap() as usize,
                ) == Some("implemented by")
        })
        .expect("complete label span");
    assert_eq!(complete["completes_source"], true);
    let plain = spans
        .iter()
        .find(|span| {
            span["kind"] == "relation_label"
                && json["input"].as_str().unwrap().get(
                    span["start"].as_u64().unwrap() as usize
                        ..span["end"].as_u64().unwrap() as usize,
                ) == Some("see")
        })
        .expect("plain label span");
    assert!(plain.get("completes_source").is_none());

    let bare = parse_json("Fix trip ~work:foo|\"implemented by\"");
    assert!(bare["spans"]
        .as_array()
        .unwrap()
        .iter()
        .all(|span| { span.get("completes_source").is_none() }));
}
