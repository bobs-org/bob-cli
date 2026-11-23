//! Authoritative note-kind and property-owner classification for props.

use std::{collections::BTreeSet, path::Path};

use serde::Serialize;

use crate::native::{
    intentions::subject::NoteSubject,
    projects::{
        frontmatter_is_album, frontmatter_is_area, frontmatter_is_notebook,
        frontmatter_is_project, frontmatter_type_names, parse_frontmatter,
    },
    records::{
        catalog::RecordCatalog,
        context::{RecordContext, RecordKindResolution},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NoteKind {
    Day,
    Week,
    Quarter,
    Year,
    AllTime,
    Record,
    Album,
    Bank,
    Area,
    Project,
    Notebook,
    Reference,
    Note,
}

impl NoteKind {
    pub(crate) fn wire_name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Quarter => "quarter",
            Self::Year => "year",
            Self::AllTime => "all_time",
            Self::Record => "record",
            Self::Album => "album",
            Self::Bank => "bank",
            Self::Area => "area",
            Self::Project => "project",
            Self::Notebook => "notebook",
            Self::Reference => "reference",
            Self::Note => "note",
        }
    }

    fn from_subject(subject: NoteSubject) -> Self {
        match subject {
            NoteSubject::Day(_) => Self::Day,
            NoteSubject::Period(
                crate::native::periods::calendar::PeriodId::Week { .. },
            ) => Self::Week,
            NoteSubject::Period(
                crate::native::periods::calendar::PeriodId::Quarter { .. },
            ) => Self::Quarter,
            NoteSubject::Period(
                crate::native::periods::calendar::PeriodId::Year(_),
            ) => Self::Year,
            NoteSubject::Period(
                crate::native::periods::calendar::PeriodId::AllTime,
            ) => Self::AllTime,
        }
    }

    fn is_temporal(self) -> bool {
        matches!(
            self,
            Self::Day | Self::Week | Self::Quarter | Self::Year | Self::AllTime
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Loose,
    Card,
    Identity,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PropertyRole {
    /// `None` means the schema needed to decide ownership is unavailable.
    pub(crate) role: Option<Role>,
    pub(crate) owner: Option<String>,
    pub(crate) editor: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) diagnostic: Option<String>,
}

/// Resolved per-note ownership context shared by show and update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoleContext {
    pub(crate) note_kind: NoteKind,
    pub(crate) record_fields: Option<BTreeSet<String>>,
    /// Any nonempty diagnostic makes writes unsafe until discovery succeeds.
    pub(crate) diagnostics: Vec<String>,
}

impl RoleContext {
    pub(crate) fn classify(&self, key: &str) -> PropertyRole {
        if key == "type" {
            return locked(
                Role::Identity,
                "`type` identifies the note's property-card species and cannot be changed here.",
            );
        }
        if matches!(self.note_kind, NoteKind::Record | NoteKind::Album)
            && key == "parent"
        {
            return locked(
                Role::Identity,
                "`parent` establishes the record or album's canonical home.",
            );
        }
        if self.note_kind == NoteKind::Album && key == "title" {
            return locked(
                Role::Identity,
                "`title` identifies the album and its schema source.",
            );
        }
        if self.note_kind.is_temporal() && key == "parent" {
            return PropertyRole {
                role: Some(Role::Derived),
                owner: Some("periods".to_owned()),
                editor: None,
                reason: Some(
                    "Bob derives this parent link from the canonical day or period hierarchy."
                        .to_owned(),
                ),
                diagnostic: None,
            };
        }
        if is_derived_key(key)
            || (self.note_kind == NoteKind::Reference
                && key == "source_markdown")
            || (self.note_kind == NoteKind::Project
                && matches!(key, "status" | "scheduled"))
        {
            return derived_role(key, self.note_kind);
        }
        if self.note_kind.is_temporal()
            && matches!(key, "intentions" | "summary" | "reflection")
        {
            return card("intentions");
        }
        if self.note_kind == NoteKind::Album && key == "record" {
            return card("album");
        }
        if self.note_kind == NoteKind::Bank && key == "bank" {
            return card("bank");
        }
        if self.note_kind == NoteKind::Record {
            if let Some(fields) = &self.record_fields {
                if fields.contains(key) {
                    return card("record");
                }
            } else {
                return PropertyRole {
                    role: None,
                    owner: None,
                    editor: None,
                    reason: None,
                    diagnostic: self.diagnostics.first().cloned().or_else(|| {
                        Some("record schema could not be resolved; property ownership is unknown".to_owned())
                    }),
                };
            }
        }
        if self.note_kind == NoteKind::Note
            && let Some(diagnostic) = self.diagnostics.first()
        {
            return PropertyRole {
                role: None,
                owner: None,
                editor: None,
                reason: None,
                diagnostic: Some(diagnostic.clone()),
            };
        }
        loose()
    }

    pub(crate) fn writes_are_safe(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Resolve kinds from Bob's established classifiers and canonical indexes.
/// `records` must belong to `vault`; a partial registry is carried as a
/// diagnostic so a caller can report it and refuse dependent writes.
pub(crate) fn resolve_note_context(
    vault: &Path,
    relative_path: &str,
    markdown: &str,
    records: &RecordContext,
) -> RoleContext {
    if let Some(subject) = NoteSubject::canonical(relative_path) {
        return temporal_context(NoteKind::from_subject(subject));
    }

    let mut diagnostics = Vec::new();
    let path = Path::new(relative_path);
    if relative_path.replace('\\', "/") == "bank.md" {
        return RoleContext {
            note_kind: NoteKind::Bank,
            record_fields: None,
            diagnostics,
        };
    }

    let frontmatter = parse_frontmatter(markdown);
    if let Some(frontmatter) = frontmatter.as_ref() {
        let canonical_album = records.album_claims().iter().any(|album| {
            album.declaration.path == relative_path.replace('\\', "/")
        });
        if canonical_album || frontmatter_is_album(frontmatter) {
            return RoleContext {
                note_kind: NoteKind::Album,
                record_fields: None,
                diagnostics,
            };
        }
        if frontmatter_is_area(frontmatter) {
            return RoleContext {
                note_kind: NoteKind::Area,
                record_fields: None,
                diagnostics,
            };
        }
        if frontmatter_is_project(frontmatter) {
            return RoleContext {
                note_kind: NoteKind::Project,
                record_fields: None,
                diagnostics,
            };
        }
        if frontmatter_is_notebook(frontmatter) {
            return RoleContext {
                note_kind: NoteKind::Notebook,
                record_fields: None,
                diagnostics,
            };
        }
        let type_names = frontmatter_type_names(frontmatter);
        if type_names.iter().any(|name| name == "ref") {
            return RoleContext {
                note_kind: NoteKind::Reference,
                record_fields: None,
                diagnostics,
            };
        }

        if !records.registry().is_for_target(vault)
            && type_names.iter().any(|name| {
                path.parent().is_some_and(|parent| {
                    parent.file_name().is_some_and(|folder| {
                        folder.to_string_lossy().eq_ignore_ascii_case(name)
                    })
                })
            })
        {
            diagnostics.push(
                "record schema registry belongs to a different vault; property ownership is unresolved"
                    .to_owned(),
            );
            return RoleContext {
                note_kind: NoteKind::Record,
                record_fields: None,
                diagnostics,
            };
        }
        if let Some((kind, unresolved)) =
            resolve_record_kind(vault, path, &type_names, records)
        {
            match kind {
                Some(record_fields) => {
                    return RoleContext {
                        note_kind: NoteKind::Record,
                        record_fields: Some(record_fields),
                        diagnostics,
                    };
                }
                None => {
                    diagnostics.push(unresolved.unwrap_or_else(|| {
                        "record schema could not be resolved".to_owned()
                    }));
                    return RoleContext {
                        note_kind: NoteKind::Record,
                        record_fields: None,
                        diagnostics,
                    };
                }
            }
        }
        let has_custom_type = type_names.iter().any(|name| {
            !matches!(
                name.as_str(),
                "area"
                    | "project"
                    | "notebook"
                    | "album"
                    | "record"
                    | "ref"
                    | "article"
                    | "mail"
                    | "done"
                    | "day"
            )
        });
        if has_custom_type
            && !matches!(
                &records.registry().health,
                crate::native::albums::registry::DiscoveryHealth::Complete
            )
        {
            diagnostics.push(
                "record schema discovery is incomplete; this custom note type may own properties"
                    .to_owned(),
            );
        }
    }

    RoleContext {
        note_kind: NoteKind::Note,
        record_fields: None,
        diagnostics,
    }
}

fn temporal_context(note_kind: NoteKind) -> RoleContext {
    RoleContext {
        note_kind,
        record_fields: None,
        diagnostics: Vec::new(),
    }
}

fn resolve_record_kind(
    vault: &Path,
    path: &Path,
    type_names: &[String],
    records: &RecordContext,
) -> Option<(Option<BTreeSet<String>>, Option<String>)> {
    for type_name in type_names {
        match records.resolve(type_name) {
            RecordKindResolution::Valid { kind, health, .. } => {
                let expected_dir = Path::new(&kind.record_type().dir);
                if path.parent() != Some(expected_dir) {
                    continue;
                }
                if !matches!(
                    health,
                    crate::native::albums::registry::DiscoveryHealth::Complete
                ) {
                    return Some((
                        None,
                        Some(
                            "record schema discovery is incomplete".to_owned(),
                        ),
                    ));
                }
                let mut catalog = RecordCatalog::new(vault);
                let folder = catalog.folder(&kind.record_type().dir);
                if folder.notes().iter().any(|note| note.relative_path == path)
                {
                    let fields = kind
                        .record_type()
                        .fields
                        .iter()
                        .map(|field| field.key.clone())
                        .collect();
                    return Some((Some(fields), None));
                }
                return Some((
                    None,
                    Some(format!(
                        "{} is in record folder `{}` but is absent from the canonical record index",
                        path.display(),
                        kind.record_type().dir
                    )),
                ));
            }
            RecordKindResolution::Invalid { paths, .. } => {
                if potential_record_folder(path, &paths) {
                    return Some((
                        None,
                        Some(format!(
                            "record type `{type_name}` has an invalid album schema; property ownership is unresolved"
                        )),
                    ));
                }
            }
            RecordKindResolution::DiscoveryIncomplete { errors } => {
                if path.parent().is_some_and(|parent| {
                    parent.file_name().is_some_and(|name| {
                        name.to_string_lossy().eq_ignore_ascii_case(type_name)
                    })
                }) {
                    return Some((
                        None,
                        Some(format!(
                            "record schema discovery failed: {}",
                            errors.join("; ")
                        )),
                    ));
                }
            }
            RecordKindResolution::Unclaimed { .. } => {}
        }
    }
    None
}

fn potential_record_folder(path: &Path, album_paths: &[String]) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    album_paths.iter().any(|album_path| {
        Path::new(album_path)
            .file_stem()
            .is_some_and(|stem| Some(stem) == parent.file_name())
    })
}

/// Derived patterns are audited from the current frontmatter writers:
/// `intentions::write`, `periods::draft`, and `periods::migrate_intentions`
/// write the digest/drafted markers, while `periods::note` carries them across
/// rewrites; the Zorg migration and Gmail archive paths own `zorg_*` and
/// `generated_from_zorg*`; reference sync/attach/overwrite own the write-once
/// `source_markdown` pointer; `bob task archive` owns `done_tasks`; the
/// project-tasks plugin owns `task_count`/`open_task_count`; `bob projects
/// sync` owns project `status`/`scheduled`. `created` and `template` stay
/// loose: a creation default alone does not make a field derived.
fn is_derived_key(key: &str) -> bool {
    key.ends_with("_digest")
        || key.ends_with("_drafted")
        || key.starts_with("review_")
        || key.starts_with("zorg_")
        || key.starts_with("generated_from_zorg")
        || key == "done_tasks"
        || key == "task_count"
        || key == "open_task_count"
}

fn derived_role(key: &str, note_kind: NoteKind) -> PropertyRole {
    let (owner, reason) = if key == "source_markdown"
        && note_kind == NoteKind::Reference
    {
        (
            "reference importer",
            "the reference importer points this key at write-once saved source Markdown.",
        )
    } else if key.starts_with("zorg_") || key.starts_with("generated_from_zorg")
    {
        (
            "Zorg importer",
            "the importer records source lineage and generated-note status here.",
        )
    } else if key == "done_tasks" {
        (
            "bob task archive",
            "bob task archive owns the done_tasks archive link.",
        )
    } else if key == "task_count" || key == "open_task_count" {
        (
            "bob-project-tasks",
            "the project-tasks plugin derives these counts from the note's tasks.",
        )
    } else if note_kind == NoteKind::Project
        && matches!(key, "status" | "scheduled")
    {
        (
            "bob projects sync",
            "bob projects sync owns project status and scheduled dates.",
        )
    } else if key.ends_with("_digest")
        || key.ends_with("_drafted")
        || key.starts_with("review_")
    {
        (
            "intentions",
            "Bob derives review and drafted markers from the intention-card workflow.",
        )
    } else {
        (
            "Bob",
            "Bob derives this metadata; edit the source or use its owning command.",
        )
    };
    PropertyRole {
        role: Some(Role::Derived),
        owner: Some(owner.to_owned()),
        editor: None,
        reason: Some(reason.to_owned()),
        diagnostic: None,
    }
}

fn locked(role: Role, reason: &str) -> PropertyRole {
    PropertyRole {
        role: Some(role),
        owner: None,
        editor: None,
        reason: Some(reason.to_owned()),
        diagnostic: None,
    }
}

fn card(owner: &str) -> PropertyRole {
    PropertyRole {
        role: Some(Role::Card),
        owner: Some(owner.to_owned()),
        editor: Some("==".to_owned()),
        reason: None,
        diagnostic: None,
    }
}

fn loose() -> PropertyRole {
    PropertyRole {
        role: Some(Role::Loose),
        owner: Some("props".to_owned()),
        editor: None,
        reason: None,
        diagnostic: None,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::Path};

    use super::{
        resolve_note_context, NoteKind, NoteSubject, RecordContext, Role,
        RoleContext,
    };

    fn context(note_kind: NoteKind, fields: Option<&[&str]>) -> RoleContext {
        RoleContext {
            note_kind,
            record_fields: fields.map(|fields| {
                fields
                    .iter()
                    .map(|field| (*field).to_owned())
                    .collect::<BTreeSet<_>>()
            }),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn roles_enforce_identity_derived_and_single_card_owners() {
        let day = context(NoteKind::Day, None);
        assert_eq!(day.classify("type").role, Some(Role::Identity));
        assert_eq!(day.classify("parent").role, Some(Role::Derived));
        assert_eq!(
            day.classify("summary").owner.as_deref(),
            Some("intentions")
        );
        assert_eq!(day.classify("summary_drafted").role, Some(Role::Derived));
        assert_eq!(day.classify("created").role, Some(Role::Loose));

        let reference = context(NoteKind::Reference, None);
        assert_eq!(
            reference.classify("source_markdown").role,
            Some(Role::Derived)
        );

        let album = context(NoteKind::Album, None);
        assert_eq!(album.classify("parent").role, Some(Role::Identity));
        assert_eq!(album.classify("title").role, Some(Role::Identity));
        assert_eq!(album.classify("record").owner.as_deref(), Some("album"));
    }

    #[test]
    fn record_schema_fields_route_to_records_and_unknown_schema_is_not_loose() {
        let record = context(NoteKind::Record, Some(&["rating", "visited"]));
        assert_eq!(record.classify("rating").owner.as_deref(), Some("record"));
        assert_eq!(record.classify("rating").editor.as_deref(), Some("=="));
        assert_eq!(record.classify("custom").role, Some(Role::Loose));

        let unknown = RoleContext {
            note_kind: NoteKind::Record,
            record_fields: None,
            diagnostics: vec!["album schema unreadable".to_owned()],
        };
        assert_eq!(unknown.classify("rating").role, None);
        assert!(unknown.classify("rating").diagnostic.is_some());
        assert!(!unknown.writes_are_safe());
    }

    #[test]
    fn wire_note_kind_names_are_stable() {
        assert_eq!(NoteKind::AllTime.wire_name(), "all_time");
        assert_eq!(NoteKind::Reference.wire_name(), "reference");
        assert_eq!(NoteKind::Bank.wire_name(), "bank");
    }

    #[test]
    fn canonical_temporal_subjects_use_the_existing_period_classifier() {
        assert_eq!(
            NoteKind::from_subject(
                NoteSubject::canonical("2026/20261122.md").unwrap()
            ),
            NoteKind::Day
        );
        assert_eq!(
            NoteKind::from_subject(
                NoteSubject::canonical("2026/2026-W47.md").unwrap()
            ),
            NoteKind::Week
        );
        assert_eq!(
            NoteKind::from_subject(
                NoteSubject::canonical("2026/2026-Q4.md").unwrap()
            ),
            NoteKind::Quarter
        );
        assert_eq!(
            NoteKind::from_subject(NoteSubject::canonical("2026.md").unwrap()),
            NoteKind::Year
        );
        assert_eq!(
            NoteKind::from_subject(
                NoteSubject::canonical("all_time.md").unwrap()
            ),
            NoteKind::AllTime
        );
        assert!(NoteSubject::canonical("notes/20261122.md").is_none());
    }

    #[test]
    fn every_note_kind_classifies_card_and_prospective_keys() {
        for note_kind in [
            NoteKind::Day,
            NoteKind::Week,
            NoteKind::Quarter,
            NoteKind::Year,
            NoteKind::AllTime,
        ] {
            let temporal = context(note_kind, None);
            assert_eq!(temporal.classify("parent").role, Some(Role::Derived));
            assert_eq!(
                temporal.classify("intentions").owner.as_deref(),
                Some("intentions")
            );
        }

        let project = context(NoteKind::Project, None);
        let notebook = context(NoteKind::Notebook, None);
        let reference = context(NoteKind::Reference, None);
        let note = context(NoteKind::Note, None);
        assert_eq!(project.classify("created").role, Some(Role::Loose));
        assert_eq!(project.classify("template").role, Some(Role::Loose));
        assert_eq!(project.classify("status").role, Some(Role::Derived));
        assert_eq!(
            project.classify("status").owner.as_deref(),
            Some("bob projects sync")
        );
        assert_eq!(project.classify("scheduled").role, Some(Role::Derived));
        assert_eq!(project.classify("done_tasks").role, Some(Role::Derived));
        assert_eq!(
            project.classify("done_tasks").owner.as_deref(),
            Some("bob task archive")
        );
        assert_eq!(project.classify("task_count").role, Some(Role::Derived));
        assert_eq!(
            project.classify("open_task_count").role,
            Some(Role::Derived)
        );
        assert_eq!(notebook.classify("created").role, Some(Role::Loose));
        assert_eq!(
            reference.classify("source_markdown").role,
            Some(Role::Derived)
        );
        assert_eq!(reference.classify("created").role, Some(Role::Loose));
        assert_eq!(note.classify("custom_new_key").role, Some(Role::Loose));
    }

    #[test]
    fn resolver_uses_canonical_album_record_area_and_bank_rules() {
        fn write(root: &Path, relative: &str, text: &str) {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("fixture directories");
            }
            fs::write(path, text).expect("fixture note");
        }

        let temp = tempfile::tempdir().expect("fixture vault");
        let vault = temp.path();
        write(vault, "body.md", "---\ntype: [[area]]\n---\n# Body\n");
        write(
            vault,
            "dev.md",
            "---\ntype: [[project]]\nparent: [[body]]\nroot: true\n---\n# Development\n",
        );
        write(
            vault,
            "eat.md",
            "---\ntype: [[album]]\nparent: [[dev]]\ntitle: Restaurants\nrecord:\n  noun: restaurant\n  fields:\n    rating:\n      values: number\n---\n# Restaurants\n",
        );
        write(vault, "notebook.md", "---\ntype: [[notebook]]\n---\n");
        write(vault, "reference.md", "---\ntype: [[ref]]\n---\n");
        let record = "---\ntype: [[restaurant]]\nparent: [[eat]]\ntitle: Ramen\nrating: 5\ncreated: 2026-11-22\n---\n# Ramen\n";
        write(vault, "eat/ramen.md", record);
        let bank = "---\ntype: [[area]]\nbank: {accounts: {}}\n---\n# Bank\n";
        write(vault, "bank.md", bank);

        let records = RecordContext::discover(vault);
        let album = resolve_note_context(
            vault,
            "eat.md",
            &fs::read_to_string(vault.join("eat.md")).unwrap(),
            &records,
        );
        assert_eq!(album.note_kind, NoteKind::Album);
        assert_eq!(album.classify("record").owner.as_deref(), Some("album"));
        assert_eq!(album.classify("parent").role, Some(Role::Identity));

        let record_context =
            resolve_note_context(vault, "eat/ramen.md", record, &records);
        assert_eq!(record_context.note_kind, NoteKind::Record);
        assert_eq!(
            record_context.classify("rating").owner.as_deref(),
            Some("record")
        );
        assert_eq!(
            record_context.classify("parent").role,
            Some(Role::Identity)
        );
        assert_eq!(record_context.classify("created").role, Some(Role::Loose));

        let bank_context =
            resolve_note_context(vault, "bank.md", bank, &records);
        assert_eq!(bank_context.note_kind, NoteKind::Bank);
        assert_eq!(
            bank_context.classify("bank").owner.as_deref(),
            Some("bank")
        );
        assert_eq!(
            bank_context.classify("account_note").role,
            Some(Role::Loose)
        );

        let area = resolve_note_context(
            vault,
            "body.md",
            &fs::read_to_string(vault.join("body.md")).unwrap(),
            &records,
        );
        assert_eq!(area.note_kind, NoteKind::Area);
        assert_eq!(area.classify("done_tasks").role, Some(Role::Derived));
        assert_eq!(area.classify("status").role, Some(Role::Loose));
        assert_eq!(area.classify("task_count").role, Some(Role::Derived));
        assert_eq!(area.classify("template").role, Some(Role::Loose));

        let bom_area = resolve_note_context(
            vault,
            "body.md",
            "\u{feff}---\ntype: [[area]]\n---\n# Body\n",
            &records,
        );
        assert_eq!(bom_area.note_kind, NoteKind::Area);

        let project = resolve_note_context(
            vault,
            "dev.md",
            &fs::read_to_string(vault.join("dev.md")).unwrap(),
            &records,
        );
        assert_eq!(project.note_kind, NoteKind::Project);

        let notebook = resolve_note_context(
            vault,
            "notebook.md",
            &fs::read_to_string(vault.join("notebook.md")).unwrap(),
            &records,
        );
        assert_eq!(notebook.note_kind, NoteKind::Notebook);

        let reference = resolve_note_context(
            vault,
            "reference.md",
            &fs::read_to_string(vault.join("reference.md")).unwrap(),
            &records,
        );
        assert_eq!(reference.note_kind, NoteKind::Reference);
        assert_eq!(
            reference.classify("source_markdown").role,
            Some(Role::Derived)
        );
    }
}
