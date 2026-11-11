//! Strict byte-span handling for the Bob-owned `## Notebooks` region.
//!
//! The owned-callout engine lives in [`crate::native::host_catalog`]. This
//! module is the notebooks spec: `[!notebooks]`, `## Notebooks`, first write
//! before `## Tasks`, and v1 marker migration. Bob owns exactly the
//! `> [!notebooks]` callout block; the position of the block, its heading,
//! and every other byte belong to the note's author.

use crate::native::host_catalog::{self, NOTEBOOKS};

#[allow(unused_imports)]
pub(crate) use crate::native::host_catalog::{
    EditState, HostOutcome, RegionDamage, LEGACY_BEGIN_MARKER,
    LEGACY_END_MARKER,
};

/// Plan the edit that makes the host's owned notebooks callout match
/// `callout`; an empty `callout` means the host should hold no region.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_host(text: &str, callout: &[String]) -> HostOutcome {
    host_catalog::plan_host(&NOTEBOOKS, text, callout)
}

#[cfg(test)]
fn full_block(callout: &[String], eol: &str) -> String {
    format!(
        "## Notebooks{eol}{eol}{}",
        host_catalog::bare_text(callout, eol)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callout(rows: &[&str]) -> Vec<String> {
        let mut lines = vec!["> [!notebooks]".to_string(), ">".to_string()];
        lines.extend(rows.iter().map(|row| format!("> {row}")));
        lines
    }

    fn block(rows: &[&str]) -> String {
        full_block(&callout(rows), "\n")
    }

    fn update(outcome: HostOutcome) -> (EditState, String) {
        match outcome {
            HostOutcome::Update { state, after } => (state, after),
            other => panic!("expected update, got {other:?}"),
        }
    }

    #[test]
    fn inserts_before_first_tasks_heading_with_blank_line() {
        let text =
            "---\ntype: \"[[area]]\"\n---\n\n# Dev\n\n## Tasks\n\n- [ ] x\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (state, after) = update(plan_host(text, &rows));
        assert_eq!(state, EditState::Missing);
        assert_eq!(
            after,
            format!(
                "---\ntype: \"[[area]]\"\n---\n\n# Dev\n\n{}\n## Tasks\n\n- [ ] x\n",
                block(&["- **[[a|A]]**"])
            )
        );
    }

    #[test]
    fn inserts_at_eof_with_one_blank_line() {
        let text = "---\ntype: \"[[area]]\"\n---\n\n# Dev\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert_eq!(after, format!("{text}\n{}", block(&["- **[[a|A]]**"])));
    }

    #[test]
    fn eof_without_final_newline_gets_separator() {
        let text = "---\ntype: \"[[area]]\"\n---\n# Dev";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert_eq!(after, format!("{text}\n\n{}", block(&["- **[[a|A]]**"])));
    }

    #[test]
    fn tasks_heading_inside_fence_or_frontmatter_is_ignored() {
        let text = "---\nnote: |\n  ## Tasks\n---\n\n```md\n## Tasks\n```\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert!(after.starts_with(text));
        assert!(after.ends_with(&block(&["- **[[a|A]]**"])));
    }

    #[test]
    fn in_place_rewrite_keeps_moved_block_and_surroundings() {
        let old = block(&["- **[[a|Old]]**"]);
        let text =
            format!("---\n---\n\n{old}\nprose after\n\n## Tasks\n\n- x\n");
        let rows = callout(&["- **[[a|New]]**"]);
        let (state, after) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(
            after,
            format!(
                "---\n---\n\n{}\nprose after\n\n## Tasks\n\n- x\n",
                block(&["- **[[a|New]]**"])
            )
        );
    }

    #[test]
    fn identical_block_is_current_wherever_it_sits() {
        let rows = callout(&["- **[[a|A]]**"]);
        let text =
            format!("---\n---\n\ntop\n\n{}\ntail\n", block(&["- **[[a|A]]**"]));
        assert_eq!(plan_host(&text, &rows), HostOutcome::Current);
    }

    #[test]
    fn deletion_removes_empty_section_and_heading() {
        let text = format!(
            "---\n---\n\n# Dev\n\n{}\n## Tasks\n\n- x\n",
            block(&["- **[[a|A]]**"])
        );
        let (state, after) = update(plan_host(&text, &[]));
        assert_eq!(state, EditState::Stale);
        assert_eq!(after, "---\n---\n\n# Dev\n\n## Tasks\n\n- x\n");
    }

    #[test]
    fn deletion_at_eof_restores_original_bytes() {
        let original = "---\n---\n\n# Dev\n";
        let with = format!("{original}\n{}", block(&["- **[[a|A]]**"]));
        let (_, after) = update(plan_host(&with, &[]));
        assert_eq!(after, original);
    }

    #[test]
    fn deletion_keeps_authored_prose_and_heading() {
        let text = format!(
            "---\n---\n\n## Notebooks\n\nMy own words.\n\n{}\n## Tasks\n",
            full_block(&callout(&["- **[[a|A]]**"]), "\n")
                .trim_start_matches("## Notebooks\n\n")
        );
        let (_, after) = update(plan_host(&text, &[]));
        assert_eq!(
            after,
            "---\n---\n\n## Notebooks\n\nMy own words.\n\n## Tasks\n"
        );
    }

    #[test]
    fn nothing_when_no_rows_and_no_region() {
        assert_eq!(plan_host("---\n---\n\n# Dev\n", &[]), HostOutcome::Nothing);
    }

    #[test]
    fn damaged_markers_refuse_with_line() {
        let rows = callout(&["- **[[a|A]]**"]);
        for (text, line) in [
            (format!("---\n---\n{LEGACY_BEGIN_MARKER}\nbody\n"), 3),
            (format!("---\n---\nbody\n{LEGACY_END_MARKER}\n"), 4),
            (format!("---\n---\n{LEGACY_END_MARKER}\n{LEGACY_BEGIN_MARKER}\n"), 3),
            (
                format!(
                    "---\n---\n{LEGACY_BEGIN_MARKER}\n{LEGACY_END_MARKER}\n{LEGACY_BEGIN_MARKER}\n{LEGACY_END_MARKER}\n"
                ),
                3,
            ),
            ("---\n---\n<!-- bob:notebooks:begin v2 -->\n".to_string(), 3),
        ] {
            match plan_host(&text, &rows) {
                HostOutcome::Damaged(damage) => assert_eq!(damage.line, line, "{text}"),
                other => panic!("expected damage for {text:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn fenced_marker_examples_are_inert() {
        let text = format!("---\n---\n\n```md\n{LEGACY_BEGIN_MARKER}\n```\n");
        let rows = callout(&["- **[[a|A]]**"]);
        let (state, _) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Missing);
    }

    #[test]
    fn crlf_and_bom_survive_every_edit() {
        let text =
            "\u{feff}---\r\ntype: \"[[area]]\"\r\n---\r\n\r\n# Dev\r\n\r\n## Tasks\r\n";
        let rows = callout(&["- **[[a|A]]**"]);
        let (_, after) = update(plan_host(text, &rows));
        assert!(after.starts_with("\u{feff}---\r\n"));
        assert!(!after.replace("\r\n", "").contains('\n'));
        // Second pass is current, then deletion round-trips the bytes.
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
        let (_, back) = update(plan_host(&after, &[]));
        assert_eq!(back, text);
    }

    #[test]
    fn legacy_pair_migrates_to_exact_bare_callout() {
        let rows = callout(&["- **[[a|A]]**"]);
        let legacy = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{LEGACY_BEGIN_MARKER}\n\n{}\n\n{LEGACY_END_MARKER}\n",
            rows.join("\n")
        );
        let (state, after) = update(plan_host(&legacy, &rows));
        assert_eq!(state, EditState::Stale);
        let expected = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{}\n",
            rows.join("\n")
        );
        assert_eq!(after, expected);
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
        assert!(!after.contains("bob:notebooks:"));
    }

    #[test]
    fn legacy_plus_bare_duplicate_heals_to_first() {
        let rows = callout(&["- **[[a|A]]**"]);
        let bare = rows.join("\n");
        let text = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{bare}\n\n## Notebooks\n\n{LEGACY_BEGIN_MARKER}\n\n{bare}\n\n{LEGACY_END_MARKER}\n\n## Tasks\n"
        );
        let (state, after) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(after.matches("> [!notebooks]").count(), 1);
        assert!(!after.contains("bob:notebooks:"));
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn two_bare_duplicates_heal_to_first() {
        let rows = callout(&["- **[[a|A]]**"]);
        let bare = rows.join("\n");
        let text = format!(
            "---\n---\n\n# Dev\n\n## Notebooks\n\n{bare}\n\n## Notebooks\n\n{bare}\n\n## Tasks\n"
        );
        let (state, after) = update(plan_host(&text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(after.matches("> [!notebooks]").count(), 1);
        // Duplicate heading whose section went blank is dropped.
        assert_eq!(after.matches("## Notebooks").count(), 1);
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn folded_head_canonicalizes() {
        let rows = callout(&["- **[[a|A]]**"]);
        let folded = "---\n---\n\n# Dev\n\n## Notebooks\n\n> [!notebooks]-\n>\n> - **[[a|A]]**\n".to_string();
        let (state, after) = update(plan_host(&folded, &rows));
        assert_eq!(state, EditState::Stale);
        assert!(after.contains("> [!notebooks]\n"));
        assert!(!after.contains("> [!notebooks]-"));
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn callout_in_fence_or_frontmatter_stays_inert() {
        let rows = callout(&["- **[[a|A]]**"]);
        let fenced = "---\n---\n\n```md\n> [!notebooks]\n> - x\n```\n";
        let (state, after) = update(plan_host(fenced, &rows));
        assert_eq!(state, EditState::Missing);
        assert!(after.starts_with(fenced));
        let front = "---\n> [!notebooks]\n> - x\n---\n\n# Dev\n";
        let (state, _) = update(plan_host(front, &rows));
        assert_eq!(state, EditState::Missing);
    }

    #[test]
    fn quote_after_blank_does_not_join_region() {
        let rows = callout(&["- **[[a|A]]**"]);
        // A `>` line after a blank line is a second owned region, so the
        // first (partial) plus the orphan heal to one current block.
        let text =
            "---\n---\n\n## Notebooks\n\n> [!notebooks]\n>\n\n> - orphan\n";
        let (state, after) = update(plan_host(text, &rows));
        assert_eq!(state, EditState::Stale);
        assert_eq!(plan_host(&after, &rows), HostOutcome::Current);
    }

    #[test]
    fn bare_crlf_bom_round_trip() {
        let rows = callout(&["- **[[a|A]]**"]);
        let text = "\u{feff}---\r\n---\r\n\r\n# Dev\r\n\r\n## Notebooks\r\n\r\n> [!notebooks]\r\n>\r\n> - **[[a|A]]**\r\n";
        assert_eq!(plan_host(text, &rows), HostOutcome::Current);
        let (_, back) = update(plan_host(text, &[]));
        assert!(!back.contains("> [!notebooks]"));
        assert!(back.starts_with("\u{feff}---\r\n"));
    }
}
