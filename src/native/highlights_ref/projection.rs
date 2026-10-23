//! Sync projection resolution and diagnostics.
use super::*;

/// Strip `parent` from a marker/frontmatter projection: v2 notes project
/// residence separately, so parent never joins the sync snapshot, the
/// stored base, or the hash (design call 1).
pub(super) fn without_parent(projection: &Projection) -> Projection {
    let mut parent_free = projection.clone();
    parent_free.remove(FIELD_PARENT);
    parent_free
}

/// Migrate an old stored base to the parent-free shape before conflict
/// detection, so a residence move (or the first v2 transition) does not
/// read as a two-sided edit. Status and every other key pass through
/// untouched; refusal on insufficient base evidence is unchanged.
pub(super) fn normalize_v2_base(base: &Projection) -> Projection {
    without_parent(base)
}

/// Frontmatter `parent` rendered from residence, kept out of the sync
/// snapshot: `parent: "[[<route>]]"`, like the command-managed `type`.
pub(super) fn residence_parent_value(route: &str) -> MarkerValue {
    MarkerValue::String(format!("[[{route}]]"))
}

/// Full marker projection with the birth-hint `parent` restored for PDF
/// marker writes. Hashes and bases always use the parent-free shape; only
/// `--write-pdf(s)` refreshes a stale hint through this helper, and normal
/// scans preserve the marker's birth hint untouched.
pub(super) fn marker_projection_with_parent_hint(
    parent_free: &Projection,
    route: &str,
) -> Projection {
    let mut marker = parent_free.clone();
    marker.insert(FIELD_PARENT.to_string(), residence_parent_value(route));
    marker
}

/// Appended to `sync_reason` when a stale editor save is restored.
pub(super) const STALE_SAVE_REASON: &str =
    "marker reverted to the copy bob replaced (stale editor save); restoring it";

/// Dim concise-scan detail when a stale editor save is restored.
pub(super) const STALE_SAVE_SCAN_DETAIL: &str =
    "marker restored after a stale save";

/// True when the PDF marker matches the copy bob's latest marker write
/// replaced (`M == R && M != L`) and the note still has a stored base.
pub(super) fn is_stale_editor_save(
    replaced_hash: Option<&str>,
    last_hash: Option<&str>,
    has_base: bool,
    pdf_marker_hash: &str,
) -> bool {
    match (replaced_hash, last_hash) {
        (Some(replaced), Some(last)) => {
            has_base && pdf_marker_hash == replaced && pdf_marker_hash != last
        }
        _ => false,
    }
}

pub(super) fn append_stale_save_reason(reason: &mut String) {
    if !reason.is_empty() {
        reason.push_str("; ");
    }
    reason.push_str(STALE_SAVE_REASON);
}

/// Restore-alone writes are deferred to the next `-w` run instead of
/// raising `needs_write_pdfs`. Dry runs still preview them.
pub(super) fn skip_stale_restore_without_write_opt_in(
    stale_restore: bool,
    frontmatter_contributed: bool,
    marker_normalized: bool,
    options: SyncOptions,
) -> bool {
    stale_restore
        && !frontmatter_contributed
        && !marker_normalized
        && !options.write_pdf
        && !options.dry_run
}

/// Value of `highlights_marker_replaced_hash` for this note write.
///
/// A marker write stores the pre-write PDF marker hash. A note write that
/// observes agreement (`M == L`) drops the field. Any other note write
/// keeps the existing value.
pub(super) fn replaced_hash_to_store(
    existing: Option<&str>,
    pdf_marker_hash: &str,
    last_hash: Option<&str>,
    marker_write_needed: bool,
) -> Option<String> {
    if marker_write_needed {
        Some(pdf_marker_hash.to_string())
    } else if last_hash == Some(pdf_marker_hash) {
        None
    } else {
        existing.map(str::to_string)
    }
}

