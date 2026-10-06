//! Attach mode for `--listen`: bind a new episode to an existing capture.
//!
//! When `--listen` is given and the target is already captured, create
//! attaches the new episode instead of refusing: it runs the listen
//! command against the existing PDF and drops the audio at
//! `xlib/<rel>.mp3`, and scan late-pairs it onto the existing PDF and
//! ref note. The PDF and the ref note are never touched. Identity is
//! proven, never guessed: it comes from a source-URL dedupe hit or from
//! the target path itself being a captured PDF.

use std::{fs, path::PathBuf};

use super::listen::{ListenCommand, ListenValues};
use super::sources::RecordedSource;
use super::{CommandError, Config, Result};

/// An existing capture that `--listen` can attach a new episode to.
pub(super) struct AttachTarget {
    /// PDF path relative to `lib_dir` (library capture) or `xlib_dir`
    /// (queued intake).
    pub(super) rel: PathBuf,
    /// The existing PDF on disk.
    pub(super) existing_pdf: PathBuf,
    /// The ref note behind a dedupe hit, if any.
    pub(super) ref_note: Option<PathBuf>,
    /// Whether that note already carries an `audio` field.
    pub(super) note_has_audio: bool,
    /// Title for `{title}`: the note title or the marker title.
    pub(super) title: String,
}

/// Attach to the capture behind a source-URL dedupe hit: a ref-note hit
/// resolves through the note's `source_pdf` (which must sit inside
/// `lib_dir` and exist), an intake hit uses the queued intake PDF.
pub(super) fn attach_for_dedupe_hit(
    config: &Config,
    hit: &RecordedSource,
) -> Result<AttachTarget> {
    if hit.is_ref_note {
        let vault_rel = hit.source_pdf.clone().unwrap_or_default();
        let pdf = config.bob_dir.join(&vault_rel);
        let rel =
            super::relative_inside(&pdf, &config.lib_dir).ok_or_else(|| {
                CommandError::new(
                    "ref note has no resolvable source_pdf".to_string(),
                )
            })?;
        if vault_rel.is_empty() || !pdf.is_file() {
            return Err(CommandError::new(
                "ref note has no resolvable source_pdf".to_string(),
            ));
        }
        let (note_title, note_has_audio) = read_note_title_audio(&hit.path);
        let title = note_title
            .or_else(|| marker_title(&pdf))
            .unwrap_or_else(|| super::clip_url::humanize_stem(&stem_of(&rel)));
        return Ok(AttachTarget {
            rel,
            existing_pdf: pdf,
            ref_note: Some(hit.path.clone()),
            note_has_audio: note_has_audio || hit.has_audio,
            title,
        });
    }
    let rel = super::relative_inside(&hit.path, &config.xlib_dir).ok_or_else(
        || {
            CommandError::new(format!(
                "queued intake PDF is outside the Highlights intake: {}",
                hit.path.display()
            ))
        },
    )?;
    let title = marker_title(&hit.path)
        .unwrap_or_else(|| super::clip_url::humanize_stem(&stem_of(&rel)));
    Ok(AttachTarget {
        rel,
        existing_pdf: hit.path.clone(),
        ref_note: None,
        note_has_audio: false,
        title,
    })
}

/// Attach to a local PDF target that is itself an existing capture: the
/// canonical path sits inside the library or intake and carries a
/// Highlights marker. Returns `None` for PDFs outside the vault (the
/// normal route handles those) and for unmarked in-vault PDFs (the
/// identity check still refuses those with its move-it-out error).
pub(super) fn attach_for_local_pdf(
    config: &Config,
    canonical: &std::path::Path,
) -> Result<Option<AttachTarget>> {
    let rel =
        if let Some(rel) = super::relative_inside(canonical, &config.lib_dir) {
            rel
        } else if let Some(rel) =
            super::relative_inside(canonical, &config.xlib_dir)
        {
            rel
        } else {
            return Ok(None);
        };
    if !super::pdf_target::pdf_already_captured(canonical) {
        return Ok(None);
    }
    let vault_rel = super::vault_relative_path_value(config, canonical);
    let (ref_note, note_has_audio) =
        find_note_audio_for_source_pdf(config, &vault_rel);
    let title = ref_note
        .as_ref()
        .and_then(|note| read_note_title_audio(note).0)
        .or_else(|| marker_title(canonical))
        .unwrap_or_else(|| super::clip_url::humanize_stem(&stem_of(&rel)));
    Ok(Some(AttachTarget {
        rel,
        existing_pdf: canonical.to_path_buf(),
        ref_note,
        note_has_audio,
        title,
    }))
}

