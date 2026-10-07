//! Vault doctor checks and xlib intake.
use super::*;

pub(super) fn show_marker(config: &Config, pdf: &Path) -> Result<()> {
    let marker = read_pdf_marker(pdf)?;
    let marker_input = parse_marker_with_normalization(&marker.contents)?;
    print_config_report("marker", config);
    println!("pdf: {}", pdf.display());
    println!("marker_page: {}", marker.page_number);
    println!("marker_note: {}", marker.note_number);
    if let Some(label) = status_normalization_label(StatusNormalization {
        marker: marker_input.status_normalized,
        ..StatusNormalization::default()
    }) {
        println!("status_normalization: {label}");
    }
    println!("marker_raw:");
    print!("{}", marker.contents);
    if !marker.contents.ends_with('\n') {
        println!();
    }
    println!("marker_rendered:");
    print!("{}", render_marker(&marker_input.projection)?);
    println!("writes: none");
    Ok(())
}

pub(super) fn doctor_vault(config: &Config, no_hooks: bool) -> Result<()> {
    print_config_report("doctor", config);
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    if no_hooks {
        println!("pre_scan_hook: skipped (--no-hooks)");
    } else {
        match configured_pre_scan_hook(no_hooks) {
            Ok(pre_scan_hook) => {
                check_pre_scan_hook(pre_scan_hook.as_ref(), &mut failures);
            }
            Err(error) => {
                println!("pre_scan_hook: fail ({error})");
                failures.push(error.to_string());
            }
        }
    }
    let layout_valid = match validate_library_layout(config) {
        Ok(()) => true,
        Err(error) => {
            failures.push(error.to_string());
            false
        }
    };

    print_path_check("vault_path", &config.bob_dir, config.bob_dir.is_dir());
    if !config.bob_dir.is_dir() {
        failures.push(format!(
            "vault path does not exist or is not a directory: {}",
            config.bob_dir.display()
        ));
    }

    print_path_check("library_dir", &config.lib_dir, config.lib_dir.is_dir());
    if !config.lib_dir.is_dir() {
        failures.push(format!(
            "library directory does not exist or is not a directory: {}",
            config.lib_dir.display()
        ));
    }

    print_path_check("ref_dir", &config.ref_dir, config.ref_dir.is_dir());
    if !config.ref_dir.is_dir() {
        failures.push(format!(
            "reference directory does not exist or is not a directory: {}",
            config.ref_dir.display()
        ));
    }

    print_path_check("xlib_dir", &config.xlib_dir, config.xlib_dir.is_dir());
    if !config.xlib_dir.is_dir() {
        warnings.push(format!(
            "xlib intake directory does not exist: {} (bob ref create creates it on demand)",
            config.xlib_dir.display()
        ));
    }
    let xlib_pending = if layout_valid && config.xlib_dir.is_dir() {
        let mut pending = Vec::new();
        match collect_pdf_paths_from_dir(&config.xlib_dir, &mut pending) {
            Ok(()) => pending.len(),
            Err(error) => {
                failures.push(error.to_string());
                0
            }
        }
    } else {
        0
    };
    println!("xlib_pending: {xlib_pending}");
    let orphan_audio = if layout_valid && config.xlib_dir.is_dir() {
        match collect_orphan_audio(config) {
            Ok(paths) => paths,
            Err(error) => {
                failures.push(error.to_string());
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    println!("xlib_orphan_audio: {}", orphan_audio.len());
    for path in &orphan_audio {
        warnings.push(format!(
            "orphan companion audio has no PDF in xlib or lib: {}",
            display_vault_relative_path(config, path)
        ));
    }
    if layout_valid && let Err(error) = plan_xlib_intake(config) {
        failures.push(error.to_string());
    }

    let pdfs = if config.lib_dir.is_dir() {
        match collect_pdf_paths(config, false) {
            Ok(pdfs) => pdfs,
            Err(error) => {
                failures.push(error.to_string());
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    println!("pdf_count: {}", pdfs.len());

    let mut sidecar_count = 0usize;
    let mut missing_sidecars = Vec::new();
    for pdf in &pdfs {
        match discover_sidecar_path(pdf) {
            Ok(Some(_)) => sidecar_count += 1,
            Ok(None) => missing_sidecars.push(pdf.clone()),
            Err(error) => failures.push(error.to_string()),
        }
    }
    println!("sidecars_found: {sidecar_count}");
    println!("sidecars_missing: {}", missing_sidecars.len());
    if !missing_sidecars.is_empty() {
        warnings.push(format!(
            "{} PDF(s) do not have a Highlights sidecar",
            missing_sidecars.len()
        ));
    }

    let mut readable_markers = 0usize;
    for pdf in &pdfs {
        match read_pdf_marker(pdf)
            .and_then(|marker| parse_marker(&marker.contents).map(|_| marker))
        {
            Ok(_) => readable_markers += 1,
            Err(error) => failures.push(format!("{}: {error}", pdf.display())),
        }
    }
    println!("pdf_markers_readable: {readable_markers}");
    println!(
        "pdf_marker_errors: {}",
        pdfs.len().saturating_sub(readable_markers)
    );

    match git_status(config, &[])? {
        GitStatus::MissingCommand => {
            println!("git: fail (command not found)");
            failures.push("git command not found".to_string());
        }
        GitStatus::NotWorktree => {
            println!("git: fail (vault is not a worktree)");
            failures.push(format!(
                "vault is not a Git worktree: {}",
                config.bob_dir.display()
            ));
        }
        GitStatus::Worktree { entries } if entries.is_empty() => {
            println!("git: ok (clean worktree)");
        }
        GitStatus::Worktree { entries } => {
            println!("git: fail (dirty worktree)");
            println!("git_dirty_count: {}", entries.len());
            for entry in entries.iter().take(20) {
                println!("  {}", entry.raw);
            }
            failures
                .push(format!("vault has {} dirty Git path(s)", entries.len()));
        }
    }

    match create::pandoc_command() {
        Some(command) => {
            println!("pandoc: available ({})", command.to_string_lossy());
        }
        None => {
            println!("pandoc: warn (command not found)");
            warnings.push(
                "pandoc command not found; Markdown PDF creation is unavailable"
                    .to_string(),
            );
        }
    }

    // The fetcher checks `BOB_HIGHLIGHTS_CURL` first, so doctor does too.
    if let Some(override_curl) =
        std::env::var_os(super::fetch::ENV_CURL_OVERRIDE)
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
    {
        #[cfg(unix)]
        let usable = if override_curl.components().count() == 1 {
            super::clip_adapter::find_on_path(&override_curl.to_string_lossy())
                .is_some()
        } else {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(&override_curl).is_ok_and(|m| {
                m.is_file() && m.permissions().mode() & 0o111 != 0
            })
        };
        #[cfg(not(unix))]
        let usable = if override_curl.components().count() == 1 {
            super::clip_adapter::find_on_path(&override_curl.to_string_lossy())
                .is_some()
        } else {
            override_curl.is_file()
        };
        if usable {
            println!("curl: available ({})", override_curl.display());
        } else {
            println!(
                "curl: warn (override {} not found or not executable)",
                override_curl.display()
            );
            warnings.push(format!(
                "curl override {} does not exist or is not executable; PDF URL and arXiv targets are unavailable",
                override_curl.display()
            ));
        }
    } else {
        match super::clip_adapter::find_on_path("curl") {
            Some(path) => {
                println!("curl: available ({})", path.display());
            }
            None => {
                println!("curl: warn (command not found)");
                warnings.push(
                    "curl command not found; PDF URL and arXiv targets are unavailable"
                        .to_string(),
                );
            }
        }
    }

    append_web_clip_doctor_rows(&mut warnings);
    append_listen_doctor_row(&mut warnings);
    append_url_routing_doctor_row(&mut warnings);
    crate::native::ref_jobs::append_ref_jobs_doctor_row(&mut warnings);
    append_library_doctor_rows(config, &mut warnings);

    if !warnings.is_empty() {
        println!("warnings:");
        for warning in &warnings {
            println!("  {warning}");
        }
    }

    println!("writes: none");
    if failures.is_empty() {
        println!("result: ok");
        Ok(())
    } else {
        println!("result: failed");
        Err(CommandError::new(format!(
            "doctor found failing checks:\n  {}",
            failures.join("\n  ")
        )))
    }
}

/// URL-routing policy row: the effective toggles and exclusions.
/// A config error turns routing off with a warning; it never fails
/// the vault doctor.
fn append_url_routing_doctor_row(warnings: &mut Vec<String>) {
    match crate::native::url_routing::UrlRoutingPolicy::load() {
        Ok(policy) => {
            let excludes = if policy.exclude_hosts.is_empty() {
                "none".to_string()
            } else {
                policy.exclude_hosts.join(", ")
            };
            println!(
                "url routing: capture {} · gkeep {} · excludes {excludes}",
                if policy.capture { "on" } else { "off" },
                if policy.gkeep { "on" } else { "off" },
            );
        }
        Err(error) => {
            let message = match error {
                bob_config::ConfigError::Read(message)
                | bob_config::ConfigError::Invalid(message) => message,
            };
            println!("url routing: warn (off: {message})");
            warnings.push(format!("URL routing is off: {message}"));
        }
    }
}

/// Library health and coverage rows (phase `doctor` of
/// `plan:202610/bob_ref_reference_library.md`).
///
/// These rows read the vault through the read-only ref index and never fail
/// the command: every problem is a warning. The `library diagnostics` rollup
/// skips `marker_mirror_excluded`, which has its own `annotations` row.
fn append_library_doctor_rows(config: &Config, warnings: &mut Vec<String>) {
    let library = crate::native::ref_library::LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    };
    let index = match crate::native::ref_library::build_index(&library) {
        Ok(index) => index,
        Err(error) => {
            println!("library: unavailable ({error})");
            warnings
                .push(format!("reference library index unavailable: {error}"));
            return;
        }
    };

    let counts = &index.counts;
    println!(
        "library: ok ({} · {} finished · {} started · {} queued · {} dropped · {} unknown)",
        plural_notes(counts.notes),
        counts.finished,
        counts.started,
        counts.queued,
        counts.dropped,
        counts.unknown,
    );

    append_library_diagnostics_row(&index.rows, warnings);
    append_library_identity_row(&index.rows, warnings);
    append_library_annotations_row(&index.rows, warnings);

    let zorg: crate::native::ref_library::ZorgCoverage =
        crate::native::ref_library::count_zorg_records(
            &config.bob_dir,
            &config.ref_dir,
            &index.rows,
        );
    if zorg.total == 0 {
        println!(
            "coverage: ok (no unindexed zorg-era reading records outside ref/)"
        );
    } else {
        let mut entries = zorg
            .top_files()
            .iter()
            .map(|(path, count)| format!("{path} {count}"))
            .collect::<Vec<_>>();
        if zorg.per_file.len() > entries.len() {
            entries.push("…".to_string());
        }
        let record_word = if zorg.total == 1 {
            "record outside ref/ is"
        } else {
            "records outside ref/ are"
        };
        println!(
            "coverage: warn (~{} zorg-era reading {record_word} not indexed: {})",
            zorg.total,
            entries.join(", "),
        );
        warnings.push(format!(
            "{} unindexed zorg-era reading records outside ref/",
            zorg.total,
        ));
    }
}

fn append_library_diagnostics_row(
    rows: &[crate::native::ref_library::RefRow],
    warnings: &mut Vec<String>,
) {
    let mut by_code: BTreeMap<&str, usize> = BTreeMap::new();
    let mut notes = 0usize;
    let mut examples: Vec<&str> = Vec::new();
    for row in rows {
        let mut row_codes = false;
        for diagnostic in &row.diagnostics {
            if diagnostic.code == "marker_mirror_excluded" {
                continue;
            }
            *by_code.entry(diagnostic.code.as_str()).or_default() += 1;
            row_codes = true;
        }
        if row_codes {
            notes += 1;
            if examples.len() < 3 {
                examples.push(row.path.as_str());
            }
        }
    }
    if notes == 0 {
        println!("library diagnostics: ok (no diagnostics)");
        return;
    }
    let mut summary = by_code
        .iter()
        .map(|(code, count)| format!("{count} {code}"))
        .collect::<Vec<_>>()
        .join(", ");
    summary.push_str(&format!(" · e.g. {}", examples.join(", ")));
    println!(
        "library diagnostics: warn ({}: {summary})",
        plural_notes(notes),
    );
    warnings.push(format!(
        "{} reference notes carry diagnostics",
        plural_notes(notes),
    ));
}

fn append_library_identity_row(
    rows: &[crate::native::ref_library::RefRow],
    warnings: &mut Vec<String>,
) {
    let mut by_key: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for row in rows.iter().filter(|row| row.source_pdf.is_some()) {
        let mut seen = BTreeSet::new();
        for key in &row.identity.keys {
            if seen.insert(key.as_str()) {
                by_key
                    .entry(key.as_str())
                    .or_default()
                    .insert(row.path.as_str());
            }
        }
    }
    let mut shared: Vec<&str> = by_key
        .iter()
        .filter_map(|(key, paths)| (paths.len() > 1).then_some(*key))
        .collect();
    shared.sort();
    if !shared.is_empty() {
        let listed = shared.iter().take(3).collect::<Vec<_>>();
        let mut keys = listed
            .iter()
            .map(|key| key.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if shared.len() > listed.len() {
            keys.push_str(", …");
        }
        println!(
            "identity: warn ({} shared by more than one PDF-backed note: {keys})",
            plural_noun(shared.len(), "identity key", "identity keys"),
        );
        warnings.push(format!(
            "{} shared by more than one PDF-backed note",
            plural_noun(shared.len(), "identity key", "identity keys"),
        ));
        return;
    }
    let superseded = rows
        .iter()
        .filter(|row| row.superseded_by.is_some())
        .count();
    if superseded == 0 {
        println!("identity: ok (no superseded notes)");
    } else if superseded == 1 {
        println!("identity: ok (1 legacy note superseded by a newer capture)");
    } else {
        println!(
            "identity: ok ({superseded} legacy notes superseded by newer captures)"
        );
    }
}

fn append_library_annotations_row(
    rows: &[crate::native::ref_library::RefRow],
    warnings: &mut Vec<String>,
) {
    let mirrors = rows
        .iter()
        .filter(|row| {
            row.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "marker_mirror_excluded")
        })
        .count();
    if mirrors == 0 {
        println!("annotations: ok (no leaked marker mirrors)");
    } else if mirrors == 1 {
        println!(
            "annotations: warn (1 note still renders a leaked marker mirror; the next bob ref scan removes it)"
        );
        warnings.push(
            "1 reference note still renders a leaked marker mirror".to_string(),
        );
    } else {
        println!(
            "annotations: warn ({mirrors} notes still render a leaked marker mirror; the next bob ref scan removes them)"
        );
        warnings.push(format!(
            "{mirrors} reference notes still render a leaked marker mirror"
        ));
    }
}

fn plural_notes(count: usize) -> String {
    plural_noun(count, "note", "notes")
}

fn plural_noun(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

pub(super) fn print_path_check(name: &str, path: &Path, ok: bool) {
    println!(
        "{name}: {} ({})",
        if ok { "ok" } else { "fail" },
        path.display()
    );
}

pub(super) fn validate_library_layout(config: &Config) -> Result<()> {
    if config.xlib_dir == config.lib_dir {
        return Err(CommandError::new(format!(
            "xlib_dir and lib_dir must be distinct: {}",
            config.lib_dir.display()
        )));
    }
    if config.xlib_dir.starts_with(&config.lib_dir) {
        return Err(CommandError::new(format!(
            "xlib_dir must not be inside lib_dir: {} is under {}",
            config.xlib_dir.display(),
            config.lib_dir.display()
        )));
    }
    if config.lib_dir.starts_with(&config.xlib_dir) {
        return Err(CommandError::new(format!(
            "lib_dir must not be inside xlib_dir: {} is under {}",
            config.lib_dir.display(),
            config.xlib_dir.display()
        )));
    }
    Ok(())
}

pub(super) fn plan_xlib_intake(config: &Config) -> Result<Vec<IntakeMove>> {
    if !config.xlib_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut pdfs = Vec::new();
    collect_pdf_paths_from_dir(&config.xlib_dir, &mut pdfs)?;
    pdfs.sort();

    let mut moves = Vec::new();
    let mut conflicts = Vec::new();
    let mut pdf_stems = BTreeSet::new();
    for source in pdfs {
        let relative =
            source.strip_prefix(&config.xlib_dir).map_err(|error| {
                CommandError::new(format!(
                    "derive xlib-relative path for {}: {error}",
                    source.display()
                ))
            })?;
        let destination = config.lib_dir.join(relative);
        let companions = intake_companion_moves(&source, &destination)?;
        let mut stem = relative.to_path_buf();
        stem.set_extension("");
        pdf_stems.insert(stem);

        if destination.exists() {
            conflicts.push((source.clone(), destination.clone()));
        }
        for (companion_source, companion_destination) in &companions {
            if companion_destination.exists() {
                conflicts.push((
                    companion_source.clone(),
                    companion_destination.clone(),
                ));
            }
        }

        moves.push(IntakeMove {
            source,
            destination,
            companions,
        });
    }

    let mut audio_files = Vec::new();
    collect_audio_paths_from_dir(&config.xlib_dir, &mut audio_files)?;
    audio_files.sort();
    for source in audio_files {
        let relative =
            source.strip_prefix(&config.xlib_dir).map_err(|error| {
                CommandError::new(format!(
                    "derive xlib-relative path for {}: {error}",
                    source.display()
                ))
            })?;
        let mut stem = relative.to_path_buf();
        stem.set_extension("");
        if pdf_stems.contains(&stem) {
            continue;
        }
        if library_pdf_for_xlib_audio(config, &source).is_none() {
            continue;
        }
        let destination = config.lib_dir.join(relative);
        if destination.exists() {
            conflicts.push((source.clone(), destination.clone()));
        }
        moves.push(IntakeMove {
            source,
            destination,
            companions: Vec::new(),
        });
    }

    if !conflicts.is_empty() {
        let mut message =
            String::from("xlib intake collision(s) detected before writes:");
        for (source, destination) in conflicts {
            message.push('\n');
            message.push_str("  ");
            message.push_str(&source.display().to_string());
            message.push_str(" -> ");
            message.push_str(&destination.display().to_string());
        }
        message.push_str(
            "\nremove or rename the existing library destination(s) before rerunning scan",
        );
        return Err(CommandError::new(message));
    }

    Ok(moves)
}

pub(super) fn intake_companion_moves(
    source: &Path,
    destination: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut companions = Vec::new();

    let markdown = source.with_extension("md");
    if markdown.is_file() {
        companions.push((markdown, destination.with_extension("md")));
    }

    let textbundle = source.with_extension("textbundle");
    if textbundle.exists() {
        if !textbundle.is_dir() {
            return Err(CommandError::new(format!(
                "unsupported sidecar {}: expected a .textbundle directory",
                textbundle.display()
            )));
        }
        let has_text_file = TEXTBUNDLE_TEXT_FILES
            .iter()
            .any(|file_name| textbundle.join(file_name).is_file());
        if !has_text_file {
            return Err(CommandError::new(format!(
                "unsupported textbundle sidecar {}: expected text.md or text.markdown",
                textbundle.display()
            )));
        }
        companions.push((textbundle, destination.with_extension("textbundle")));
    }

    for extension in AUDIO_COMPANION_EXTENSIONS {
        let audio = source.with_extension(extension);
        if audio.is_file() {
            companions.push((audio, destination.with_extension(extension)));
        }
    }

    Ok(companions)
}

pub(super) fn collect_orphan_audio(config: &Config) -> Result<Vec<PathBuf>> {
    let mut audio_files = Vec::new();
    collect_audio_paths_from_dir(&config.xlib_dir, &mut audio_files)?;
    audio_files.sort();
    Ok(audio_files
        .into_iter()
        .filter(|path| {
            !same_stem_pdf_exists(path)
                && library_pdf_for_xlib_audio(config, path).is_none()
        })
        .collect())
}

pub(super) fn execute_xlib_intake(moves: &[IntakeMove]) -> Result<()> {
    for intake_move in moves {
        execute_intake_move(&intake_move.source, &intake_move.destination)?;
        for (source, destination) in &intake_move.companions {
            execute_intake_move(source, destination)?;
        }
    }
    Ok(())
}

pub(super) fn execute_intake_move(
    source: &Path,
    destination: &Path,
) -> Result<()> {
    if let Some(parent) = destination.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::rename(source, destination).map_err(|error| {
        CommandError::new(format!(
            "move {} to {}: {error}",
            source.display(),
            destination.display()
        ))
    })
}

pub(super) fn collect_pdf_paths(
    config: &Config,
    include_xlib: bool,
) -> Result<Vec<PathBuf>> {
    if config.lib_dir.is_dir() {
        let mut paths = Vec::new();
        collect_pdf_paths_from_dir(&config.lib_dir, &mut paths)?;
        if include_xlib && config.xlib_dir.is_dir() {
            collect_pdf_paths_from_dir(&config.xlib_dir, &mut paths)?;
        }
        paths.sort();
        return Ok(paths);
    }

    if !include_xlib || !config.xlib_dir.is_dir() {
        return Err(CommandError::new(format!(
            "library directory does not exist or is not a directory: {}",
            config.lib_dir.display()
        )));
    }

    let mut paths = Vec::new();
    collect_pdf_paths_from_dir(&config.xlib_dir, &mut paths)?;
    paths.sort();
    Ok(paths)
}

pub(super) fn collect_pdf_paths_from_dir(
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
                collect_pdf_paths_from_dir(&path, paths)?;
            }
        } else if file_type.is_file() && is_pdf_path(&path) {
            paths.push(path);
        }
    }

    Ok(())
}

pub(super) fn should_skip_scan_dir(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == OsStr::new(".git"))
}

pub(super) fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}
