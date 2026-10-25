//! Note snapshot print profile: pandoc-to-TeX then xelatex.
//!
//! Callable renderer for the later create-wiring phase. It turns already
//! serialized projected Markdown into a polished PDF, preserves links and
//! selectable text, and reports TeX failures and missing glyphs.
//!
//! Seam: `render_note_pdf(projected_md, scratch, filters, meta)` writes
//! `projected.md`, profile assets, `fonts/`, and `glyphs.lua` into the
//! caller-owned scratch directory, runs pandoc then xelatex with cwd=scratch,
//! and returns the scratch PDF path, typed warnings, and the return-link
//! report. It never stamps, installs, or creates the scratch directory; the
//! caller's `ScratchDir` keeps cleanup/debug ownership.
use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::super::{CommandError, Result};
use super::fonts::GlyphCoverage;

/// Embedded TeX header (distinct name `note-profile.tex` in scratch; the
/// pandoc-generated intermediate is `note.tex`).
pub(crate) const NOTE_HEADER: &str = include_str!("note.tex");
/// Embedded Lua contract filter (written as `note.lua` in scratch).
pub(crate) const NOTE_FILTER: &str = include_str!("note.lua");

/// Snapshot-route pandoc reader. Shared with the diagrams collect pass
/// so those two invocations cannot drift.
pub(crate) const SNAPSHOT_PANDOC_READER: &str =
    "commonmark_x-tex_math_dollars-emoji";
/// Text box for budgeted scale warnings. Matches
/// `-V geometry:top=0.9in,bottom=1in,left=1.1in,right=1.1in,letterpaper`
/// in [`render_note_pdf`]: `612 - 2×1.1×72` by `792 - (0.9+1)×72`.
const SNAPSHOT_LETTER_WIDTH_PT: f64 = 612.0;
const SNAPSHOT_LETTER_HEIGHT_PT: f64 = 792.0;
const SNAPSHOT_PT_PER_IN: f64 = 72.0;
const SNAPSHOT_SIDE_IN: f64 = 1.1;
const SNAPSHOT_TOP_IN: f64 = 0.9;
const SNAPSHOT_BOTTOM_IN: f64 = 1.0;
pub(crate) const SNAPSHOT_DIAGRAM_TEXT_WIDTH_PT: f64 =
    SNAPSHOT_LETTER_WIDTH_PT - 2.0 * SNAPSHOT_SIDE_IN * SNAPSHOT_PT_PER_IN;
pub(crate) const SNAPSHOT_DIAGRAM_TEXT_HEIGHT_PT: f64 =
    SNAPSHOT_LETTER_HEIGHT_PT
        - (SNAPSHOT_TOP_IN + SNAPSHOT_BOTTOM_IN) * SNAPSHOT_PT_PER_IN;
const ENV_XELATEX_COMMAND: &str = "BOB_XELATEX_COMMAND";
const GENERATED_TEX: &str = "note.tex";
const GENERATED_PDF: &str = "note.pdf";
const HEADER_NAME: &str = "note-profile.tex";
const FILTER_NAME: &str = "note.lua";
const PROJECTED_NAME: &str = "projected.md";
const GLYPHS_NAME: &str = "glyphs.lua";

/// Identity and vault context for PDF Info and the listen filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteMeta {
    /// Identity title (`Display · YYYY-MM-DD`) for PDF Info Title.
    pub(crate) identity_title: String,
    /// Vault-relative source path (e.g. `harbor_cranes.md`) for Subject.
    pub(crate) source_path: String,
    /// Note kind (`area`, `project`, or `note`) for Keywords.
    pub(crate) kind: String,
    /// Optional listen URI for the existing code-break/listen filter.
    pub(crate) listen_uri: Option<String>,
}

/// Typed render warnings (deduplicated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteWarning {
    /// A `Missing character` line from the xelatex log.
    MissingGlyph(String),
    /// An `Overfull` box wider than 20pt.
    Overfull(String),
}

impl NoteWarning {
    /// Human-readable warning line.
    pub(crate) fn message(&self) -> String {
        match self {
            NoteWarning::MissingGlyph(detail) => {
                format!("missing glyph: {detail}")
            }
            NoteWarning::Overfull(detail) => {
                format!("overfull: {detail}")
            }
        }
    }
}

/// What the two-step render produced.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenderOutcome {
    /// Scratch PDF path (`<scratch>/note.pdf`).
    pub(crate) pdf: PathBuf,
    /// Deduplicated `Missing character` and `>20pt Overfull` warnings.
    pub(crate) warnings: Vec<NoteWarning>,
    /// Return-link report for create-wiring to stamp its marker correctly.
    pub(crate) report: super::super::return_links::ReportOutcome,
}

