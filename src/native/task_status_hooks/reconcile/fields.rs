use super::super::*;

/// Rewrite a task line's `[id::]` / `[dependsOn::]` fields: every
/// existing `id` and `dependsOn` field is removed wherever it sits in
/// the Tasks suffix (trailing tags included, both `[]` and `()` forms),
/// and the new values are inserted in Tasks key order (`id` before
/// `dependsOn`) right of any `fresh` and before the trailing
/// `^block-id` (`contract` §3). Shared with `bob projects` through
/// the `projects::edits` upsert helpers rather than a bespoke peeler,
/// so a line like `[id:: …] #hide ^prj` never gains a duplicate field.
pub fn set_task_fields(
    line: &str,
    task_id: Option<&str>,
    depends_on: &[String],
) -> String {
    let trimmed = line.trim_end();
    let known_block = trailing_block_id(trimmed);
    let stem = match &known_block {
        Some(block) => trimmed
            .strip_suffix(&format!("^{block}"))
            .map(str::trim_end)
            .unwrap_or(trimmed),
        None => trimmed,
    };
    let cleaned = projects::edits::remove_all_inline_fields(stem, "id");
    let cleaned =
        projects::edits::remove_all_inline_fields(&cleaned, "dependsOn");
    let insertion = projects::edits::task_metadata_insertion_offset(&cleaned);
    let (before, _) = cleaned.split_at(insertion.min(cleaned.len()));
    let mut rebuilt = before.trim_end().to_string();
    if let Some(id) = task_id {
        rebuilt.push_str(&format!(" [id:: {id}]"));
    }
    if !depends_on.is_empty() {
        rebuilt.push_str(&format!(" [dependsOn:: {}]", depends_on.join(", ")));
    }
    if let Some(block) = known_block {
        rebuilt.push_str(&format!(" ^{block}"));
    }
    rebuilt
}

/// Rebuild a Depends-On child line around new link texts, preserving
/// the original indent and list marker.
pub fn rebuild_child_line(original: &str, link_texts: &[String]) -> String {
    let indent_len = original
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let marker_end =
        after_list_marker(original, indent_len).unwrap_or(indent_len);
    format!(
        "{} {} **DEPENDS ON:**{}",
        &original[..marker_end],
        task_dependencies::format::DEPENDS_ON_EMOJI,
        if link_texts.is_empty() {
            String::new()
        } else {
            format!(
                " {}",
                link_texts
                    .join(task_dependencies::format::DEPENDS_ON_SEPARATOR)
            )
        }
    )
}

/// Leading whitespace bytes of a line (the indent prefix to reuse).
pub fn leading_bytes(line: &str) -> &str {
    let len = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    &line[..len]
}

/// Whether a child bullet is the `❌ **CANCEL LOG**` slot: an adopted
/// line goes second when it holds the first slot (`contract` §2.1).
pub fn is_cancel_log_line(line: &str) -> bool {
    let indent_len = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let rest = match after_list_marker(line, indent_len) {
        Some(marker_end) => line[marker_end..].trim_start(),
        None => return false,
    };
    rest.starts_with('❌') && rest.contains("CANCEL LOG")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depends(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn field_writer_places_id_before_depends_on_before_block_id() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it ^ship",
                Some("tasks__ship"),
                &depends(&["tasks__tests"]),
            ),
            "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__tests] ^ship"
        );
    }

    #[test]
    fn field_writer_lands_right_of_fresh_and_replaces_stale() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it [fresh:: 2026-10-02] [dependsOn:: stale] [scheduled:: 2026-10-03] ^ship",
                Some("tasks__ship"),
                &depends(&["tasks__tests"]),
            ),
            "- [ ] #task Ship it [fresh:: 2026-10-02] [scheduled:: 2026-10-03] [id:: tasks__ship] [dependsOn:: tasks__tests] ^ship"
        );
    }

    #[test]
    fn field_writer_removes_depends_on_when_empty() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__tests] ^ship",
                Some("tasks__ship"),
                &[],
            ),
            "- [ ] #task Ship it [id:: tasks__ship] ^ship"
        );
    }

    #[test]
    fn field_writer_drops_both_fields_for_r9() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Bare [id:: tasks__bare] [dependsOn:: tasks__ghost]",
                None,
                &[],
            ),
            "- [ ] #task Bare"
        );
    }

    #[test]
    fn field_writer_replaces_field_before_trailing_tags() {
        // A `[dependsOn::]` buried before trailing tags is replaced in
        // place, never duplicated (`contract` §3).
        assert_eq!(
            set_task_fields(
                "- [ ] #task D [dependsOn:: tasks__old] #hide ^d",
                None,
                &depends(&["tasks__new"]),
            ),
            "- [ ] #task D #hide [dependsOn:: tasks__new] ^d"
        );
    }

    #[test]
    fn field_writer_replaces_fields_between_tags() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task D #hide [dependsOn:: tasks__old] #later ^d",
                Some("tasks__d"),
                &depends(&["tasks__new"]),
            ),
            "- [ ] #task D #hide #later [id:: tasks__d] [dependsOn:: tasks__new] ^d"
        );
    }

    #[test]
    fn field_writer_stamps_target_past_hide_tag() {
        // The `[id:: …] #hide ^prj` vault shape: the stamp lands before
        // the block id without duplicating the field.
        assert_eq!(
            set_task_fields(
                "- [ ] #task Prj [id:: tasks__old] #hide ^prj",
                Some("tasks__prj"),
                &[],
            ),
            "- [ ] #task Prj #hide [id:: tasks__prj] ^prj"
        );
    }

    #[test]
    fn field_writer_replaces_parenthesized_metadata() {
        assert_eq!(
            set_task_fields(
                "- [ ] #task Ship it (dependsOn:: tasks__tests) ^ship",
                None,
                &depends(&["tasks__other"]),
            ),
            "- [ ] #task Ship it [dependsOn:: tasks__other] ^ship"
        );
    }

    #[test]
    fn dr_field_projection_vectors() {
        // Table-driven DR coverage for the reconcile field writer: each
        // row pins how a dependent's line links plus legacy children
        // project into `[dependsOn::]` in set order. Full R1–R10 flow
        // (adopt/heal/canonicalize/warn) is pinned by the hooks' CLI
        // `dependency_lines` suite; this table pins the pure projection.
        for (vector, line, id, depends_on, expected) in [
            (
                "DR1: two resolved targets in line order",
                "- [ ] #task Ship it ^ship",
                Some("tasks__ship"),
                depends(&["tasks__a", "tasks__b"]),
                "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__a, tasks__b] ^ship",
            ),
            (
                "DR2: target gains an id; field set",
                "- [ ] #task Ship it ^ship",
                Some("tasks__ship"),
                depends(&["tasks__tests"]),
                "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__tests] ^ship",
            ),
            (
                "DR3: stale third id dropped",
                "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__a, tasks__b, tasks__stale] ^ship",
                Some("tasks__ship"),
                depends(&["tasks__a", "tasks__b"]),
                "- [ ] #task Ship it [id:: tasks__ship] [dependsOn:: tasks__a, tasks__b] ^ship",
            ),
            (
                "DR16/R9: label-only removal drops the field",
                "- [ ] #task Bare [id:: tasks__bare] [dependsOn:: tasks__ghost]",
                None,
                depends(&[]),
                "- [ ] #task Bare",
            ),
        ] {
            assert_eq!(
                set_task_fields(line, id, &depends_on),
                expected,
                "{vector}"
            );
        }
    }
}
