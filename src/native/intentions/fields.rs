use std::collections::BTreeSet;

use chrono::NaiveDate;
use serde_yaml::{Mapping, Value};

use super::{
    legacy::{self, IntentionsKind},
    subject::NoteSubject,
};
use crate::native::{periods::calendar::PeriodId, yaml_spans};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldProvenance {
    Frontmatter,
    Legacy,
    LegacyReview,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field<T> {
    /// `true` even when a stored null or empty value resolves to `None`.
    pub(crate) present: bool,
    pub(crate) value: Option<T>,
    pub(crate) valid: bool,
    pub(crate) provenance: FieldProvenance,
}

impl<T> Field<T> {
    fn absent() -> Self {
        Self {
            present: false,
            value: None,
            valid: true,
            provenance: FieldProvenance::Absent,
        }
    }
    fn from_frontmatter(present: bool, value: Option<T>, valid: bool) -> Self {
        Self {
            present,
            value,
            valid,
            provenance: if present {
                FieldProvenance::Frontmatter
            } else {
                FieldProvenance::Absent
            },
        }
    }
    fn from_legacy(value: Option<T>, provenance: FieldProvenance) -> Self {
        let present = value.is_some();
        Self {
            present,
            value,
            valid: true,
            provenance: if present {
                provenance
            } else {
                FieldProvenance::Absent
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Diagnostic {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrontmatterState {
    Absent,
    Valid,
    Malformed,
}

#[derive(Debug, Clone)]
pub(crate) struct Fields {
    pub(crate) intentions: Field<Vec<String>>,
    pub(crate) summary: Field<String>,
    pub(crate) summary_digest: Field<String>,
    pub(crate) summary_drafted: Field<String>,
    pub(crate) reflection: Field<String>,
    pub(crate) review_digest: Field<String>,
    pub(crate) review_drafted: Field<String>,
    pub(crate) review_body: String,
    pub(crate) frontmatter: Option<Mapping>,
    pub(crate) frontmatter_state: FrontmatterState,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl Fields {
    fn blank() -> Self {
        Self {
            intentions: Field::absent(),
            summary: Field::absent(),
            summary_digest: Field::absent(),
            summary_drafted: Field::absent(),
            reflection: Field::absent(),
            review_digest: Field::absent(),
            review_drafted: Field::absent(),
            review_body: String::new(),
            frontmatter: None,
            frontmatter_state: FrontmatterState::Absent,
            diagnostics: Vec::new(),
        }
    }
}

/// Read the canonical fields for a day or period. Legacy text is consulted
/// only for absent keys and only when frontmatter is absent or valid.
pub(crate) fn read(contents: &str, subject: NoteSubject) -> Fields {
    let kind = match subject {
        NoteSubject::Day(_) => IntentionsKind::Day,
        NoteSubject::Period(id) => IntentionsKind::Period(id),
    };
    let review_body = legacy::period_sections(contents)
        .get("Review")
        .cloned()
        .unwrap_or_default();
    let mut output = Fields {
        review_body,
        ..Fields::blank()
    };
    if matches!(kind, IntentionsKind::Period(PeriodId::AllTime)) {
        output.frontmatter_state = frontmatter(contents).0;
        return output;
    }

    let (state, mapping, frontmatter_error) = frontmatter(contents);
    output.frontmatter_state = state;
    output.frontmatter = mapping.clone();
    if let Some(message) = frontmatter_error {
        output.diagnostics.push(Diagnostic {
            code: "frontmatter_invalid",
            message,
        });
        // The frontmatter may contain a key, so absence cannot be inferred.
        return output;
    }

    let map = mapping.as_ref();
    output.intentions = read_intentions(map, &mut output.diagnostics);
    output.summary =
        read_string(map, "summary", "summary_shape", &mut output.diagnostics);
    output.summary_digest =
        read_stamp(map, "summary_digest", &mut output.diagnostics);
    output.summary_drafted =
        read_date_stamp(map, "summary_drafted", &mut output.diagnostics);
    output.reflection = read_string(
        map,
        "reflection",
        "reflection_shape",
        &mut output.diagnostics,
    );
    output.review_digest =
        read_stamp(map, "review_digest", &mut output.diagnostics);
    output.review_drafted =
        read_date_stamp(map, "review_drafted", &mut output.diagnostics);

    if !output.intentions.present {
        output.intentions = Field::from_legacy(
            legacy::intentions(kind, contents),
            FieldProvenance::Legacy,
        );
    }
    if !output.summary.present {
        let provenance = if matches!(kind, IntentionsKind::Period(_)) {
            FieldProvenance::LegacyReview
        } else {
            FieldProvenance::Legacy
        };
        output.summary =
            Field::from_legacy(legacy::summary(kind, contents), provenance);
    }
    if !output.reflection.present {
        output.reflection = Field::from_legacy(
            legacy::reflection(kind, contents),
            FieldProvenance::Legacy,
        );
    }
    if output
        .intentions
        .value
        .as_ref()
        .is_some_and(|values| values.len() > 3)
    {
        output.diagnostics.push(Diagnostic {
            code: "intentions_over_soft_cap",
            message: "intentions contain more than the suggested three items"
                .to_owned(),
        });
    }
    output
}

fn frontmatter(
    contents: &str,
) -> (FrontmatterState, Option<Mapping>, Option<String>) {
    let lines = yaml_spans::split_lines(contents);
    let Some(first) = lines.first() else {
        return (FrontmatterState::Absent, None, None);
    };
    let opener = first.text.strip_prefix('\u{feff}').unwrap_or(&first.text);
    if opener.trim() != "---" {
        return (FrontmatterState::Absent, None, None);
    }
    let Some(close) =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            (line.text.trim() == "---").then_some(index)
        })
    else {
        return (
            FrontmatterState::Malformed,
            None,
            Some("frontmatter block is not closed".to_owned()),
        );
    };
    let front = &lines[1..close];
    let yaml = yaml_spans::yaml_text(front);
    if yaml.lines().all(|line| {
        line.trim().is_empty() || line.trim_start().starts_with('#')
    }) {
        return (FrontmatterState::Valid, Some(Mapping::new()), None);
    }
    if let Some(key) = duplicate_root_key(front) {
        return (
            FrontmatterState::Malformed,
            None,
            Some(format!("duplicate frontmatter key {key:?}")),
        );
    }
    if yaml_spans::validate_flow_map_keys(&yaml)
        == Err(yaml_spans::FlowMapError::DuplicateKey)
    {
        return (
            FrontmatterState::Malformed,
            None,
            Some("duplicate key in frontmatter flow mapping".to_owned()),
        );
    }
    let value: Value = match serde_yaml::from_str(&yaml) {
        Ok(value) => value,
        Err(error) => {
            return (
                FrontmatterState::Malformed,
                None,
                Some(format!("invalid frontmatter YAML: {error}")),
            )
        }
    };
    match value {
        Value::Mapping(mapping) => {
            (FrontmatterState::Valid, Some(mapping), None)
        }
        _ => (
            FrontmatterState::Malformed,
            None,
            Some("frontmatter YAML must be a mapping".to_owned()),
        ),
    }
}

fn duplicate_root_key(lines: &[yaml_spans::Line]) -> Option<String> {
    let mut seen = BTreeSet::new();
    for line in lines {
        if line.text.trim().is_empty()
            || line.text.trim_start().starts_with('#')
            || yaml_spans::indentation(&line.text) != 0
        {
            continue;
        }
        let Some((key, _)) = yaml_spans::parse_key(&line.text, 0) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            return Some(key);
        }
    }
    None
}

fn key(name: &str) -> Value {
    Value::String(name.to_owned())
}

fn read_intentions(
    map: Option<&Mapping>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Field<Vec<String>> {
    let Some(map) = map else {
        return Field::absent();
    };
    let Some(value) = map.get(key("intentions")) else {
        return Field::absent();
    };
    let mut values = Vec::new();
    let mut valid = true;
    match value {
        Value::Null => {}
        Value::Sequence(items) => {
            for item in items {
                match item {
                    Value::String(text)
                        if !text.trim().is_empty()
                            && !text.contains(['\n', '\r']) =>
                    {
                        values.push(text.clone())
                    }
                    Value::String(text) => {
                        valid = false;
                        diagnostics.push(Diagnostic { code: "intentions_value", message: format!("empty or multiline intention is not a one-line value: {text:?}") });
                    }
                    _ => {
                        valid = false;
                        diagnostics.push(Diagnostic {
                            code: "intentions_type",
                            message: "non-string intention item was skipped"
                                .to_owned(),
                        });
                    }
                }
            }
        }
        Value::String(text) => {
            diagnostics.push(Diagnostic {
                code: "intentions_shape",
                message:
                    "intentions should be a YAML list; scalar read as one item"
                        .to_owned(),
            });
            if !text.trim().is_empty() && !text.contains(['\n', '\r']) {
                values.push(text.clone());
            } else {
                valid = false;
                diagnostics.push(Diagnostic {
                    code: "intentions_value",
                    message:
                        "empty or multiline intention is not a one-line value"
                            .to_owned(),
                });
            }
        }
        _ => {
            valid = false;
            diagnostics.push(Diagnostic {
                code: "intentions_shape",
                message: "intentions must be a list or string".to_owned(),
            });
        }
    }
    Field::from_frontmatter(true, (!values.is_empty()).then_some(values), valid)
}

fn read_string(
    map: Option<&Mapping>,
    name: &str,
    code: &'static str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Field<String> {
    let Some(value) = map.and_then(|map| map.get(key(name))) else {
        return Field::absent();
    };
    match value {
        Value::Null => Field::from_frontmatter(true, None, true),
        Value::String(text) => Field::from_frontmatter(
            true,
            (!text.is_empty()).then(|| text.clone()),
            true,
        ),
        _ => {
            diagnostics.push(Diagnostic {
                code,
                message: format!("{name} must be a YAML string"),
            });
            Field::from_frontmatter(true, None, false)
        }
    }
}

fn read_stamp(
    map: Option<&Mapping>,
    name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Field<String> {
    let mut field = read_string(map, name, "stamp_shape", diagnostics);
    let invalid = field.value.as_deref().is_some_and(|value| {
        !(value.len() == 23
            && value.starts_with("sha256:")
            && value[7..].bytes().all(|byte| {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }))
    });
    if invalid {
        field.valid = false;
        diagnostics.push(Diagnostic {
            code: "stamp_value",
            message: format!(
                "{name} must be sha256: plus 16 lowercase hex characters"
            ),
        });
    }
    field
}

fn read_date_stamp(
    map: Option<&Mapping>,
    name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Field<String> {
    let mut field = read_string(map, name, "stamp_shape", diagnostics);
    if let Some(value) = field.value.as_deref() {
        let valid = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == value);
        if !valid {
            field.valid = false;
            diagnostics.push(Diagnostic {
                code: "stamp_value",
                message: format!("{name} must be a YYYY-MM-DD date"),
            });
        }
    }
    field
}

/// Canonical subject recognition used by adapters that already have the
/// vault-relative path. Arbitrary date-like basenames are not enough.
#[allow(dead_code)]
pub(crate) fn canonical_subject(
    path: &str,
    today: NaiveDate,
) -> Option<NoteSubject> {
    super::subject::NoteSubject::from_canonical_note(path, today)
        .map(|info| info.subject)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::intentions::{
        state::{self, ReviewState, SummaryState},
        subject::NoteSubject,
    };

    fn day() -> NoteSubject {
        NoteSubject::Day(NaiveDate::from_ymd_opt(2026, 11, 19).unwrap())
    }
    fn week() -> NoteSubject {
        NoteSubject::Period(PeriodId::Week {
            year: 2026,
            week: 47,
        })
    }

    #[test]
    fn yaml_values_are_typed_and_scalar_intentions_warn_without_splitting() {
        let parsed = read("---\nintentions: [\"a, b\", true, \"two\"]\nsummary: >-\n  First line\n  continued\n---\n", day());
        assert_eq!(
            parsed.intentions.value,
            Some(vec!["a, b".to_owned(), "two".to_owned()])
        );
        assert!(parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "intentions_type"));
        assert_eq!(
            parsed.summary.value.as_deref(),
            Some("First line continued")
        );

        let scalar = read("---\nintentions: \"a, b\"\n---\n", day());
        assert_eq!(scalar.intentions.value, Some(vec!["a, b".to_owned()]));
        assert!(scalar
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "intentions_shape"));
    }

    #[test]
    fn explicit_empty_and_malformed_frontmatter_never_resurrect_legacy_text() {
        let explicit = read(
            "---\nintentions: []\n---\n**Headline:** old intention\n",
            day(),
        );
        assert!(explicit.intentions.present);
        assert_eq!(explicit.intentions.value, None);

        let duplicate = read("---\nintentions: [new]\n\"intentions\": [old]\n---\n**Headline:** legacy\n", day());
        assert_eq!(duplicate.frontmatter_state, FrontmatterState::Malformed);
        assert!(duplicate.intentions.value.is_none());
        assert!(duplicate
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "frontmatter_invalid"));

        let flow_duplicate = read(
            "---\n{intentions: [new], \"intentions\": [old]}\n---\n",
            day(),
        );
        assert_eq!(
            flow_duplicate.frontmatter_state,
            FrontmatterState::Malformed
        );
    }

    #[test]
    fn legacy_fallback_is_per_key_and_respects_period_kind() {
        let daily = read("# Day\n\n**Headline:** Add [[album|records]]\n\n**Reflection:** Tired.\n\n## Pomodoros\n", day());
        assert_eq!(
            daily.intentions.value.as_deref(),
            Some(&["Add [[album|records]]".to_owned()][..])
        );
        assert_eq!(daily.reflection.value.as_deref(), Some("Tired."));

        let period = read("## Theme\n\nMake room\n\n## Outcomes\n\n- Ship the thing\n- Keep the notes\n\n## Review\n\n**Headline:** A useful week\n\n- shipped\n\n## Reflection\n\nBetter than expected.\n", NoteSubject::Period(PeriodId::Year(2026)));
        assert_eq!(
            period.intentions.value,
            Some(vec![
                "Make room".to_owned(),
                "Ship the thing".to_owned(),
                "Keep the notes".to_owned()
            ])
        );
        assert_eq!(period.summary.value.as_deref(), Some("A useful week"));
        assert_eq!(period.summary.provenance, FieldProvenance::LegacyReview);
        assert_eq!(
            period.reflection.value.as_deref(),
            Some("Better than expected.")
        );
        let outcomes_only = read(
            "## Outcomes\n\n- Finish the outline\n",
            NoteSubject::Period(PeriodId::Year(2026)),
        );
        assert_eq!(
            outcomes_only.intentions.value,
            Some(vec!["Finish the outline".to_owned()])
        );
    }

    #[test]
    fn states_check_summary_and_review_independently_with_reflection_precedence(
    ) {
        let review = "**Headline:** Old\n\n- work\n";
        let review_digest = state::digest(review);
        let summary_digest = state::digest("New");
        let drafted = format!("---\nsummary: New\nsummary_digest: {summary_digest}\nreview_digest: {review_digest}\n---\n## Review\n\n{review}\n## Reflection\n\n");
        let parsed = read(&drafted, week());
        assert_eq!(state::summary_state(&parsed), SummaryState::Drafted);
        assert_eq!(state::evaluate(&parsed, true), ReviewState::Drafted);

        let edited =
            drafted.replace(&summary_digest, "sha256:0000000000000000");
        let parsed = read(&edited, week());
        assert_eq!(state::evaluate(&parsed, true), ReviewState::Edited);

        let reflected = edited
            .replace("## Reflection\n\n", "## Reflection\n\nWhat it meant.\n");
        assert_eq!(
            state::evaluate(&read(&reflected, week()), true),
            ReviewState::Reflected
        );

        let intentions_only = read("---\nintentions: [plan]\nsummary_digest: sha256:0000000000000000\n---\n", day());
        assert_eq!(state::summary_state(&intentions_only), SummaryState::Empty);
        assert_eq!(
            state::evaluate(&intentions_only, false),
            ReviewState::Empty
        );
    }
}
