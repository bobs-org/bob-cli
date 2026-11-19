//! Guarded field edits for canonical daily and period notes.

use std::{
    ffi::{OsStr, OsString},
    fs, io,
    io::Read,
    path::Path,
    time::Duration,
};

use serde_json::{json, Map, Value};
use serde_yaml::Value as YamlValue;
use sha2::{Digest, Sha256};

use super::{calendar::PeriodId, cli};
use crate::native::{
    env as bob_env,
    intentions::{
        fields::{Diagnostic, Fields},
        protocol::{self, Envelope, Op, ProtocolError},
        state,
        subject::{NoteSubject, SubjectInfo},
        write::{self, OwnedPatch, Patch},
    },
    ob,
    style::Styler,
};

const COMMAND_NAME: &str = "bob periods update";
const LOCK_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
struct UpdateError {
    code: String,
    message: String,
    hint: String,
    exit_code: i32,
    op_index: Option<usize>,
}

impl UpdateError {
    fn usage(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            hint: "See `bob periods update --help` and docs/periods.md for the update contract.".to_owned(),
            exit_code: 2,
            op_index: None,
        }
    }

    fn io(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            hint: "Check the vault path, note, lock, and file permissions."
                .to_owned(),
            exit_code: 1,
            op_index: None,
        }
    }

    fn protocol(error: ProtocolError) -> Self {
        Self {
            code: error.code.to_owned(),
            message: error.message,
            hint: error.hint.unwrap_or_else(|| {
                "See `bob periods update --help` for the envelope and operation shapes.".to_owned()
            }),
            exit_code: 2,
            op_index: error.op_index,
        }
    }

    fn json(&self) -> Value {
        let mut error = json!({
            "code": self.code,
            "message": self.message,
            "hint": self.hint,
        });
        if let (Some(index), Some(object)) =
            (self.op_index, error.as_object_mut())
        {
            object.insert("op_index".to_owned(), json!(index));
        }
        json!({"ok": false, "schema_version": protocol::SCHEMA_VERSION, "error": error})
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = cli::update_command();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let json_output = matches
        .get_one::<String>("format")
        .is_some_and(|value| value == "json");
    match execute(&matches) {
        Ok(report) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string(&report.json()).unwrap_or_default()
                );
            } else {
                print!("{}", report.human(&Styler::detect()));
            }
            0
        }
        Err(error) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string(&error.json()).unwrap_or_default()
                );
            } else {
                eprintln!("{COMMAND_NAME}: {}", error.message);
            }
            error.exit_code
        }
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

struct UpdateReport {
    subject: SubjectInfo,
    dry_run: bool,
    fingerprint_before: String,
    fingerprint_after: String,
    before: Fields,
    after: Fields,
    changed: bool,
    inverse_ops: Vec<Op>,
}

impl UpdateReport {
    fn json(&self) -> Value {
        json!({
            "ok": true,
            "schema_version": protocol::SCHEMA_VERSION,
            "subject": {
                "id": self.subject.id,
                "kind": self.subject.kind,
                "label": self.subject.label,
                "path": self.subject.path.to_string_lossy(),
            },
            "dry_run": self.dry_run,
            "fingerprint_before": self.fingerprint_before,
            "fingerprint_after": self.fingerprint_after,
            "before": public_fields(&self.before),
            "after": public_fields(&self.after),
            "changed": self.changed,
            "inverse_ops": self.inverse_ops.iter().map(protocol::op_json).collect::<Vec<_>>(),
            "warnings": warnings(&self.after.diagnostics),
        })
    }

    fn human(&self, styler: &Styler) -> String {
        let action = if self.dry_run {
            "Would update"
        } else if self.changed {
            "Updated"
        } else {
            "Unchanged"
        };
        let mut lines = vec![format!(
            "{}  {} · {}",
            styler.bold(action),
            self.subject.id,
            self.subject.path.display(),
        )];
        lines.push(format!(
            "  {}",
            styler.dim(&format!(
                "fingerprint {} → {}",
                self.fingerprint_before, self.fingerprint_after
            ))
        ));
        push_field_preview(
            &mut lines,
            "intentions",
            &self.before.intentions.value,
            &self.after.intentions.value,
            styler,
        );
        push_field_preview(
            &mut lines,
            "summary",
            &self.before.summary.value,
            &self.after.summary.value,
            styler,
        );
        push_field_preview(
            &mut lines,
            "reflection",
            &self.before.reflection.value,
            &self.after.reflection.value,
            styler,
        );
        lines.push(format!(
            "  {}  {} → {}",
            styler.cyan("summary state"),
            state::summary_state(&self.before).as_str(),
            state::summary_state(&self.after).as_str(),
        ));
        format!("{}\n", lines.join("\n"))
    }
}

