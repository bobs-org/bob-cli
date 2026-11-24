//! Golden fixture runner for the props span planner.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde::Deserialize;
    use serde_json::Value as JsonValue;

    use crate::native::props::{
        document::PropsDocument,
        plan::plan,
        protocol::{parse_envelope, Op},
        roles::{NoteKind, RoleContext},
        InspectContext,
    };

    #[derive(Debug, Deserialize)]
    struct Case {
        id: String,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        note: Option<String>,
        kind: String,
        ops: Vec<JsonValue>,
        #[serde(default)]
        expect: Option<String>,
        #[serde(default)]
        candidate: Option<String>,
        #[serde(default)]
        contains: Vec<String>,
        #[serde(default)]
        untouched: Vec<String>,
        #[serde(default)]
        absent: Vec<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        no_final_newline: bool,
    }

    fn load_source(name: &str) -> String {
        match name {
            "comments.md" => include_str!(
                "../../../tests/fixtures/props/sources/comments.md"
            )
            .to_owned(),
            "crlf.md" => {
                include_str!("../../../tests/fixtures/props/sources/crlf.md")
                    .to_owned()
            }
            "mixed.md" => {
                include_str!("../../../tests/fixtures/props/sources/mixed.md")
                    .to_owned()
            }
            "bom.md" => {
                include_str!("../../../tests/fixtures/props/sources/bom.md")
                    .to_owned()
            }
            "no_final_newline.md" => include_str!(
                "../../../tests/fixtures/props/sources/no_final_newline.md"
            )
            .to_owned(),
            "bank_flow.md" => include_str!(
                "../../../tests/fixtures/props/sources/bank_flow.md"
            )
            .to_owned(),
            "flow_list.md" => include_str!(
                "../../../tests/fixtures/props/sources/flow_list.md"
            )
            .to_owned(),
            "wikilinks.md" => include_str!(
                "../../../tests/fixtures/props/sources/wikilinks.md"
            )
            .to_owned(),
            "blocks.md" => {
                include_str!("../../../tests/fixtures/props/sources/blocks.md")
                    .to_owned()
            }
            "keys.md" => {
                include_str!("../../../tests/fixtures/props/sources/keys.md")
                    .to_owned()
            }
            "datetimes.md" => include_str!(
                "../../../tests/fixtures/props/sources/datetimes.md"
            )
            .to_owned(),
            "coercibles.md" => include_str!(
                "../../../tests/fixtures/props/sources/coercibles.md"
            )
            .to_owned(),
            "empty_values.md" => include_str!(
                "../../../tests/fixtures/props/sources/empty_values.md"
            )
            .to_owned(),
            "no_frontmatter.md" => include_str!(
                "../../../tests/fixtures/props/sources/no_frontmatter.md"
            )
            .to_owned(),
            "empty_frontmatter.md" => include_str!(
                "../../../tests/fixtures/props/sources/empty_frontmatter.md"
            )
            .to_owned(),
            "unclosed.md" => include_str!(
                "../../../tests/fixtures/props/sources/unclosed.md"
            )
            .to_owned(),
            "duplicates.md" => include_str!(
                "../../../tests/fixtures/props/sources/duplicates.md"
            )
            .to_owned(),
            "unsafe.md" => {
                include_str!("../../../tests/fixtures/props/sources/unsafe.md")
                    .to_owned()
            }
            "resembling.md" => include_str!(
                "../../../tests/fixtures/props/sources/resembling.md"
            )
            .to_owned(),
            "day.md" => {
                include_str!("../../../tests/fixtures/props/sources/day.md")
                    .to_owned()
            }
            other => panic!("unknown props fixture {other}"),
        }
    }

    fn note_kind(name: &str) -> NoteKind {
        match name {
            "day" => NoteKind::Day,
            "week" => NoteKind::Week,
            "quarter" => NoteKind::Quarter,
            "year" => NoteKind::Year,
            "all_time" => NoteKind::AllTime,
            "record" => NoteKind::Record,
            "album" => NoteKind::Album,
            "bank" => NoteKind::Bank,
            "area" => NoteKind::Area,
            "project" => NoteKind::Project,
            "notebook" => NoteKind::Notebook,
            "reference" => NoteKind::Reference,
            "note" => NoteKind::Note,
            other => panic!("unknown note kind {other}"),
        }
    }

    fn context(kind: NoteKind) -> InspectContext {
        InspectContext {
            contract: None,
            normalized_path: "notes/example.md".to_owned(),
            roles: RoleContext {
                note_kind: kind,
                record_fields: Some(BTreeSet::new()),
                diagnostics: Vec::new(),
            },
        }
    }

    #[test]
    fn golden_props_span_cases() {
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/props/cases.json"
        ))
        .expect("cases.json");
        assert!(!cases.is_empty());
        for case in cases {
            let note = case.note.clone().unwrap_or_else(|| {
                load_source(case.source.as_deref().expect("source"))
            });
            let kind = note_kind(&case.kind);
            let document = PropsDocument::parse(&note);
            let expect = case.expect.clone().unwrap_or_else(|| {
                document
                    .fingerprint
                    .clone()
                    .unwrap_or_else(|| "sha256:invalid".to_owned())
            });
            let envelope = parse_envelope(
                &serde_json::json!({
                    "schema_version": 1,
                    "expect": expect,
                    "ops": case.ops,
                })
                .to_string(),
            );
            let envelope = match envelope {
                Ok(envelope) => envelope,
                Err(error)
                    if case.error.as_deref() == Some(error.code.as_str()) =>
                {
                    continue;
                }
                Err(error) => panic!("{}: envelope {}", case.id, error.code),
            };
            match plan(&note, &context(kind), &envelope) {
                Err(error) => {
                    assert_eq!(
                        Some(error.code.as_str()),
                        case.error.as_deref(),
                        "{}: unexpected error {} ({})",
                        case.id,
                        error.code,
                        error.message
                    );
                }
                Ok(outcome) => {
                    assert!(
                        case.error.is_none(),
                        "{}: expected error {:?}",
                        case.id,
                        case.error
                    );
                    if let Some(candidate) = &case.candidate {
                        assert_eq!(
                            &outcome.candidate, candidate,
                            "{} candidate mismatch:\n{}",
                            case.id, outcome.candidate
                        );
                    }
                    for needle in &case.contains {
                        assert!(
                            outcome.candidate.contains(needle),
                            "{} missing {needle:?} in\n{}",
                            case.id,
                            outcome.candidate.escape_debug()
                        );
                    }
                    for needle in &case.absent {
                        assert!(
                            !outcome.candidate.contains(needle),
                            "{} unexpectedly contains {needle:?}\n{}",
                            case.id,
                            outcome.candidate
                        );
                    }
                    if case.no_final_newline {
                        assert!(
                            !outcome.candidate.ends_with('\n'),
                            "{} gained a final newline",
                            case.id
                        );
                    }
                    let original = PropsDocument::parse(&note);
                    let candidate = PropsDocument::parse(&outcome.candidate);
                    for key in &case.untouched {
                        let before = original
                            .entries
                            .iter()
                            .find(|entry| entry.key == *key)
                            .unwrap_or_else(|| {
                                panic!("{} missing original {key}", case.id)
                            });
                        let after = candidate
                            .entries
                            .iter()
                            .find(|entry| entry.key == *key)
                            .unwrap_or_else(|| {
                                panic!("{} missing candidate {key}", case.id)
                            });
                        assert_eq!(
                            before.raw(&original.source),
                            after.raw(&candidate.source),
                            "{} touched {key}",
                            case.id
                        );
                        assert_eq!(
                            before.raw_value, after.raw_value,
                            "{} changed YAML for {key}",
                            case.id
                        );
                    }
                    if outcome.inverse_ops.is_empty() {
                        assert_eq!(outcome.candidate, note, "{} noop", case.id);
                        continue;
                    }
                    let redo_expect =
                        candidate.fingerprint.clone().expect("fp");
                    let inverse = parse_envelope(
                        &serde_json::json!({
                            "schema_version": 1,
                            "expect": redo_expect,
                            "ops": outcome.inverse_ops.iter().map(Op::to_json).collect::<Vec<_>>(),
                        })
                        .to_string(),
                    )
                    .unwrap_or_else(|error| {
                        panic!("{} inverse envelope: {}", case.id, error.code)
                    });
                    let undone =
                        plan(&outcome.candidate, &context(kind), &inverse)
                            .unwrap_or_else(|error| {
                                panic!(
                                    "{} inverse plan: {}",
                                    case.id, error.code
                                )
                            });
                    assert_eq!(
                        undone.candidate,
                        note,
                        "{} inverse did not restore original bytes\n{}",
                        case.id,
                        undone.candidate.escape_debug()
                    );
                    if undone.inverse_ops.is_empty() {
                        continue;
                    }
                    let redo_envelope = parse_envelope(
                        &serde_json::json!({
                            "schema_version": 1,
                            "expect": PropsDocument::parse(&undone.candidate).fingerprint.clone().expect("fp"),
                            "ops": undone.inverse_ops.iter().map(Op::to_json).collect::<Vec<_>>(),
                        })
                        .to_string(),
                    )
                    .expect("redo envelope");
                    let redone =
                        plan(&undone.candidate, &context(kind), &redo_envelope)
                            .unwrap_or_else(|error| {
                                panic!("{} redo: {}", case.id, error.code)
                            });
                    assert_eq!(
                        redone.candidate, outcome.candidate,
                        "{} redo did not restore candidate",
                        case.id
                    );
                }
            }
        }
    }
}