pub(super) fn is_dropping_replaced_hash(
    last_hash: Option<&str>,
    pdf_marker_hash: &str,
    marker_write_needed: bool,
    existing: Option<&str>,
) -> bool {
    !marker_write_needed
        && last_hash == Some(pdf_marker_hash)
        && existing.is_some()
}

/// Never write a note whose only change is dropping the replaced hash.
/// Returns the kept original contents when that would have been the
/// only edit, plus the existing hash to keep storing.
pub(super) fn finalize_replaced_hash_render(
    note: &ParsedNote,
    rendered: String,
    replaced_hash: Option<String>,
    dropping: bool,
) -> (String, Option<String>) {
    if dropping
        && let Some(current) = note.contents()
        && same_except_replaced_hash_line(&current, &rendered)
    {
        return (current.to_string(), note.marker_replaced_hash());
    }
    (rendered, replaced_hash)
}

fn same_except_replaced_hash_line(left: &str, right: &str) -> bool {
    strip_replaced_hash_lines(left) == strip_replaced_hash_lines(right)
}

fn strip_replaced_hash_lines(text: &str) -> String {
    let trailing = text.ends_with('\n');
    let mut out = String::new();
    let prefix = format!("{FIELD_MARKER_REPLACED_HASH}:");
    for line in text.lines() {
        if line.starts_with(&prefix) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !trailing && out.ends_with('\n') {
        out.pop();
    }
    out
}

pub(super) fn resolve_sync_projection(
    inputs: SyncInputs<'_>,
) -> Result<SyncResolution> {
    let SyncInputs {
        last_hash,
        base_projection,
        marker_projection,
        marker_hash,
        frontmatter_projection,
        frontmatter_hash,
        note_exists,
        prefer,
    } = inputs;

    let Some(last_hash) = last_hash else {
        return Ok(match prefer {
            Some(Prefer::Frontmatter) if note_exists => sync_resolution(
                SyncSource::Frontmatter,
                "initial sync; --prefer frontmatter supplied",
                frontmatter_projection.clone(),
                false,
                true,
            ),
            _ => sync_resolution(
                SyncSource::Marker,
                "initial sync",
                marker_projection.clone(),
                true,
                false,
            ),
        });
    };

    let marker_changed = marker_hash != last_hash;
    let frontmatter_changed = frontmatter_hash != last_hash;

    match (marker_changed, frontmatter_changed) {
        (false, false) => Ok(sync_resolution(
            SyncSource::Marker,
            "marker and frontmatter match the stored hash",
            marker_projection.clone(),
            false,
            false,
        )),
        (true, false) => Ok(sync_resolution(
            SyncSource::Marker,
            "marker changed since last sync",
            marker_projection.clone(),
            true,
            false,
        )),
        (false, true) => Ok(sync_resolution(
            SyncSource::Frontmatter,
            "frontmatter changed since last sync",
            frontmatter_projection.clone(),
            false,
            true,
        )),
        (true, true) if marker_hash == frontmatter_hash => Ok(sync_resolution(
            SyncSource::Marker,
            "marker and frontmatter changed to the same projection",
            marker_projection.clone(),
            true,
            true,
        )),
        (true, true) => match prefer {
            Some(Prefer::Marker) => Ok(sync_resolution(
                SyncSource::Marker,
                "conflict overridden with --prefer marker",
                marker_projection.clone(),
                true,
                false,
            )),
            Some(Prefer::Frontmatter) => Ok(sync_resolution(
                SyncSource::Frontmatter,
                "conflict overridden with --prefer frontmatter",
                frontmatter_projection.clone(),
                false,
                true,
            )),
            None => {
                let Some(base_projection) = base_projection else {
                    return Err(CommandError::new(format!(
                        "marker/frontmatter conflict: marker hash {marker_hash}, frontmatter hash {frontmatter_hash}, stored hash {last_hash}; rerun with --prefer marker or --prefer frontmatter after reviewing both sides"
                    )));
                };
                let merge = merge_projection_changes(
                    base_projection,
                    marker_projection,
                    frontmatter_projection,
                );
                if !merge.conflicts.is_empty() {
                    return Err(marker_frontmatter_conflict_error(
                        &merge.conflicts,
                    ));
                }
                Ok(sync_resolution(
                    SyncSource::AutoMerge,
                    "marker and frontmatter changed compatible fields; auto-merged",
                    merge.projection,
                    merge.marker_contributed,
                    merge.frontmatter_contributed,
                ))
            }
        },
    }
}

pub(super) fn sync_resolution(
    source: SyncSource,
    reason: impl Into<String>,
    projection: Projection,
    marker_contributed: bool,
    frontmatter_contributed: bool,
) -> SyncResolution {
    SyncResolution {
        decision: SyncDecision {
            source,
            reason: reason.into(),
            marker_contributed,
            frontmatter_contributed,
        },
        projection,
    }
}

#[derive(Debug, Clone)]
pub(super) struct ProjectionMerge {
    pub(super) projection: Projection,
    pub(super) conflicts: Vec<ProjectionConflict>,
    pub(super) marker_contributed: bool,
    pub(super) frontmatter_contributed: bool,
}

pub(super) fn merge_projection_changes(
    base: &Projection,
    marker: &Projection,
    frontmatter: &Projection,
) -> ProjectionMerge {
    let mut keys = BTreeSet::new();
    keys.extend(base.keys().cloned());
    keys.extend(marker.keys().cloned());
    keys.extend(frontmatter.keys().cloned());

    let mut projection = Projection::new();
    let mut conflicts = Vec::new();
    let mut marker_contributed = false;
    let mut frontmatter_contributed = false;

    for key in keys {
        let base_value = base.get(&key);
        let marker_value = marker.get(&key);
        let frontmatter_value = frontmatter.get(&key);
        let marker_changed = marker_value != base_value;
        let frontmatter_changed = frontmatter_value != base_value;

        marker_contributed |= marker_changed;
        frontmatter_contributed |= frontmatter_changed;

        let selected = match (marker_changed, frontmatter_changed) {
            (false, false) => base_value,
            (true, false) => marker_value,
            (false, true) => frontmatter_value,
            (true, true) if marker_value == frontmatter_value => marker_value,
            (true, true) => {
                conflicts.push(ProjectionConflict {
                    key,
                    base: base_value.cloned(),
                    marker: marker_value.cloned(),
                    frontmatter: frontmatter_value.cloned(),
                });
                continue;
            }
        };

        if let Some(value) = selected {
            projection.insert(key, value.clone());
        }
    }

    ProjectionMerge {
        projection,
        conflicts,
        marker_contributed,
        frontmatter_contributed,
    }
}

pub(super) fn marker_frontmatter_conflict_error(
    conflicts: &[ProjectionConflict],
) -> CommandError {
    let mut message = String::from("marker/frontmatter conflict:");
    for conflict in conflicts.iter().take(8) {
        message.push_str(&format!(
            "\n  {}: marker={}, frontmatter={}, base={}",
            conflict.key,
            diagnostic_projection_value(conflict.marker.as_ref()),
            diagnostic_projection_value(conflict.frontmatter.as_ref()),
            diagnostic_projection_value(conflict.base.as_ref()),
        ));
    }
    if conflicts.len() > 8 {
        message.push_str(&format!(
            "\n  ... {} more conflict(s)",
            conflicts.len() - 8
        ));
    }
    message.push_str(
        "\nrerun with --prefer marker or --prefer frontmatter after reviewing both sides",
    );
    CommandError::new(message)
}

pub(super) fn diagnostic_projection_value(
    value: Option<&MarkerValue>,
) -> String {
    let rendered = match value {
        Some(MarkerValue::String(value)) => quote_string(value),
        Some(value) => value.as_marker_value(),
        None => "<deleted>".to_string(),
    };
    truncate_diagnostic_value(&rendered, 80)
}

pub(super) fn truncate_diagnostic_value(
    value: &str,
    max_chars: usize,
) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