fn push_field_preview<T: std::fmt::Debug + PartialEq>(
    lines: &mut Vec<String>,
    label: &str,
    before: &Option<T>,
    after: &Option<T>,
    styler: &Styler,
) {
    if before == after {
        return;
    }
    lines.push(format!("  {}", styler.cyan(label)));
    lines.push(format!(
        "    {}",
        styler.dim(&format!("before: {before:?}"))
    ));
    lines.push(format!(
        "    {}",
        styler.green(&format!("after:  {after:?}"))
    ));
}

fn execute(matches: &clap::ArgMatches) -> Result<UpdateReport, UpdateError> {
    let today = bob_env::current_datetime().date();
    let subject_text = matches
        .get_one::<String>("subject")
        .expect("required subject");
    let subject = NoteSubject::resolve(subject_text, today)
        .map_err(|message| UpdateError::usage("invalid_subject", message))?;
    if matches!(subject.subject, NoteSubject::Period(PeriodId::AllTime)) {
        return Err(UpdateError::usage(
            "no_intention_fields",
            "all_time has no intentions, summary, or reflection fields",
        ));
    }
    let input = read_input(matches)?;
    let dry_run = matches.get_flag("dry-run");
    let envelope =
        protocol::parse(&input, dry_run).map_err(UpdateError::protocol)?;
    let vault = bob_env::bob_dir();
    let original =
        crate::native::intentions::subject::read_existing(&vault, &subject)
            .map_err(|error| UpdateError {
                code: error.code.to_owned(),
                message: error.message,
                hint: error.hint,
                exit_code: 1,
                op_index: None,
            })?;
    let before = crate::native::intentions::read(&original, subject.subject);
    let fingerprint_before = fingerprint(&before);
    check_expect(&envelope, &fingerprint_before)?;
    let (patch, touched) = plan_ops(&before, &envelope.ops)?;
    let proposed = write::apply(&original, &patch)
        .map_err(|error| UpdateError::usage(error.code, error.message))?;
    let proposed_fields =
        crate::native::intentions::read(&proposed, subject.subject);
    let fingerprint_after = fingerprint(&proposed_fields);
    let changed = proposed != original;

    if dry_run || !changed {
        let inverse_ops = if changed {
            inverse_ops(&before, &touched)
        } else {
            Vec::new()
        };
        return Ok(UpdateReport {
            subject,
            dry_run,
            fingerprint_before,
            fingerprint_after,
            before,
            after: proposed_fields,
            changed,
            inverse_ops,
        });
    }

    let _lock =
        ob::acquire_lock_waiting(LOCK_WAIT, || {}).map_err(lock_error)?;
    let path = vault.join(&subject.path);
    let current = fs::read_to_string(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            UpdateError::io(
                "missing_note",
                format!(
                    "note disappeared while waiting for the vault lock: {}",
                    subject.path.display()
                ),
            )
        } else {
            UpdateError::io(
                "note_read_failed",
                format!("read {}: {error}", path.display()),
            )
        }
    })?;
    let current_fields =
        crate::native::intentions::read(&current, subject.subject);
    let current_fingerprint = fingerprint(&current_fields);
    if envelope.expect.as_deref() != Some(current_fingerprint.as_str()) {
        return Err(UpdateError::io(
            "stale_note",
            "the owned fields changed while waiting for the vault lock; refresh the card and retry",
        ));
    }
    // Apply to the bytes re-read under lock. Body-only edits do not make the
    // field fingerprint stale and therefore survive this write.
    let final_markdown = write::apply(&current, &patch)
        .map_err(|error| UpdateError::usage(error.code, error.message))?;
    let final_fields =
        crate::native::intentions::read(&final_markdown, subject.subject);
    let final_fingerprint = fingerprint(&final_fields);
    atomic_write(&path, &final_markdown).map_err(|error| {
        UpdateError::io(
            "write_failed",
            format!("write {}: {error}", path.display()),
        )
    })?;
    Ok(UpdateReport {
        subject,
        dry_run: false,
        fingerprint_before: current_fingerprint,
        fingerprint_after: final_fingerprint,
        inverse_ops: inverse_ops(&current_fields, &touched),
        before: current_fields,
        after: final_fields,
        changed: final_markdown != current,
    })
}

