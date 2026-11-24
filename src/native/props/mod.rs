//! Source-preserving note-property inventory and ownership model.
//!
//! This module stays independent of filesystem I/O. The runtime resolves a
//! [`RoleContext`] from the current note and schema, then calls [`inspect`]
//! with that context and a normalized path. [`PropsInspection`] carries the
//! ordered byte spans, parsed semantic mapping, show report, and shared typed
//! diagnostics the writer can use when it replans against fresh input under
//! its lock. Context discovery stays in [`roles::resolve_note_context`] so it
//! can be rerun after locking.

pub(crate) mod cli;
pub(crate) mod contract;
pub(crate) mod document;
pub(crate) mod emit;
pub(crate) mod fingerprint;
#[cfg(test)]
mod golden;
pub(crate) mod plan;
pub(crate) mod protocol;
pub(crate) mod roles;
pub(crate) mod show;

use serde::Serialize;

use self::{
    document::PropsDocument,
    roles::RoleContext,
    show::{build_report, ShowReport},
};

/// The common refusal and diagnostic shape used by inspection and the writer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PropsError {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) op_index: Option<usize>,
}

/// Per-note inputs already resolved by the caller. `roles` carries the note
/// kind and any discovered record schema; this pure seam performs no I/O.
#[derive(Debug, Clone)]
pub(crate) struct InspectContext {
    pub(crate) normalized_path: String,
    pub(crate) roles: RoleContext,
    pub(crate) contract: Option<contract::Context>,
}

/// Shared source, semantic, report, and diagnostic view for one inspection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PropsInspection {
    pub(crate) document: PropsDocument,
    pub(crate) report: ShowReport,
    pub(crate) diagnostics: Vec<PropsError>,
}

/// Inspect one Markdown note using caller-resolved note context.
pub(crate) fn inspect(
    contents: &str,
    context: &InspectContext,
) -> PropsInspection {
    let document = PropsDocument::parse(contents);
    let mut report =
        build_report(&context.normalized_path, &document, &context.roles);
    if let Some(contract) = &context.contract {
        report.contract = contract::read::build(
            contract,
            &context.normalized_path,
            contents,
            context.roles.note_kind,
            &document,
        );
        if report.contract.is_some() {
            report.capabilities =
                contract::read::capabilities(context.roles.note_kind);
        }
    }
    let mut diagnostics = Vec::new();
    if let Some(error) = &document.error {
        diagnostics.push(error.clone());
    }
    diagnostics.extend(context.roles.diagnostics.iter().map(|message| {
        PropsError {
            code: "prop_ownership_unresolved".to_owned(),
            message: message.clone(),
            hint:
                "Resolve note and schema ownership before editing properties."
                    .to_owned(),
            key: None,
            line: None,
            op_index: None,
        }
    }));
    diagnostics.extend(
        document
            .entries
            .iter()
            .filter(|entry| entry.opaque && !entry.duplicate)
            .map(|entry| {
                PropsError {
            code: "prop_opaque".to_owned(),
            message: entry
                .diagnostic
                .clone()
                .unwrap_or_else(|| "property value is opaque".to_owned()),
            hint: "Repair or edit this YAML value outside the property writer."
                .to_owned(),
            key: Some(entry.key.clone()),
            line: entry.line,
            op_index: None,
        }
            }),
    );
    PropsInspection {
        document,
        report,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_yaml::Value as YamlValue;

    use super::{inspect, InspectContext};
    use crate::native::props::roles::{NoteKind, Role, RoleContext};

    fn context(note_kind: NoteKind) -> InspectContext {
        InspectContext {
            contract: None,
            normalized_path: "notes/example.md".to_owned(),
            roles: RoleContext {
                note_kind,
                record_fields: Some(BTreeSet::new()),
                diagnostics: Vec::new(),
            },
        }
    }

    #[test]
    fn inspect_returns_source_model_and_report_from_resolved_context() {
        let contents = "---\ncreated: 2026-11-22\nsummary: draft\n---\nbody\n";
        let result = inspect(contents, &context(NoteKind::Day));
        assert_eq!(result.document.source, contents);
        assert_eq!(result.document.entries.len(), 2);
        assert!(result.document.frontmatter_range.is_some());
        assert_eq!(result.report.path, "notes/example.md");
        assert_eq!(result.report.note_kind, "day");
        assert_eq!(result.report.properties[0].role, Some(Role::Loose));
        assert_eq!(
            result.report.properties[1].owner.as_deref(),
            Some("intentions")
        );
        let mapping = result
            .document
            .semantic_mapping
            .as_ref()
            .unwrap()
            .as_mapping()
            .unwrap();
        assert_eq!(
            mapping.get(YamlValue::String("summary".to_owned())),
            Some(&YamlValue::String("draft".to_owned()))
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn inspect_returns_shared_typed_frontmatter_and_ownership_errors() {
        let mut context = context(NoteKind::Record);
        context.roles.record_fields = None;
        context.roles.diagnostics =
            vec!["record schema unavailable".to_owned()];
        let result = inspect("---\nrating: [\n---\n", &context);
        assert!(result
            .diagnostics
            .iter()
            .any(|error| error.code == "prop_invalid_yaml"
                && error.line.is_some()));
        assert!(result.diagnostics.iter().any(|error| {
            error.code == "prop_ownership_unresolved"
                && error.hint.contains("ownership")
        }));
        assert_eq!(
            result.report.properties[0].role, None,
            "unresolved record fields must not become loose"
        );
    }
}
