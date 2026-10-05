//! Companion audio discovery for Highlights scan and intake.
use super::*;

pub(super) const AUDIO_COMPANION_EXTENSIONS: &[&str] =
    &["mp3", "m4a", "ogg", "opus"];

pub(super) fn is_audio_companion_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            AUDIO_COMPANION_EXTENSIONS
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

pub(super) fn audio_beside(path: &Path) -> Option<PathBuf> {
    AUDIO_COMPANION_EXTENSIONS.iter().find_map(|extension| {
        let candidate = path.with_extension(extension);
        candidate.is_file().then_some(candidate)
    })
}

/// First same-stem companion beside the PDF, then (for a library PDF) the
/// matching file still sitting in xlib so dry-run late pairing can plan the
/// embed before the move happens.
pub(super) fn discover_companion_audio(
    config: &Config,
    pdf: &Path,
) -> Option<PathBuf> {
    if let Some(path) = audio_beside(pdf) {
        return Some(path);
    }
    if let Ok(relative) = pdf.strip_prefix(&config.lib_dir) {
        return audio_beside(&config.xlib_dir.join(relative));
    }
    None
}

/// Vault-relative library location used in `audio:` frontmatter and the
/// `![[...]]` embed. Intake PDFs still resolve to `lib/<rel>.<ext>`.
pub(super) fn companion_audio_vault_path(
    config: &Config,
    pdf: &Path,
    audio: &Path,
) -> String {
    let extension = audio.extension().unwrap_or_default();
    let library_path = if let Ok(relative) = pdf.strip_prefix(&config.xlib_dir)
    {
        config.lib_dir.join(relative).with_extension(extension)
    } else if let Ok(relative) = pdf.strip_prefix(&config.lib_dir) {
        config.lib_dir.join(relative).with_extension(extension)
    } else {
        pdf.with_extension(extension)
    };
    vault_relative_path_value(config, &library_path)
}

pub(super) fn audio_embed_line(vault_path: &str) -> String {
    format!("![[{vault_path}]]")
}

pub(super) fn audio_frontmatter_value(vault_path: &str) -> String {
    format!("[[{vault_path}]]")
}

pub(super) fn collect_audio_paths_from_dir(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CommandError::new(format!("scan {}: {error}", directory.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            CommandError::new(format!("scan {}: {error}", directory.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CommandError::new(format!("stat {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            if !should_skip_scan_dir(&path) {
                collect_audio_paths_from_dir(&path, paths)?;
            }
        } else if file_type.is_file() && is_audio_companion_path(&path) {
            paths.push(path);
        }
    }

    Ok(())
}

pub(super) fn same_stem_pdf_exists(path: &Path) -> bool {
    path.with_extension("pdf").is_file()
}

pub(super) fn library_pdf_for_xlib_audio(
    config: &Config,
    audio: &Path,
) -> Option<PathBuf> {
    let relative = audio.strip_prefix(&config.xlib_dir).ok()?;
    let destination = config.lib_dir.join(relative).with_extension("pdf");
    destination.is_file().then_some(destination)
}

pub(super) fn maybe_insert_audio_embed(
    note: &ParsedNote,
    metadata: &PipelineMetadata,
    body: &str,
) -> String {
    let Some(vault_path) = metadata.audio.as_deref() else {
        return body.to_string();
    };
    if note_has_audio_field(note) {
        return body.to_string();
    }
    let embed = audio_embed_line(vault_path);
    if body.contains(&embed) {
        return body.to_string();
    }
    insert_audio_embed(body, &embed)
}

pub(super) fn note_has_audio_field(note: &ParsedNote) -> bool {
    note.frontmatter
        .iter()
        .any(|entry| entry.key.as_deref() == Some(FIELD_AUDIO))
}

fn insert_audio_embed(body: &str, embed: &str) -> String {
    let after_task = [String::new(), embed.to_string()];
    if let Ok(PdfTaskLineState::Present(task)) = parse_pdf_task_line(body) {
        return insert_lines_after(body, task.line_index, &after_task);
    }
    let before_anchor = [String::new(), embed.to_string(), String::new()];
    if let Some(index) = heading_line_index(body, "Highlights") {
        return insert_lines_before(body, index, &before_anchor);
    }
    if let Some(index) = line_index_containing(body, MANAGED_BODY_BEGIN) {
        return insert_lines_before(body, index, &before_anchor);
    }
    body.to_string()
}

fn heading_line_index(body: &str, title: &str) -> Option<usize> {
    body.lines().enumerate().find_map(|(index, line)| {
        markdown::atx_heading(line)
            .filter(|(_, heading)| *heading == title)
            .map(|_| index)
    })
}

fn line_index_containing(body: &str, needle: &str) -> Option<usize> {
    body.lines().position(|line| line.contains(needle))
}

fn insert_lines_before(
    body: &str,
    line_index: usize,
    lines: &[String],
) -> String {
    if line_index == 0 {
        let mut rendered = String::new();
        let line_ending = preferred_line_ending(body);
        for line in lines {
            rendered.push_str(line);
            rendered.push_str(line_ending);
        }
        rendered.push_str(body);
        return rendered;
    }
    insert_lines_after(body, line_index - 1, lines)
}
