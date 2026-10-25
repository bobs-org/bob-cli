use std::ffi::OsStr;

use super::{
    super::{
        model::Candidates, render::candidate_lines, shell::shell_completion,
    },
    result, write_file, write_settings, TempDir,
};
use crate::native::{capture_language::CompletionContext, env::TestEnvGuard};

/// Relation-label completion loads the relation catalog, which writes
/// its cache under `XDG_CACHE_HOME`. Point that at a tempdir so `cargo
/// test` never touches the real `~/.cache/bob-cli/relations`.
fn isolate_catalog_env(cache: &std::path::Path) -> TestEnvGuard {
    TestEnvGuard::set(&[
        ("XDG_CACHE_HOME", Some(cache.as_os_str())),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent/bob-cli-test-config.yml")),
        ),
    ])
}

fn relation_fixture() -> TempDir {
    let temp = TempDir::new("bob-cli-capture-complete-relations");
    write_settings(temp.path());
    write_file(
        &temp.path().join("mac_inbox.md"),
        "- [ ] #task Owner ^owner\n  - 🪢 **LINKS:** [[tasks#^target|follow-up of]]\n  - 🪢 **LINKS:** [[tasks#^target|inspired by]]\n  - 🪢 **LINKS:** [[tasks#^target|inspired by]]\n  - ⛓️ **DEPENDS ON:** [[tasks#^target]]\n- [ ] #task Owner neighbor ^owner-neighbor\n",
    );
    write_file(
        &temp.path().join("tasks.md"),
        "- [ ] #task Target ^target\n- [ ] #task Source ^source\n  - 🪢 **LINKS:** follow-up of [[mac_inbox#^owner]]\n  - 🪢 **LINKS:** Inspired By [[mac_inbox#^owner]]\n- [ ] #task Malformed source ^malformed\n  - 🪢 **LINKS:** see (old) [[mac_inbox#^owner]]\n- [ ] #task Idless source\n  - 🪢 **LINKS:** follow-up of [[mac_inbox#^owner]]\n- [/] #task In progress ^progress\n- [*] #task Next ^next\n- [x] #task Closed ^closed\n",
    );
    write_file(
        &temp.path().join("done/archive.md"),
        "- [ ] #task Archived without ID\n- [ ] #task Archived source ^archived-source\n  - 🪢 **LINKS:** follow-up of [[mac_inbox#^owner]]\n",
    );
    temp
}

#[test]
fn relation_target_contexts_use_directional_markers_and_owner_links() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();

    let bare_to = result(temp.path(), "New task ~", "New task ~".len());
    assert_eq!(bare_to.context, Some(CompletionContext::RelationTo));
    let note_prefix = "New task ~tasks:";
    let prefixed = result(temp.path(), note_prefix, note_prefix.len());
    assert_eq!(prefixed.context, Some(CompletionContext::RelationTo));
    assert_eq!(prefixed.query.as_deref(), Some("tasks:"));
    let to_marker = "New task ~tasks:tar";
    let from_marker = "New task ~~tasks:tar";
    let to_flip = result(temp.path(), to_marker, to_marker.len());
    let from_flip = result(temp.path(), from_marker, from_marker.len());
    assert_eq!(to_flip.context, Some(CompletionContext::RelationTo));
    assert_eq!(from_flip.context, Some(CompletionContext::RelationFrom));
    assert_eq!(to_flip.replacement.start, from_flip.replacement.start);

    let raw = "Use this @mac_inbox+owner ~tar";
    let to = result(temp.path(), raw, raw.len());
    assert_eq!(to.context, Some(CompletionContext::RelationTo));
    assert_eq!(to.replacement.start, raw.find('~').expect("sigil"));
    let json = serde_json::to_value(&to).expect("serialize to picker");
    let target = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["block_id"] == "target")
        .expect("target candidate");
    assert_eq!(target["replacement"], "~tasks:target");
    assert_eq!(target["already_linked"], true);
    assert_eq!(target["existing_label"], "follow-up of");
    assert_eq!(target["also_prerequisite"], true);

    let raw = "Use this @mac_inbox+owner ~~sou";
    let from = result(temp.path(), raw, raw.len());
    assert_eq!(from.context, Some(CompletionContext::RelationFrom));
    let json = serde_json::to_value(&from).expect("serialize from picker");
    let source = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["block_id"] == "source")
        .expect("source candidate");
    assert_eq!(source["replacement"], "~~tasks:source");
    assert_eq!(source["already_linked"], true);
    assert_eq!(source["existing_label"], "follow-up of");
    let json = serde_json::to_value(&from).expect("serialize guarded rows");
    let malformed = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["block_id"] == "malformed")
        .expect("malformed source candidate");
    assert_eq!(malformed["disabled_reason"], "its LINKS line is malformed");
    let archived = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["block_id"] == "archived-source")
        .expect("archived source candidate");
    assert_eq!(archived["disabled_reason"], "read-only note");
}

