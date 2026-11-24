use std::{collections::BTreeMap, path::PathBuf};

use serde_json::{json, Value};

use super::Context;
use crate::native::{
    config::{themes::ThemeSources, FreshnessConfig, PlanConfig},
    notebooks::snapshot::VaultSnapshot,
    props::{
        document::PropsDocument,
        inspect,
        plan::{plan, PlanOutcome},
        protocol::{parse_envelope, Op},
        roles::{NoteKind, RoleContext},
        InspectContext, PropsError,
    },
};

fn context(kind: NoteKind, note: &str) -> InspectContext {
    let files: BTreeMap<PathBuf, String> = [
        ("area.md", "# Areas\n"),
        (
            "dev.md",
            "---\ntype: '[[area]]'\nparent: '[[area]]'\ncolor: blue\n---\n",
        ),
        (
            "body.md",
            "---\ntype: '[[area]]'\nparent: '[[area]]'\n---\n",
        ),
        (
            "sase.md",
            "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: wip\n---\n",
        ),
        (
            "closed.md",
            "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: done\n---\n",
        ),
        (
            "notebook.md",
            "---\ntype: '[[notebook]]'\nparent: '[[dev]]'\n---\n",
        ),
        (
            "child.md",
            "---\ntype: '[[area]]'\nparent: '[[note]]'\n---\n",
        ),
        ("one/duplicate.md", "---\ntype: '[[area]]'\n---\n"),
        ("two/duplicate.md", "---\ntype: '[[area]]'\n---\n"),
        ("note.md", note),
    ]
    .into_iter()
    .map(|(path, text)| (path.into(), text.to_owned()))
    .collect();
    InspectContext {
        normalized_path: "note.md".to_owned(),
        roles: RoleContext {
            note_kind: kind,
            record_fields: None,
            diagnostics: Vec::new(),
        },
        contract: Some(Context {
            snapshot: VaultSnapshot::from_prefixes(
                files.keys().cloned().collect(),
                files,
            ),
            targets: Vec::new(),
            types: ThemeSources {
                exempt: Default::default(),
                inventory: Default::default(),
                themes: None,
                warning: None,
            },
            plan: PlanConfig::default(),
            freshness: FreshnessConfig::default(),
        }),
    }
}

fn run(
    note: &str,
    kind: NoteKind,
    ops: Value,
) -> Result<PlanOutcome, PropsError> {
    let envelope = parse_envelope(&json!({"schema_version": 1, "expect": PropsDocument::parse(note).fingerprint.unwrap(), "ops": ops}).to_string())?;
    plan(note, &context(kind, note), &envelope)
}

fn roundtrip(note: &str, kind: NoteKind, ops: Value) -> PlanOutcome {
    let result = run(note, kind, ops).unwrap();
    assert!(result
        .inverse_ops
        .iter()
        .all(|op| matches!(op, Op::Restore { .. })));
    let undo = run(
        &result.candidate,
        kind,
        json!(result
            .inverse_ops
            .iter()
            .map(Op::to_json)
            .collect::<Vec<_>>()),
    )
    .unwrap();
    assert_eq!(undo.candidate, note, "undo must recover every byte");
    let redo = run(
        &undo.candidate,
        kind,
        json!(undo.inverse_ops.iter().map(Op::to_json).collect::<Vec<_>>()),
    )
    .unwrap();
    assert_eq!(
        redo.candidate, result.candidate,
        "redo must recover every byte"
    );
    result
}

#[test]
fn setting_goldens_and_shadowed_cleanup_undo() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/props/settings.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let note = case["note"].as_str().unwrap();
        let kind = if case["kind"] == "project" {
            NoteKind::Project
        } else {
            NoteKind::Area
        };
        let result = roundtrip(note, kind, case["ops"].clone());
        assert_eq!(
            result.candidate,
            case["candidate"].as_str().unwrap(),
            "{}",
            case["id"]
        );
        let repeated =
            run(&result.candidate, kind, case["ops"].clone()).unwrap();
        assert!(!repeated.changed, "{} must be idempotent", case["id"]);
    }
}

