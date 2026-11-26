//! Stable show-report projection for `bob props show`.

use serde::Serialize;
use serde_json::Value as JsonValue;

use super::{
    document::{FrontmatterError, PropertyKind, PropsDocument, ValueStyle},
    roles::{PropertyRole, RoleContext},
};

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const CAPABILITY: &str = "props_v1";
pub(crate) const CONTRACT_CAPABILITY: &str = "props_contract_v1";
pub(crate) const REF_CONTRACT_CAPABILITY: &str = "props_ref_contract_v1";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ShowReport {
    pub(crate) schema_version: u32,
    /// `true` means the show request succeeded; damaged YAML is still a
    /// successful recovery report, with its state described below.
    pub(crate) ok: bool,
    pub(crate) path: String,
    pub(crate) note_kind: String,
    pub(crate) capabilities: Vec<&'static str>,
    pub(crate) fingerprint: Option<String>,
    pub(crate) frontmatter: FrontmatterReport,
    pub(crate) properties: Vec<ShowProperty>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) contract: Option<super::contract::read::ContractBlock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) ownership_diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct FrontmatterReport {
    pub(crate) state: super::document::FrontmatterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<FrontmatterError>,
    pub(crate) start_line: Option<usize>,
    pub(crate) end_line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ShowProperty {
    pub(crate) key: String,
    pub(crate) line: Option<usize>,
    pub(crate) end_line: Option<usize>,
    pub(crate) value: JsonValue,
    pub(crate) kind: PropertyKind,
    pub(crate) style: ValueStyle,
    pub(crate) role: Option<super::roles::Role>,
    pub(crate) owner: Option<String>,
    pub(crate) editor: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) duplicate: bool,
    pub(crate) opaque: bool,
    pub(crate) legacy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) setting: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic: Option<String>,
}

pub(crate) fn build_report(
    path: &str,
    document: &PropsDocument,
    roles: &RoleContext,
) -> ShowReport {
    let properties = document
        .entries
        .iter()
        .map(|entry| {
            let PropertyRole {
                role,
                owner,
                editor,
                reason,
                diagnostic: role_diagnostic,
            } = roles.classify(&entry.key);
            ShowProperty {
                key: entry.key.clone(),
                line: entry.line,
                end_line: entry.end_line,
                value: entry.value.clone(),
                kind: entry.kind,
                style: entry.style.clone(),
                role,
                owner,
                editor,
                reason,
                duplicate: entry.duplicate,
                opaque: entry.opaque,
                legacy: matches!(
                    entry.key.as_str(),
                    "task_count" | "open_task_count"
                ) || (roles.note_kind
                    == super::roles::NoteKind::Project
                    && entry.key == "scheduled")
                    || (super::contract::settings::for_key(
                        roles.note_kind,
                        &entry.key,
                    )
                    .is_some()
                        && entry.key == "theme"),
                setting: super::contract::settings::for_key(
                    roles.note_kind,
                    &entry.key,
                )
                .map(|setting| setting.id),
                diagnostic: entry.diagnostic.clone().or(role_diagnostic),
            }
        })
        .collect();
    ShowReport {
        schema_version: SCHEMA_VERSION,
        ok: true,
        path: path.to_owned(),
        note_kind: roles.note_kind.wire_name().to_owned(),
        capabilities: vec![CAPABILITY],
        fingerprint: document.fingerprint.clone(),
        frontmatter: FrontmatterReport {
            state: document.state,
            error: document.error.clone(),
            start_line: document.start_line,
            end_line: document.end_line,
        },
        properties,
        contract: None,
        ownership_diagnostics: roles.diagnostics.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::build_report;
    use crate::native::props::{
        document::{FrontmatterState, PropsDocument},
        roles::{NoteKind, RoleContext},
    };

    #[test]
    fn report_keeps_contract_fields_and_reports_damaged_yaml() {
        let document = PropsDocument::parse("---\nname: [\n---\nbody\n");
        let roles = RoleContext {
            note_kind: NoteKind::Note,
            record_fields: Some(BTreeSet::new()),
            reference_has_task: false,
            diagnostics: Vec::new(),
        };
        let report = build_report("notes/a.md", &document, &roles);
        let json = serde_json::to_value(&report).expect("report JSON");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["ok"], true);
        assert_eq!(json["path"], "notes/a.md");
        assert_eq!(json["note_kind"], "note");
        assert_eq!(json["capabilities"][0], "props_v1");
        assert_eq!(json["fingerprint"], serde_json::Value::Null);
        assert_eq!(report.frontmatter.state, FrontmatterState::Invalid);
        assert_eq!(
            report.properties[0].kind,
            crate::native::props::document::PropertyKind::Opaque
        );
        assert_eq!(report.properties[0].line, Some(2));
    }
}
