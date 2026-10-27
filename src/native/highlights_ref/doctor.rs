//! Vault doctor checks and xlib intake.
use super::intake_status;
use super::*;
use crate::native::{env as bob_env, ref_jobs::age_short_secs};
use std::time::SystemTime;

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

/// A queued `xlib/` PDF older than this makes `doctor` warn.
const XLIB_STALE_SECS: i64 = 3600;

/// Age of the oldest file by modification time; `None` when no mtime reads.
fn oldest_age_secs(paths: &[PathBuf]) -> Option<i64> {
    let now = SystemTime::now();
    paths
        .iter()
        .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
        .filter_map(|modified| now.duration_since(modified).ok())
        .map(|age| age.as_secs() as i64)
        .max()
}

/// The `intake_last:` row: the recorded success and failure, newest first.
fn intake_last_summary() -> String {
    let status = intake_status::read().unwrap_or_default();
    let age = |at: &str| {
        intake_status::age_secs(at)
            .map_or_else(|| "time unknown".to_string(), age_short_secs)
    };
    let mut parts = Vec::new();
    if let Some(ok) = &status.last_success {
        parts.push((
            ok.at.clone(),
            format!(
                "ok {} ago ({}, {} filed)",
                age(&ok.at),
                ok.command,
                ok.filed
            ),
        ));
    }
    if let Some(fail) = &status.last_failure {
        parts.push((
            fail.at.clone(),
            format!(
                "failure {} ago ({}: {})",
                age(&fail.at),
                fail.reason,
                one_line_message(&fail.message)
            ),
        ));
    }
    if parts.is_empty() {
        return "none recorded".to_string();
    }
    parts.sort_by(|a, b| b.0.cmp(&a.0));
    parts
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// First non-empty line of a recorded failure, trimmed and capped so the
/// doctor row stays on one line.
fn one_line_message(message: &str) -> String {
    let line = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    const MAX: usize = 120;
    let mut chars = line.chars();
    let truncated: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
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
    let mut xlib_oldest_secs = None;
    let xlib_pending = if layout_valid && config.xlib_dir.is_dir() {
        let mut pending = Vec::new();
        match collect_pdf_paths_from_dir(&config.xlib_dir, &mut pending) {
            Ok(()) => {
                xlib_oldest_secs = oldest_age_secs(&pending);
                pending.len()
            }
            Err(error) => {
                failures.push(error.to_string());
                0
            }
        }
    } else {
        0
    };
    match xlib_oldest_secs {
        Some(secs) if xlib_pending > 0 => {
            println!(
                "xlib_pending: {xlib_pending} · oldest {}",
                age_short_secs(secs)
            );
            if secs >= XLIB_STALE_SECS {
                warnings.push(format!(
                    "xlib intake: {xlib_pending} queued, oldest {} — run bob ref scan -i (the bob-ref-intake timer drains it on athena and apollo)",
                    age_short_secs(secs)
                ));
            }
        }
        _ => println!("xlib_pending: {xlib_pending}"),
    }
    println!("intake_last: {}", intake_last_summary());
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

    append_pdf_write_coordination_row(&mut warnings);

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

    super::render_tex::append_tex_doctor_rows(&mut warnings);

    // The fetcher checks `BOB_HIGHLIGHTS_CURL` first, so doctor does too.
    if let Some(override_curl) =
        bob_env::var_os(super::fetch::ENV_CURL_OVERRIDE)
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
    super::listen_feed::append_listen_feed_doctor_row(&mut warnings);
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

/// macOS reports `on`/`off`; elsewhere the key is accepted and inert.
pub(super) struct PdfWriteCoordinationDoctor {
    pub line: &'static str,
    pub warning: Option<&'static str>,
}

/// Both branches stay compile-visible on Linux via `cfg!`.
pub(super) fn pdf_write_coordination_doctor(
    coordinate: bool,
) -> PdfWriteCoordinationDoctor {
    if cfg!(target_os = "macos") {
        if coordinate {
            PdfWriteCoordinationDoctor {
                line: "on",
                warning: None,
            }
        } else {
            PdfWriteCoordinationDoctor {
                line: "off",
                warning: Some(
                    "highlights.coordinate_pdf_writes is off: marker writes replace PDFs open in Highlights without asking it to save first",
                ),
            }
        }
    } else {
        PdfWriteCoordinationDoctor {
            line: "n/a (no OS file coordination)",
            warning: None,
        }
    }
}

fn append_pdf_write_coordination_row(warnings: &mut Vec<String>) {
    let coordinate =
        match bob_config::load_highlights_config(&bob_config::config_path()) {
            Ok(config) => config.coordinate_pdf_writes(),
            Err(error) => {
                let message = match error {
                    bob_config::ConfigError::Read(message)
                    | bob_config::ConfigError::Invalid(message) => message,
                };
                println!("pdf_write_coordination: warn ({message})");
                warnings.push(format!(
                "highlights.coordinate_pdf_writes could not be read: {message}"
            ));
                return;
            }
        };
    let status = pdf_write_coordination_doctor(coordinate);
    println!("pdf_write_coordination: {}", status.line);
    if let Some(warning) = status.warning {
        warnings.push(warning.to_string());
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
    append_ref_tasks_row(&index, warnings);
    append_parents_row(&config.bob_dir, warnings);
    append_library_identity_row(&index.rows, warnings);
    append_reference_ids_row(&index.rows, warnings);
    append_library_annotations_row(&index.rows, warnings);
    append_ref_sources_row(config, &index.rows, warnings);

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
            if diagnostic.code == "marker_mirror_excluded"
                || crate::native::ref_tasks::REF_TASK_DIAGNOSTIC_CODES
                    .contains(&diagnostic.code.as_str())
            {
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

/// The `ref tasks` rollup: live/archived/open-v1 counts plus warn-level
/// ref-task codes in code order. `open_v1_tracker` is count-only and
/// expected until migration; it never warns.
fn append_ref_tasks_row(
    index: &crate::native::ref_library::RefIndex,
    warnings: &mut Vec<String>,
) {
    let rows = &index.rows;
    let live = rows
        .iter()
        .filter(|row| {
            row.task.as_ref().is_some_and(|task| {
                task.block_id.as_deref() != Some("ref") && !task.archived
            })
        })
        .count();
    let archived = rows
        .iter()
        .filter(|row| {
            row.task.as_ref().is_some_and(|task| {
                task.block_id.as_deref() != Some("ref") && task.archived
            })
        })
        .count();
    let open_v1 = rows
        .iter()
        .filter(|row| {
            row.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "open_v1_tracker")
        })
        .count();
    // Warn-level codes in REF_TASK_DIAGNOSTIC_CODES order, excluding the
    // count-only open_v1_tracker.
    let mut by_code: Vec<(&str, usize)> = Vec::new();
    let mut examples: Vec<String> = Vec::new();
    for code in crate::native::ref_tasks::REF_TASK_DIAGNOSTIC_CODES {
        if code == "open_v1_tracker" {
            continue;
        }
        if code == "orphan_ref_task" {
            let count = index.ref_tasks.orphans().len();
            if count > 0 {
                by_code.push((code, count));
                for orphan in index.ref_tasks.orphans().iter().take(3) {
                    if examples.len() < 3 {
                        examples.push(orphan.path.clone());
                    }
                }
            }
            continue;
        }
        let mut count = 0usize;
        for row in rows {
            if row
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code)
            {
                count += 1;
                if examples.len() < 3 {
                    examples.push(row.path.clone());
                }
            }
        }
        if count > 0 {
            by_code.push((code, count));
        }
    }
    let base = format!("{live} live · {archived} archived · {open_v1} open v1");
    if by_code.is_empty() {
        println!("ref tasks: ok ({base})");
        return;
    }
    let summary = by_code
        .iter()
        .map(|(code, count)| format!("{count} {code}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "ref tasks: warn ({base} · {summary} · e.g. {})",
        examples.join(", "),
    );
    let total: usize = by_code.iter().map(|(_, count)| count).sum();
    warnings.push(format!("{total} reading-task problems"));
}

/// The `parents` rollup: parent-note and alias counts, or the structural
/// alias problems collected by `capture_targets`.
fn append_parents_row(bob_dir: &Path, warnings: &mut Vec<String>) {
    let report = crate::native::capture_targets::scan_capture_targets(bob_dir);
    if report.alias_warnings.is_empty() {
        let notes = report.targets.len();
        let aliases: usize = report
            .targets
            .iter()
            .map(|target| target.project_name_aliases.len())
            .sum();
        println!("parents: ok ({notes} parent notes · {aliases} aliases)",);
        return;
    }
    let problems: Vec<String> = report
        .alias_warnings
        .iter()
        .map(|note| note.display())
        .collect();
    let shown = problems
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let mut line =
        format!("parents: warn ({} alias problems: {shown}", problems.len(),);
    if problems.len() > 3 {
        line.push_str("; …");
    }
    line.push(')');
    println!("{line}");
    warnings.push(format!("{} project_name_aliases problems", problems.len(),));
}

fn append_library_identity_row(
    rows: &[crate::native::ref_library::RefRow],
    warnings: &mut Vec<String>,
) {
    let mut by_key: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut by_path: BTreeMap<&str, &str> = BTreeMap::new();
    for row in rows.iter().filter(|row| row.source_pdf.is_some()) {
        by_path.insert(row.path.as_str(), row.family.as_str());
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
    // Keys shared by several PDF-backed notes — unless every note sharing
    // the key is in one reading family, in which case the lineage explains
    // the sharing and the key stays quiet.
    let mut shared: Vec<&str> = by_key
        .iter()
        .filter_map(|(key, paths)| {
            if paths.len() <= 1 {
                return None;
            }
            let mut families = BTreeSet::new();
            for path in paths {
                families.insert(by_path.get(path).copied().unwrap_or(path));
            }
            (families.len() > 1).then_some(*key)
        })
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

/// The `reference ids` rollup: frontmatter ids that differ from their note
/// stem (`id_stem_mismatch`) and ids recorded by more than one note
/// (`duplicate_ref_id`), both repaired through `bob ref rename` (phase
/// `ref-doctor-identity`). Like every library row it never fails the
/// command: problems are warnings.
fn append_reference_ids_row(
    rows: &[crate::native::ref_library::RefRow],
    warnings: &mut Vec<String>,
) {
    let mut mismatches: Vec<&str> = Vec::new();
    let mut duplicated: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for row in rows {
        let mut mismatch = false;
        let mut duplicate = false;
        for diagnostic in &row.diagnostics {
            match diagnostic.code.as_str() {
                "id_stem_mismatch" => mismatch = true,
                "duplicate_ref_id" => duplicate = true,
                _ => {}
            }
        }
        if mismatch {
            mismatches.push(row.path.as_str());
        }
        if duplicate {
            if let Some(id) = row.id.as_deref() {
                duplicated
                    .entry(id.to_ascii_lowercase())
                    .or_default()
                    .push(row.path.as_str());
            }
        }
    }
    let with_ids = rows.iter().filter(|row| row.id.is_some()).count();
    if mismatches.is_empty() && duplicated.is_empty() {
        if with_ids == 0 {
            println!("reference ids: ok (no recorded ids)");
        } else {
            println!(
                "reference ids: ok ({} {}, all match stems)",
                with_ids,
                if with_ids == 1 {
                    "note with an id"
                } else {
                    "notes with ids"
                },
            );
        }
        return;
    }
    let mut parts: Vec<String> = Vec::new();
    if !duplicated.is_empty() {
        let notes: usize = duplicated.values().map(Vec::len).sum();
        if duplicated.len() == 1 {
            let (key, _) = duplicated.iter().next().expect("one duplicate id");
            let recorded = rows
                .iter()
                .find_map(|row| {
                    row.id
                        .as_deref()
                        .filter(|id| id.to_ascii_lowercase() == *key)
                })
                .unwrap_or(key);
            parts.push(format!(
                "1 duplicate id {recorded:?} across {}",
                plural_notes(notes),
            ));
        } else {
            parts.push(format!(
                "{} duplicate ids across {}",
                duplicated.len(),
                plural_notes(notes),
            ));
        }
    }
    if !mismatches.is_empty() {
        parts.push(if mismatches.len() == 1 {
            "1 id/stem mismatch".to_string()
        } else {
            format!("{} id/stem mismatches", mismatches.len())
        });
    }
    let mut examples: Vec<&str> = Vec::new();
    for path in &mismatches {
        if examples.len() >= 3 {
            break;
        }
        examples.push(path);
    }
    if examples.len() < 3 {
        for paths in duplicated.values() {
            for path in paths {
                if examples.len() >= 3 {
                    break;
                }
                if !examples.contains(path) {
                    examples.push(path);
                }
            }
        }
    }
    println!(
        "reference ids: warn ({} · e.g. {} — repair with bob ref rename <REF> <NAME>)",
        parts.join(" · "),
        examples.join(", "),
    );
    warnings.push(format!(
        "{}; repair with bob ref rename <REF> <NAME>",
        parts.join(" · "),
    ));
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

fn append_ref_sources_row(
    config: &Config,
    rows: &[crate::native::ref_library::RefRow],
    warnings: &mut Vec<String>,
) {
    let health =
        super::source_store::inspect_source_stores(&config.bob_dir, rows);
    let links_hint = if health.without_links == 0 {
        String::new()
    } else {
        format!(
            " · {} without links — bob ref recatalog",
            format_count(health.without_links),
        )
    };
    let mut parts: Vec<String> = Vec::new();
    push_issue_part(&mut parts, health.dangling.len(), "dangling", "dangling");
    push_issue_part(
        &mut parts,
        health.digest_mismatch.len(),
        "digest mismatch",
        "digest mismatches",
    );
    push_issue_part(
        &mut parts,
        health.missing_catalog.len(),
        "missing catalog",
        "missing catalogs",
    );
    push_issue_part(&mut parts, health.orphans.len(), "orphan", "orphans");
    if health.ignored_catalogs > 0 {
        parts.push(if health.ignored_catalogs == 1 {
            "ignored catalog".to_string()
        } else {
            format!("{} ignored catalogs", health.ignored_catalogs)
        });
    }
    if parts.is_empty() {
        println!(
            "ref sources: ok ({} saved · {} code blocks · {} links{links_hint})",
            format_count(health.saved),
            format_count(health.code_blocks),
            format_count(health.links),
        );
        return;
    }
    let mut examples: Vec<String> = Vec::new();
    for issue in health
        .dangling
        .iter()
        .chain(health.digest_mismatch.iter())
        .chain(health.missing_catalog.iter())
        .chain(health.orphans.iter())
    {
        if examples.len() >= 3 {
            break;
        }
        let example = match issue {
            super::source_store::SourceStoreIssue::Dangling {
                note, ..
            }
            | super::source_store::SourceStoreIssue::DigestMismatch {
                note,
                ..
            }
            | super::source_store::SourceStoreIssue::MissingCatalog {
                note,
                ..
            } => note.clone(),
            super::source_store::SourceStoreIssue::Orphan { store } => {
                store.clone()
            }
        };
        if !examples.contains(&example) {
            examples.push(example);
        }
    }
    let mut hint = String::new();
    if health.ignored_catalogs > 0 {
        hint.push_str(" — ");
        hint.push_str(
            "add \"!/.bob/ref-sources/**/*.json\" (docs/vault-git-sync.md#reference-sources)",
        );
    }
    if !health.dangling.is_empty()
        || !health.digest_mismatch.is_empty()
        || !health.orphans.is_empty()
    {
        hint.push_str(" — bob ref attach-source");
    }
    let example_text = if examples.is_empty() {
        String::new()
    } else {
        format!(" · e.g. {}", examples.join(", "))
    };
    println!(
        "ref sources: warn ({}{example_text}{hint}{links_hint})",
        parts.join(" · "),
    );
    warnings.push(format!("reference sources: {}{hint}", parts.join(" · "),));
}

fn push_issue_part(
    parts: &mut Vec<String>,
    count: usize,
    singular: &str,
    plural: &str,
) {
    if count == 0 {
        return;
    }
    if count == 1 {
        parts.push(format!("1 {singular}"));
    } else {
        parts.push(format!("{count} {plural}"));
    }
}

fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
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
        let intake_move = plan_single_xlib_pdf_intake(config, &source)?;
        let unit = super::intake::IntakeUnit::Pdf {
            source: intake_move.source.clone(),
            destination: intake_move.destination.clone(),
        };
        let occupants = super::intake::recheck_reread_occupancy(
            config,
            &intake_move.destination,
            &super::intake::reread_own_files(&unit),
        )?;
        conflicts.extend(
            occupants
                .into_iter()
                .map(|occupant| (intake_move.source.clone(), occupant)),
        );
        let mut stem = intake_move
            .source
            .strip_prefix(&config.xlib_dir)
            .expect("single-source planner returns an xlib path")
            .to_path_buf();
        stem.set_extension("");
        pdf_stems.insert(stem);

        if intake_move.destination.exists() {
            conflicts.push((
                intake_move.source.clone(),
                intake_move.destination.clone(),
            ));
        }
        for (companion_source, companion_destination) in &intake_move.companions
        {
            if companion_destination.exists() {
                conflicts.push((
                    companion_source.clone(),
                    companion_destination.clone(),
                ));
            }
        }

        moves.push(intake_move);
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
        let mut destinations = Vec::new();
        for (source, destination) in conflicts {
            message.push('\n');
            message.push_str("  ");
            message.push_str(&source.display().to_string());
            message.push_str(" -> ");
            message.push_str(&destination.display().to_string());
            destinations.push(destination);
        }
        message.push_str(
            "\nremove or rename the existing library destination(s) before rerunning scan",
        );
        return Err(CommandError::new(message)
            .with_code("intake_collision")
            .with_paths(destinations));
    }

    Ok(moves)
}

/// Plan one xlib PDF and only its same-stem companions. Shared with the
/// create-time intake session so an unrelated queued item cannot block a
/// successful `bob ref create`.
pub(super) fn plan_single_xlib_pdf_intake(
    config: &Config,
    source: &Path,
) -> Result<IntakeMove> {
    let relative = source.strip_prefix(&config.xlib_dir).map_err(|error| {
        CommandError::new(format!(
            "derive xlib-relative path for {}: {error}",
            source.display()
        ))
    })?;
    let destination = config.lib_dir.join(relative);
    let companions = intake_companion_moves(source, &destination)?;
    Ok(IntakeMove {
        source: source.to_path_buf(),
        destination,
        companions,
    })
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

/// Move every planned intake pair, reporting how many top-level renames
/// completed before the first error alongside that error. A main rename
/// counts immediately, before companions are attempted, so an already
/// moved PDF is still reported when a companion fails.
pub(super) fn execute_xlib_intake_counting(
    moves: &[IntakeMove],
) -> (usize, Result<()>) {
    let mut moved = 0usize;
    for intake_move in moves {
        if let Err(error) =
            execute_intake_move(&intake_move.source, &intake_move.destination)
        {
            return (moved, Err(error));
        }
        moved += 1;
        for (source, destination) in &intake_move.companions {
            if let Err(error) = execute_intake_move(source, destination) {
                return (moved, Err(error));
            }
        }
    }
    (moved, Ok(()))
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

#[cfg(test)]
mod tests {
    use super::pdf_write_coordination_doctor;

    #[test]
    fn pdf_write_coordination_doctor_keeps_both_cfg_branches() {
        let on = pdf_write_coordination_doctor(true);
        let off = pdf_write_coordination_doctor(false);
        if cfg!(target_os = "macos") {
            assert_eq!(on.line, "on");
            assert_eq!(on.warning, None);
            assert_eq!(off.line, "off");
            assert_eq!(
                off.warning,
                Some(
                    "highlights.coordinate_pdf_writes is off: marker writes replace PDFs open in Highlights without asking it to save first",
                )
            );
        } else {
            assert_eq!(on.line, "n/a (no OS file coordination)");
            assert_eq!(on.warning, None);
            assert_eq!(off.line, "n/a (no OS file coordination)");
            assert_eq!(off.warning, None);
        }
    }
}