#[test]
fn generic_write_paths_cannot_bypass_domain_rules() {
    let note = "---\ntype: '[[area]]'\nparent: '[[dev]]'\ncolor: blue\nspare: wrong\n---\nbody\n";
    for (key, value, code) in [
        ("summary", json!("two\nlines"), "area_summary_multiline"),
        ("summary", json!(5), "prop_bad_value"),
        ("color", json!("purple"), "prop_bad_value"),
        ("task_type", json!(["BUILD"]), "prop_bad_value"),
        ("theme", json!("BUILD + FIX"), "prop_bad_value"),
        ("offclock", json!("false"), "prop_bad_value"),
        ("offclock_types", json!([]), "prop_bad_value"),
        ("offclock_types", json!(["MEET", ""]), "prop_bad_value"),
        ("offclock_types", json!(["MEET", "off"]), "prop_bad_value"),
        ("ready_cap", json!(0), "prop_bad_value"),
        ("ready_cap", json!(1000), "prop_bad_value"),
        ("ready_cap", json!(false), "prop_bad_value"),
        ("task_refresh", json!(366), "prop_bad_value"),
        ("task_refresh", json!(1.5), "prop_bad_value"),
        (
            "project_name_aliases",
            json!(["one", "ONE"]),
            "prop_bad_value",
        ),
        ("project_name_aliases", json!([""]), "prop_bad_value"),
        ("parent", json!(["[[dev]]"]), "prop_bad_value"),
    ] {
        let yaml = serde_yaml::to_string(&value).unwrap();
        for op in [
            json!({"op":"prop.set","key":key,"value":value}),
            json!({"op":"prop.set_yaml","key":key,"yaml":yaml}),
        ] {
            let error = run(note, NoteKind::Area, json!([op])).unwrap_err();
            assert_eq!(error.code, code, "{key}: {op}");
            assert_eq!(error.op_index, Some(0));
        }
        // A destination that was absent is validated by rename, too.
        let original = format!(
            "---\n{}---\nbody\n",
            serde_yaml::to_string(&json!({"type": "[[area]]", "spare": value}))
                .unwrap()
        );
        let error = run(
            &original,
            NoteKind::Area,
            json!([{"op":"prop.rename","key":"spare","to":key}]),
        )
        .unwrap_err();
        assert_eq!(error.code, code, "rename to {key}: {original}");
    }
}

#[test]
fn parent_refusals_and_required_key_cannot_be_renamed_away() {
    let note = "---\ntype: '[[area]]'\nparent: '[[dev]]'\n---\n";
    for (target, code) in [
        ("note", "prop_parent_cycle"),
        ("child", "prop_parent_cycle"),
        ("missing", "prop_parent_missing"),
        ("duplicate", "prop_parent_missing"),
        ("sase", "prop_parent_kind"),
        ("notebook", "prop_parent_kind"),
        ("done/dev", "prop_parent_kind"),
        ("_conflicts/dev", "prop_parent_kind"),
    ] {
        for op in [
            json!({"op":"setting.set","setting":"parent","value":format!("[[{target}]]")}),
            json!({"op":"prop.set","key":"parent","value":format!("[[{target}]]")}),
        ] {
            assert_eq!(
                run(note, NoteKind::Area, json!([op])).unwrap_err().code,
                code,
                "{target}"
            );
        }
    }
    for op in [
        json!({"op":"prop.unset","key":"parent"}),
        json!({"op":"setting.clear","setting":"parent"}),
        json!({"op":"prop.rename","key":"parent","to":"old_parent"}),
    ] {
        assert_eq!(
            run(note, NoteKind::Area, json!([op])).unwrap_err().code,
            "prop_required"
        );
    }
    for target in ["closed", "area", "notebook"] {
        assert_eq!(run(note, NoteKind::Project, json!([{"op":"setting.set","setting":"parent","value":format!("[[{target}]]")}])).unwrap_err().code, "prop_parent_kind");
    }
}

