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
            "xlib intake directory does not exist: {} (bob highlights create creates it on demand)",
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

    Ok(companions)
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
