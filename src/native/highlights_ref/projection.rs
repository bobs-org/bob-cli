//! Sync projection resolution and diagnostics.
use super::*;

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
