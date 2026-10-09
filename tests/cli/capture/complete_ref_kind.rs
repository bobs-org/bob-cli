//! `task_kind: "ref"` appears on `#task #ref` rows and is absent elsewhere.

use crate::support::*;

fn write_vault(vault: &std::path::Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": []
          }
        }"##,
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[area]]\n---\n- [/] #task #ref Read paper ^ref-read\n- [/] #task Plain task ^plain\n",
    );
}

fn complete(
    vault: &std::path::Path,
    text: &str,
    cursor: usize,
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("--cursor")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", vault)
        .arg("--")
        .arg(text)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).expect("parse JSON")
}

fn kinds(doc: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    doc["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| {
            (
                c["text"].as_str().unwrap_or("").to_string(),
                c["task_kind"].clone(),
            )
        })
        .collect()
}

#[test]
fn ref_kind_appears_on_hat_picker() {
    let temp = TempDir::new("bob-cli-complete-ref-hat");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("vault");
    write_vault(&vault);
    let doc = complete(&vault, "^", 1);
    let rows = kinds(&doc);
    let ref_row = rows
        .iter()
        .find(|(t, _)| t.contains("Read paper"))
        .expect("ref row");
    assert_eq!(ref_row.1, serde_json::json!("ref"));
    let plain = rows
        .iter()
        .find(|(t, _)| t.contains("Plain task"))
        .expect("plain");
    assert!(plain.1.is_null(), "plain: {rows:?}");
}

#[test]
fn ref_kind_appears_on_colon_and_plus_pickers() {
    let temp = TempDir::new("bob-cli-complete-ref-colon");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("vault");
    write_vault(&vault);
    for text in [":", "+"] {
        let doc = complete(&vault, text, 1);
        let rows = kinds(&doc);
        // Colon/plus pickers only list ID-bearing linkable tasks; both rows
        // are linkable here, so both appear.
        let ref_row = rows
            .iter()
            .find(|(t, _)| t.contains("Read paper"))
            .expect("ref row");
        assert_eq!(ref_row.1, serde_json::json!("ref"), "{text}: {rows:?}");
    }
}

#[test]
fn ref_kind_appears_on_amp_and_bang_pickers() {
    let temp = TempDir::new("bob-cli-complete-ref-amp");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("vault");
    write_vault(&vault);
    for text in ["&", "!"] {
        let doc = complete(&vault, text, 1);
        let rows = kinds(&doc);
        let ref_row = rows
            .iter()
            .find(|(t, _)| t.contains("Read paper"))
            .expect("ref row");
        assert_eq!(ref_row.1, serde_json::json!("ref"), "{text}: {rows:?}");
        let plain = rows
            .iter()
            .find(|(t, _)| t.contains("Plain task"))
            .expect("plain");
        assert!(plain.1.is_null(), "{text}: {rows:?}");
    }
}
