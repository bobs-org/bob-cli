//! Companion audio discovery for Highlights scan and intake.
use super::*;

pub(super) const AUDIO_COMPANION_EXTENSIONS: &[&str] =
    &["mp3", "m4a", "ogg", "opus"];

pub(super) const ENV_AUDIO_LIBRARY: &str = "BOB_HIGHLIGHTS_AUDIO_LIBRARY";

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

/// Library root for sase-listen companion audio, first hit wins:
/// `BOB_HIGHLIGHTS_AUDIO_LIBRARY`, then `highlights.audio_library`,
/// then `$XDG_DATA_HOME/sase-listen/library`,
/// then `~/.local/share/sase-listen/library`.
pub(super) fn audio_library_root(configured: Option<&str>) -> PathBuf {
    if let Some(value) =
        env::var_os(ENV_AUDIO_LIBRARY).filter(|value| !value.is_empty())
    {
        return bob_env::expand_tilde(&PathBuf::from(value));
    }
    if let Some(value) =
        configured.map(str::trim).filter(|value| !value.is_empty())
    {
        return bob_env::expand_tilde(Path::new(value));
    }
    if let Some(xdg) =
        env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty())
    {
        return PathBuf::from(xdg).join("sase-listen/library");
    }
    bob_env::home_dir().join(".local/share/sase-listen/library")
}

/// Episode ids are single path components drawn from
/// `[A-Za-z0-9.*-]+`, rejecting `.` and `..`.
pub(super) fn is_valid_episode_id(id: &str) -> bool {
    if id.is_empty() || id == "." || id == ".." {
        return false;
    }
    if id.contains('/') || id.contains('\\') {
        return false;
    }
    id.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'.' | b'*' | b'-' | b'+')
    })
}

/// `audio.episode_id` from report frontmatter, parsed the same way as the
/// frontmatter title helper (YAML block between leading `---` markers).
pub(super) fn frontmatter_episode_id(markdown: &str) -> Result<Option<String>> {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(None);
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if matches!(line.trim(), "---" | "...") {
            closed = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return Ok(None);
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(&yaml).map_err(|error| {
            CommandError::new(format!("parse Markdown frontmatter: {error}"))
        })?;
    let episode = value
        .as_mapping()
        .and_then(|mapping| {
            mapping.get(serde_yaml::Value::String("audio".into()))
        })
        .and_then(|audio| audio.as_mapping())
        .and_then(|audio| {
            audio.get(serde_yaml::Value::String("episode_id".into()))
        })
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|episode| !episode.is_empty())
        .map(str::to_string);
    Ok(episode)
}

fn is_bare_filename(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if name.contains('/') || name.contains('\\') {
        return false;
    }
    !Path::new(name).is_absolute()
}

/// Resolve `<library>/<id>/manifest.json` to its audio file when
/// `audio.file` is a bare filename that exists in that episode directory.
pub(super) fn episode_audio_source(
    library: &Path,
    episode_id: &str,
) -> Option<PathBuf> {
    if !is_valid_episode_id(episode_id) {
        return None;
    }
    let episode_dir = library.join(episode_id);
    let manifest_path = episode_dir.join("manifest.json");
    let contents = fs::read_to_string(&manifest_path).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let file = manifest
        .get("audio")?
        .get("file")?
        .as_str()
        .map(str::trim)
        .filter(|file| !file.is_empty())?;
    if !is_bare_filename(file) {
        return None;
    }
    if !is_audio_companion_path(Path::new(file)) {
        return None;
    }
    let candidate = episode_dir.join(file);
    candidate.is_file().then_some(candidate)
}

/// Sibling narration script `<md-dir>/<md-stem>_narration.md`, stripping a
/// trailing `__final` from the stem.
pub(super) fn narration_script_candidate(source_md: &Path) -> Option<PathBuf> {
    let stem = source_md.file_stem()?.to_str()?;
    if stem.is_empty() {
        return None;
    }
    let base = stem.strip_suffix("__final").unwrap_or(stem);
    if base.is_empty() {
        return None;
    }
    let parent = source_md.parent().unwrap_or_else(|| Path::new("."));
    Some(parent.join(format!("{base}_narration.md")))
}

fn manifest_sha_matches(manifest: &serde_json::Value, digest: &str) -> bool {
    for section in ["source", "script"] {
        if manifest
            .get(section)
            .and_then(|value| value.get("sha256"))
            .and_then(serde_json::Value::as_str)
            == Some(digest)
        {
            return true;
        }
    }
    false
}

fn manifest_audio_candidate(
    episode_dir: &Path,
    manifest: &serde_json::Value,
) -> Option<PathBuf> {
    let file = manifest
        .get("audio")?
        .get("file")?
        .as_str()
        .map(str::trim)
        .filter(|file| !file.is_empty())?;
    if !is_bare_filename(file) {
        return None;
    }
    if !is_audio_companion_path(Path::new(file)) {
        return None;
    }
    let candidate = episode_dir.join(file);
    candidate.is_file().then_some(candidate)
}

fn manifest_created_at(manifest: &serde_json::Value) -> String {
    manifest
        .get("created_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Scan `<library>/*/manifest.json` for `source.sha256` or `script.sha256`
/// equal to `digest`. Newest `created_at` wins. Unreadable manifests are
/// skipped.
pub(super) fn find_audio_by_script_hash(
    library: &Path,
    digest: &str,
) -> Option<PathBuf> {
    let entries = fs::read_dir(library).ok()?;
    let mut best: Option<(String, PathBuf)> = None;
    for entry in entries {
        let entry = entry.ok()?;
        let episode_dir = entry.path();
        if !episode_dir.is_dir() {
            continue;
        }
        if entry.file_name().to_str() == Some(".staging") {
            continue;
        }
        let manifest_path = episode_dir.join("manifest.json");
        let contents = match fs::read_to_string(&manifest_path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };
        let manifest: serde_json::Value = match serde_json::from_str(&contents)
        {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        if !manifest_sha_matches(&manifest, digest) {
            continue;
        }
        let Some(candidate) = manifest_audio_candidate(&episode_dir, &manifest)
        else {
            continue;
        };
        let created_at = manifest_created_at(&manifest);
        let newer = match &best {
            None => true,
            Some((best_created_at, _)) => created_at > *best_created_at,
        };
        if newer {
            best = Some((created_at, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Validate an explicit `--audio PATH`: the file must exist and its
/// extension must be in the companion allowlist.
pub(super) fn validate_explicit_audio(path: &Path) -> Result<PathBuf> {
    if !path.is_file() {
        return Err(CommandError::new(format!(
            "audio file does not exist or is not a file: {}",
            path.display()
        )));
    }
    if !is_audio_companion_path(path) {
        return Err(CommandError::new(format!(
            "audio file must have one of .mp3, .m4a, .ogg, .opus extensions: {}",
            path.display()
        )));
    }
    Ok(path.to_path_buf())
}
