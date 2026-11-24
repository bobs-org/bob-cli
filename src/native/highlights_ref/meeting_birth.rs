//! Meeting birth: marker-carried identity births the LINKS relation and recap.
//!
//! A PDF whose `ref_type` is `meetings` and whose marker carries `event`
//! births its reading task with one inert `LINKS` child and its note with
//! the managed recap region, byte-identical whether ingested inline or
//! later drained by `bob ref scan -i`. The relation label comes from
//! `meetings.relation_label` (default `notes from`); an invalid label or
//! one carrying a close/successor effect is a config error, never a silent
//! skip. Adopting an orphan task never adds a second LINKS line; later
//! syncs, scans, renames, and status changes preserve the recap and LINKS
//! byte for byte because this module only writes at birth.

use std::path::{Path, PathBuf};

use super::{
    model::{CommandError, Config, MarkerValue, Projection, Result},
    source_store,
};
use crate::native::{
    config as bob_config,
    meetings::{
        meeting_links_child, parse_dossier, parse_event_link,
        render_birth_recap, render_empty_recap, RecapMeta,
    },
};

/// Planned meeting birth for one PDF, carried outside the reading-task plan
/// so inline ingest and the queued drain share the same bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MeetingBirth {
    pub relation_label: String,
    /// Canonical event link for the LINKS child, when the event resolved.
    pub event_link: Option<String>,
    /// Full owned recap region, between the embed and `## Highlights`.
    pub recap: String,
    /// Meeting warnings, in birth order (after any parent fallback warning
    /// and the LINKS line).
    pub warnings: Vec<String>,
}

/// Whether this PDF is a meetings birth: `ref_type == meetings` with a
/// non-empty marker `event`.
pub(super) fn is_meetings_birth(
    ref_type: Option<&str>,
    marker: &Projection,
) -> bool {
    ref_type == Some("meetings")
        && marker
            .get("event")
            .and_then(MarkerValue::as_string)
            .is_some_and(|event| !event.trim().is_empty())
}

/// Load and validate the meetings relation label.
///
/// An invalid label or one carrying a cancel/complete/merge/successor
/// effect is a config error naming the key and path.
pub(super) fn meeting_relation_label() -> Result<String> {
    let path = bob_config::config_path();
    let meetings = bob_config::load_meetings_config(&path)
        .map_err(super::hooks::config_error)?;
    let raw = meetings.relation_label().to_string();
    let normalized =
        crate::native::task_relations::label::normalize_relation_label(&raw)
            .map_err(|error| {
                CommandError::new(format!(
                    "invalid meetings.relation_label in {}: {}; fix or remove it",
                    path.display(),
                    error.as_str()
                ))
            })?;
    let relations = bob_config::load_relations_config(&path)
        .map_err(super::hooks::config_error)?;
    if let Some(effect) = relations.label_effect(
        &crate::native::relation_labels::relation_label_key(&normalized),
    ) {
        let effect_name = match effect {
            bob_config::LabelEffect::Cancel => "cancel",
            bob_config::LabelEffect::Complete => "complete",
            bob_config::LabelEffect::Merge => "merge",
            bob_config::LabelEffect::Successor => "successor",
        };
        return Err(CommandError::new(format!(
            "invalid meetings.relation_label in {}: '{normalized}' carries a {effect_name} effect; choose a label with no close or successor effect",
            path.display()
        )));
    }
    Ok(normalized)
}

/// Plan a meetings birth: validate the label, resolve the event, and render
/// the recap from the birth store. Returns `None` when this PDF is not a
/// meetings birth. Errors only on invalid config; missing events or stores
/// birth with warnings instead of failing.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_meeting_birth(
    config: &Config,
    _pdf: &Path,
    marker: &Projection,
    pdf_sha256: &str,
    stem: &str,
    ref_type: Option<&str>,
    destination_route: Option<&str>,
) -> Result<Option<MeetingBirth>> {
    if !is_meetings_birth(ref_type, marker) {
        return Ok(None);
    }
    let relation_label = meeting_relation_label()?;
    let event_raw = marker
        .get("event")
        .and_then(MarkerValue::as_string)
        .map(str::trim)
        .filter(|event| !event.is_empty())
        .unwrap_or("")
        .to_string();
    let producer = marker
        .get("notes_producer")
        .and_then(MarkerValue::as_string)
        .map(str::trim)
        .filter(|producer| !producer.is_empty())
        .map(str::to_string);

    let mut warnings = Vec::new();
    let event_link = resolve_meeting_event(
        config,
        &event_raw,
        destination_route,
        &mut warnings,
    );

    let recap = render_meeting_recap(
        config,
        marker,
        &event_raw,
        producer.as_deref(),
        pdf_sha256,
        stem,
        ref_type,
        &mut warnings,
    );

    Ok(Some(MeetingBirth {
        relation_label,
        event_link,
        recap,
        warnings,
    }))
}

