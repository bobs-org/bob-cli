//! Scaffold a record type's type note, collection note, and starter Base.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::schema::{FieldKind, RecordChoice, RecordType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PieceKind {
    TypeNote,
    Collection,
    Base,
}

impl PieceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TypeNote => "type_note",
            Self::Collection => "collection",
            Self::Base => "base",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PieceAction {
    Create,
    Keep,
}

impl PieceAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Keep => "keep",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedPiece {
    pub(crate) kind: PieceKind,
    pub(crate) path: PathBuf,
    pub(crate) action: PieceAction,
    pub(crate) markdown: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InitPlan {
    pub(crate) record_type: String,
    pub(crate) label: String,
    pub(crate) dir: String,
    pub(crate) emoji: Option<String>,
    pub(crate) pieces: Vec<PlannedPiece>,
}

impl InitPlan {
    pub(crate) fn creates(&self) -> impl Iterator<Item = &PlannedPiece> {
        self.pieces
            .iter()
            .filter(|piece| piece.action == PieceAction::Create)
    }
}

pub(crate) fn plan_init(
    vault: &Path,
    record_type: &RecordType,
    parent: Option<&str>,
    created: &str,
) -> Result<InitPlan, String> {
    let type_note = PathBuf::from(format!("{}.md", record_type.id));
    let collection = PathBuf::from(format!("{}.md", record_type.dir));
    let base = PathBuf::from(format!("{}.base", record_type.dir));

    let type_piece = plan_piece(
        vault,
        PieceKind::TypeNote,
        type_note,
        || Ok(type_note_markdown(record_type, created)),
        None,
    )?;
    let collection_piece = plan_piece(
        vault,
        PieceKind::Collection,
        collection,
        || {
            let parent = required_parent(parent, &record_type.dir)?;
            Ok(collection_markdown(record_type, &parent, created))
        },
        Some(missing_parent_message(&record_type.dir)),
    )?;
    let base_piece = plan_piece(
        vault,
        PieceKind::Base,
        base,
        || Ok(base_markdown(record_type)),
        None,
    )?;

    Ok(InitPlan {
        record_type: record_type.id.clone(),
        label: record_type.label.clone(),
        dir: record_type.dir.clone(),
        emoji: record_type.emoji.clone(),
        pieces: vec![type_piece, collection_piece, base_piece],
    })
}

fn plan_piece(
    vault: &Path,
    kind: PieceKind,
    relative: PathBuf,
    render: impl FnOnce() -> Result<String, String>,
    missing_parent: Option<String>,
) -> Result<PlannedPiece, String> {
    let target = vault.join(&relative);
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            return Ok(PlannedPiece {
                kind,
                path: relative,
                action: PieceAction::Keep,
                markdown: None,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "could not inspect {}: {error}",
                relative.display()
            ));
        }
    }
    let markdown = match render() {
        Ok(markdown) => markdown,
        Err(error) => {
            if let Some(missing_parent) = missing_parent {
                return Err(missing_parent);
            }
            return Err(error);
        }
    };
    Ok(PlannedPiece {
        kind,
        path: relative,
        action: PieceAction::Create,
        markdown: Some(markdown),
    })
}

fn required_parent(parent: Option<&str>, dir: &str) -> Result<String, String> {
    match parent.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Ok(parent_wikilink(value)),
        None => Err(missing_parent_message(dir)),
    }
}

fn missing_parent_message(dir: &str) -> String {
    format!(
        "{dir}.md is missing; pass -p/--parent NOTE so bob never guesses (for example `-p fun`)"
    )
}

pub(crate) fn parent_wikilink(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("[[") {
        return trimmed.to_owned();
    }
    let stem = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".MD"))
        .unwrap_or(trimmed);
    format!("[[{stem}]]")
}

fn type_note_markdown(record_type: &RecordType, created: &str) -> String {
    format!(
        "---\nparent: \"[[type]]\"\ncreated: {created}\n---\n\n{label} notes link to this note as their `type`.\n\nSchema: `bob records show {id}`\n",
        label = record_type.label,
        id = record_type.id,
    )
}

fn collection_markdown(
    record_type: &RecordType,
    parent: &str,
    created: &str,
) -> String {
    let heading = collection_heading(&record_type.label);
    let dir = &record_type.dir;
    let id = &record_type.id;
    let label = &record_type.label;
    format!(
        "---\nparent: \"{parent}\"\ncreated: {created}\n---\n\n# {heading}\n\n{label} records live under `{dir}/`. Browse them in [[{dir}.base]].\n\nSchema: `bob records show {id}`\n\n![[{dir}.base]]\n"
    )
}

fn collection_heading(label: &str) -> String {
    if label.ends_with('s') || label.ends_with('S') {
        label.to_owned()
    } else {
        format!("{label}s")
    }
}

