//! Canonical owned-field link repair through the verified YAML writer.

use std::path::Path;

use super::{
    fields::{self, FieldProvenance, FrontmatterState},
    subject::NoteSubject,
    write::{self, OwnedPatch, Patch, WriteError},
};
use crate::native::collect_done::{MovedBlockTargets, ResolveMovedNote};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairResult {
    pub(crate) contents: String,
    pub(crate) link_count: usize,
}

/// Which stored field supplies one rewrite callback value. The index is the
/// position inside a sequence-valued `intentions` field and zero otherwise,
/// so callers can map a rewritten value back to its original-source lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedField {
    Intentions,
    Summary,
    Reflection,
}

/// Rewrite links only in stored, valid intention fields on a canonical note.
/// The caller supplies the Markdown resolver so each rewrite surface keeps
/// its existing target resolution and code/comment exclusions.
pub(crate) fn rewrite_owned_links(
    contents: &str,
    subject: NoteSubject,
    mut rewrite: impl FnMut(&str) -> (String, usize),
) -> Result<RepairResult, WriteError> {
    rewrite_owned_links_with(contents, subject, |_, _, value| rewrite(value))
}

/// Field-aware variant of [`rewrite_owned_links`] for callers that report
/// per-value evidence against original-source lines.
pub(crate) fn rewrite_owned_links_with(
    contents: &str,
    subject: NoteSubject,
    mut rewrite: impl FnMut(OwnedField, usize, &str) -> (String, usize),
) -> Result<RepairResult, WriteError> {
    let stored = fields::read(contents, subject);
    if stored.frontmatter_state != FrontmatterState::Valid {
        return Ok(RepairResult {
            contents: contents.to_owned(),
            link_count: 0,
        });
    }

    let mut patch = OwnedPatch::default();
    let mut link_count = 0;

    if stored.intentions.provenance == FieldProvenance::Frontmatter
        && stored.intentions.valid
        && let Some(values) = stored.intentions.value.as_ref()
    {
        let mut rewritten_values = Vec::with_capacity(values.len());
        let mut changed = false;
        for (index, value) in values.iter().enumerate() {
            let (rewritten, count) =
                rewrite(OwnedField::Intentions, index, value);
            if rewritten != *value {
                changed = true;
                link_count += count;
            }
            rewritten_values.push(rewritten);
        }
        if changed {
            // The verified writer represents a valid scalar intention as its
            // normal one-item list when a link rewrite changes it.
            patch.intentions = Patch::Set(rewritten_values);
        }
    }

    if stored.summary.provenance == FieldProvenance::Frontmatter
        && stored.summary.valid
        && let Some(value) = stored.summary.value.as_deref()
    {
        let (rewritten, count) = rewrite(OwnedField::Summary, 0, value);
        if rewritten != value {
            link_count += count;
            patch.summary = Patch::Set(rewritten);
        }
    }

    if stored.reflection.provenance == FieldProvenance::Frontmatter
        && stored.reflection.valid
        && let Some(value) = stored.reflection.value.as_deref()
    {
        let (rewritten, count) = rewrite(OwnedField::Reflection, 0, value);
        if rewritten != value {
            link_count += count;
            patch.reflection = Patch::Set(rewritten);
        }
    }

    let repaired = write::apply(contents, &patch)?;
    Ok(RepairResult {
        contents: repaired,
        link_count,
    })
}