#[test]
fn canonical_simple_settings_and_parent_quotes_roundtrip() {
    let note = "---\ntype: '[[project]]'\nparent: [[sase]] # home\ntheme: write # legacy\nready_cap: false # disabled\ncolor: GRAY\n---\n^prj body without final newline";
    let result = roundtrip(
        note,
        NoteKind::Project,
        json!([
            {"op":"setting.set","setting":"task_type","value":"build"},
            {"op":"setting.set","setting":"ready_cap","value":"OFF"},
            {"op":"setting.set","setting":"color","value":"gray"},
            {"op":"setting.set","setting":"parent","value":"[[sase]]"},
            {"op":"setting.set","setting":"root","value":true},
            {"op":"setting.set","setting":"task_refresh","value":365},
            {"op":"setting.set","setting":"project_name_aliases","value":["bob-cli"]}
        ]),
    );
    assert!(result.candidate.contains("parent: \"[[sase]]\" # home"));
    assert!(result.candidate.contains("color: grey"));
    assert!(result.candidate.contains("ready_cap: \"off\" # disabled"));
    assert!(result.candidate.contains("task_type: BUILD"));
    assert!(!result.candidate.contains("theme:"));
    assert_eq!(result.warnings[0].code, "note_theme_unconfigured");
    let cleared = roundtrip(
        &result.candidate,
        NoteKind::Project,
        json!([
            {"op":"setting.clear","setting":"task_type"}, {"op":"setting.clear","setting":"root"},
            {"op":"setting.clear","setting":"ready_cap"}, {"op":"setting.clear","setting":"task_refresh"},
            {"op":"setting.clear","setting":"project_name_aliases"}, {"op":"setting.clear","setting":"color"}
        ]),
    );
    assert!(!cleared.candidate.contains("root:"));
}

#[test]
fn parent_edges_with_comments_cannot_hide_cycles_or_disable_roots() {
    let note = "---\ntype: '[[area]]'\nparent: '[[dev]]'\n---\n";
    for edge in [
        "parent: '[[note]]' # home",
        "parent: [[note]] # legacy",
        "\"parent\": \"[[note]]\" # quoted key",
    ] {
        let mut context = context(NoteKind::Area, note);
        let contract = context.contract.as_mut().unwrap();
        contract.snapshot = contract.snapshot.with_overlay([(
            PathBuf::from("child.md"),
            format!("---\ntype: '[[area]]'\n{edge}\n---\n"),
        )]);
        let envelope = parse_envelope(&json!({"schema_version":1,"expect":PropsDocument::parse(note).fingerprint.unwrap(),"ops":[{"op":"setting.set","setting":"parent","value":"[[child]]"}]}).to_string()).unwrap();
        assert_eq!(
            plan(note, &context, &envelope).unwrap_err().code,
            "prop_parent_cycle",
            "{edge}"
        );
    }
    for edge in ["parent: [[sase]] # home", "\"parent\": '[[sase]]' # home"] {
        roundtrip(
            &format!("---\ntype: '[[project]]'\n{edge}\n---\n"),
            NoteKind::Project,
            json!([{"op":"setting.set","setting":"root","value":true}]),
        );
    }
    // Validation must inspect the proposed graph rather than the broken preimage.
    roundtrip(
        "---\ntype: '[[area]]'\nparent: '[[child]]'\n---\n",
        NoteKind::Area,
        json!([{"op":"setting.set","setting":"parent","value":"[[dev]]"}]),
    );
}

