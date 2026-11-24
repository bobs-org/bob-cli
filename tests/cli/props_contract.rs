use super::*;

const AREA: &str = "---\ntype: '[[area]]'\nparent: '[[area]]'\nsummary: Home\noffclock: true\noffclock_types: [MEET]\nready_cap: off\n---\n# Mac Inbox\n";

fn homes(vault: &Path) {
    write_note(vault, "area.md", "# Areas\n");
    write_note(vault, "dev.md", "---\ntype: '[[area]]'\nparent: '[[area]]'\nproject_name_aliases: [shared]\n---\n");
    write_note(vault, "mac_inbox.md", AREA);
}

fn envelope(vault: &Path, note: &str, ops: Value) -> String {
    json!({"schema_version":1,"expect":fingerprint(vault, note),"ops":ops})
        .to_string()
}

#[test]
fn home_settings_preview_apply_and_undo_preserve_bytes() {
    let vault = vault("settings");
    homes(vault.path());
    let input = envelope(
        vault.path(),
        "mac_inbox.md",
        json!([
            {"op":"setting.set","setting":"clock","value":{"mode":"on"}},
            {"op":"setting.set","setting":"project_name_aliases","value":["shared","dev"]},
            {"op":"setting.set","setting":"summary","value":"x".repeat(141)}
        ]),
    );
    let shown = report(&show(vault.path(), "mac_inbox.md", &[]));
    let cap = shown["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == "ready_cap")
        .unwrap();
    assert_eq!(cap["setting"], "ready_cap");
    assert_eq!(cap["role"], "loose");
    let before_mtime = mtime(&vault.path().join("mac_inbox.md"));
    let preview =
        update(vault.path(), "mac_inbox.md", &input, &["-d", "-f", "json"]);
    assert_success(&preview);
    let preview = report(&preview);
    assert_eq!(
        fs::read_to_string(vault.path().join("mac_inbox.md")).unwrap(),
        AREA
    );
    assert_eq!(mtime(&vault.path().join("mac_inbox.md")), before_mtime);
    let warnings = preview["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w["code"] == "area_summary_long"));
    assert_eq!(
        warnings
            .iter()
            .filter(|w| w["code"] == "project_name_aliases_conflict")
            .count(),
        2
    );
    let applied = update(vault.path(), "mac_inbox.md", &input, &["-f", "json"]);
    assert_success(&applied);
    let applied = report(&applied);
    assert_eq!(applied["inverse_ops"], preview["inverse_ops"]);
    let undo = json!({"schema_version":1,"expect":applied["fingerprint_after"],"ops":applied["inverse_ops"]});
    assert_success(&update(
        vault.path(),
        "mac_inbox.md",
        &undo.to_string(),
        &["-f", "json"],
    ));
    assert_eq!(
        fs::read_to_string(vault.path().join("mac_inbox.md")).unwrap(),
        AREA
    );
}

#[test]
fn bank_hybrid_and_refused_batches_never_write() {
    let vault = vault("bank-settings");
    homes(vault.path());
    let bank = "---\ntype: '[[area]]'\nparent: '[[dev]]'\nsummary: Bank\nbank: {currency: USD}\n---\n# 🏦 Bank\n";
    write_note(vault.path(), "bank.md", bank);
    let shown = report(&show(vault.path(), "bank.md", &[]));
    assert_eq!(shown["note_kind"], "bank");
    assert_eq!(shown["properties"][2]["setting"], "summary");
    assert_eq!(shown["properties"][3]["role"], "card");
    for op in [
        json!({"op":"prop.set_yaml","key":"ready_cap","yaml":"1000"}),
        json!({"op":"prop.set","key":"task_refresh","value":0}),
        json!({"op":"prop.unset","key":"parent"}),
        json!({"op":"setting.set","setting":"root","value":true}),
        json!({"op":"prop.set","key":"bank","value":{}}),
    ] {
        let input = envelope(
            vault.path(),
            "bank.md",
            json!([
                {"op":"setting.set","setting":"color","value":"red"}, op
            ]),
        );
        let output = update(vault.path(), "bank.md", &input, &["-f", "json"]);
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        assert_eq!(report(&output)["error"]["op_index"], 1);
        assert_eq!(
            fs::read_to_string(vault.path().join("bank.md")).unwrap(),
            bank
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn parent_validation_reloads_the_destination_and_ancestry_under_lock() {
    for changed in [
        "---\ntype: '[[notebook]]'\nparent: '[[area]]'\n---\n",
        "---\ntype: '[[area]]'\nparent: '[[mac_inbox]]'\n---\n",
    ] {
        let vault = vault("parent-lock");
        homes(vault.path());
        let input = envelope(
            vault.path(),
            "mac_inbox.md",
            json!([{"op":"setting.set","setting":"parent","value":"[[dev]]"}]),
        );
        let held = hold_lock(vault.path());
        let child = spawn_update(vault.path(), "mac_inbox.md", &input);
        wait_until_child_opens_lock(
            child.id(),
            &vault.path().join("vault.lock"),
        );
        write_note(vault.path(), "dev.md", changed);
        drop(held);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        assert!(matches!(
            report(&output)["error"]["code"].as_str(),
            Some("prop_parent_kind" | "prop_parent_cycle")
        ));
        assert_eq!(
            fs::read_to_string(vault.path().join("mac_inbox.md")).unwrap(),
            AREA
        );
    }
}

fn setting<'a>(shown: &'a Value, id: &str) -> &'a Value {
    shown["contract"]["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("missing setting {id} in {shown}"))
}

#[test]
fn show_contract_covers_inherited_hue_shadowed_clock_inbox_and_bank() {
    let vault = vault("contract-show");
    homes(vault.path());
    write_note(
        vault.path(),
        "dev.md",
        "---\ntype: '[[area]]'\nparent: '[[area]]'\ncolor: blue\n---\n",
    );
    write_note(
        vault.path(),
        "child.md",
        "---\ntype: '[[area]]'\nparent: '[[dev]]'\n---\n# Child\n",
    );
    let mut text = bob_command();
    text.args(["props", "show", "child.md"])
        .env("BOB_DIR", vault.path())
        .env("NO_COLOR", "1");
    let text = text.output().expect("text show");
    assert_success(&text);
    let shown_text = stdout(&text);
    assert!(
        shown_text.contains("Settings") && shown_text.contains("Hue"),
        "{shown_text}"
    );

    let child = report(&show(vault.path(), "child.md", &[]));
    assert_eq!(
        child["capabilities"],
        json!(["props_v1", "props_contract_v1"])
    );
    assert_eq!(child["contract"]["kind"], "area");
    let color = setting(&child, "color");
    assert_eq!(color["declared"], false);
    assert_eq!(color["effective"]["source"], "inherited");
    assert_eq!(color["effective"]["value"], "blue");
    assert_eq!(color["effective"]["from"], "dev.md");
    assert!(color["choices"].as_array().unwrap().len() >= 9);

    let inbox = report(&show(vault.path(), "mac_inbox.md", &[]));
    let clock = setting(&inbox, "clock");
    assert_eq!(clock["declared"], true);
    assert!(clock["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "note_offclock_shadowed"));
    let cap = setting(&inbox, "ready_cap");
    assert_eq!(cap["effective"]["value"], "off");
    assert_eq!(cap["effective"]["source"], "self");

    let bank = "---\ntype: '[[area]]'\nparent: '[[dev]]'\nsummary: Bank\nbank: {currency: USD}\n---\n# 🏦 Bank\n";
    write_note(vault.path(), "bank.md", bank);
    let shown = report(&show(vault.path(), "bank.md", &[]));
    assert_eq!(shown["note_kind"], "bank");
    assert_eq!(
        shown["capabilities"],
        json!(["props_v1", "props_contract_v1"])
    );
    assert_eq!(shown["contract"]["kind"], "bank");
    assert_eq!(setting(&shown, "summary")["effective"]["value"], "Bank");
}

#[test]
fn show_contract_covers_root_blocked_and_terminal_projects() {
    let vault = vault("contract-projects");
    homes(vault.path());
    write_note(
        vault.path(),
        "sase.md",
        "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: wip\n---\n- [?] #task #prj Parent work ^prj\n  - ⛓️ **DEPENDS ON:** [[need#^need]]\n",
    );
    write_note(
        vault.path(),
        "need.md",
        "---\ntype: '[[area]]'\nparent: '[[area]]'\n---\n- [ ] #task Prerequisite ^need\n",
    );
    write_note(
        vault.path(),
        "root.md",
        "---\ntype: '[[project]]'\nparent: '[[sase]]'\nroot: true\nstatus: wip\n---\n- [ ] #task #prj Root work ^prj\n",
    );
    write_note(
        vault.path(),
        "child.md",
        "---\ntype: '[[project]]'\nparent: '[[sase]]'\nstatus: wip\n---\n- [?] #task #prj Child work ^prj\n  - ⛓️ **DEPENDS ON:** [[need#^need]]\n",
    );
    write_note(
        vault.path(),
        "done.md",
        "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: done\n---\n- [x] #task #prj Shipped [completion:: 2026-11-22] ^prj\n",
    );
    let root = report(&show(vault.path(), "root.md", &[]));
    assert_eq!(setting(&root, "root")["effective"]["value"], true);
    assert_eq!(setting(&root, "root")["disabled"], false);
    assert_eq!(root["contract"]["prj"]["criterion"], "Root work");
    let child = report(&show(vault.path(), "child.md", &[]));
    assert_eq!(
        child["contract"]["prj"]["depends_on"],
        json!(["need#^need"])
    );
    let done = report(&show(vault.path(), "done.md", &[]));
    assert_eq!(done["contract"]["prj"]["criterion"], "Shipped");
}

#[test]
fn update_json_returns_candidate_contract_after_clear_apply_and_undo() {
    let vault = vault("update-contract");
    homes(vault.path());
    write_note(
        vault.path(),
        "dev.md",
        "---\ntype: '[[area]]'\nparent: '[[area]]'\ncolor: blue\n---\n",
    );
    let child = "---\ntype: '[[area]]'\nparent: '[[dev]]'\ncolor: red\nready_cap: 3\ntask_refresh: 2\n---\n# Child\n";
    write_note(vault.path(), "child.md", child);
    let groceries = "---\ntitle: Groceries\ncount: 3\n---\n\n# Groceries\n";
    write_note(vault.path(), "notes/groceries.md", groceries);
    let bank = "---\ntype: '[[area]]'\nparent: '[[dev]]'\nsummary: Bank\nbank: {currency: USD}\n---\n# 🏦 Bank\n";
    write_note(vault.path(), "bank.md", bank);

    let noop = envelope(vault.path(), "child.md", json!([]));
    let noop_preview = report(&update(
        vault.path(),
        "child.md",
        &noop,
        &["-d", "-f", "json"],
    ));
    assert_eq!(
        fs::read_to_string(vault.path().join("child.md")).unwrap(),
        child
    );
    assert_eq!(noop_preview["contract"]["kind"], "area");
    assert_eq!(setting(&noop_preview, "color")["declared"], true);
    assert_eq!(
        setting(&noop_preview, "color")["effective"]["source"],
        "self"
    );

    let input = envelope(
        vault.path(),
        "child.md",
        json!([
            {"op":"setting.clear","setting":"color"},
            {"op":"setting.clear","setting":"ready_cap"},
            {"op":"setting.clear","setting":"task_refresh"}
        ]),
    );
    let before_mtime = mtime(&vault.path().join("child.md"));
    let preview_output =
        update(vault.path(), "child.md", &input, &["-d", "-f", "json"]);
    assert_success(&preview_output);
    let preview = report(&preview_output);
    assert_eq!(
        fs::read_to_string(vault.path().join("child.md")).unwrap(),
        child
    );
    assert_eq!(mtime(&vault.path().join("child.md")), before_mtime);
    let color = setting(&preview, "color");
    assert_eq!(color["declared"], false);
    assert_eq!(color["value"], json!(null));
    assert_eq!(color["effective"]["source"], "inherited");
    assert_eq!(color["effective"]["value"], "blue");
    assert_eq!(color["effective"]["from"], "dev.md");
    assert_eq!(color["effective"]["label"], "● blue · from dev");
    let cap = setting(&preview, "ready_cap");
    assert_eq!(cap["declared"], false);
    assert_eq!(cap["effective"]["source"], "config");
    assert_eq!(
        cap["effective"]["label"],
        "5 · Bob setting (plan.max_ready_per_note)"
    );
    let review = setting(&preview, "task_refresh");
    assert_eq!(review["declared"], false);
    assert_eq!(review["effective"]["source"], "config");
    assert_eq!(
        review["effective"]["label"],
        "7 days · Bob setting (freshness.interval)"
    );

    let applied_output =
        update(vault.path(), "child.md", &input, &["-f", "json"]);
    assert_success(&applied_output);
    let applied = report(&applied_output);
    assert_eq!(applied["contract"], preview["contract"]);
    let shown = report(&show(vault.path(), "child.md", &[]));
    assert_eq!(applied["contract"], shown["contract"]);

    let undo = json!({
        "schema_version": 1,
        "expect": applied["fingerprint_after"],
        "ops": applied["inverse_ops"]
    });
    let restored_output =
        update(vault.path(), "child.md", &undo.to_string(), &["-f", "json"]);
    assert_success(&restored_output);
    let restored = report(&restored_output);
    assert_eq!(
        fs::read_to_string(vault.path().join("child.md")).unwrap(),
        child
    );
    assert_eq!(setting(&restored, "color")["declared"], true);
    assert_eq!(setting(&restored, "color")["effective"]["source"], "self");
    assert_eq!(setting(&restored, "ready_cap")["declared"], true);
    assert_eq!(setting(&restored, "task_refresh")["declared"], true);

    let bank_input = envelope(vault.path(), "bank.md", json!([]));
    let bank_preview = report(&update(
        vault.path(),
        "bank.md",
        &bank_input,
        &["-d", "-f", "json"],
    ));
    assert_eq!(
        fs::read_to_string(vault.path().join("bank.md")).unwrap(),
        bank
    );
    assert_eq!(bank_preview["contract"]["kind"], "bank");
    assert_eq!(
        setting(&bank_preview, "summary")["effective"]["value"],
        "Bank"
    );

    let ordinary = envelope(
        vault.path(),
        "notes/groceries.md",
        json!([{"op":"prop.set","key":"count","value":4}]),
    );
    let ordinary_preview = report(&update(
        vault.path(),
        "notes/groceries.md",
        &ordinary,
        &["-d", "-f", "json"],
    ));
    assert!(
        ordinary_preview.get("contract").is_none(),
        "{ordinary_preview}"
    );
    assert_eq!(
        fs::read_to_string(vault.path().join("notes/groceries.md")).unwrap(),
        groceries
    );
}

#[test]
fn parent_rehome_dry_run_names_catalog_syncs() {
    let vault = vault("contract-effects");
    homes(vault.path());
    write_note(
        vault.path(),
        "body.md",
        "---\ntype: '[[area]]'\nparent: '[[area]]'\ncolor: red\n---\n",
    );
    write_note(
        vault.path(),
        "sase.md",
        "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: wip\n---\n- [ ] #task #prj Work ^prj\n",
    );
    let input = envelope(
        vault.path(),
        "sase.md",
        json!([{"op":"setting.set","setting":"parent","value":"[[body]]"}]),
    );
    let preview = report(&update(
        vault.path(),
        "sase.md",
        &input,
        &["-d", "-f", "json"],
    ));
    let effects = preview["effects"].as_array().unwrap();
    assert!(
        effects.iter().any(|effect| {
            effect["kind"] == "syncs"
                && effect["syncs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|name| name == "projects")
        }),
        "{effects:?}"
    );
    assert!(
        fs::read_to_string(vault.path().join("sase.md"))
            .unwrap()
            .contains("parent: '[[dev]]'"),
        "dry-run must not write"
    );
    let text = update(vault.path(), "sase.md", &input, &["-d"]);
    assert_success(&text);
    let shown = stdout(&text);
    assert!(
        shown.contains("Syncs") && shown.contains("projects"),
        "{shown}"
    );
}

#[test]
fn areas_and_projects_list_include_contract_issues() {
    let vault = vault("contract-list");
    homes(vault.path());
    write_note(
        vault.path(),
        "stained.md",
        "---\ntype: '[[area]]'\nparent: '[[area]]'\ncolor: puce\noffclock: true\noffclock_types: [MEET]\n---\n# Stained\n",
    );
    write_note(
        vault.path(),
        "sase.md",
        "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: wip\noffclock: true\noffclock_types: [MEET]\n---\n- [ ] #task #prj Work ^prj\n",
    );
    let mut areas = bob_command();
    areas
        .args(["areas", "list", "-f", "json"])
        .env("BOB_DIR", vault.path())
        .env("NO_COLOR", "1");
    let areas = areas.output().expect("areas list");
    assert_success(&areas);
    let areas = report(&areas);
    let stained = areas["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["stem"] == "stained")
        .unwrap();
    let codes: Vec<&str> = stained["contract_issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"area_color_invalid"), "{codes:?}");
    assert!(codes.contains(&"note_offclock_shadowed"), "{codes:?}");
    assert!(stained["issues"].is_array());

    let mut projects = bob_command();
    projects
        .args(["projects", "list", "-f", "json"])
        .env("BOB_DIR", vault.path())
        .env("NO_COLOR", "1");
    let projects = projects.output().expect("projects list");
    let projects = report(&projects);
    let sase = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["stem"] == "sase")
        .unwrap();
    assert!(sase["contract_issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "note_offclock_shadowed"));
}