fn stem_of(rel: &std::path::Path) -> String {
    rel.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

/// The `title` the marker stamps on an existing PDF, if it parses.
fn marker_title(pdf: &std::path::Path) -> Option<String> {
    let marker = super::marker::read_pdf_marker(pdf).ok()?;
    let projection = super::marker::parse_marker(&marker.contents).ok()?;
    projection
        .get("title")
        .and_then(|value| value.as_string())
        .map(str::to_string)
}

/// A ref note's frontmatter `title` plus whether its `audio` field is
/// set. Unreadable notes report neither.
fn read_note_title_audio(note: &std::path::Path) -> (Option<String>, bool) {
    let contents = fs::read_to_string(note).unwrap_or_default();
    let Some((frontmatter, _)) =
        super::frontmatter::split_frontmatter(&contents)
    else {
        return (None, false);
    };
    let mut title = None;
    let mut has_audio = false;
    for raw in frontmatter {
        let entry = super::frontmatter::parse_frontmatter_entry(&raw);
        let key = entry.key.as_deref().unwrap_or_default();
        let value = entry
            .value
            .as_ref()
            .and_then(|value| value.as_string().map(str::to_string));
        match key {
            "title" => {
                if title.is_none() {
                    title = value;
                }
            }
            "audio" => {
                if value.is_some_and(|value| !value.is_empty()) {
                    has_audio = true;
                }
            }
            _ => {}
        }
    }
    (title, has_audio)
}

/// Find the ref note whose `source_pdf` names `vault_rel`, if any, and
/// whether it already carries an `audio` field.
fn find_note_audio_for_source_pdf(
    config: &Config,
    vault_rel: &str,
) -> (Option<PathBuf>, bool) {
    // Scan's own convention first: the note mirrors the PDF path.
    if let Ok(note) =
        super::ref_note_path(config, &config.bob_dir.join(vault_rel))
        && note.is_file()
    {
        let (_, has_audio) = read_note_title_audio(&note);
        return (Some(note), has_audio);
    }
    // Fall back to a full walk for notes scan wrote elsewhere.
    let mut found = (None, false);
    walk_notes_for_source_pdf(&config.ref_dir, vault_rel, &mut found);
    found
}

fn walk_notes_for_source_pdf(
    dir: &std::path::Path,
    vault_rel: &str,
    found: &mut (Option<PathBuf>, bool),
) {
    if found.0.is_some() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if path
                .file_name()
                .is_none_or(|name| name != std::ffi::OsStr::new(".git"))
            {
                walk_notes_for_source_pdf(&path, vault_rel, found);
            }
            continue;
        }
        if !file_type.is_file()
            || !path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let contents = fs::read_to_string(&path).unwrap_or_default();
        let Some((frontmatter, _)) =
            super::frontmatter::split_frontmatter(&contents)
        else {
            continue;
        };
        let mut source_pdf = None;
        let mut has_audio = false;
        for raw in frontmatter {
            let entry = super::frontmatter::parse_frontmatter_entry(&raw);
            match entry.key.as_deref().unwrap_or_default() {
                "source_pdf" => {
                    if source_pdf.is_none() {
                        source_pdf = entry.value.as_ref().and_then(|value| {
                            value.as_string().map(str::to_string)
                        });
                    }
                }
                "audio" => {
                    if entry.value.as_ref().is_some_and(|value| {
                        value.as_string().is_some_and(|value| !value.is_empty())
                    }) {
                        has_audio = true;
                    }
                }
                _ => {}
            }
        }
        if source_pdf.as_deref() == Some(vault_rel) {
            *found = (Some(path), has_audio);
            return;
        }
    }
}