#[test]
fn root_checks_and_parent_changes_use_ordered_candidate() {
    let note = "---\ntype: '[[project]]'\nparent: '[[dev]]'\n---\n";
    for op in [
        json!({"op":"setting.set","setting":"root","value":true}),
        json!({"op":"prop.set","key":"root","value":true}),
        json!({"op":"prop.set_yaml","key":"root","yaml":"true"}),
    ] {
        assert_eq!(
            run(note, NoteKind::Project, json!([op])).unwrap_err().code,
            "prop_setting_disabled"
        );
    }
    assert_eq!(
        run(
            note,
            NoteKind::Project,
            json!([{"op":"prop.set","key":"root","value":false}])
        )
        .unwrap_err()
        .code,
        "prop_bad_value"
    );
    roundtrip(
        note,
        NoteKind::Project,
        json!([
            {"op":"setting.set","setting":"parent","value":"[[sase]]"},
            {"op":"setting.set","setting":"root","value":true}
        ]),
    );
}

#[test]
fn restore_bypasses_domain_validation_but_not_ownership_or_structure() {
    let note = "---\ntype: '[[area]]'\ncolor: purple\nready_cap: false\nsummary: |\n  old\n  multiline\n---\n";
    roundtrip(
        note,
        NoteKind::Area,
        json!([
            {"op":"setting.set","setting":"color","value":"red"},
            {"op":"setting.set","setting":"ready_cap","value":1},
            {"op":"setting.clear","setting":"summary"}
        ]),
    );
    assert_eq!(
        run(
            note,
            NoteKind::Area,
            json!([{"op":"prop.restore","key":"type","raw":"type: note\n"}])
        )
        .unwrap_err()
        .code,
        "prop_identity"
    );
}

#[test]
fn setting_protocol_is_strict_and_disabled_on_other_kinds() {
    let note = "---\ntitle: hello\n---\n";
    for op in [
        json!({"op":"setting.set","setting":"unknown","value":1}),
        json!({"op":"setting.set","setting":"color"}),
        json!({"op":"setting.clear","setting":"color","value":1}),
    ] {
        assert!(run(note, NoteKind::Area, json!([op])).is_err());
    }
    for value in [
        json!("off"),
        json!({"mode":"off","types":[]}),
        json!({"mode":"types","types":[]}),
        json!({"mode":"types","types":"MEET"}),
        json!({"mode":"nope"}),
    ] {
        assert_eq!(
            run(
                note,
                NoteKind::Area,
                json!([{"op":"setting.set","setting":"clock","value":value}])
            )
            .unwrap_err()
            .code,
            "prop_bad_value"
        );
    }
    assert_eq!(
        run(
            note,
            NoteKind::Note,
            json!([{"op":"setting.set","setting":"color","value":"red"}])
        )
        .unwrap_err()
        .code,
        "prop_setting_disabled"
    );
    let result = run(
        note,
        NoteKind::Area,
        json!([{"op":"setting.clear","setting":"color"}]),
    )
    .unwrap();
    assert!(!result.changed);
    assert!(result.inverse_ops.is_empty());
}

#[test]
fn home_and_bank_roles_match_the_editor_contract() {
    let note = "---\ntype: '[[project]]'\nparent: '[[sase]]'\ntheme: WRITE\nroot: true\ntask_count: 5\nopen_task_count: 2\nscheduled: 2026-11-24\nstatus: wip\ndone_tasks: '[[done/note]]'\ntemplate: template_project\n---\n";
    let report = inspect(note, &context(NoteKind::Project, note)).report;
    let property =
        |key: &str| report.properties.iter().find(|p| p.key == key).unwrap();
    assert_eq!(property("theme").setting, Some("task_type"));
    assert!(property("theme").legacy);
    assert!(property("task_count").legacy);
    assert_eq!(
        property("task_count").role,
        Some(crate::native::props::roles::Role::Loose)
    );
    assert_eq!(property("status").owner.as_deref(), Some("projects"));
    roundtrip(
        note,
        NoteKind::Project,
        json!([{"op":"prop.unset","key":"task_count"},{"op":"prop.unset","key":"open_task_count"}]),
    );
    let bank = "---\ntype: '[[area]]'\nparent: '[[dev]]'\nsummary: Banking\nbank: {currency: USD}\n---\n# Bank\n";
    let bank_report = inspect(bank, &context(NoteKind::Bank, bank)).report;
    assert_eq!(bank_report.note_kind, "bank");
    assert_eq!(bank_report.properties[2].setting, Some("summary"));
    assert_eq!(bank_report.properties[3].owner.as_deref(), Some("bank"));
    roundtrip(
        bank,
        NoteKind::Bank,
        json!([{"op":"setting.set","setting":"summary","value":"Household finances"}]),
    );
    assert_eq!(
        run(
            bank,
            NoteKind::Bank,
            json!([{"op":"prop.set","key":"color","value":"nope"}])
        )
        .unwrap_err()
        .code,
        "prop_bad_value"
    );
    assert_eq!(
        run(
            bank,
            NoteKind::Bank,
            json!([{"op":"prop.unset","key":"bank"}])
        )
        .unwrap_err()
        .code,
        "prop_owned"
    );
}