fn check_expect(envelope: &Envelope, current: &str) -> Result<(), UpdateError> {
    match envelope.expect.as_deref() {
        Some(expect) if expect == current => Ok(()),
        Some(_) => Err(UpdateError::io(
            "stale_note",
            "the intention fields changed; refresh the card and retry",
        )),
        None => Ok(()),
    }
}

fn read_input(matches: &clap::ArgMatches) -> Result<String, UpdateError> {
    let path = matches
        .get_one::<OsString>("input")
        .expect("required input");
    if path == OsStr::new("-") {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text).map_err(|error| {
            UpdateError::io("input_read_failed", format!("read stdin: {error}"))
        })?;
        return Ok(text);
    }
    fs::read_to_string(Path::new(path)).map_err(|error| {
        UpdateError::io(
            "input_read_failed",
            format!("could not read {}: {error}", Path::new(path).display()),
        )
    })
}

fn plan_ops(
    fields: &Fields,
    ops: &[Op],
) -> Result<(OwnedPatch, Touched), UpdateError> {
    let malformed_intentions = !fields.intentions.valid
        || fields.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code,
                "intentions_shape" | "intentions_value" | "intentions_type"
            )
        });
    let mut intentions = fields.intentions.value.clone().unwrap_or_default();
    let mut patch = OwnedPatch::default();
    let mut touched = Touched::default();
    for (index, op) in ops.iter().enumerate() {
        match op {
            Op::IntentionsSet { values } => {
                intentions = values
                    .iter()
                    .map(|value| validate_intention(value, index))
                    .collect::<Result<_, _>>()?;
                patch.intentions = Patch::Set(intentions.clone());
                touched.intentions = true;
            }
            Op::IntentionsInsert {
                index: position,
                value,
            } => {
                ensure_intentions_well_formed(malformed_intentions, index)?;
                let value = validate_intention(value, index)?;
                if *position > intentions.len() {
                    return Err(index_error(index, "index_out_of_range", format!("insert index {position} exceeds intention count {}", intentions.len())));
                }
                intentions.insert(*position, value);
                patch.intentions = Patch::Set(intentions.clone());
                touched.intentions = true;
            }
            Op::IntentionsReplace {
                index: position,
                value,
            } => {
                ensure_intentions_well_formed(malformed_intentions, index)?;
                let value = validate_intention(value, index)?;
                let Some(current) = intentions.get_mut(*position) else {
                    return Err(index_error(index, "index_out_of_range", format!("replace index {position} exceeds intention count {}", intentions.len())));
                };
                *current = value;
                patch.intentions = Patch::Set(intentions.clone());
                touched.intentions = true;
            }
            Op::IntentionsMove { from, to } => {
                ensure_intentions_well_formed(malformed_intentions, index)?;
                if *from >= intentions.len() || *to >= intentions.len() {
                    return Err(index_error(
                        index,
                        "index_out_of_range",
                        format!(
                            "move indexes must be below intention count {}",
                            intentions.len()
                        ),
                    ));
                }
                let value = intentions.remove(*from);
                intentions.insert(*to, value);
                patch.intentions = Patch::Set(intentions.clone());
                touched.intentions = true;
            }
            Op::IntentionsRemove { index: position } => {
                ensure_intentions_well_formed(malformed_intentions, index)?;
                if *position >= intentions.len() {
                    return Err(index_error(index, "index_out_of_range", format!("remove index {position} exceeds intention count {}", intentions.len())));
                }
                intentions.remove(*position);
                patch.intentions = Patch::Set(intentions.clone());
                touched.intentions = true;
            }
            Op::ReflectionSet { value } => {
                let value = value.trim();
                if value.is_empty() {
                    return Err(index_error(index, "empty_value", "reflection value must not be empty; use reflection.unset"));
                }
                patch.reflection = Patch::Set(value.to_owned());
                touched.reflection = true;
            }
            Op::ReflectionUnset => {
                patch.reflection = Patch::Unset;
                touched.reflection = true;
            }
            Op::SummarySet { value } => {
                let value = value.trim();
                if value.is_empty() {
                    return Err(index_error(
                        index,
                        "empty_value",
                        "summary value must not be empty; use summary.unset",
                    ));
                }
                if value.contains(['\n', '\r']) {
                    return Err(index_error(
                        index,
                        "multiline_summary",
                        "summary must be a single paragraph on one YAML line",
                    ));
                }
                patch.summary = Patch::Set(value.to_owned());
                patch.summary_digest = Patch::Unset;
                patch.summary_drafted = Patch::Unset;
                touched.summary = true;
            }
            Op::SummaryUnset => {
                patch.summary = Patch::Unset;
                patch.summary_digest = Patch::Unset;
                patch.summary_drafted = Patch::Unset;
                touched.summary = true;
            }
            Op::SummaryRestore {
                present,
                source,
                value,
                digest,
                drafted,
            } => {
                if *present && source == "frontmatter" {
                    patch.summary =
                        value.clone().map(Patch::Set).unwrap_or(Patch::Unset);
                } else {
                    patch.summary = Patch::Unset;
                }
                patch.summary_digest =
                    digest.clone().map(Patch::Set).unwrap_or(Patch::Unset);
                patch.summary_drafted =
                    drafted.clone().map(Patch::Set).unwrap_or(Patch::Unset);
                touched.summary = true;
            }
        }
    }
    Ok((patch, touched))
}

