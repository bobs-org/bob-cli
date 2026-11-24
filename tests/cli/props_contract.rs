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