fn setting<'a>(
    report: &'a crate::native::props::show::ShowReport,
    id: &str,
) -> &'a super::read::SettingReport {
    report
        .contract
        .as_ref()
        .expect("contract")
        .settings
        .iter()
        .find(|setting| setting.id == id)
        .unwrap_or_else(|| panic!("missing setting {id}"))
}

#[test]
fn show_contract_reports_inherited_hue_shadowed_clock_and_cap() {
    let note = "---\ntype: '[[area]]'\nparent: '[[dev]]'\noffclock: true\noffclock_types: [MEET]\nready_cap: off\n---\n# Inbox\n";
    let report = inspect(note, &context(NoteKind::Area, note)).report;
    assert_eq!(report.capabilities, vec!["props_v1", "props_contract_v1"]);
    let color = setting(&report, "color");
    assert!(!color.declared);
    assert_eq!(color.effective.source, "inherited");
    assert_eq!(color.effective.value, json!("blue"));
    assert_eq!(color.effective.from.as_deref(), Some("dev.md"));
    let clock = setting(&report, "clock");
    assert!(clock.declared);
    assert_eq!(clock.effective.source, "self");
    assert!(clock
        .issues
        .iter()
        .any(|issue| issue.code == "note_offclock_shadowed"));
    let cap = setting(&report, "ready_cap");
    assert_eq!(cap.effective.value, json!("off"));
    assert_eq!(cap.effective.source, "self");
}

#[test]
fn show_contract_reports_root_blocked_and_terminal_projects() {
    let root = "---\ntype: '[[project]]'\nparent: '[[sase]]'\nroot: true\nstatus: wip\n---\n- [ ] #task #prj Root work ^prj\n";
    let report = inspect(root, &context(NoteKind::Project, root)).report;
    let root_setting = setting(&report, "root");
    assert!(root_setting.declared);
    assert_eq!(root_setting.effective.value, json!(true));
    assert!(!root_setting.disabled);
    assert_eq!(
        report
            .contract
            .as_ref()
            .unwrap()
            .prj
            .as_ref()
            .unwrap()
            .criterion
            .as_deref(),
        Some("Root work")
    );

    let blocked = "---\ntype: '[[project]]'\nparent: '[[sase]]'\nstatus: wip\n---\n- [?] #task #prj Child ^prj\n  - ⛓️ **DEPENDS ON:** [[sase#^need]]\n";
    let blocked_report =
        inspect(blocked, &context(NoteKind::Project, blocked)).report;
    assert_eq!(
        blocked_report
            .contract
            .as_ref()
            .unwrap()
            .prj
            .as_ref()
            .unwrap()
            .depends_on,
        vec!["sase#^need"]
    );

    let terminal = "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: done\n---\n- [x] #task #prj Shipped [completion:: 2026-11-22] ^prj\n";
    let terminal_report =
        inspect(terminal, &context(NoteKind::Project, terminal)).report;
    assert_eq!(terminal_report.contract.as_ref().unwrap().kind, "project");
    assert_eq!(
        terminal_report
            .contract
            .as_ref()
            .unwrap()
            .prj
            .as_ref()
            .unwrap()
            .criterion
            .as_deref(),
        Some("Shipped")
    );
}