/// Render projected Markdown to PDF via pandoc then xelatex.
///
/// `scratch` is the caller-owned work directory (already created). This
/// writes `projected.md`, `note-profile.tex`, `note.lua`, `fonts/`, and
/// `glyphs.lua`, runs pandoc in scratch to `note.tex`, then xelatex (twice,
/// plus a bounded third pass when the second log requests `Rerun`) to
/// `note.pdf`. It returns the PDF path, typed warnings, and the return-link
/// report without stamping or installing.
pub(crate) fn render_note_pdf(
    projected_md: &str,
    scratch: &Path,
    filters: &super::super::create::RenderFilters,
    meta: &NoteMeta,
) -> Result<RenderOutcome> {
    let projected_path = scratch.join(PROJECTED_NAME);
    fs::write(&projected_path, projected_md).map_err(|error| {
        CommandError::new(format!(
            "write {}: {error}",
            projected_path.display()
        ))
    })?;
    let header_path = scratch.join(HEADER_NAME);
    fs::write(&header_path, NOTE_HEADER).map_err(|error| {
        CommandError::new(format!("write {}: {error}", header_path.display()))
    })?;
    let note_filter_path = scratch.join(FILTER_NAME);
    fs::write(&note_filter_path, NOTE_FILTER).map_err(|error| {
        CommandError::new(format!(
            "write {}: {error}",
            note_filter_path.display()
        ))
    })?;
    GlyphCoverage::write_fonts(&scratch.join("fonts")).map_err(|error| {
        CommandError::new(format!("write scratch fonts/: {error}"))
    })?;
    let coverage = GlyphCoverage::new().map_err(|error| {
        CommandError::new(format!("read bundled fonts: {error}"))
    })?;
    coverage
        .write_glyph_table(&scratch.join(GLYPHS_NAME))
        .map_err(|error| {
            CommandError::new(format!("write scratch {GLYPHS_NAME}: {error}"))
        })?;

    let pandoc = super::super::create::pandoc_command().ok_or_else(|| {
        CommandError::new(
            "pandoc command not found; install pandoc or set BOB_PANDOC_COMMAND",
        )
    })?;
    let xelatex = xelatex_command().ok_or_else(|| {
        CommandError::new(
            "xelatex command not found; install a TeX distribution that provides xelatex (e.g. TinyTeX), then run `bob ref doctor`",
        )
    })?;
    // Resolve relative executable/filter/report paths before switching cwd
    // so cwd=scratch still finds them. Bare names stay for PATH lookup.
    // Every argument stays a single Command arg (no shell), so spaces are
    // preserved.
    let pandoc_path = resolve_for_cwd(&pandoc);
    let xelatex_path = resolve_for_cwd(&xelatex);
    let code_break = resolve_path_for_cwd(&filters.code_break);
    let return_links = resolve_path_for_cwd(&filters.return_links);
    let report = resolve_path_for_cwd(&filters.report);

    let tex_path = scratch.join(GENERATED_TEX);
    let mut pandoc_cmd = Command::new(&pandoc_path);
    pandoc_cmd
        .arg(&projected_path)
        .arg("-f")
        .arg(SNAPSHOT_PANDOC_READER)
        .arg("-t")
        .arg("latex")
        .arg("-s")
        .arg("-o")
        .arg(&tex_path);
    if let Some(diagrams) = &filters.diagrams {
        super::super::diagrams::apply_swap_filter_args(
            &mut pandoc_cmd,
            &resolve_path_for_cwd(&diagrams.filter),
            &resolve_path_for_cwd(&diagrams.results),
        );
    }
    pandoc_cmd
        .arg("--lua-filter")
        .arg(&code_break)
        .arg("--lua-filter")
        .arg(&return_links)
        .arg("--lua-filter")
        .arg(&note_filter_path)
        .arg("--include-in-header")
        .arg(&header_path)
        .arg("--highlight-style=tango")
        .arg("-V")
        .arg("colorlinks=true")
        .arg("-V")
        .arg("linkcolor=BobLinkInk")
        .arg("-V")
        .arg("urlcolor=BobLinkInk")
        .arg("-V")
        .arg("filecolor=BobLinkInk")
        .arg("-V")
        .arg("citecolor=BobLinkInk")
        .arg("-V")
        .arg("geometry:top=0.9in,bottom=1in,left=1.1in,right=1.1in,letterpaper")
        .arg("-V")
        .arg(format!(
            "header-includes={}\n{}\n{}",
            super::super::create::PANDOC_HEADER_INCLUDES,
            super::super::return_links::HEADER_INCLUDES,
            super::super::diagrams::HEADER_INCLUDES
        ))
        .arg("--metadata")
        .arg(format!("title={}", meta.identity_title))
        .arg("--metadata")
        .arg(format!("subject=Snapshot of {}", meta.source_path))
        .arg("--metadata")
        .arg(format!("keywords=bob,snapshot,{}", meta.kind))
        .arg("--metadata")
        .arg("creator=bob ref create (note snapshot)")
        .arg("--metadata")
        .arg(format!(
            "{}={}",
            super::super::return_links::REPORT_METADATA_KEY,
            report.display()
        ))
        .arg("--metadata")
        .arg(format!("bob-glyph-table={GLYPHS_NAME}"));
    if let Some(uri) = &meta.listen_uri {
        pandoc_cmd
            .arg("--metadata")
            .arg(format!("bob-listen-uri={uri}"));
    }
    pandoc_cmd.current_dir(scratch);
    let pandoc_out = pandoc_cmd.output().map_err(|error| {
        CommandError::new(format!(
            "run pandoc command {}: {error}",
            pandoc_path.display()
        ))
    })?;
    if !pandoc_out.status.success() {
        let stderr = String::from_utf8_lossy(&pandoc_out.stderr)
            .trim()
            .to_string();
        let stdout = String::from_utf8_lossy(&pandoc_out.stdout)
            .trim()
            .to_string();
        let detail = match (stderr.is_empty(), stdout.is_empty()) {
            (false, false) => format!("{stderr}\n{stdout}"),
            (false, true) => stderr,
            (true, false) => stdout,
            (true, true) => "pandoc produced no diagnostic output".to_string(),
        };
        let base = format!(
            "pandoc failed while rendering {} (exit {}):\n{}",
            projected_path.display(),
            pandoc_out
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            detail
        );
        if let Some(hint) =
            super::super::render_tex::render_failure_hint(&detail)
        {
            return Err(CommandError::new(format!("{base}\nhint: {hint}")));
        }
        return Err(CommandError::new(base));
    }

    let pdf_path = scratch.join(GENERATED_PDF);
    let mut last_log = String::new();
    let mut last_stdout = String::new();
    let mut last_stderr = String::new();
    let mut passes = 0;
    for pass in 1..=3 {
        let out = Command::new(&xelatex_path)
            .arg("-interaction=nonstopmode")
            .arg("-halt-on-error")
            .arg(GENERATED_TEX)
            .current_dir(scratch)
            .output()
            .map_err(|error| {
                CommandError::new(format!(
                    "run xelatex command {}: {error}",
                    xelatex_path.display()
                ))
            })?;
        passes = pass;
        last_stdout = String::from_utf8_lossy(&out.stdout).to_string();
        last_stderr = String::from_utf8_lossy(&out.stderr).to_string();
        last_log =
            fs::read_to_string(scratch.join("note.log")).unwrap_or_default();
        if !out.status.success() {
            return Err(xelatex_failure(
                &out,
                &last_log,
                &last_stdout,
                &last_stderr,
                pass,
            ));
        }
        if pass == 1 {
            continue;
        }
        if pass == 2 && last_log.contains("Rerun") {
            continue;
        }
        break;
    }
    let _ = passes;
    if !pdf_path.is_file() {
        return Err(xelatex_failure_no_output(
            &last_log,
            &last_stdout,
            &last_stderr,
        ));
    }
    let warnings = collect_warnings(&last_log);
    let report_outcome =
        super::super::return_links::read_report(&filters.report);
    Ok(RenderOutcome {
        pdf: pdf_path,
        warnings,
        report: report_outcome,
    })
}

/// Discover xelatex through `BOB_XELATEX_COMMAND` then PATH, mirroring the
/// `BOB_PANDOC_COMMAND` override conventions.
fn xelatex_command() -> Option<OsString> {
    if let Some(command) = crate::native::env::var_os(ENV_XELATEX_COMMAND)
        .filter(|value| !value.is_empty())
    {
        return Some(command);
    }
    crate::native::env::find_on_path("xelatex").map(PathBuf::into_os_string)
}

/// Resolve an executable for a cwd=scratch Command: bare names stay for PATH
/// lookup, while relative paths with a separator become absolute via the
/// current directory before the switch.
fn resolve_for_cwd(command: &OsStr) -> PathBuf {
    let path = Path::new(command);
    if path.components().count() > 1 && path.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join(path);
        }
    }
    path.to_path_buf()
}

/// Resolve a file path for cwd=scratch: relative paths become absolute via
/// the current directory; absolute paths pass through.
fn resolve_path_for_cwd(path: &Path) -> PathBuf {
    if path.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join(path);
        }
    }
    path.to_path_buf()
}

/// Typed, deduplicated warnings from a xelatex log: `Missing character` and
/// `Overfull` wider than 20pt (20pt and smaller do not warn).
fn collect_warnings(log: &str) -> Vec<NoteWarning> {
    use std::collections::BTreeSet;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in log.lines() {
        if line.contains("Missing character") {
            let key = format!("missing:{line}");
            if seen.insert(key) {
                out.push(NoteWarning::MissingGlyph(line.trim().to_string()));
            }
        } else if line.contains("Overfull") {
            if let Some(width) = overfull_width_pt(line) {
                if width > 20.0 {
                    let key = format!("overfull:{line}");
                    if seen.insert(key) {
                        out.push(NoteWarning::Overfull(
                            line.trim().to_string(),
                        ));
                    }
                }
            }
        }
    }
    out
}

/// Parse `Overfull \hbox (12.3pt too wide)` width, if present.
fn overfull_width_pt(line: &str) -> Option<f64> {
    let start = line.find('(')?;
    let rest = line.get(start + 1..)?;
    let end = rest.find("pt")?;
    rest.get(..end)?.trim().parse::<f64>().ok()
}

/// Failure after a failed xelatex pass: first `!` diagnostic plus ten context
/// lines, exit information, and a missing-package hint when applicable. Falls
/// back to captured stderr/stdout when the log has no useful diagnostic.
fn xelatex_failure(
    output: &std::process::Output,
    log: &str,
    stdout: &str,
    stderr: &str,
    pass: u32,
) -> CommandError {
    let excerpt = first_error_excerpt(log).unwrap_or_else(|| {
        let combined = format!("{stderr}\n{stdout}");
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            "xelatex produced no diagnostic output".to_string()
        } else {
            trimmed.to_string()
        }
    });
    let base = format!(
        "xelatex failed on pass {pass} (exit {}):\n{}",
        output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |code| code.to_string()),
        excerpt
    );
    if let Some(hint) = super::super::render_tex::render_failure_hint(&excerpt)
    {
        return CommandError::new(format!("{base}\nhint: {hint}"));
    }
    CommandError::new(base)
}

/// Success exit but no PDF: same excerpt/hint handling without pass info.
fn xelatex_failure_no_output(
    log: &str,
    stdout: &str,
    stderr: &str,
) -> CommandError {
    let excerpt = first_error_excerpt(log).unwrap_or_else(|| {
        let combined = format!("{stderr}\n{stdout}");
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            "xelatex succeeded but note.pdf is missing with no diagnostic output"
                .to_string()
        } else {
            format!("xelatex succeeded but note.pdf is missing:\n{trimmed}")
        }
    });
    if let Some(hint) = super::super::render_tex::render_failure_hint(&excerpt)
    {
        return CommandError::new(format!("{excerpt}\nhint: {hint}"));
    }
    CommandError::new(excerpt)
}