#[derive(Debug, Default)]
struct Touched {
    intentions: bool,
    summary: bool,
    reflection: bool,
}

fn ensure_intentions_well_formed(
    malformed: bool,
    index: usize,
) -> Result<(), UpdateError> {
    if malformed {
        Err(index_error(
            index,
            "malformed_field",
            "stored intentions are malformed; replace them with intentions.set before editing individual items",
        ))
    } else {
        Ok(())
    }
}

fn validate_intention(
    value: &str,
    index: usize,
) -> Result<String, UpdateError> {
    if value.contains(['\n', '\r']) {
        return Err(index_error(
            index,
            "multiline_intention",
            "an intention must be one line",
        ));
    }
    let value = value.trim();
    if value.is_empty() {
        return Err(index_error(
            index,
            "empty_value",
            "intention values must not be empty",
        ));
    }
    Ok(value.to_owned())
}

fn index_error(
    index: usize,
    code: &str,
    message: impl Into<String>,
) -> UpdateError {
    UpdateError {
        code: code.to_owned(),
        message: message.into(),
        hint: "Check the op index and current field values, then retry with a fresh fingerprint.".to_owned(),
        exit_code: 2,
        op_index: Some(index),
    }
}

fn inverse_ops(fields: &Fields, touched: &Touched) -> Vec<Op> {
    let mut ops = Vec::new();
    if touched.intentions {
        ops.push(Op::IntentionsSet {
            values: fields.intentions.value.clone().unwrap_or_default(),
        });
    }
    if touched.summary {
        let source = match fields.summary.provenance {
            crate::native::intentions::fields::FieldProvenance::Frontmatter => "frontmatter",
            crate::native::intentions::fields::FieldProvenance::Legacy => "legacy",
            crate::native::intentions::fields::FieldProvenance::LegacyReview => "legacy_review",
            crate::native::intentions::fields::FieldProvenance::Absent => "absent",
        };
        ops.push(Op::SummaryRestore {
            present: fields.summary.present,
            source: source.to_owned(),
            value: fields.summary.value.clone(),
            digest: fields.summary_digest.value.clone(),
            drafted: fields.summary_drafted.value.clone(),
        });
    }
    if touched.reflection {
        if fields.reflection.present {
            if let Some(value) = fields.reflection.value.clone() {
                ops.push(Op::ReflectionSet { value });
            } else {
                ops.push(Op::ReflectionUnset);
            }
        } else {
            ops.push(Op::ReflectionUnset);
        }
    }
    ops
}

fn public_fields(fields: &Fields) -> Value {
    json!({
        "intentions": fields.intentions.value.clone().unwrap_or_default(),
        "summary": fields.summary.value,
        "summary_state": state::summary_state(fields).as_str(),
        "reflection": fields.reflection.value,
    })
}

fn warnings(diagnostics: &[Diagnostic]) -> Vec<Value> {
    diagnostics
        .iter()
        .map(|diagnostic| json!({"code": diagnostic.code, "message": diagnostic.message}))
        .collect()
}