/// Refuse when the capture already has companion audio: any allowed
/// companion beside `lib/<rel>.pdf` or `xlib/<rel>.<ext>`, or an
/// `audio` field on the ref note.
pub(super) fn refuse_existing_audio(
    config: &Config,
    attach: &AttachTarget,
) -> Result<()> {
    let library_pdf = config.lib_dir.join(&attach.rel);
    if let Some(beside) = super::audio::audio_beside(&library_pdf) {
        return Err(CommandError::new(format!(
            "already has companion audio: {}",
            beside.display()
        )));
    }
    if let Some(beside) =
        super::audio::audio_beside(&config.xlib_dir.join(&attach.rel))
    {
        return Err(CommandError::new(format!(
            "already has companion audio: {}",
            beside.display()
        )));
    }
    if attach.note_has_audio {
        let where_note = attach
            .ref_note
            .as_ref()
            .map(|note| note.display().to_string())
            .unwrap_or_else(|| "the ref note".to_string());
        return Err(CommandError::new(format!(
            "already has companion audio: {where_note}"
        )));
    }
    Ok(())
}

/// Where the new episode lands: `xlib/<rel>.mp3`.
pub(super) fn attach_audio_dest(
    config: &Config,
    attach: &AttachTarget,
) -> PathBuf {
    config.xlib_dir.join(&attach.rel).with_extension("mp3")
}

/// Run attach mode: refuse existing audio, run the listen command
/// against the existing PDF, and install the episode at
/// `xlib/<rel>.mp3`. A dry run prints the plan with
/// `listen: would-run …` and writes nothing.
pub(super) fn run_attach(
    config: &Config,
    listen: &ListenCommand,
    attach: &AttachTarget,
    source_line: &str,
    target_value: &str,
    dry_run: bool,
    scratch: &mut super::ScratchDir,
) -> Result<()> {
    refuse_existing_audio(config, attach)?;
    let dest = attach_audio_dest(config, attach);
    let scratch_audio =
        scratch.path().join(format!("{}.mp3", stem_of(&attach.rel)));
    let values = ListenValues {
        target: target_value.to_string(),
        pdf: attach.existing_pdf.clone(),
        audio: scratch_audio.clone(),
        title: attach.title.clone(),
    };
    if dry_run {
        println!("would attach listen episode to existing capture");
        println!("{source_line}");
        println!("pdf: {} (unchanged)", attach.existing_pdf.display());
        if let Some(note) = &attach.ref_note {
            println!("ref: {}", note.display());
        }
        println!("audio: {} (from --listen)", dest.display());
        println!("title: {}", attach.title);
        println!("{}", super::listen::would_run_line(listen, &values));
        println!("writes: none");
        return Ok(());
    }
    let produced = super::listen::run_listen(listen, &values)
        .map_err(|error| error.into_command_error())?;
    let _ = produced;
    if let Err(error) = super::atomic_copy(&scratch_audio, &dest) {
        return Err(super::listen::post_listen_error(
            scratch,
            &scratch_audio,
            error.to_string(),
            format!("copy it to {} for scan to pair", dest.display()),
        ));
    }
    println!("ok attached listen episode to existing capture");
    println!("{source_line}");
    println!("pdf: {} (unchanged)", attach.existing_pdf.display());
    if let Some(note) = &attach.ref_note {
        println!("ref: {}", note.display());
    }
    println!("audio: {} (from --listen)", dest.display());
    println!("title: {}", attach.title);
    println!("next: bob highlights scan");
    Ok(())
}