#[test]
fn relation_target_replacement_stops_before_an_existing_label() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    let raw = "New task ~tasks:tar|\"follow-up of\"";
    let cursor = raw.find('|').expect("label separator");
    let value = result(temp.path(), raw, cursor);
    assert_eq!(value.context, Some(CompletionContext::RelationTo));
    assert_eq!(
        &raw[value.replacement.start..value.replacement.end],
        "~tasks:tar"
    );
    assert_eq!(value.query.as_deref(), Some("tasks:tar"));
}

#[test]
fn relation_pools_follow_directional_tiers_and_editability() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    let raw = "Use this @mac_inbox+owner ~";
    let to = result(temp.path(), raw, raw.len());
    let json = serde_json::to_value(to).expect("serialize relation-to rows");
    let texts = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["text"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            "Owner",
            "Owner neighbor",
            "In progress",
            "Next",
            "Target",
            "Source",
            "Malformed source",
            "Idless source",
            "Closed",
            "Archived without ID",
            "Archived source",
        ]
    );

    let raw = "Use this @mac_inbox+owner ~~";
    let from = result(temp.path(), raw, raw.len());
    let json =
        serde_json::to_value(from).expect("serialize relation-from rows");
    let rows = json["candidates"].as_array().unwrap();
    let texts = rows
        .iter()
        .map(|row| row["text"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(!texts.contains(&"Closed"));
    let archived = rows
        .iter()
        .find(|row| row["text"] == "Archived source")
        .expect("read-only archived source remains visible");
    assert_eq!(archived["disabled_reason"], "read-only note");
}

#[test]
fn relation_label_suggestions_count_links_and_put_typed_spelling_first() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    let raw = "Use this @mac_inbox+owner ~tasks:target|follow";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RelationLabel));
    assert_eq!(
        &raw[value.replacement.start..value.replacement.end],
        "|follow"
    );
    let Candidates::RelationLabel(rows) = &value.candidates else {
        panic!("expected relation label suggestions");
    };
    assert_eq!(rows[0].label, "follow");
    assert!(rows[0].typed);
    assert_eq!(rows[0].replacement, "|follow");
    assert!(rows.iter().any(|row| {
        row.label == "follow-up of"
            && row.count == Some(4)
            && row.replacement == "|\"follow-up of\""
    }));
    assert_eq!(serde_json::to_value(&value).unwrap()["hidden_count"], 0);
    let canonical = result(
        temp.path(),
        "New task ~tasks:target|\"INSPIRED BY",
        "New task ~tasks:target|\"INSPIRED BY".len(),
    );
    let canonical_json = serde_json::to_value(canonical).unwrap();
    let typed = &canonical_json["candidates"][0];
    assert_eq!(typed["label"], "inspired by");
    assert_eq!(typed["key"], "inspired by");
    assert_eq!(typed["existing"], true);
    assert_eq!(typed["count"], 3);
    assert_eq!(typed["replacement"], "|\"inspired by\"");

    write_file(
        &temp.path().join(".bob/relations/test.jsonl"),
        "{\"v\":1,\"ts\":\"2026-10-24T09:41:07Z\",\"host\":\"test\",\"kind\":\"relation_label.hide\",\"key\":\"follow-up of\",\"name\":\"follow-up of\",\"via\":\"test\"}\n",
    );
    let prefix = result(
        temp.path(),
        "New task ~tasks:target|follow",
        "New task ~tasks:target|follow".len(),
    );
    let prefix_json = serde_json::to_value(prefix).unwrap();
    assert_eq!(prefix_json["hidden_count"], 1);
    assert!(!prefix_json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["key"] == "follow-up of"));
    let exact = result(
        temp.path(),
        "New task ~tasks:target|\"follow-up of",
        "New task ~tasks:target|\"follow-up of".len(),
    );
    let exact_json = serde_json::to_value(exact).unwrap();
    assert_eq!(exact_json["candidates"][0]["typed"], true);
    assert_eq!(exact_json["candidates"][0]["existing"], true);
    assert_eq!(exact_json["candidates"][0]["hidden"], true);
    assert_eq!(exact_json["candidates"][0]["count"], 4);

    let tasks_path = temp.path().join("tasks.md");
    let mut tasks = std::fs::read_to_string(&tasks_path).unwrap();
    tasks.push_str(
        "- [ ] #task New source ^new-source\n  - 🪢 **LINKS:** prompted by [[mac_inbox#^owner]]\n",
    );
    write_file(&tasks_path, &tasks);
    crate::native::relation_labels::invalidate_scan(temp.path());
    let refreshed = result(
        temp.path(),
        "New task ~tasks:target|prompted",
        "New task ~tasks:target|prompted".len(),
    );
    let refreshed_json = serde_json::to_value(refreshed).unwrap();
    let refreshed_row = refreshed_json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "prompted by")
        .expect("new relation label appears after the note changes");
    assert_eq!(refreshed_row["count"], 1);
    let raw = "Use this @mac_inbox+owner ~";
    let to = result(temp.path(), raw, raw.len());
    let json = serde_json::to_value(to).expect("serialize archived rows");
    let archived = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["text"] == "Archived without ID")
        .expect("archived task candidate");
    assert_eq!(
        archived["disabled_reason"],
        "archived tasks can't get an id"
    );
    assert_eq!(
        candidate_lines(&value.candidates, value.context)[0].0,
        "|follow"
    );
    let raw = "New task ~tasks:target|\"see (old";
    let invalid = result(temp.path(), raw, raw.len());
    assert_eq!(invalid.context, Some(CompletionContext::RelationLabel));
    let Candidates::RelationLabel(rows) = invalid.candidates else {
        panic!("expected invalid typed relation label");
    };
    assert_eq!(rows[0].label, "see (old");
    assert!(rows[0].typed);
    assert_eq!(rows[0].replacement, "");
    assert_eq!(
        rows[0].disabled_reason.as_deref(),
        Some("label contains a forbidden character")
    );
}