#[test]
fn plan_clear_reports_ghost_color_and_residence_from_inherited_and_config() {
    let note = "---\ntype: '[[area]]'\nparent: '[[dev]]'\ncolor: red\nready_cap: 3\ntask_refresh: 2\n---\n";
    let noop = run(note, NoteKind::Area, json!([])).unwrap();
    assert!(!noop.changed);
    assert_eq!(setting(&noop.report, "color").effective.source, "self");
    assert_eq!(setting(&noop.report, "ready_cap").effective.source, "self");

    let result = roundtrip(
        note,
        NoteKind::Area,
        json!([
            {"op":"setting.clear","setting":"color"},
            {"op":"setting.clear","setting":"ready_cap"},
            {"op":"setting.clear","setting":"task_refresh"}
        ]),
    );
    let color = setting(&result.report, "color");
    assert!(!color.declared);
    assert_eq!(color.value, json!(null));
    assert_eq!(color.effective.source, "inherited");
    assert_eq!(color.effective.value, json!("blue"));
    assert_eq!(color.effective.from.as_deref(), Some("dev.md"));
    assert_eq!(color.effective.label, "● blue · from dev");
    let cap = setting(&result.report, "ready_cap");
    assert!(!cap.declared);
    assert_eq!(cap.effective.source, "config");
    assert_eq!(cap.effective.value, json!(5));
    assert_eq!(
        cap.effective.label,
        "5 · Bob setting (plan.max_ready_per_note)"
    );
    let review = setting(&result.report, "task_refresh");
    assert!(!review.declared);
    assert_eq!(review.effective.source, "config");
    assert_eq!(
        review.effective.label,
        "7 days · Bob setting (freshness.interval)"
    );
}

#[test]
fn parent_rehome_names_syncs_and_inherited_settings_name_scope() {
    let note = "---\ntype: '[[project]]'\nparent: '[[dev]]'\nstatus: wip\n---\n- [ ] #task #prj Work ^prj\n";
    let moved = run(
        note,
        NoteKind::Project,
        json!([{"op":"setting.set","setting":"parent","value":"[[body]]"}]),
    )
    .unwrap();
    assert!(moved.effects.iter().any(|effect| {
        effect.kind == "syncs"
            && effect.syncs.iter().any(|name| name == "projects")
    }));
    let colored = run(
        note,
        NoteKind::Project,
        json!([{"op":"setting.set","setting":"color","value":"red"}]),
    )
    .unwrap();
    assert!(colored.effects.iter().any(|effect| {
        effect.setting == "color"
            && effect.kind == "inherited"
            && effect.message.contains("notes below")
    }));
    let cap = run(
        note,
        NoteKind::Project,
        json!([{"op":"setting.set","setting":"ready_cap","value":"off"}]),
    )
    .unwrap();
    assert!(cap.effects.iter().any(|effect| {
        effect.setting == "ready_cap" && effect.kind == "residence"
    }));
}

#[test]
fn bank_show_uses_the_area_contract() {
    let bank = "---\ntype: '[[area]]'\nparent: '[[dev]]'\nsummary: Banking\nbank: {currency: USD}\n---\n# Bank\n";
    let report = inspect(bank, &context(NoteKind::Bank, bank)).report;
    assert_eq!(report.note_kind, "bank");
    assert_eq!(report.capabilities, vec!["props_v1", "props_contract_v1"]);
    assert_eq!(report.contract.as_ref().unwrap().kind, "bank");
    assert_eq!(
        setting(&report, "summary").effective.value,
        json!("Banking")
    );
}
