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
    let reference_has_task = if kind == NoteKind::Reference {
        let document = PropsDocument::parse(note);
        let body = &note[document.body_range.clone()];
        crate::native::ref_tasks::find_managed_embed(body).is_some()
            || !crate::native::ref_tasks::find_trackers(body).is_empty()
    } else {
        false
    };
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
            reference_has_task,
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
    let bank = "---\nparent: '[[cash]]'\nsummary: Banking\nbank: {currency: USD}\n---\n# Bank\n";
    let bank_report = inspect(bank, &context(NoteKind::Bank, bank)).report;
    assert_eq!(bank_report.note_kind, "bank");
    assert_eq!(bank_report.capabilities, vec!["props_v1"]);
    assert!(bank_report.contract.is_none());
    assert_eq!(bank_report.properties[2].owner.as_deref(), Some("bank"));
    assert_eq!(
        bank_report.properties[2].setting, None,
        "bank hub exposes no settings contract"
    );
    assert_eq!(
        run(
            bank,
            NoteKind::Bank,
            json!([{"op":"setting.set","setting":"summary","value":"Household finances"}])
        )
        .unwrap_err()
        .code,
        "prop_setting_disabled"
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
fn bank_show_has_no_contract_but_keeps_card_owned_bank_key() {
    let bank = "---\nparent: '[[cash]]'\nsummary: Banking\nbank: {currency: USD}\n---\n# Bank\n";
    let report = inspect(bank, &context(NoteKind::Bank, bank)).report;
    assert_eq!(report.note_kind, "bank");
    assert_eq!(report.capabilities, vec!["props_v1"]);
    assert!(report.contract.is_none());
    let owned = report
        .properties
        .iter()
        .find(|property| property.key == "bank")
        .expect("bank key");
    assert_eq!(owned.owner.as_deref(), Some("bank"));
    assert_eq!(owned.setting, None);
    assert_eq!(
        run(
            bank,
            NoteKind::Bank,
            json!([{"op":"setting.set","setting":"summary","value":"x"}])
        )
        .unwrap_err()
        .code,
        "prop_setting_disabled"
    );
}

#[test]
fn reference_contract_reports_routes_capability_and_managed_target() {
    let note = "---\ntype: '[[ref]]'\ntitle: Old\nstatus: wip\nparent: '[[reading]]'\n---\n# Old\n\n![[reading#^ref-modern]]\n";
    let report = inspect(note, &context(NoteKind::Reference, note)).report;
    assert_eq!(report.note_kind, "reference");
    assert_eq!(
        report.capabilities,
        vec!["props_v1", "props_ref_contract_v1"]
    );
    let contract = report.contract.as_ref().expect("reference contract");
    assert_eq!(contract.kind, "reference");
    assert_eq!(
        contract
            .settings
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [
            "title",
            "summary",
            "author",
            "published",
            "source",
            "aliases",
            "reading",
            "parent",
            "file_name"
        ]
    );
    for id in ["reading", "parent"] {
        let row = setting(&report, id);
        assert!(row.disabled);
        assert_eq!(row.route, Some("reading_task"));
        assert_eq!(row.target.as_deref(), Some("![[reading#^ref-modern]]"));
        assert!(row.reason.is_some());
    }
    let file_name = setting(&report, "file_name");
    assert!(file_name.disabled);
    assert_eq!(file_name.route, Some("ref_rename"));
    assert!(file_name.target.is_none());
    assert!(!setting(&report, "title").disabled);
    assert!(setting(&report, "title").target.is_none());
    let status = report
        .properties
        .iter()
        .find(|row| row.key == "status")
        .unwrap();
    assert_eq!(
        status.role,
        Some(crate::native::props::roles::Role::Derived)
    );
    assert_eq!(status.owner.as_deref(), Some("reading task"));
    let renamed = run(
        note,
        NoteKind::Reference,
        json!([{"op":"setting.set","setting":"title","value":"New"}]),
    )
    .unwrap();
    assert!(renamed.effects.iter().any(|effect| {
        effect.setting == "title" && effect.kind == "reading_task_link_text"
    }));
    let taskless = "---\ntype: '[[ref]]'\nstatus: wip\n---\n# Legacy\n";
    let taskless_report =
        inspect(taskless, &context(NoteKind::Reference, taskless)).report;
    assert_eq!(
        setting(&taskless_report, "reading").effective.source,
        "none"
    );
    assert_eq!(
        taskless_report
            .properties
            .iter()
            .find(|row| row.key == "status")
            .unwrap()
            .role,
        Some(crate::native::props::roles::Role::Loose)
    );
}

#[test]
fn reference_contract_validates_settings_and_reports_marker_syncs() {
    let note = "---\ntype: '[[ref]]'\ntitle: Old\nauthor: Writer\npublished: 2020\nsource_url: https://example.com\naliases: [Old]\nhighlights_marker_base: marker-v1\n---\n# Old\n";
    for (setting_id, value) in [
        ("title", json!("  ")),
        ("title", json!("two\nlines")),
        ("author", json!("two\nlines")),
        ("published", json!("2024-13")),
        ("published", json!("24")),
        ("source", json!("file:///tmp/paper.pdf")),
        ("aliases", json!(["valid", " "])),
    ] {
        assert_eq!(
            run(
                note,
                NoteKind::Reference,
                json!([{"op":"setting.set","setting":setting_id,"value":value}])
            )
            .unwrap_err()
            .code,
            "prop_bad_value",
            "{setting_id}: {value}"
        );
    }
    for setting_id in ["reading", "parent", "file_name", "title"] {
        assert_eq!(
            run(
                note,
                NoteKind::Reference,
                json!([{"op":"setting.clear","setting":setting_id}])
            )
            .unwrap_err()
            .code,
            "prop_setting_disabled",
            "{setting_id} cannot be cleared"
        );
    }
    assert_eq!(
        run(
            note,
            NoteKind::Reference,
            json!([{"op":"setting.clear","setting":"source"}]),
        )
        .unwrap_err()
        .code,
        "prop_setting_disabled"
    );
    let result = run(
        note,
        NoteKind::Reference,
        json!([
            {"op":"setting.set","setting":"title","value":"New"},
            {"op":"setting.set","setting":"author","value":"New Writer"},
            {"op":"setting.set","setting":"published","value":"2024-02-29"},
            {"op":"setting.set","setting":"source","value":"https://new.example/paper"}
        ]),
    )
    .unwrap();
    for id in ["title", "author", "published", "source"] {
        assert!(
            result.effects.iter().any(|effect| {
                effect.setting == id
                    && effect.kind == "syncs"
                    && effect.syncs == ["pdf_marker"]
            }),
            "missing marker effect for {id}: {:?}",
            result.effects
        );
    }
    assert!(result
        .candidate
        .contains("source_url: https://new.example/paper"));
    let legacy = "---\ntype: '[[ref]]'\nurl: https://old.example/paper\n---\n";
    let updated = roundtrip(
        legacy,
        NoteKind::Reference,
        json!([{"op":"setting.set","setting":"source","value":"https://new.example/paper"}]),
    );
    assert!(updated.candidate.contains("url: https://new.example/paper"));
    assert!(!updated.candidate.contains("source_url:"));
}

#[test]
fn reference_title_retargets_only_matching_first_h1_and_undoes_exactly() {
    let crlf_bom = "\u{feff}---\r\ntype: '[[ref]]'\r\ntitle: Old\r\n---\r\n# Old ###\r\n\r\nText\r\n";
    let changed = roundtrip(
        crlf_bom,
        NoteKind::Reference,
        json!([{"op":"setting.set","setting":"title","value":"New"}]),
    );
    assert!(changed.candidate.starts_with("\u{feff}---\r\n"));
    assert!(changed.candidate.contains("# New ###\r\n\r\nText\r\n"));

    for (note, expected_body) in [
        (
            "---\ntype: '[[ref]]'\ntitle: Old\n---\n# Different\n\nBody\n",
            "# Different\n\nBody\n",
        ),
        (
            "---\ntype: '[[ref]]'\ntitle: Old\n---\n```md\n# Old\n```\n",
            "```md\n# Old\n```\n",
        ),
        (
            "---\ntype: '[[ref]]'\ntitle: Old\n---\n<!--\n# Old\n-->\n## Intro\n# Old\n",
            "<!--\n# Old\n-->\n## Intro\n# New\n",
        ),
        (
            "---\ntype: '[[ref]]'\n---\n# Stem\n",
            "# New\n",
        ),
        (
            "---\ntype: '[[ref]]'\ntitle: Old\n---\n# Old",
            "# New",
        ),
    ] {
        let result = roundtrip(
            note,
            NoteKind::Reference,
            json!([{"op":"setting.set","setting":"title","value":"New"}]),
        );
        let parsed = PropsDocument::parse(&result.candidate);
        assert_eq!(
            &result.candidate[parsed.body_range],
            expected_body,
            "{note}"
        );
    }
}

#[test]
fn reference_owned_fields_refuse_every_generic_mutation_path() {
    let note = "---\ntype: '[[ref]]'\nid: ref-old\nref_type: article\nsource_pdf: old.pdf\nsource_pdf_sha256: abc\npipeline_version: v1\nhighlights_marker_base: marker\naudio: recording.mp3\nreturn_links: []\nearlier: []\nrecaptured: false\nsource_markdown: old.md\nsource_note: old\nsource_block: block\nsource_blocks: []\nsource_id: old-id\nsource_path: old/path\nsource_line_start: 1\nsource_line_end: 2\nevent: event\nevent_id: event-id\nmeeting_start: start\nmeeting_end: end\nnotes_producer: producer\nstatus: wip\nparent: '[[reading]]'\ntitle: Old\nspare: value\n---\n# Old\n\n![[reading#^ref-old]]\n";
    let protected = [
        ("id", "prop_identity"),
        ("ref_type", "prop_identity"),
        ("source_pdf", "prop_derived"),
        ("source_pdf_sha256", "prop_derived"),
        ("pipeline_version", "prop_derived"),
        ("highlights_marker_base", "prop_derived"),
        ("audio", "prop_derived"),
        ("return_links", "prop_derived"),
        ("earlier", "prop_derived"),
        ("recaptured", "prop_derived"),
        ("source_markdown", "prop_derived"),
        ("source_note", "prop_derived"),
        ("source_block", "prop_derived"),
        ("source_blocks", "prop_derived"),
        ("source_id", "prop_derived"),
        ("source_path", "prop_derived"),
        ("source_line_start", "prop_derived"),
        ("source_line_end", "prop_derived"),
        ("event", "prop_derived"),
        ("event_id", "prop_derived"),
        ("meeting_start", "prop_derived"),
        ("meeting_end", "prop_derived"),
        ("notes_producer", "prop_derived"),
        ("status", "prop_derived"),
        ("parent", "prop_derived"),
    ];
    for (key, code) in protected {
        let expected = PropsDocument::parse(note).fingerprint.unwrap();
        let raw = format!("{key}: replacement\n");
        let ops = [
            json!({"op":"prop.set","key":key,"value":"replacement"}),
            json!({"op":"prop.set_yaml","key":key,"yaml":"replacement"}),
            json!({"op":"prop.unset","key":key}),
            json!({"op":"prop.rename","key":key,"to":"renamed"}),
            json!({"op":"prop.move","key":key,"after":"title"}),
            json!({"op":"prop.restore","key":key,"raw":raw}),
        ];
        for op in ops {
            let envelope = parse_envelope(&json!({"schema_version":1,"expect":expected,"ops":[op.clone()]}).to_string()).unwrap();
            let error =
                plan(note, &context(NoteKind::Reference, note), &envelope)
                    .unwrap_err();
            assert_eq!(error.code, code, "{key}: {op}");
        }
    }
}