#[test]
fn relation_label_completions_carry_relation_and_owner() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    for (raw, direction, token, separator) in [
        ("New task ~tasks:target|", "to", "~tasks:target", 22),
        ("New task ~~tasks:source|", "from", "~~tasks:source", 23),
        (
            "New task ~\"My Note\":target|",
            "to",
            "~\"My Note\":target",
            26,
        ),
    ] {
        let value = result(temp.path(), raw, raw.len());
        assert_eq!(
            value.context,
            Some(CompletionContext::RelationLabel),
            "{raw}"
        );
        assert_eq!(
            (value.replacement.start, value.replacement.end),
            (separator, raw.len()),
            "{raw}"
        );
        let json =
            serde_json::to_value(&value).expect("serialize relation label");
        assert_eq!(json["relation"]["direction"], direction, "{raw}");
        assert_eq!(
            json["relation"]["range"],
            serde_json::json!({
                "start": raw.find(token).expect("token"),
                "end": separator,
            }),
            "{raw}"
        );
        assert_eq!(
            json["replacement"],
            serde_json::json!({ "start": separator, "end": raw.len() }),
            "{raw}"
        );
        assert_eq!(json["owner"]["text"], "New task", "{raw}");
    }

    // Inside a typed label the replacement owns the pipe plus the partial
    // label, while the relation range still ends at the separator.
    let raw = "Use this @mac_inbox+owner ~tasks:target|follow";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RelationLabel));
    let json = serde_json::to_value(&value).expect("serialize label");
    assert_eq!(
        &raw[value.replacement.start..value.replacement.end],
        "|follow"
    );
    assert_eq!(json["relation"]["direction"], "to");
    assert_eq!(
        json["relation"]["range"]["end"],
        json["replacement"]["start"]
    );
    assert_eq!(json["owner"]["text"], "Owner");

    // Target contexts carry no relation object.
    let raw = "New task ~tasks:tar";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RelationTo));
    let json = serde_json::to_value(&value).expect("serialize target");
    assert!(json.get("relation").is_none(), "{json}");
    let raw = "New task ~~tasks:sou";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RelationFrom));
    let json = serde_json::to_value(&value).expect("serialize target");
    assert!(json.get("relation").is_none(), "{json}");
}