/// Resolve the marker `event` wikilink to a canonical link for the LINKS
/// child. Pushes a warning and returns `None` when the event no longer
/// resolves, so birth still succeeds with no dangling link.
fn resolve_meeting_event(
    config: &Config,
    event_raw: &str,
    destination_route: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let Some((note_str, block_id)) = parse_event_link(event_raw) else {
        warnings.push(format!(
            "⚠️ meeting event '{event_raw}' is not a valid event link; filed without a LINKS relation"
        ));
        return None;
    };
    let index = crate::native::meetings::vault_index(&config.bob_dir);
    let resolved = index.resolve_detailed(None, &note_str);
    let Some(target_rel) = resolved.path().map(Path::to_path_buf) else {
        warnings.push(format!(
            "⚠️ meeting event '{event_raw}' was not found in the vault; filed without a LINKS relation"
        ));
        return None;
    };
    let target_abs = config.bob_dir.join(&target_rel);
    let contains_block = std::fs::read_to_string(&target_abs)
        .map(|contents| contents.contains(&format!("^{block_id}")))
        .unwrap_or(false);
    if !contains_block {
        warnings.push(format!(
            "⚠️ meeting event '{event_raw}' no longer resolves; filed without a LINKS relation"
        ));
        return None;
    }
    let dest_rel = destination_route
        .map(|route| PathBuf::from(format!("{route}.md")))
        .unwrap_or_else(|| PathBuf::from("mac_inbox.md"));
    Some(crate::native::task_dependencies::format::canonical_link(
        &target_rel,
        &block_id,
        &dest_rel,
        &index,
    ))
}

/// Render the birth recap from the write-once store's dossier source.
///
/// Parses the stored dossier with the round-trip parser and renders the
/// full recap (recap_scope = full). When the store is missing or
/// unreadable, births an empty owned region and records a warning.
#[allow(clippy::too_many_arguments)]
fn render_meeting_recap(
    config: &Config,
    marker: &Projection,
    event_raw: &str,
    producer: Option<&str>,
    pdf_sha256: &str,
    stem: &str,
    ref_type: Option<&str>,
    warnings: &mut Vec<String>,
) -> String {
    let ref_type = ref_type.unwrap_or("meetings");
    let pointer = source_store::find_birth_store(
        &config.bob_dir,
        ref_type,
        stem,
        pdf_sha256,
    );
    let Some(pointer) = pointer else {
        warnings.push(
            "⚠️ meeting notes store is missing; filed an empty recap"
                .to_string(),
        );
        return render_empty_recap(&marker_recap_meta(
            marker, event_raw, producer,
        ));
    };
    let stored = source_store::read_store(&config.bob_dir, &pointer);
    let Some(bytes) = stored.bytes else {
        warnings.push(format!(
            "⚠️ meeting notes store is unreadable ({}); filed an empty recap",
            stored.detail.unwrap_or_else(|| "unknown error".to_string())
        ));
        return render_empty_recap(&marker_recap_meta(
            marker, event_raw, producer,
        ));
    };
    let text = String::from_utf8_lossy(&bytes);
    match parse_dossier(&text) {
        Ok(document) => {
            let meta = RecapMeta {
                date: document.meta.date.clone(),
                span: document.meta.span.clone(),
                event_link: event_raw.to_string(),
                producer: document
                    .meta
                    .producer
                    .clone()
                    .or_else(|| producer.map(str::to_string)),
            };
            render_birth_recap(&document.notes, &meta)
        }
        Err(error) => {
            warnings.push(format!(
                "⚠️ meeting dossier source is unreadable ({error}); filed an empty recap"
            ));
            render_empty_recap(&marker_recap_meta(marker, event_raw, producer))
        }
    }
}