/// Fingerprint only the five intention-owned fields. For stored keys use
/// canonical typed YAML so malformed-but-valid YAML edits are detectable;
/// absent keys use the typed reader, including its one-release legacy values.
fn fingerprint(fields: &Fields) -> String {
    let owned = [
        ("intentions", json!(fields.intentions.value)),
        ("summary", json!(fields.summary.value)),
        ("summary_digest", json!(fields.summary_digest.value)),
        ("summary_drafted", json!(fields.summary_drafted.value)),
        ("reflection", json!(fields.reflection.value)),
    ];
    let mut canonical = Map::new();
    for (name, fallback) in owned {
        let value = fields
            .frontmatter
            .as_ref()
            .and_then(|mapping| mapping.get(YamlValue::String(name.to_owned())))
            .map(crate::native::records::fingerprint::yaml_to_canonical_json)
            .unwrap_or(fallback);
        canonical.insert(name.to_owned(), value);
    }
    let bytes = serde_json::to_vec(&Value::Object(canonical))
        .expect("intention fingerprint values are JSON serializable");
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn lock_error(error: ob::LockWaitError) -> UpdateError {
    match error {
        ob::LockWaitError::Timeout { path } => UpdateError::io(
            "lock_busy",
            format!(
                "another Bob vault maintenance run is active (waited on {})",
                path.display()
            ),
        ),
        ob::LockWaitError::Open { path, error } => UpdateError::io(
            "io_error",
            format!("could not open lock file {}: {error}", path.display()),
        ),
        ob::LockWaitError::Acquire { path, error } => UpdateError::io(
            "io_error",
            format!("could not lock {}: {error}", path.display()),
        ),
    }
}

fn atomic_write(target: &Path, contents: &str) -> io::Result<()> {
    use io::Write;
    let mut temp_name = OsString::from(".");
    temp_name.push(target.file_name().unwrap_or_else(|| OsStr::new("note.md")));
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let temp = target.with_file_name(temp_name);
    let _ = fs::remove_file(&temp);
    let mut file = fs::File::create(&temp)?;
    let result = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);
    match fs::rename(&temp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::intentions::state::SummaryState;
    use crate::native::intentions::NoteSubject;
    use chrono::NaiveDate;

    fn day() -> NoteSubject {
        NoteSubject::Day(NaiveDate::from_ymd_opt(2026, 11, 19).unwrap())
    }

    #[test]
    fn owned_fingerprint_ignores_body_edits_and_unowned_keys() {
        let left = crate::native::intentions::read(
            "---\nother: one\nintentions: [Plan]\n---\nbody one\n",
            day(),
        );
        let right = crate::native::intentions::read(
            "---\nintentions:\n  - Plan\nother: two\n---\nbody two\n",
            day(),
        );
        assert_eq!(fingerprint(&left), fingerprint(&right));
    }

    #[test]
    fn planning_applies_ordered_intention_ops_and_reports_bad_indexes() {
        let fields = crate::native::intentions::read(
            "---\nintentions: [one, two]\n---\n",
            day(),
        );
        let ops = [
            Op::IntentionsInsert {
                index: 1,
                value: "middle".into(),
            },
            Op::IntentionsMove { from: 2, to: 0 },
            Op::IntentionsReplace {
                index: 1,
                value: "two again".into(),
            },
        ];
        let (patch, _) = plan_ops(&fields, &ops).unwrap();
        assert_eq!(
            patch.intentions,
            Patch::Set(vec!["two".into(), "two again".into(), "one".into()])
        );
        assert_eq!(
            plan_ops(&fields, &[Op::IntentionsRemove { index: 9 }])
                .unwrap_err()
                .code,
            "index_out_of_range"
        );
    }

    #[test]
    fn summary_edit_clears_draft_stamps_and_undo_restores_them() {
        let text = "---\nsummary: draft\nsummary_digest: sha256:8b2f5d9b0f0a6d39\nsummary_drafted: 2026-11-19\n---\n";
        let before = crate::native::intentions::read(text, day());
        let (patch, touched) = plan_ops(
            &before,
            &[Op::SummarySet {
                value: "Bryan edit".into(),
            }],
        )
        .unwrap();
        let after_text = write::apply(text, &patch).unwrap();
        let after = crate::native::intentions::read(&after_text, day());
        assert_eq!(state::summary_state(&after), SummaryState::Edited);
        let undo = inverse_ops(&before, &touched);
        let (undo_patch, _) = plan_ops(&after, &undo).unwrap();
        let restored = crate::native::intentions::read(
            &write::apply(&after_text, &undo_patch).unwrap(),
            day(),
        );
        assert_eq!(restored.summary.value, before.summary.value);
        assert_eq!(restored.summary_digest.value, before.summary_digest.value);
        assert_eq!(
            restored.summary_drafted.value,
            before.summary_drafted.value
        );
        assert_eq!(
            state::summary_state(&restored),
            state::summary_state(&before)
        );
    }
}
