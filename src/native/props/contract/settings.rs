//! Stable setting order and key membership for area, project, and reference
//! notes; the bank hub is untyped and exposes no settings contract.

use serde::Serialize;

use crate::native::props::roles::NoteKind;

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Setting {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) keys: &'static [&'static str],
    pub(crate) stage: &'static str,
    pub(crate) kinds: &'static [&'static str],
}

const SUMMARY: Setting = Setting {
    id: "summary",
    label: "Summary",
    keys: &["summary"],
    stage: "summary",
    kinds: &["text"],
};
const REF_TITLE: Setting = Setting {
    id: "title",
    label: "Title",
    keys: &["title"],
    stage: "title",
    kinds: &["text"],
};
const AUTHOR: Setting = Setting {
    id: "author",
    label: "Authors",
    keys: &["author"],
    stage: "text",
    kinds: &["text"],
};
const PUBLISHED: Setting = Setting {
    id: "published",
    label: "Published",
    keys: &["published"],
    stage: "published",
    kinds: &["text"],
};
const SOURCE: Setting = Setting {
    id: "source",
    label: "Source",
    keys: &["source_url", "url"],
    stage: "url",
    kinds: &["text"],
};
const REF_ALIASES: Setting = Setting {
    id: "aliases",
    label: "Aliases",
    keys: &["aliases"],
    stage: "list",
    kinds: &["aliases"],
};
const READING: Setting = Setting {
    id: "reading",
    label: "Reading",
    keys: &["status"],
    stage: "read_only",
    kinds: &["text"],
};
const REF_PARENT: Setting = Setting {
    id: "parent",
    label: "Lives under",
    keys: &["parent"],
    stage: "read_only",
    kinds: &["text"],
};
const FILE_NAME: Setting = Setting {
    id: "file_name",
    label: "File name",
    keys: &[],
    stage: "read_only",
    kinds: &["text"],
};
const PARENT: Setting = Setting {
    id: "parent",
    label: "Lives under",
    keys: &["parent"],
    stage: "parent",
    kinds: &["text"],
};
const COLOR: Setting = Setting {
    id: "color",
    label: "Hue",
    keys: &["color"],
    stage: "hue",
    kinds: &["text"],
};
const TYPE: Setting = Setting {
    id: "task_type",
    label: "Default type",
    keys: &["task_type", "theme"],
    stage: "type",
    kinds: &["text"],
};
const CLOCK: Setting = Setting {
    id: "clock",
    label: "Clock",
    keys: &["offclock", "offclock_types"],
    stage: "clock",
    kinds: &["checkbox", "list"],
};
const ROOT: Setting = Setting {
    id: "root",
    label: "Stats root",
    keys: &["root"],
    stage: "toggle",
    kinds: &["checkbox"],
};
const CAP: Setting = Setting {
    id: "ready_cap",
    label: "Ready cap",
    keys: &["ready_cap"],
    stage: "count_or_off",
    kinds: &["number", "text"],
};
const REVIEW: Setting = Setting {
    id: "task_refresh",
    label: "Review",
    keys: &["task_refresh"],
    stage: "days",
    kinds: &["number"],
};
const ALIASES: Setting = Setting {
    id: "project_name_aliases",
    label: "Capture names",
    keys: &["project_name_aliases"],
    stage: "list",
    kinds: &["list"],
};

pub(crate) fn for_kind(kind: NoteKind) -> &'static [Setting] {
    match kind {
        NoteKind::Area => {
            &[SUMMARY, PARENT, COLOR, TYPE, CLOCK, CAP, REVIEW, ALIASES]
        }
        NoteKind::Project => {
            &[PARENT, COLOR, TYPE, CLOCK, ROOT, CAP, REVIEW, ALIASES]
        }
        NoteKind::Reference => &[
            REF_TITLE,
            SUMMARY,
            AUTHOR,
            PUBLISHED,
            SOURCE,
            REF_ALIASES,
            READING,
            REF_PARENT,
            FILE_NAME,
        ],
        _ => &[],
    }
}

pub(crate) fn for_key(kind: NoteKind, key: &str) -> Option<&'static Setting> {
    for_kind(kind)
        .iter()
        .find(|setting| setting.keys.contains(&key))
}

pub(crate) fn by_id(kind: NoteKind, id: &str) -> Option<&'static Setting> {
    for_kind(kind).iter().find(|setting| setting.id == id)
}

pub(crate) fn known_id(id: &str) -> bool {
    by_id(NoteKind::Area, id).is_some()
        || by_id(NoteKind::Project, id).is_some()
        || by_id(NoteKind::Reference, id).is_some()
}

/// Choose the stored source key for the one-field Source setting. Legacy
/// notes retain `url`; new notes use `source_url`.
pub(crate) fn source_key(
    document: &super::super::document::PropsDocument,
) -> &'static str {
    if document
        .entries
        .iter()
        .any(|entry| entry.key == "source_url")
    {
        "source_url"
    } else if document.entries.iter().any(|entry| entry.key == "url") {
        "url"
    } else {
        "source_url"
    }
}