/// Recap meta from marker fields when the birth store is missing.
///
/// Prefers `meeting_start`/`meeting_end` for the date and span, falling
/// back to empty strings that still digest deterministically.
fn marker_recap_meta(
    marker: &Projection,
    event_raw: &str,
    producer: Option<&str>,
) -> RecapMeta {
    let start = marker
        .get("meeting_start")
        .and_then(MarkerValue::as_string)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    let end = marker
        .get("meeting_end")
        .and_then(MarkerValue::as_string)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    let (date, span) = meeting_date_span(start, end);
    RecapMeta {
        date,
        span,
        event_link: event_raw.to_string(),
        producer: producer.map(str::to_string),
    }
}

/// Split `meeting_start`/`meeting_end` (`2026-11-23T15:00`) into the recap
/// date (`2026-11-23`) and span (`15:00–15:30`, en dash). Unparseable
/// inputs fall back to the raw values or empty strings.
fn meeting_date_span(start: &str, end: &str) -> (String, String) {
    let (start_date, start_time) = start.split_once('T').unwrap_or((start, ""));
    let (end_date, end_time) = end.split_once('T').unwrap_or((end, ""));
    if start.is_empty() {
        return (String::new(), String::new());
    }
    if end.is_empty() || start_time.is_empty() || end_time.is_empty() {
        return (start_date.to_string(), start_time.to_string());
    }
    let span = if start_date == end_date {
        format!("{start_time}–{end_time}")
    } else {
        format!("{start}–{end}")
    };
    (start_date.to_string(), span)
}

/// Render a v2 birth body carrying the meeting recap between the embed and
/// the Highlights region.
pub(super) fn meeting_birth_body(
    title: &str,
    embed_target: &str,
    block_id: &str,
    recap: &str,
    highlights: &str,
    audio: Option<&str>,
) -> String {
    let mut body = String::from("\n# ");
    body.push_str(title);
    body.push_str("\n\n");
    body.push_str(&crate::native::ref_tasks::managed_embed_line(
        embed_target,
        block_id,
    ));
    body.push_str("\n\n");
    body.push_str(recap);
    body.push_str("\n\n");
    if let Some(audio) = audio {
        body.push_str(&super::audio::audio_embed_line(audio));
        body.push_str("\n\n");
    }
    body.push_str("## Highlights\n\n");
    body.push_str(super::MANAGED_BODY_BEGIN);
    body.push_str("\n\n");
    body.push_str(highlights);
    if !highlights.is_empty() && !highlights.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(super::MANAGED_BODY_END);
    body.push('\n');
    body
}