fn base_markdown(record_type: &RecordType) -> String {
    let dir = yaml_double_quoted(&record_type.dir);
    let type_id = yaml_double_quoted(&record_type.id);
    let display_name = yaml_plain_or_quoted(&record_type.label);
    let heading = collection_heading(&record_type.label);
    let view_name = match record_type.emoji.as_deref() {
        Some(emoji) if !emoji.is_empty() => {
            yaml_plain_or_quoted(&format!("{emoji} All {heading}"))
        }
        _ => yaml_plain_or_quoted(&format!("All {heading}")),
    };
    let mut out = String::new();
    out.push_str("filters:\n");
    out.push_str("  and:\n");
    out.push_str(&format!("    - file.inFolder({dir})\n"));
    out.push_str(&format!("    - note.type == link({type_id})\n"));
    out.push_str("formulas:\n");
    out.push_str("  name: if(title, file.asLink(title), file.asLink())\n");
    let status_property = if let Some(choices) = status_choices(record_type) {
        out.push_str("  status_label: ");
        out.push_str(&status_label_formula(choices));
        out.push('\n');
        "formula.status_label"
    } else {
        "status"
    };
    out.push_str("properties:\n");
    out.push_str("  formula.name:\n");
    out.push_str(&format!("    displayName: {display_name}\n"));
    if status_property == "formula.status_label" {
        out.push_str("  formula.status_label:\n");
        out.push_str("    displayName: Status\n");
    }
    out.push_str("views:\n");
    out.push_str("  - type: table\n");
    out.push_str(&format!("    name: {view_name}\n"));
    out.push_str("    groupBy:\n");
    out.push_str(&format!("      property: {status_property}\n"));
    out.push_str("      direction: ASC\n");
    out.push_str("    order:\n");
    out.push_str("      - formula.name\n");
    if status_property == "formula.status_label" {
        out.push_str("      - formula.status_label\n");
    } else {
        out.push_str("      - status\n");
    }
    out.push_str("    sort:\n");
    out.push_str("      - property: formula.name\n");
    out.push_str("        direction: ASC\n");
    out.push_str("    summaries:\n");
    out.push_str("      formula.name: Count\n");
    out
}

fn status_choices(record_type: &RecordType) -> Option<&[RecordChoice]> {
    record_type.fields.iter().find_map(|field| {
        if field.key != "status" {
            return None;
        }
        match &field.kind {
            FieldKind::Choices(choices) if !choices.is_empty() => {
                Some(choices.as_slice())
            }
            _ => None,
        }
    })
}

pub(crate) fn status_label_formula(choices: &[RecordChoice]) -> String {
    let mut expr = "status".to_owned();
    for choice in choices.iter().rev() {
        expr = format!(
            "if(status == {}, {}, {expr})",
            yaml_double_quoted(&choice.value),
            yaml_double_quoted(&choice.label),
        );
    }
    expr
}

fn yaml_double_quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn yaml_plain_or_quoted(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value.chars().any(|ch| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    ':' | '#'
                        | '{'
                        | '}'
                        | '['
                        | ']'
                        | ','
                        | '&'
                        | '*'
                        | '!'
                        | '|'
                        | '>'
                        | '\''
                        | '"'
                        | '%'
                        | '@'
                        | '`'
                )
        });
    if needs_quotes {
        yaml_double_quoted(value)
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::records::schema::{RecordField, RecordType};

    fn widget_type() -> RecordType {
        RecordType {
            id: "widget".to_owned(),
            dir: "widgets".to_owned(),
            label: "Widget".to_owned(),
            emoji: Some("🧩".to_owned()),
            parent: None,
            dated_notes: false,
            fields: vec![RecordField {
                key: "status".to_owned(),
                label: "Status".to_owned(),
                kind: FieldKind::Choices(vec![
                    RecordChoice {
                        value: "to_try".to_owned(),
                        label: "🔖 To try".to_owned(),
                    },
                    RecordChoice {
                        value: "done".to_owned(),
                        label: "✅ Done".to_owned(),
                    },
                ]),
                required: true,
                list: false,
                default: None,
                suggest: false,
                min: None,
                max: None,
            }],
            legacy: Vec::new(),
        }
    }

    #[test]
    fn parent_wikilink_wraps_stems_and_keeps_existing_links() {
        assert_eq!(parent_wikilink("fun"), "[[fun]]");
        assert_eq!(parent_wikilink("fun.md"), "[[fun]]");
        assert_eq!(parent_wikilink("[[fun.md|fun]]"), "[[fun.md|fun]]");
    }

    #[test]
    fn status_formula_nests_choice_labels() {
        let formula = status_label_formula(&[
            RecordChoice {
                value: "to_watch".to_owned(),
                label: "🔖 To watch".to_owned(),
            },
            RecordChoice {
                value: "watched".to_owned(),
                label: "✅ Watched".to_owned(),
            },
        ]);
        assert_eq!(
            formula,
            r#"if(status == "to_watch", "🔖 To watch", if(status == "watched", "✅ Watched", status))"#
        );
    }

    #[test]
    fn starter_base_is_valid_yaml_grouped_by_status_labels() {
        let markdown = base_markdown(&widget_type());
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&markdown).expect("parse starter Base");
        assert_eq!(
            parsed["filters"]["and"][0],
            serde_yaml::Value::String("file.inFolder(\"widgets\")".to_owned())
        );
        assert_eq!(
            parsed["views"][0]["groupBy"]["property"],
            serde_yaml::Value::String("formula.status_label".to_owned())
        );
        assert!(
            markdown.contains(
                r#"if(status == "to_try", "🔖 To try", if(status == "done", "✅ Done", status))"#
            ),
            "{markdown}"
        );
    }

    #[test]
    fn collection_heading_pluralizes_the_label() {
        assert_eq!(collection_heading("Movie"), "Movies");
        assert_eq!(collection_heading("TV show"), "TV shows");
        assert_eq!(collection_heading("Notes"), "Notes");
    }
}