#[test]
fn relation_label_double_pipe_keeps_pipes_and_reports_one_off() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    // A `||` probe reads the label text past both pipes: the query drops
    // the second pipe, the typed replacement keeps `||`, every row gains
    // the `||` hide form, and the response reports `one_off`.
    let raw = "Use this @mac_inbox+owner ~tasks:target||follow";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RelationLabel));
    assert_eq!(value.query.as_deref(), Some("follow"));
    assert_eq!(
        &raw[value.replacement.start..value.replacement.end],
        "||follow"
    );
    let json = serde_json::to_value(&value).expect("serialize one-off");
    assert_eq!(json["one_off"], true);
    let rows = json["candidates"].as_array().expect("rows");
    let typed = &rows[0];
    assert_eq!(typed["typed"], true);
    assert_eq!(typed["replacement"], "||follow");
    assert_eq!(typed["hide_replacement"], "||follow");
    let catalog = rows
        .iter()
        .find(|row| row["key"] == "follow-up of")
        .expect("catalog row");
    assert_eq!(catalog["replacement"], "||\"follow-up of\"");
    assert_eq!(catalog["hide_replacement"], "||\"follow-up of\"");

    // The empty `||` probe is the same context with an empty query.
    let raw = "Use this @mac_inbox+owner ~tasks:target||";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RelationLabel));
    assert_eq!(value.query.as_deref(), Some(""));
    assert_eq!(&raw[value.replacement.start..value.replacement.end], "||");
    let json = serde_json::to_value(&value).expect("serialize empty probe");
    assert_eq!(json["one_off"], true);

    // A `|` probe omits `one_off` but still carries `hide_replacement`
    // on every row.
    let raw = "Use this @mac_inbox+owner ~tasks:target|follow";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RelationLabel));
    let json = serde_json::to_value(&value).expect("serialize pipe probe");
    assert!(json.get("one_off").is_none(), "{json}");
    for row in json["candidates"].as_array().expect("rows") {
        let hide = row["hide_replacement"].as_str().expect("hide form");
        assert!(hide.starts_with("||"), "{row}");
    }
    let typed = &json["candidates"][0];
    assert_eq!(typed["replacement"], "|follow");
}

#[test]
fn relation_label_leading_and_dependency_tail_keep_narrow_ranges() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    for (raw, token_start, separator, owner) in [
        ("~tasks:target| Fix it", 0, 13, "Fix it"),
        ("Fix it ~tasks:target| &tasks:source", 7, 20, "Fix it"),
    ] {
        let cursor = separator + 1;
        let value = result(temp.path(), raw, cursor);
        assert_eq!(
            value.context,
            Some(CompletionContext::RelationLabel),
            "{raw}"
        );
        assert_eq!(
            (value.replacement.start, value.replacement.end),
            (separator, cursor),
            "{raw}"
        );
        let json =
            serde_json::to_value(&value).expect("serialize leading/tail");
        assert_eq!(json["relation"]["direction"], "to", "{raw}");
        assert_eq!(
            json["relation"]["range"],
            serde_json::json!({ "start": token_start, "end": separator }),
            "{raw}"
        );
        assert_eq!(
            json["replacement"],
            serde_json::json!({ "start": separator, "end": cursor }),
            "{raw}"
        );
        assert_eq!(json["owner"]["text"], owner, "{raw}");
    }
}

#[test]
fn parent_picker_reports_typed_relation_direction_and_target_text() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    let raw = "~tasks:target New task +";
    let value = result(temp.path(), raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::TaskParent));
    let json = serde_json::to_value(value).expect("serialize parent picker");
    assert_eq!(
        json["picker"]["relationship"]["relations"],
        serde_json::json!([
            { "direction": "to", "text": "Target" }
        ])
    );
}

#[test]
fn relation_shell_groups_are_directional_and_skip_guarded_rows() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let temp = relation_fixture();
    for (raw, group, expected) in [
        ("New task ~", "links to", "~tasks:target"),
        ("New task ~~", "linked from", "~~tasks:source"),
    ] {
        let completion = shell_completion(temp.path(), raw, raw.len())
            .expect("shell completion")
            .expect("relation shell rows");
        assert!(completion
            .rows
            .iter()
            .any(|row| { row.full == expected && row.group == group }));
        assert!(completion
            .rows
            .iter()
            .all(|row| { !row.full.contains("Archived without ID") }));
    }
}