/// Ordered extra child lines for a meetings birth insert, after any parent
/// fallback warning the plan already carries: the LINKS line (when the
/// event resolved), then meeting warnings.
pub(super) fn meeting_birth_children(
    relation_label: &str,
    event_link: Option<&str>,
    meeting_warnings: &[String],
) -> Vec<String> {
    let mut children = Vec::new();
    if let Some(link) = event_link {
        children.push(meeting_links_child(relation_label, link));
    }
    for warning in meeting_warnings {
        children.push(format!("  - {warning}"));
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env::TestEnvGuard;
    use std::ffi::OsStr;

    fn projection(entries: Vec<(&str, &str)>) -> Projection {
        entries
            .into_iter()
            .map(|(key, value)| {
                (key.to_string(), MarkerValue::String(value.to_string()))
            })
            .collect()
    }

    #[test]
    fn detects_meetings_birth_only_with_event() {
        let marker = projection(vec![
            ("status", "ready"),
            ("parent", "[[job]]"),
            ("event", "[[2026/20261123#^acme-qbr]]"),
        ]);
        assert!(is_meetings_birth(Some("meetings"), &marker));
        assert!(!is_meetings_birth(Some("papers"), &marker));
        assert!(!is_meetings_birth(
            Some("meetings"),
            &projection(vec![("status", "ready"), ("parent", "[[job]]")])
        ));
    }

    #[test]
    fn birth_children_order_links_then_warnings() {
        let children = meeting_birth_children(
            "notes from",
            Some("[[2026/20261123#^acme-qbr]]"),
            &["⚠️ meeting notes store is missing; filed an empty recap"
                .to_string()],
        );
        assert_eq!(children.len(), 2);
        assert_eq!(
            children[0],
            "  - 🪢 **LINKS:** notes from [[2026/20261123#^acme-qbr]]"
        );
        assert!(children[1].contains("store is missing"));
    }

    #[test]
    fn birth_body_carries_recap_between_embed_and_highlights() {
        let recap = render_empty_recap(&RecapMeta {
            date: "2026-11-23".to_string(),
            span: "15:00–15:30".to_string(),
            event_link: "[[2026/20261123#^acme-qbr]]".to_string(),
            producer: Some("VoiceQuill".to_string()),
        });
        let body = meeting_birth_body(
            "Acme QBR",
            "job",
            "ref-acme-qbr",
            &recap,
            "",
            None,
        );
        let embed = body.find("![[job#^ref-acme-qbr]]").expect("embed");
        let recap_pos =
            body.find("<!-- bob:meeting-recap:begin").expect("recap");
        let highlights = body.find("## Highlights").expect("highlights");
        assert!(embed < recap_pos && recap_pos < highlights);
        assert_eq!(
            crate::native::meetings::recap_state(&body),
            crate::native::meetings::RecapState::Owned
        );
    }

    #[test]
    fn missing_event_births_warning_without_links() {
        let temp = tempfile::tempdir().expect("temp");
        let bob_dir = temp.path();
        std::fs::create_dir_all(bob_dir.join("lib/meetings")).expect("mkdir");
        let config = Config {
            bob_dir: bob_dir.to_path_buf(),
            lib_dir: bob_dir.join("lib"),
            ref_dir: bob_dir.join("ref"),
            xlib_dir: bob_dir.join("xlib"),
        };
        let marker = projection(vec![
            ("status", "ready"),
            ("parent", "[[job]]"),
            ("event", "[[2026/20261123#^missing]]"),
        ]);
        let _guard = TestEnvGuard::set(&[(
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/definitely/missing/config.yml")),
        )]);
        let birth = plan_meeting_birth(
            &config,
            &bob_dir.join("lib/meetings/stem.pdf"),
            &marker,
            "abc",
            "stem",
            Some("meetings"),
            Some("job"),
        )
        .expect("missing event still births");
        let birth = birth.expect("is meetings birth");
        assert!(birth.event_link.is_none());
        assert!(birth
            .warnings
            .iter()
            .any(|warning| warning.contains("was not found")
                || warning.contains("no longer resolves")));
        assert!(meeting_birth_children(
            &birth.relation_label,
            birth.event_link.as_deref(),
            &birth.warnings
        )
        .iter()
        .all(|child| !child.contains("🪢 **LINKS:**")));
    }

    #[test]
    fn rejected_effect_label_is_config_error() {
        let temp = tempfile::tempdir().expect("temp");
        let config_path = temp.path().join("config.yml");
        std::fs::write(
            &config_path,
            "meetings:\n  relation_label: 'superseded by'\nrelations:\n  cancel_labels: ['superseded by']\n",
        )
        .expect("write config");
        let _guard = TestEnvGuard::set(&[(
            "BOB_CONFIG_FILE",
            Some(config_path.as_os_str()),
        )]);
        let error =
            meeting_relation_label().expect_err("effect label must fail");
        assert!(
            error.message.contains("carries a cancel effect"),
            "{}",
            error.message
        );
    }

    #[test]
    fn non_meetings_pdf_plans_nothing() {
        let temp = tempfile::tempdir().expect("temp");
        let bob_dir = temp.path();
        let config = Config {
            bob_dir: bob_dir.to_path_buf(),
            lib_dir: bob_dir.join("lib"),
            ref_dir: bob_dir.join("ref"),
            xlib_dir: bob_dir.join("xlib"),
        };
        let marker =
            projection(vec![("status", "ready"), ("parent", "[[job]]")]);
        let _guard = TestEnvGuard::set(&[(
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/definitely/missing/config.yml")),
        )]);
        let birth = plan_meeting_birth(
            &config,
            &bob_dir.join("lib/papers/stem.pdf"),
            &marker,
            "abc",
            "stem",
            Some("papers"),
            Some("job"),
        )
        .expect("non-meetings plans nothing");
        assert!(birth.is_none());
    }
}
