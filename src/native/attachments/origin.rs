//! Attachment origin resolution (`docs/attachments.md`).
//!
//! `resolve_origin` answers where a vault copy came from: first events
//! whose `sha256` equals the copy's current bytes (so Obsidian renames
//! and dedupe still resolve), then events whose `saved` equals the
//! vault-relative path (so an edited copy still resolves). The pick is
//! the newest candidate in writer order recorded on this host whose
//! source is present, else the newest candidate overall. States:
//! `present` (source on this host, bytes match), `missing` (source gone),
//! `changed` (source bytes differ), `elsewhere` (another host recorded
//! it; never probed).

use std::{fs, io, path::Path};

use chrono::{DateTime, FixedOffset};

use super::journal::{self, OriginEvent};
use crate::native::capture_clip::sha256_hex;
use crate::native::pref_journal;

/// One resolved origin: the recorded copy plus its current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginReport {
    pub(crate) source: String,
    pub(crate) display: String,
    pub(crate) host: String,
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) via: String,
    pub(crate) state: OriginState,
}

/// Whether the recorded source still holds the recorded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OriginState {
    Present,
    Missing,
    Changed,
    Elsewhere,
}

impl OriginState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Changed => "changed",
            Self::Elsewhere => "elsewhere",
        }
    }
}

/// A vault copy lookup failure: the copy is gone, or it (or the journal)
/// could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OriginError {
    NotFound(String),
    Io(String),
}

impl OriginError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::Io(_) => "io",
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::NotFound(message) | Self::Io(message) => message.clone(),
        }
    }
}

/// Resolve the origin of the vault-relative path `saved`. A missing
/// vault copy fails with `not_found`; a copy with no recorded origin
/// resolves to `None`.
pub(crate) fn resolve_origin(
    vault: &Path,
    saved: &str,
    current_host: &str,
) -> Result<(Option<OriginReport>, Vec<String>), OriginError> {
    let bytes = fs::read(vault.join(saved)).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            OriginError::NotFound(format!("no such vault file: {saved}"))
        } else {
            OriginError::Io(format!(
                "read vault file {}: {error}",
                vault.join(saved).display()
            ))
        }
    })?;
    let digest = sha256_hex(&bytes);
    let (mut events, warnings) =
        journal::read_journal(vault).map_err(|error| {
            OriginError::Io(format!(
                "read attachment journal in {}: {error}",
                vault.display()
            ))
        })?;
    pref_journal::sort_by_writer_order(&mut events, |event| {
        (
            event.ts,
            event.host.as_str(),
            event.path.as_str(),
            event.line,
        )
    });
    let mut by_hash = Vec::new();
    let mut by_saved = Vec::new();
    for event in &events {
        if event.sha256 == digest {
            by_hash.push(event);
        } else if event.saved == saved {
            by_saved.push(event);
        }
    }
    let candidates = if by_hash.is_empty() {
        by_saved
    } else {
        by_hash
    };
    let Some(picked) = pick_candidate(&candidates, current_host) else {
        return Ok((None, warnings));
    };
    Ok((Some(report_for(picked, current_host)), warnings))
}

/// The newest candidate in writer order recorded on this host whose
/// source is present, else the newest candidate overall. Events arrive
/// in writer order, so the newest is last.
fn pick_candidate<'a>(
    candidates: &[&'a OriginEvent],
    current_host: &str,
) -> Option<&'a OriginEvent> {
    if let Some(picked) = candidates
        .iter()
        .rev()
        .find(|event| {
            event.host == current_host && fs::metadata(&event.source).is_ok()
        })
        .copied()
    {
        return Some(picked);
    }
    candidates.last().copied()
}

fn report_for(event: &OriginEvent, current_host: &str) -> OriginReport {
    let state = if event.host != current_host {
        OriginState::Elsewhere
    } else {
        match fs::read(&event.source) {
            Ok(contents) if sha256_hex(&contents) == event.sha256 => {
                OriginState::Present
            }
            Ok(_) => OriginState::Changed,
            Err(_) => OriginState::Missing,
        }
    };
    OriginReport {
        source: event.source.clone(),
        display: event.display.clone(),
        host: event.host.clone(),
        ts: event.ts,
        via: event.via.clone(),
        state,
    }
}