/// Rewrite moved-block links inside stored, valid owned fields on a
/// canonical note, reusing the caller's vault-wide moved-target resolver.
/// Decoded YAML values are Markdown: fenced blocks and inline code spans
/// stay excluded, matching the archive repair semantics. Non-canonical
/// notes never acquire intention-field semantics and pass through
/// unchanged. Dependency metadata is deliberately untouched here; YAML
/// values are not dependency fields.
pub(crate) fn rewrite_owned_moved_links<R: ResolveMovedNote>(
    contents: &str,
    path: &Path,
    note_index: &R,
    moved_targets: &MovedBlockTargets,
) -> Result<RepairResult, WriteError> {
    let Some(subject) = fields::canonical_subject(&path.to_string_lossy())
    else {
        return Ok(RepairResult {
            contents: contents.to_owned(),
            link_count: 0,
        });
    };
    rewrite_owned_links(contents, subject, |text| {
        let wiki = crate::native::collect_done::repair_wiki_links(
            text,
            path,
            note_index,
            moved_targets,
            true,
            false,
        );
        let markdown = crate::native::collect_done::repair_markdown_links(
            &wiki.contents,
            path,
            note_index,
            moved_targets,
            true,
            false,
        );
        (markdown.contents, wiki.link_count + markdown.link_count)
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn day() -> NoteSubject {
        NoteSubject::Day(NaiveDate::from_ymd_opt(2026, 11, 19).unwrap())
    }

    fn week() -> NoteSubject {
        NoteSubject::Period(crate::native::periods::calendar::PeriodId::Week {
            year: 2026,
            week: 47,
        })
    }

    fn rewrite_links(text: &str) -> (String, usize) {
        let count = text.matches("[[old#^").count();
        (text.replace("[[old#^", "[[done/old#^"), count)
    }

    #[test]
    fn rewrites_stored_fields_and_preserves_unrelated_yaml_and_body() {
        let original = "\u{feff}---\r\ncustom: [[old#^x]] # unrelated\r\nintentions: [\"[[old#^x|alias]]\"] # keep\r\nsummary: >-\r\n  [[old#^x]]\r\nreflection: |-\r\n  First [[old#^x]]\r\n\r\n  Second paragraph.\r\nsummary_digest: sha256:0000000000000000\r\nsummary_drafted: 2026-11-19\r\n---\r\nBody [[old#^x]]\r\n";
        let result =
            rewrite_owned_links(original, week(), rewrite_links).unwrap();

        assert_eq!(result.link_count, 3);
        assert!(result.contents.starts_with("\u{feff}---\r\n"));
        assert!(result
            .contents
            .contains("custom: [[old#^x]] # unrelated\r\n"));
        assert!(result.contents.contains(
            "intentions: # keep\r\n  - \"[[done/old#^x|alias]]\"\r\n"
        ));
        assert!(result.contents.contains("summary: \"[[done/old#^x]]\"\r\n"));
        assert!(
            result.contents.contains("reflection: |-\r\n  First [[done/old#^x]]\r\n  \r\n  Second paragraph.\r\n"),
            "{:?}",
            result.contents
        );
        assert!(result.contents.ends_with("Body [[old#^x]]\r\n"));
        assert!(result
            .contents
            .contains("summary_digest: sha256:0000000000000000\r\n"));
        assert!(result.contents.contains("summary_drafted: 2026-11-19\r\n"));
    }

    #[test]
    fn supports_scalar_flow_and_folded_values_without_rewriting_legacy_text() {
        let scalar = "---\nintentions: \"[[old#^x]]\"\n---\n";
        let result = rewrite_owned_links(scalar, day(), rewrite_links).unwrap();
        assert_eq!(result.link_count, 1);
        assert!(result
            .contents
            .contains("intentions:\n  - \"[[done/old#^x]]\"\n"));

        let flow = "---\nintentions: [\"[[old#^x]]\", keep]\nsummary: >-\n  [[old#^x]]\n---\n";
        let result = rewrite_owned_links(flow, week(), rewrite_links).unwrap();
        assert_eq!(result.link_count, 2);
        assert!(result.contents.contains("  - \"[[done/old#^x]]\"\n"));
        assert!(result.contents.contains("summary: \"[[done/old#^x]]\"\n"));

        let legacy = "## Intentions\n\n- [[old#^x]]\n";
        assert_eq!(
            rewrite_owned_links(legacy, week(), rewrite_links)
                .unwrap()
                .contents,
            legacy
        );
    }

    #[test]
    fn invalid_intention_items_and_malformed_frontmatter_stay_byte_identical() {
        let mixed = "---\nintentions: [\"[[old#^x]]\", 12]\nsummary: \"[[old#^x]]\"\n---\n";
        let result = rewrite_owned_links(mixed, week(), rewrite_links).unwrap();
        assert_eq!(result.link_count, 1);
        assert!(result
            .contents
            .contains("intentions: [\"[[old#^x]]\", 12]\n"));
        assert!(result.contents.contains("summary: \"[[done/old#^x]]\"\n"));

        for malformed in [
            "---\nintentions: [\n---\nbody\n",
            "---\nintentions: [one]\nintentions: [two]\n---\nbody\n",
        ] {
            let result =
                rewrite_owned_links(malformed, week(), rewrite_links).unwrap();
            assert_eq!(result.contents, malformed);
            assert_eq!(result.link_count, 0);
        }
    }

    #[test]
    fn malformed_frontmatter_does_not_trigger_a_partial_owned_patch() {
        let malformed = "---\nintentions: [\"[[old#^x]]\"\n---\n";
        let result =
            rewrite_owned_links(malformed, day(), rewrite_links).unwrap();
        assert_eq!(result.contents, malformed);
        assert_eq!(result.link_count, 0);
    }
}