/// First `!` line plus ten following context lines from the log, if any.
fn first_error_excerpt(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.iter().position(|line| line.starts_with('!'))?;
    let end = (start + 11).min(lines.len());
    Some(lines[start..end].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Note-profile renders need the packages used by the complete Markdown
    /// render. Read the same declaration as `bob ref doctor`; keep this test
    /// gate from growing a second package list.
    fn missing_note_profile_packages(
    ) -> std::result::Result<Vec<String>, String> {
        let xelatex = crate::native::env::find_on_path("xelatex")
            .ok_or_else(|| "xelatex is required".to_string())?;
        let beside_xelatex =
            xelatex.parent().map(|dir| dir.join("kpsewhich")).filter(
                |candidate| crate::native::env::is_executable_file(candidate),
            );
        let kpsewhich = beside_xelatex
            .or_else(|| crate::native::env::find_on_path("kpsewhich"))
            .ok_or_else(|| {
                "kpsewhich is required to check TeX packages".to_string()
            })?;
        let packages = super::super::super::render_tex::RENDER_TEX_PACKAGES;
        let files: Vec<&str> =
            packages.iter().map(|package| package.file).collect();
        let output = std::process::Command::new(kpsewhich)
            .args(&files)
            .output()
            .map_err(|error| format!("kpsewhich failed: {error}"))?;
        let found: BTreeSet<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                std::path::Path::new(line.trim())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .collect();
        Ok(packages
            .iter()
            .filter(|package| !found.contains(package.file))
            .map(|package| package.file.to_string())
            .collect())
    }

    fn skip_if_missing_note_profile_packages(test_name: &str) -> bool {
        match missing_note_profile_packages() {
            Ok(missing) if missing.is_empty() => false,
            Ok(missing) => {
                eprintln!(
                    "skipping {test_name}: missing TeX packages: {}",
                    missing.join(", ")
                );
                true
            }
            Err(reason) => {
                eprintln!("skipping {test_name}: {reason}");
                true
            }
        }
    }

    fn write_executable(path: &Path, body: &str) {
        // Fork-safe scratch-then-copy writer: an in-place `fs::write`
        // leaves a writable fd a concurrently forked test thread inherits,
        // so exec-ing the stub can fail with ETXTBSY (bead `bob-cli-5l`).
        crate::native::test_stub::write_executable_stub(path, body);
    }

    fn test_meta() -> NoteMeta {
        NoteMeta {
            identity_title: "Harbor Cranes · 2026-10-16".to_string(),
            source_path: "harbor_cranes.md".to_string(),
            kind: "project".to_string(),
            listen_uri: None,
        }
    }

    fn test_filters(dir: &Path) -> super::super::super::create::RenderFilters {
        let code_break = dir.join("code-break.lua");
        let return_links = dir.join("return-links.lua");
        fs::write(&code_break, "-- fake code break").expect("write filter");
        fs::write(&return_links, "-- fake return links").expect("write filter");
        super::super::super::create::RenderFilters {
            code_break,
            return_links,
            report: dir.join("return-links.json"),
            diagrams: None,
        }
    }

    #[test]
    fn warnings_respect_the_20pt_boundary_and_deduplicate() {
        let log = "\
Missing character: There is no X in font serif!\n\
Missing character: There is no X in font serif!\n\
Overfull \\hbox (20pt too wide) in paragraph\n\
Overfull \\hbox (20.0pt too wide) in paragraph\n\
Overfull \\hbox (20.1pt too wide) in paragraph\n\
Overfull \\hbox (20.1pt too wide) in paragraph\n";
        let warnings = collect_warnings(log);
        assert_eq!(
            warnings.len(),
            2,
            "one missing + one overfull: {warnings:?}"
        );
        assert!(matches!(warnings[0], NoteWarning::MissingGlyph(_)));
        assert!(matches!(warnings[1], NoteWarning::Overfull(_)));
        assert!(warnings[1].message().contains("20.1pt"));
    }

    #[test]
    fn first_error_excerpt_takes_ten_context_lines() {
        let mut log = String::from("preamble\n");
        log.push_str("! LaTeX Error: File `titlesec.sty' not found.\n");
        for i in 1..=15 {
            log.push_str(&format!("context {i}\n"));
        }
        let excerpt = first_error_excerpt(&log).expect("excerpt");
        let lines: Vec<&str> = excerpt.lines().collect();
        assert_eq!(lines.len(), 11, "first ! plus ten: {excerpt}");
        assert!(lines[0].contains("titlesec.sty"));
        assert!(lines[10].contains("context 10"));
    }

    #[test]
    fn fake_commands_verify_flags_cwd_order_and_two_passes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let calls = temp.path().join("calls.log");
        let fake_pandoc = bin.join("fake-pandoc");
        write_executable(
            &fake_pandoc,
            &format!(
                "#!/bin/sh\necho \"pandoc-call cwd=$(pwd)\" >> \"{}\"\nfor a in \"$@\"; do echo \"PANDOC-ARG:$a\" >> \"{}\"; done\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\nprintf '\\\\documentclass{{article}}\\\\begin{{document}}Hi\\\\end{{document}}\\n' > \"$out\"\nfor a in \"$@\"; do case \"$a\" in *return-links.json*) printf '{{\"version\":1,\"enabled\":true,\"prefix\":\"bob:ret:\",\"paired\":0,\"targets\":0,\"untagged\":0,\"github\":0,\"dead\":[],\"duplicates\":[]}}' > \"$a\";; esac; done\nexit 0\n",
                calls.display(),
                calls.display()
            ),
        );
        let fake_xelatex = bin.join("fake-xelatex");
        write_executable(
            &fake_xelatex,
            &format!(
                "#!/bin/sh\necho \"xelatex-call cwd=$(pwd) args=$*\" >> \"{}\"\nprintf 'fake log\\n' > note.log\nprintf 'fake pdf' > note.pdf\nexit 0\n",
                calls.display()
            ),
        );
        let filters = test_filters(temp.path());
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("BOB_PANDOC_COMMAND", Some(fake_pandoc.as_os_str())),
            ("BOB_XELATEX_COMMAND", Some(fake_xelatex.as_os_str())),
        ]);
        let outcome =
            render_note_pdf("# Hi\n", &scratch, &filters, &test_meta())
                .expect("fake render");
        assert_eq!(outcome.pdf, scratch.join("note.pdf"));
        assert!(scratch.join("projected.md").is_file());
        assert!(scratch.join("note-profile.tex").is_file());
        assert!(scratch.join("note.lua").is_file());
        assert!(scratch.join("fonts").is_dir());
        assert!(scratch.join("glyphs.lua").is_file());
        assert!(scratch.join("note.tex").is_file());
        let log = fs::read_to_string(&calls).expect("calls log");
        assert!(log.contains("pandoc-call"), "pandoc ran: {log}");
        assert!(
            log.contains("commonmark_x-tex_math_dollars-emoji"),
            "reader flags: {log}"
        );
        assert!(log.contains("--lua-filter"), "filters: {log}");
        assert!(
            !log.lines().any(|l| l == "PANDOC-ARG:--toc"),
            "no TOC: {log}"
        );
        assert!(
            !log.lines().any(|l| l == "PANDOC-ARG:--number-sections"),
            "no numbering: {log}"
        );
        assert!(
            !log.lines().any(|l| l == "PANDOC-ARG:--pdf-engine"),
            "no pdf-engine: {log}"
        );
        // Filter order: code-break, return-links, note.lua.
        let cb = log.find("code-break.lua").expect("code-break");
        let rl = log.find("return-links.lua").expect("return-links");
        // The note filter path ends with scratch/note.lua; avoid matching
        // return-links.lua's directory prefix by searching for the trailing
        // scratch filter name explicitly.
        let nl = log
            .find("scratch/note.lua")
            .or_else(|| log.find("/note.lua"))
            .expect("note.lua");
        assert!(cb < rl && rl < nl, "filter order: {log}");
        // Two xelatex passes, same cwd=scratch, no shell splitting.
        let xelatex_calls: Vec<&str> = log
            .lines()
            .filter(|l| l.starts_with("xelatex-call"))
            .collect();
        assert_eq!(xelatex_calls.len(), 2, "two passes: {log}");
        for call in &xelatex_calls {
            assert!(
                call.contains(&scratch.display().to_string()),
                "cwd=scratch: {call}"
            );
            assert!(
                call.contains("-interaction=nonstopmode -halt-on-error"),
                "{call}"
            );
        }
    }

    #[test]
    fn spaces_in_paths_remain_single_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch dir");
        fs::create_dir_all(&scratch).expect("scratch with space");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let probe = temp.path().join("args.log");
        let fake_pandoc = bin.join("fake pandoc");
        write_executable(
            &fake_pandoc,
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do echo \"ARG:$a\" >> \"{}\"; done\nout=\"\"; prev=\"\"; for a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\nprintf 'x' > \"$out\"\nexit 0\n",
                probe.display()
            ),
        );
        let fake_xelatex = bin.join("fake-xelatex");
        write_executable(
            &fake_xelatex,
            "#!/bin/sh\nprintf 'x' > note.pdf\nprintf 'log' > note.log\nexit 0\n",
        );
        let filters = test_filters(temp.path());
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("BOB_PANDOC_COMMAND", Some(fake_pandoc.as_os_str())),
            ("BOB_XELATEX_COMMAND", Some(fake_xelatex.as_os_str())),
        ]);
        render_note_pdf("# Hi\n", &scratch, &filters, &test_meta())
            .expect("render");
        let logged = fs::read_to_string(&probe).expect("args log");
        assert!(
            logged.lines().any(|l| l.contains("scratch dir")),
            "space path stays one arg: {logged}"
        );
    }

    #[test]
    fn third_pass_runs_only_on_rerun_and_stops_on_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let count_file = temp.path().join("count");
        fs::write(&count_file, "0").expect("count");
        let fake_pandoc = bin.join("fake-pandoc");
        write_executable(
            &fake_pandoc,
            "#!/bin/sh\nout=\"\"; prev=\"\"; for a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\nprintf 'x' > \"$out\"\nexit 0\n",
        );
        // Fails on pass 2, should stop (no pass 3) and report the ! excerpt.
        let fake_xelatex = bin.join("fake-xelatex");
        write_executable(
            &fake_xelatex,
            &format!(
                "#!/bin/sh\nn=$(cat \"{}\"); n=$((n+1)); echo \"$n\" > \"{}\"\nif [ \"$n\" = \"2\" ]; then printf '! Fake error\\nline2\\n' > note.log\nexit 1\nfi\nprintf 'ok\\n' > note.log\nprintf 'pdf' > note.pdf\nexit 0\n",
                count_file.display(),
                count_file.display()
            ),
        );
        let filters = test_filters(temp.path());
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("BOB_PANDOC_COMMAND", Some(fake_pandoc.as_os_str())),
            ("BOB_XELATEX_COMMAND", Some(fake_xelatex.as_os_str())),
        ]);
        let err = render_note_pdf("# Hi\n", &scratch, &filters, &test_meta())
            .expect_err("pass 2 fails");
        assert!(
            err.to_string().contains("xelatex failed on pass 2"),
            "{err}"
        );
        assert!(err.to_string().contains("! Fake error"), "{err}");
        assert_eq!(fs::read_to_string(&count_file).expect("count").trim(), "2");
    }

    #[test]
    fn missing_package_hint_and_absent_log_fallback() {
        // Hint path: log with a missing .sty error.
        let log =
            "! LaTeX Error: File `titlesec.sty' not found.\nType X to quit.\n";
        let output =
            std::process::Command::new("false").output().expect("false");
        let err = xelatex_failure(&output, log, "", "", 1);
        assert!(err.to_string().contains("titlesec.sty"), "{err}");
        assert!(err.to_string().contains("hint:"), "{err}");
        assert!(err.to_string().contains("tlmgr install titlesec"), "{err}");
        // Absent log: fall back to stderr/stdout.
        let err2 = xelatex_failure(&output, "", "out-diag", "err-diag", 1);
        assert!(err2.to_string().contains("err-diag"), "{err2}");
        let err3 = xelatex_failure_no_output("", "", "");
        assert!(err3.to_string().contains("no diagnostic output"), "{err3}");
    }

    fn contract_projected_md() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/note_snapshot/contract.projected.md");
        std::fs::read_to_string(&path).expect("contract fixture")
    }

    fn write_filters_for_render(
        dir: &Path,
    ) -> super::super::super::create::RenderFilters {
        let code_break = dir.join("code-break.lua");
        let return_links = dir.join("return-links.lua");
        fs::write(
            &code_break,
            super::super::super::create::PANDOC_CODE_BREAK_FILTER,
        )
        .expect("code-break filter");
        fs::write(&return_links, super::super::super::return_links::FILTER)
            .expect("return-link filter");
        super::super::super::create::RenderFilters {
            code_break,
            return_links,
            report: dir.join("return-links.json"),
            diagrams: None,
        }
    }

    fn stage_synthetic_image(scratch: &Path) {
        // 1x1 RGB PNG, staged for the fixture's assets/... reference.
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00,
            0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE,
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63,
            0x38, 0xD3, 0xD1, 0x01, 0x00, 0x04, 0x00, 0x01, 0xDD, 0x34, 0xAF,
            0x0D, 0xFB, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
            0x42, 0x60, 0x82,
        ];
        let assets = scratch.join("assets");
        fs::create_dir_all(&assets).expect("assets dir");
        fs::write(assets.join("quay_crane.png"), PNG).expect("synthetic image");
    }

    #[test]
    fn pandoc_maps_every_contract_construct_to_tex() {
        let Some(pandoc) = super::super::super::create::pandoc_command() else {
            eprintln!("skipping contract AST test: pandoc is required");
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        // Minimal glyph table: fallback dice, per-role ▸ to mono.
        fs::write(
            scratch.join("glyphs.lua"),
            "return { [0x1F3B2] = true, serif_uncovered = { [0x25B8] = true, }, sans_uncovered = { [0x25B8] = true, }, mono_uncovered = { [0x1F3B2] = true, }, }\n",
        )
        .expect("glyphs");
        let header = scratch.join("note-profile.tex");
        fs::write(&header, NOTE_HEADER).expect("header");
        let note_filter = scratch.join("note.lua");
        fs::write(&note_filter, NOTE_FILTER).expect("note filter");
        let code_break = scratch.join("code-break.lua");
        fs::write(
            &code_break,
            super::super::super::create::PANDOC_CODE_BREAK_FILTER,
        )
        .expect("code-break");
        let return_filter = scratch.join("return-links.lua");
        fs::write(&return_filter, super::super::super::return_links::FILTER)
            .expect("return-links");
        let source = scratch.join("contract.projected.md");
        let projected = super::super::serialize_projected_markdown(
            &super::super::tests::contract_view(),
        )
        .expect("serialize complete contract");
        assert_eq!(projected, contract_projected_md());
        fs::write(&source, &projected).expect("fixture");
        let out = scratch.join("contract.tex");
        let output = std::process::Command::new(&pandoc)
            .arg(&source)
            .arg("-f")
            .arg(SNAPSHOT_PANDOC_READER)
            .arg("-t")
            .arg("latex")
            .arg("-s")
            .arg("-o")
            .arg(&out)
            .arg("--lua-filter")
            .arg(&code_break)
            .arg("--lua-filter")
            .arg(&return_filter)
            .arg("--lua-filter")
            .arg(&note_filter)
            .arg("--include-in-header")
            .arg(&header)
            .arg("--metadata")
            .arg(format!(
                "bob-return-links-report={}",
                scratch.join("report.json").display()
            ))
            .arg("--metadata")
            .arg("bob-glyph-table=glyphs.lua")
            .current_dir(&scratch)
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "pandoc failed: {output:?}");
        let tex = fs::read_to_string(&out).expect("read TeX");
        for status in ["Open", "Next", "Wip", "Blocked", "Done", "Canceled"] {
            assert!(
                tex.contains(&format!("\\BobTask{{{status}}}")),
                "status {status}: {tex}"
            );
        }
        let open_task_start =
            tex.find("\\BobTask{Open}").expect("open task macro");
        let open_task_paragraph = tex[open_task_start..]
            .split("\n\n")
            .next()
            .expect("open task paragraph");
        assert!(
            open_task_paragraph.contains("Open task"),
            "task text follows its status without a blank paragraph: {open_task_paragraph}"
        );
        let document_start =
            tex.find("\\begin{document}").expect("document body")
                + "\\begin{document}".len();
        let document_tex = &tex[document_start..];
        let image_begin =
            document_tex.find("\\BobImageBegin{}").expect("image begin");
        let image = document_tex.find("\\includegraphics").expect("AST image");
        let image_end =
            document_tex.find("\\BobImageEnd{}").expect("image end");
        assert!(
            image_begin < image && image < image_end,
            "fixed frame wrappers surround the AST image"
        );
        assert_eq!(document_tex.matches("\\BobImageBegin{}").count(), 1);
        assert_eq!(document_tex.matches("\\vspace{6pt}").count(), 2);
        for marker in [
            "\\begin{BobTasks}",
            "\\BobChipBegin{}",
            "\\BobChipAccentBegin{}",
            "\\BobLabelBegin{}",
            "\\BobMutedBegin{}",
            "\\BobMissingBegin{}",
            "\\begin{BobEmbed}",
            "\\begin{BobLiveQuery}",
            "\\begin{BobCallout}",
            "\\begin{BobCompact}",
            "\\BobMastheadBegin{}",
            "\\BobNavBegin{}",
            "\\label{bobsec",
            "\\BobNavPage{bobsec",
            "\\needspace{4\\baselineskip}",
            "\\allowbreak{}",
        ] {
            assert!(tex.contains(marker), "missing {marker}");
        }
        let scheduled_source = scratch.join("scheduled.projected.md");
        fs::write(
            &scheduled_source,
            "## Scheduled {#scheduled .lane}\n\n### This month {#scheduled-this-month}\n\n- []{.st .st-blocked} A compact row\n",
        )
        .expect("scheduled fixture");
        let scheduled_out = scratch.join("scheduled.tex");
        let scheduled_run = std::process::Command::new(&pandoc)
            .arg(&scheduled_source)
            .arg("-f")
            .arg("commonmark_x-tex_math_dollars-emoji")
            .arg("-t")
            .arg("latex")
            .arg("-s")
            .arg("-o")
            .arg(&scheduled_out)
            .arg("--lua-filter")
            .arg(&note_filter)
            .arg("--metadata")
            .arg("bob-glyph-table=glyphs.lua")
            .current_dir(&scratch)
            .output()
            .expect("run scheduled fixture through pandoc");
        assert!(
            scheduled_run.status.success(),
            "scheduled pandoc fixture: {scheduled_run:?}"
        );
        let scheduled_tex =
            fs::read_to_string(&scheduled_out).expect("read scheduled TeX");
        let scheduled_lane = scheduled_tex
            .find("\\BobLaneBegin{}")
            .expect("scheduled lane label");
        assert!(
            scheduled_tex[..scheduled_lane]
                .rfind("\\needspace{8\\baselineskip}")
                .is_some(),
            "Scheduled reserves space for the first date group: {scheduled_tex}"
        );
        // Glyph fallbacks on focused inputs (dice needs Noto, ▸ needs mono).
        for (text, marker) in [
            ("Dice 🎲 here", "\\BobFallbackBegin{}"),
            ("Arrow ▸ here", "\\BobMonoGlyphBegin{}"),
        ] {
            let mini = scratch.join("glyph-mini.md");
            fs::write(
                &mini,
                format!("---\nbob-glyph-table: glyphs.lua\n---\n\n{text}\n"),
            )
            .expect("mini");
            let mini_tex = scratch.join("glyph-mini.tex");
            let mini_out = std::process::Command::new(&pandoc)
                .arg(&mini)
                .arg("-f")
                .arg(SNAPSHOT_PANDOC_READER)
                .arg("-t")
                .arg("latex")
                .arg("-o")
                .arg(&mini_tex)
                .arg("--lua-filter")
                .arg(&note_filter)
                .arg("--metadata")
                .arg("bob-glyph-table=glyphs.lua")
                .current_dir(&scratch)
                .output()
                .expect("mini pandoc");
            assert!(
                mini_out.status.success(),
                "mini pandoc {text}: {mini_out:?}"
            );
            let mini_tex_text =
                fs::read_to_string(&mini_tex).expect("mini tex");
            assert!(
                mini_tex_text.contains(marker),
                "glyph {text} needs {marker}"
            );
        }
        // Anchors preserved, return-link opt-out respected in a second run.
        assert!(
            tex.contains("\\label{t-next}") || tex.contains("t-next"),
            "anchor"
        );
        // Strike formatting survives while lifted links stay after the strike.
        let strike = tex.find("\\st{struck").expect("strike formatting");
        let lifted_link = tex
            .find("\\hyperlink{t-next}{visible")
            .or_else(|| tex.find("\\hyperref[t-next]{visible link\\BobTag{ᵇ}}"))
            .expect("lifted link");
        assert!(
            tex[lifted_link..].contains("\\BobTag{ᵇ}"),
            "the lifted link keeps its return tag: {tex}"
        );
        assert!(strike < lifted_link, "{tex}");
        assert!(
            !tex[strike..lifted_link].contains("\\hyperref"),
            "a link entered strike formatting: {tex}"
        );
        assert!(tex.contains("\\%"), "percent literal");
        assert!(tex.contains("\\&"), "ampersand literal");
        assert!(tex.contains("\\$"), "dollar literal");
        assert!(tex.contains("@BANK"), "@BANK survives");
        assert!(
            tex.contains("69,044.29") || tex.contains("69{,}044"),
            "$ amount survives"
        );
        assert!(tex.contains(":shortcode:"), ":shortcode: survives");
        let image_list = scratch.join("image-list.md");
        fs::write(
            &image_list,
            "---\nbob-glyph-table: glyphs.lua\n---\n\n- []{.st .st-open} []{#task-anchor} Task image ![diagram](<assets/quay_crane.png>)\n",
        )
        .expect("write image-list fixture");
        let image_list_tex = scratch.join("image-list.tex");
        let image_list_output = std::process::Command::new(&pandoc)
            .arg(&image_list)
            .arg("-f")
            .arg(SNAPSHOT_PANDOC_READER)
            .arg("-t")
            .arg("latex")
            .arg("-s")
            .arg("-o")
            .arg(&image_list_tex)
            .arg("--lua-filter")
            .arg(&note_filter)
            .arg("--metadata")
            .arg("bob-glyph-table=glyphs.lua")
            .current_dir(&scratch)
            .output()
            .expect("pandoc list image");
        assert!(
            image_list_output.status.success(),
            "pandoc list image: {image_list_output:?}"
        );
        let image_list_tex =
            fs::read_to_string(image_list_tex).expect("read list image TeX");
        let image_list_document_start = image_list_tex
            .find("\\begin{document}")
            .expect("image-list document body")
            + "\\begin{document}".len();
        let image_list_document = &image_list_tex[image_list_document_start..];
        let task_start = image_list_document
            .find("\\BobTask{Open}")
            .expect("task row start");
        let task_paragraph = image_list_document[task_start..]
            .split("\n\n")
            .next()
            .expect("task row paragraph");
        assert!(
            task_paragraph.contains("\\phantomsection\\label{task-anchor}")
                && task_paragraph.contains("Task image"),
            "task ID anchor and text stay on the status row: {task_paragraph}"
        );
        let image_begin = image_list_document
            .find("\\BobImageBegin{}")
            .expect("task image begin");
        let image = image_list_document
            .find("\\includegraphics")
            .expect("task AST image");
        let image_end = image_list_document
            .find("\\BobImageEnd{}")
            .expect("task image end");
        assert!(
            task_start < image_begin && image_begin < image && image < image_end,
            "image stays inside its AST wrapper on a task row: {image_list_tex}"
        );
        // Opt-out: bob-return-links:false leaves doc unchanged but still converts.
        let opt_out = scratch.join("optout.md");
        fs::write(&opt_out, "---\nbob-return-links: false\nbob-glyph-table: glyphs.lua\n---\n\n# Hi {#hi}\n\nA [link](#hi).\n")
            .expect("optout");
        let opt_tex = scratch.join("optout.tex");
        let opt_out = std::process::Command::new(&pandoc)
            .arg(&opt_out)
            .arg("-f")
            .arg(SNAPSHOT_PANDOC_READER)
            .arg("-t")
            .arg("latex")
            .arg("-s")
            .arg("-o")
            .arg(&opt_tex)
            .arg("--lua-filter")
            .arg(&code_break)
            .arg("--lua-filter")
            .arg(&return_filter)
            .arg("--lua-filter")
            .arg(&note_filter)
            .arg("--include-in-header")
            .arg(&header)
            .arg("--metadata")
            .arg(format!(
                "bob-return-links-report={}",
                scratch.join("report2.json").display()
            ))
            .current_dir(&scratch)
            .output()
            .expect("optout pandoc");
        assert!(opt_out.status.success(), "optout pandoc: {opt_out:?}");
        let opt_tex_text = fs::read_to_string(&opt_tex).expect("optout tex");
        assert!(
            opt_tex_text.contains("\\BobMastheadBegin{}"),
            "masthead with opt-out"
        );
    }

    #[test]
    fn pandoc_keeps_struck_code_and_fallback_glyphs_renderable() {
        let Some(pandoc) = super::super::super::create::pandoc_command() else {
            eprintln!("skipping strike fallback test: pandoc is required");
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        fs::write(
            scratch.join("glyphs.lua"),
            "return { [0x1F3B2] = true, serif_uncovered = {}, sans_uncovered = {}, mono_uncovered = {}, }\n",
        )
        .expect("glyphs");
        fs::write(scratch.join("note.lua"), NOTE_FILTER).expect("filter");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/note_snapshot/strike-fallback.projected.md");
        let output_path = scratch.join("strike-fallback.tex");
        let output = std::process::Command::new(&pandoc)
            .arg(&source)
            .arg("-f")
            .arg(SNAPSHOT_PANDOC_READER)
            .arg("-t")
            .arg("latex")
            .arg("-s")
            .arg("-o")
            .arg(&output_path)
            .arg("--lua-filter")
            .arg(scratch.join("note.lua"))
            .arg("--metadata")
            .arg("bob-glyph-table=glyphs.lua")
            .current_dir(&scratch)
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "pandoc failed: {output:?}");
        let tex = fs::read_to_string(output_path).expect("read generated TeX");
        for expected in [
            "\\st{Read the }",
            "\\texttt{models}",
            "\\st{ config",
            "\\BobFallbackBegin{}🎲\\BobFallbackEnd{}",
            "\\st{ safely.}",
        ] {
            assert!(tex.contains(expected), "missing {expected}: {tex}");
        }
    }

    #[test]
    fn renders_contract_with_clean_text_layer_and_metadata() {
        let Some(_) = super::super::super::create::pandoc_command() else {
            eprintln!("skipping real render: pandoc is required");
            return;
        };
        if std::process::Command::new("xelatex")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping real render: xelatex is required");
            return;
        }
        if skip_if_missing_note_profile_packages("real render") {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        stage_synthetic_image(&scratch);
        let filters = write_filters_for_render(&scratch);
        let meta = test_meta();
        let outcome = render_note_pdf(
            &contract_projected_md(),
            &scratch,
            &filters,
            &meta,
        )
        .expect("real render");
        assert!(outcome.pdf.is_file(), "PDF exists");
        assert!(
            outcome.warnings.is_empty(),
            "zero missing glyphs and no >20pt overfull: {:?}",
            outcome.warnings
        );
        let list_image_scratch = temp.path().join("list-image");
        fs::create_dir_all(&list_image_scratch).expect("list image scratch");
        stage_synthetic_image(&list_image_scratch);
        let list_image_filters = write_filters_for_render(&list_image_scratch);
        let list_image = render_note_pdf(
            "---\nbob-snapshot: true\nbob-kicker: NOTE\nbob-display-title: Image row\nbob-snapshot-line: Snapshot\nbob-footer: Image row\nbob-glyph-table: glyphs.lua\nbob-return-links: false\n---\n\n- []{.st .st-open} Task with ![diagram](assets/quay_crane.png)\n",
            &list_image_scratch,
            &list_image_filters,
            &NoteMeta {
                identity_title: "Image row · 2026-10-16".to_string(),
                source_path: "image_row.md".to_string(),
                kind: "note".to_string(),
                listen_uri: None,
            },
        )
        .expect("real render of image in task row");
        assert!(
            list_image.warnings.is_empty(),
            "framed list image has no >20pt overfull: {:?}",
            list_image.warnings
        );
        // Zero Type 3 fonts via lopdf font dictionaries.
        let document = lopdf::Document::load(&outcome.pdf).expect("load PDF");
        for object in document.objects.values() {
            let debug = format!("{object:?}");
            assert!(!debug.contains("Type3"), "no Type 3 fonts: {debug}");
        }
        if std::process::Command::new("pdffonts")
            .arg("--version")
            .output()
            .is_ok()
        {
            let out = std::process::Command::new("pdffonts")
                .arg(&outcome.pdf)
                .output()
                .expect("pdffonts");
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(!text.contains("Type 3"), "pdffonts Type 3: {text}");
        }
        // PDF Info via lopdf trailer (UTF-16BE decoded, like outlines).
        let info_id = document
            .trailer
            .get(b"Info")
            .expect("Info")
            .as_reference()
            .expect("Info ref");
        let info_obj = document.get_object(info_id).expect("Info object");
        let lopdf::Object::Dictionary(info_dict) = info_obj else {
            panic!("Info is not a dictionary: {info_obj:?}");
        };
        let title =
            lopdf::decode_text_string(info_dict.get(b"Title").expect("Title"))
                .expect("decode Title");
        let subject = lopdf::decode_text_string(
            info_dict.get(b"Subject").expect("Subject"),
        )
        .expect("decode Subject");
        assert!(title.contains("Harbor Cranes"), "Title in Info: {title}");
        assert!(
            subject.contains("harbor_cranes.md"),
            "Subject path: {subject}"
        );
        // Bookmarks exist (H1 + appendix).
        let catalog = document.catalog().expect("catalog");
        assert!(catalog.get(b"Outlines").is_ok(), "outlines exist");
        // Obsidian URI annotations exist (via lopdf objects, like the
        // listen-card test, since page streams may be compressed).
        let has_obsidian = document
            .objects
            .values()
            .any(|object| format!("{object:?}").contains("obsidian://"));
        assert!(has_obsidian, "obsidian URIs in annotations");
        // Return-link report: internal #closed link pairs.
        match &outcome.report {
            super::super::super::return_links::ReportOutcome::Report(
                report,
            ) => {
                assert!(report.paired >= 1, "paired return links: {report:?}");
            }
            other => panic!("expected return-link report, got {other:?}"),
        }
        if std::process::Command::new("pdftotext")
            .arg("-v")
            .output()
            .is_err()
        {
            eprintln!("skipping pdftotext checks: pdftotext is required");
            return;
        }
        let text_out = std::process::Command::new("pdftotext")
            .arg("-layout")
            .arg(&outcome.pdf)
            .arg("-")
            .output()
            .expect("pdftotext");
        assert!(text_out.status.success(), "{text_out:?}");
        let text = String::from_utf8_lossy(&text_out.stdout).into_owned();
        assert!(text.contains("Open task"), "task words");
        assert!(text.contains("sched Oct 17"), "chip text");
        let open_task_line = text
            .lines()
            .find(|line| line.contains("Open task"))
            .expect("task text line");
        assert!(
            open_task_line.contains("sched Oct 17"),
            "short task text and chip share one line: {open_task_line}"
        );
        for symbol in ["○", "◉", "◐", "⊘", "✓", "⊗"] {
            assert!(
                !text.contains(symbol),
                "no status symbol {symbol} in text layer"
            );
        }
        assert!(text.contains("office"), "ligature-safe office");
        assert!(text.contains("%") && text.contains("&"), "literal specials");
        assert!(text.contains("@BANK"), "@BANK preserved");
        assert!(text.contains("69,044.29"), "$ amount preserved");
        assert!(text.contains(":shortcode:"), ":shortcode: preserved");
        // Footer 1 / N on first page.
        let footer_out = std::process::Command::new("pdftotext")
            .arg("-layout")
            .arg("-f")
            .arg("1")
            .arg("-l")
            .arg("1")
            .arg(&outcome.pdf)
            .arg("-")
            .output()
            .expect("footer pdftotext");
        let footer = String::from_utf8_lossy(&footer_out.stdout).into_owned();
        assert!(
            footer.contains("1 /") || footer.contains("1/"),
            "footer 1 / N: {footer}"
        );
        // Corpus glyphs and ▸ in metadata/code render without missing glyphs.
        let corpus = "→ · — – • ” “ ’ … ≤ ▸ ¶ á ķ 🎲";
        let corpus_md = format!(
            "---\ntitle: Corpus\nbob-snapshot: true\nbob-kicker: NOTE\nbob-display-title: Corpus ▸ check\nbob-meta:\n  - \"Meta ▸ arrow\"\nbob-stats:\n  - status: open\n    count: 1\nbob-snapshot-line: Snapshot\nbob-footer: Corpus\nbob-glyph-table: glyphs.lua\nbob-return-links: false\n---\n\n# Corpus {{#corpus}}\n\nCorpus line: {corpus}.\n\nInline code `code ▸ mono` and `emoji 🎲 code`.\n"
        );
        let scratch2 = temp.path().join("scratch2");
        fs::create_dir_all(&scratch2).expect("scratch2");
        let filters2 = write_filters_for_render(&scratch2);
        let outcome2 = render_note_pdf(
            &corpus_md,
            &scratch2,
            &filters2,
            &NoteMeta {
                identity_title: "Corpus · 2026-10-16".to_string(),
                source_path: "corpus.md".to_string(),
                kind: "note".to_string(),
                listen_uri: None,
            },
        )
        .expect("corpus render");
        assert!(
            outcome2.warnings.is_empty(),
            "corpus warnings: {:?}",
            outcome2.warnings
        );
        let corpus_text_out = std::process::Command::new("pdftotext")
            .arg("-layout")
            .arg(&outcome2.pdf)
            .arg("-")
            .output()
            .expect("corpus pdftotext");
        let corpus_text =
            String::from_utf8_lossy(&corpus_text_out.stdout).into_owned();
        assert!(corpus_text.contains("→"), "corpus arrow in text");
        assert!(corpus_text.contains("▸"), "▸ per-role regression in text");
    }

    #[test]
    fn navigation_shows_page_numbers_only_above_eight_pages() {
        let Some(_) = super::super::super::create::pandoc_command() else {
            eprintln!("skipping nav test: pandoc is required");
            return;
        };
        if std::process::Command::new("xelatex")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping nav test: xelatex is required");
            return;
        }
        if std::process::Command::new("pdftotext")
            .arg("-v")
            .output()
            .is_err()
        {
            eprintln!("skipping nav test: pdftotext is required");
            return;
        }
        if skip_if_missing_note_profile_packages("nav render") {
            return;
        }
        // Short doc: two H1 sections, one wrapped task with children.
        let short_md = "---\ntitle: Short\nbob-snapshot: true\nbob-kicker: NOTE\nbob-display-title: Short doc\nbob-snapshot-line: Snapshot\nbob-footer: Short\nbob-glyph-table: glyphs.lua\nbob-return-links: false\n---\n\n# Alpha {#alpha}\n\nA short paragraph.\n\n# Beta {#beta}\n\n- []{.st .st-open} []{#t-wrap} A wrapped task with children that spans lines and keeps its first child nearby\n  - child one with a [link](https://example.com)\n  - child two with `code` and *emphasis*\n".to_string();
        // Long doc: same plus enough filler H1 sections to exceed eight pages.
        let mut long_md = short_md.clone();
        for i in 3..=14 {
            long_md.push_str(&format!("\n# Section {i} {{#sec{i}}}\n\n"));
            for _ in 0..30 {
                long_md.push_str("Filler paragraph pads the document so return pills span several pages and page labels vary across the eight-page navigation threshold. ");
            }
            long_md.push_str("\n");
        }
        for (name, md, expect_pages) in
            [("short", short_md, false), ("long", long_md, true)]
        {
            let temp = tempfile::tempdir().expect("tempdir");
            let scratch = temp.path().join("scratch");
            fs::create_dir_all(&scratch).expect("scratch");
            let filters = write_filters_for_render(&scratch);
            let outcome = render_note_pdf(
                &md,
                &scratch,
                &filters,
                &NoteMeta {
                    identity_title: format!("Nav {name} · 2026-10-16"),
                    source_path: format!("nav_{name}.md"),
                    kind: "note".to_string(),
                    listen_uri: None,
                },
            )
            .expect("nav render");
            assert!(
                outcome.warnings.is_empty(),
                "nav {name} warnings: {:?}",
                outcome.warnings
            );
            let text_out = std::process::Command::new("pdftotext")
                .arg("-layout")
                .arg("-f")
                .arg("1")
                .arg("-l")
                .arg("1")
                .arg(&outcome.pdf)
                .arg("-")
                .output()
                .expect("nav pdftotext");
            let first_page =
                String::from_utf8_lossy(&text_out.stdout).into_owned();
            assert!(
                first_page.contains("In this note"),
                "nav present ({name})"
            );
            assert!(first_page.contains("Alpha"), "nav Alpha ({name})");
            if expect_pages {
                assert!(
                    first_page.contains("p."),
                    "long nav shows page numbers ({name}): {first_page}"
                );
            } else {
                assert!(
                    !first_page.contains("p. "),
                    "short nav hides page numbers ({name}): {first_page}"
                );
            }
            // Wrapped task with children survives on the short doc.
            if !expect_pages {
                let full_out = std::process::Command::new("pdftotext")
                    .arg("-layout")
                    .arg(&outcome.pdf)
                    .arg("-")
                    .output()
                    .expect("full pdftotext");
                let full =
                    String::from_utf8_lossy(&full_out.stdout).into_owned();
                assert!(full.contains("wrapped task"), "wrapped task ({name})");
                assert!(full.contains("child one"), "task child ({name})");
            }
        }
    }

    const MERMAID_FLOW: &str = "flowchart LR\n  A --> B\n";

    fn mermaid_projected() -> String {
        format!(
            "---\n\
             bob-snapshot: true\n\
             bob-kicker: NOTE\n\
             bob-display-title: Diagrams\n\
             bob-snapshot-line: Snapshot\n\
             bob-footer: Diagrams\n\
             bob-glyph-table: glyphs.lua\n\
             bob-return-links: false\n\
             ---\n\n\
             ``` {{#fig-flow .mermaid}}\n{MERMAID_FLOW}```\n"
        )
    }

    fn pdf_haystack(path: &Path) -> String {
        // Callers gate on `skip_real_diagram_render`, which requires
        // pdftotext. There is no raw-bytes fallback: searching compressed
        // PDF bytes for visible text is not a valid extraction and lets
        // negative assertions pass falsely.
        let output = std::process::Command::new("pdftotext")
            .arg("-layout")
            .arg(path)
            .arg("-")
            .output()
            .expect("pdftotext is required for PDF text assertions");
        assert!(
            output.status.success(),
            "pdftotext failed on {}: {output:?}",
            path.display()
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn skip_real_diagram_render(test_name: &str) -> bool {
        if super::super::super::create::pandoc_command().is_none() {
            eprintln!("skipping {test_name}: pandoc/xelatex is required");
            return true;
        }
        if std::process::Command::new("xelatex")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping {test_name}: pandoc/xelatex is required");
            return true;
        }
        if std::process::Command::new("pdftotext")
            .arg("-v")
            .output()
            .is_err()
        {
            eprintln!("skipping {test_name}: pdftotext is required");
            return true;
        }
        skip_if_missing_note_profile_packages(test_name)
    }

    fn filters_with_diagrams(
        dir: &Path,
        results: &Path,
    ) -> super::super::super::create::RenderFilters {
        let mut filters = write_filters_for_render(dir);
        let swap = dir.join("diagrams.lua");
        fs::write(&swap, super::super::super::diagrams::SWAP_FILTER)
            .expect("write swap");
        filters.diagrams = Some(super::super::super::diagrams::DiagramFilter {
            filter: swap,
            results: results.to_path_buf(),
        });
        filters
    }

    fn collected_snapshot_id(source: &Path, workdir: &Path) -> String {
        let pandoc = super::super::super::create::pandoc_command()
            .expect("pandoc checked above");
        let blocks = super::super::super::diagrams::collect(
            &pandoc,
            source,
            super::super::super::diagrams::DiagramReader::Snapshot,
            workdir,
        )
        .expect("collect");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        blocks[0].id.clone()
    }

    #[test]
    fn diagrams_swap_snapshot_centers_image_and_drops_source() {
        if skip_real_diagram_render("diagrams snapshot swap") {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let source = temp.path().join("doc.md");
        fs::write(&source, mermaid_projected()).expect("write source");
        let id = collected_snapshot_id(&source, &temp.path().join("collect"));
        let pdf = scratch.join("fig.pdf");
        super::super::super::diagrams::write_one_page_pdf(&pdf);
        let results_path = scratch.join("results.json");
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::super::diagrams::DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        super::super::super::diagrams::write_results(
            &results_path,
            &super::super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let filters = filters_with_diagrams(&scratch, &results_path);
        let outcome = render_note_pdf(
            &mermaid_projected(),
            &scratch,
            &filters,
            &test_meta(),
        )
        .expect("snapshot swap render");
        let text = pdf_haystack(&outcome.pdf);
        assert!(!text.contains("flowchart LR"), "source gone: {text}");
        assert!(!text.contains("Figure 1"), "no implicit caption: {text}");
        let tex = fs::read_to_string(scratch.join("note.tex"))
            .expect("read generated TeX");
        assert!(
            tex.contains("\\begin{center}"),
            "snapshot image is centered: {tex}"
        );
    }

    #[test]
    fn diagrams_swap_snapshot_error_keeps_note_and_source() {
        if skip_real_diagram_render("diagrams snapshot error") {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let source = temp.path().join("doc.md");
        fs::write(&source, mermaid_projected()).expect("write source");
        let id = collected_snapshot_id(&source, &temp.path().join("collect"));
        let results_path = scratch.join("results.json");
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::super::diagrams::DiagramOutcome::Error {
                error: "Parse error on line 4: bad".to_string(),
            },
        );
        super::super::super::diagrams::write_results(
            &results_path,
            &super::super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let filters = filters_with_diagrams(&scratch, &results_path);
        let outcome = render_note_pdf(
            &mermaid_projected(),
            &scratch,
            &filters,
            &test_meta(),
        )
        .expect("snapshot error render");
        let text = pdf_haystack(&outcome.pdf);
        assert!(
            text.contains(
                "Mermaid diagram not rendered: Parse error on line 4"
            ),
            "{text}"
        );
        assert!(text.contains("flowchart LR"), "source kept: {text}");
    }

    #[test]
    fn diagrams_filter_runs_first_on_snapshot_route() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let calls = temp.path().join("calls.log");
        let fake_pandoc = bin.join("fake-pandoc");
        write_executable(
            &fake_pandoc,
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do echo \"PANDOC-ARG:$a\" >> \"{}\"; done\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\nprintf '\\\\documentclass{{article}}\\\\begin{{document}}Hi\\\\end{{document}}\\n' > \"$out\"\nexit 0\n",
                calls.display()
            ),
        );
        let fake_xelatex = bin.join("fake-xelatex");
        write_executable(
            &fake_xelatex,
            "#!/bin/sh\nprintf 'fake log\\n' > note.log\nprintf 'fake pdf' > note.pdf\nexit 0\n",
        );
        let mut filters = test_filters(temp.path());
        let swap = temp.path().join("diagrams.lua");
        fs::write(&swap, "-- swap").expect("write swap");
        let results = temp.path().join("results.json");
        fs::write(&results, "{}").expect("write results");
        filters.diagrams = Some(super::super::super::diagrams::DiagramFilter {
            filter: swap,
            results,
        });
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("BOB_PANDOC_COMMAND", Some(fake_pandoc.as_os_str())),
            ("BOB_XELATEX_COMMAND", Some(fake_xelatex.as_os_str())),
        ]);
        render_note_pdf("# Hi\n", &scratch, &filters, &test_meta())
            .expect("fake render");
        let log = fs::read_to_string(&calls).expect("calls log");
        let diagrams = log.find("diagrams.lua").expect("diagrams filter");
        let cb = log.find("code-break.lua").expect("code-break");
        let rl = log.find("return-links.lua").expect("return-links");
        assert!(diagrams < cb && cb < rl, "diagrams filter first: {log}");
        assert!(
            log.contains("bob-diagrams-results="),
            "results metadata: {log}"
        );
    }

    #[test]
    fn snapshot_diagram_text_box_matches_note_geometry() {
        assert!((SNAPSHOT_DIAGRAM_TEXT_WIDTH_PT - 453.6).abs() < 1e-9);
        assert!((SNAPSHOT_DIAGRAM_TEXT_HEIGHT_PT - 655.2).abs() < 1e-9);
    }

    const TALL_SNAPSHOT_FLOW: &str = "flowchart TB\n  A --> B\n  B --> C\n";

    fn tall_snapshot_projected() -> String {
        format!(
            "---\n\
             bob-snapshot: true\n\
             bob-kicker: NOTE\n\
             bob-display-title: Tall diagram\n\
             bob-snapshot-line: Snapshot\n\
             bob-footer: Tall diagram\n\
             bob-glyph-table: glyphs.lua\n\
             ---\n\n\
             # Tall exercise\n\n\
             A [model](#data-model-and-ownership) opens the exercise.\n\n\
             ### Data model and ownership\n\n\
             ``` {{#fig-tall .mermaid}}\n{TALL_SNAPSHOT_FLOW}```\n\n\
             Caption paragraph that follows the diagram.\n"
        )
    }

    fn snapshot_page_text(pdf: &Path, page: u32) -> String {
        let output = std::process::Command::new("pdftotext")
            .arg("-layout")
            .arg("-f")
            .arg(page.to_string())
            .arg("-l")
            .arg(page.to_string())
            .arg(pdf)
            .arg("-")
            .output()
            .expect("run pdftotext");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn snapshot_body_empty(text: &str) -> bool {
        let footer_words = ["Tall", "diagram"];
        let kept: Vec<&str> = text
            .split_whitespace()
            .filter(|token| {
                if token.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
                if *token == "/" || *token == "·" {
                    return false;
                }
                if footer_words.contains(token) {
                    return false;
                }
                true
            })
            .collect();
        kept.join(" ").trim().is_empty()
    }

    #[test]
    fn tall_snapshot_diagram_shares_page_with_heading_without_vbox_overflow() {
        if skip_real_diagram_render("tall snapshot fit") {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let projected = tall_snapshot_projected();
        let source = temp.path().join("doc.md");
        fs::write(&source, &projected).expect("write source");
        let id = collected_snapshot_id(&source, &temp.path().join("collect"));
        let pdf = scratch.join("fig.pdf");
        super::super::super::diagrams::write_sized_pdf(&pdf, 514.5, 714.75);
        let results_path = scratch.join("results.json");
        let mut diagrams = std::collections::BTreeMap::new();
        diagrams.insert(
            id,
            super::super::super::diagrams::DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        super::super::super::diagrams::write_results(
            &results_path,
            &super::super::super::diagrams::DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let filters = filters_with_diagrams(&scratch, &results_path);
        let outcome =
            render_note_pdf(&projected, &scratch, &filters, &test_meta())
                .expect("tall snapshot render");
        let tex = fs::read_to_string(scratch.join("note.tex"))
            .expect("read note.tex");
        assert!(tex.contains("\\BobFitBegin{}"), "fit wrapper: {tex}");
        assert!(tex.contains("\\BobImageBegin{}"), "frame wrapper: {tex}");
        assert!(tex.contains("\\begin{center}"), "centered: {tex}");
        let log = fs::read_to_string(scratch.join("note.log"))
            .expect("read note.log");
        assert!(
            !log.contains("Overfull \\vbox"),
            "no vertical overflow: {}",
            log.lines()
                .filter(|l| l.contains("Overfull"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let document =
            lopdf::Document::load(&outcome.pdf).expect("load snapshot PDF");
        let pages = document.get_pages().len() as u32;
        for page in 1..=pages {
            let text = snapshot_page_text(&outcome.pdf, page);
            assert!(
                !snapshot_body_empty(&text),
                "page {page} must not be body-empty: {text:?}"
            );
        }
        let mut heading_page = 0;
        let mut top_page = 0;
        for page in 1..=pages {
            let text = snapshot_page_text(&outcome.pdf, page);
            if text.contains("Data model") {
                heading_page = page;
            }
            if text.contains("diagramtop") {
                top_page = page;
            }
        }
        assert_ne!(heading_page, 0, "heading must render");
        assert_ne!(top_page, 0, "diagramtop must render");
        assert_eq!(
            heading_page, top_page,
            "heading and diagramtop share a page"
        );
    }
}
