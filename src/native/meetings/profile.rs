//! Meeting dossier print profile: pandoc-to-TeX then xelatex.
//!
//! `render_meeting_pdf(dossier_md, scratch, filters, meta)` writes the
//! dossier Markdown through the shared snapshot engine with the meeting
//! header and Lua filter, then returns the scratch PDF path and typed
//! warnings. It never stamps, installs, or creates the scratch directory.

use std::path::Path;

use chrono::NaiveDate;

use super::{parse_dossier, DossierMeta};
use crate::native::highlights_ref::meeting_profile::{
    MEETING_FILTER, MEETING_HEADER,
};
use crate::native::highlights_ref::note_snapshot::profile::{
    render_profile_pdf, PdfProfile, RenderOutcome, SNAPSHOT_PANDOC_READER,
};
use crate::native::highlights_ref::{create, Result};

const HEADER_NAME: &str = "meeting-profile.tex";
const FILTER_NAME: &str = "meeting.lua";
const DEFAULT_ACCENT: &str = "3B6EA8";
const DEFAULT_PRODUCER: &str = "Meeting notes";

/// Identity for PDF Info and the title card extras the Lua filter reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetingMeta {
    /// Reference title for PDF Info Title.
    pub(crate) title: String,
    /// Event token for Subject (`meeting notes · <event ref>`).
    pub(crate) event_ref: String,
    /// Optional 6-digit HTML hex accent. Defaults to BobNextWip.
    pub(crate) accent: Option<String>,
}

/// Render dossier Markdown to PDF via the shared print engine.
pub(crate) fn render_meeting_pdf(
    dossier_md: &str,
    scratch: &Path,
    filters: &create::RenderFilters,
    meta: &MeetingMeta,
) -> Result<RenderOutcome> {
    let extra = meeting_extra_metadata(dossier_md, meta);
    let subject = format!("meeting notes · {}", meta.event_ref);
    render_profile_pdf(
        dossier_md,
        scratch,
        filters,
        &PdfProfile {
            header: MEETING_HEADER,
            header_name: HEADER_NAME,
            filter: MEETING_FILTER,
            filter_name: FILTER_NAME,
            reader: SNAPSHOT_PANDOC_READER,
            title: &meta.title,
            subject: &subject,
            keywords: "meetings",
            creator: "bob ref meet",
            extra_metadata: &extra,
        },
    )
}

fn meeting_extra_metadata(
    dossier_md: &str,
    meta: &MeetingMeta,
) -> Vec<(&'static str, String)> {
    let mut extra = vec![
        ("bob-return-links", "false".to_string()),
        ("bob-meeting-accent", accent_hex(meta.accent.as_deref())),
    ];
    extra.push(("bob-meeting-head", running_header(dossier_md, meta)));
    if let Ok(document) = parse_dossier(dossier_md) {
        extra
            .push(("bob-meeting-day", format_meeting_day(&document.meta.date)));
        if let Some(duration) = duration_label(&document.meta.span) {
            extra.push(("bob-meeting-duration", duration));
        }
        extra.push((
            "bob-meeting-producer",
            producer_label(document.meta.producer.as_deref()),
        ));
    }
    extra
}

fn running_header(dossier_md: &str, meta: &MeetingMeta) -> String {
    parse_dossier(dossier_md)
        .map(|document| document.meta.event_title)
        .unwrap_or_else(|_| meta.title.clone())
}

pub(crate) fn accent_hex(raw: Option<&str>) -> String {
    let Some(value) = raw else {
        return DEFAULT_ACCENT.to_string();
    };
    let value = value.trim().trim_start_matches('#');
    if value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        value.to_ascii_uppercase()
    } else {
        DEFAULT_ACCENT.to_string()
    }
}

pub(crate) fn format_meeting_day(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|parsed| parsed.format("%a %Y-%m-%d").to_string())
        .unwrap_or_else(|_| date.to_string())
}

pub(crate) fn duration_label(span: &str) -> Option<String> {
    let (start, end) = span.split_once('–').or_else(|| span.split_once('-'))?;
    let start = parse_hm(start.trim())?;
    let end = parse_hm(end.trim())?;
    let mut minutes = end - start;
    if minutes < 0 {
        minutes += 24 * 60;
    }
    if minutes <= 0 {
        return None;
    }
    let hours = minutes / 60;
    let rest = minutes % 60;
    Some(match (hours, rest) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    })
}

fn parse_hm(text: &str) -> Option<i32> {
    let (hours, minutes) = text.split_once(':')?;
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) {
        return None;
    }
    Some(hours * 60 + minutes)
}

fn producer_label(producer: Option<&str>) -> String {
    match producer.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value.to_string(),
        None => DEFAULT_PRODUCER.to_string(),
    }
}

#[cfg(test)]
fn test_print_meta() -> MeetingMeta {
    MeetingMeta {
        title: "Cedar Labs and Driftwood Analytics planning — Meeting Notes (Mon 2026-11-24, 14:00–14:30)".to_string(),
        event_ref: "20261124#^cedar-driftwood".to_string(),
        accent: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env::TestEnvGuard;
    use crate::native::highlights_ref::create::{
        default_table_settings, write_render_filters,
    };
    use crate::native::meetings::{normalize, render_dossier};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> String {
        fs::read_to_string(fixture_path(name)).expect("fixture reads")
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/meetings")
            .join(name)
    }

    fn dossier_meta(include_transcript: bool) -> DossierMeta {
        DossierMeta {
            event_title: "Cedar Labs and Driftwood Analytics planning".into(),
            date: "2026-11-24".into(),
            span: "14:00–14:30".into(),
            root: "projects/seaglass".into(),
            producer: Some("VoiceQuill".into()),
            filed_date: "2026-11-24".into(),
            include_transcript,
            edited: false,
        }
    }

    fn word_tokens(text: &str) -> Vec<String> {
        regex::Regex::new(r"[[:alnum:]]+")
            .unwrap()
            .find_iter(text)
            .map(|found| found.as_str().to_ascii_lowercase())
            .collect()
    }

    fn notes_words(name: &str, include_transcript: bool) -> Vec<String> {
        let notes = normalize(&fixture(name));
        let mut text = String::new();
        text.push_str(&notes.tldr);
        text.push('\n');
        for step in &notes.next_steps {
            if let Some(owner) = &step.owner {
                text.push_str(owner);
                text.push(' ');
            }
            text.push_str(&step.action);
            text.push('\n');
        }
        for section in &notes.sections {
            if let Some(heading) = &section.heading {
                text.push_str(heading);
                text.push('\n');
            }
            for item in &section.items {
                text.push_str(item);
                text.push('\n');
            }
        }
        if include_transcript && let Some(transcript) = &notes.transcript {
            text.push_str(transcript);
        }
        word_tokens(&text)
    }

    fn skip_if_missing_packages(test_name: &str) -> bool {
        match crate::native::highlights_ref::meeting_profile::missing_tex_package_files()
        {
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

    fn skip_if_missing_renderer(test_name: &str) -> bool {
        if crate::native::highlights_ref::create::pandoc_command().is_none() {
            eprintln!("skipping {test_name}: pandoc is required");
            return true;
        }
        if std::process::Command::new("xelatex")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping {test_name}: xelatex is required");
            return true;
        }
        skip_if_missing_packages(test_name)
    }

    fn write_filters(dir: &Path) -> create::RenderFilters {
        let (layout_default, prose_words) = default_table_settings();
        write_render_filters(dir, &layout_default, prose_words)
            .expect("write render filters")
    }

    fn write_executable(path: &Path, body: &str) {
        crate::native::test_stub::write_executable_stub(path, body);
    }

    fn dump_meeting_pdf(name: &str, pdf: &Path) {
        let Ok(dir) = crate::native::env::var("BOB_MEETING_DUMP") else {
            return;
        };
        let dest = PathBuf::from(dir);
        fs::create_dir_all(&dest).expect("dump dir");
        fs::copy(pdf, dest.join(name)).expect("dump pdf");
    }

    fn pdf_text(
        pdf: &Path,
        first: Option<u32>,
        last: Option<u32>,
    ) -> Option<String> {
        let mut pdftotext = std::process::Command::new("pdftotext");
        pdftotext.arg("-layout");
        if let Some(page) = first {
            pdftotext.arg("-f").arg(page.to_string());
        }
        if let Some(page) = last {
            pdftotext.arg("-l").arg(page.to_string());
        }
        if let Ok(out) = pdftotext.arg(pdf).arg("-").output()
            && out.status.success()
        {
            return Some(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let mut mutool = std::process::Command::new("mutool");
        mutool
            .arg("draw")
            .arg("-F")
            .arg("txt")
            .arg("-q")
            .arg("-o")
            .arg("-")
            .arg(pdf);
        match (first, last) {
            (Some(page), Some(end)) if page == end => {
                mutool.arg(page.to_string());
            }
            (Some(page), Some(end)) => {
                mutool.arg(format!("{page}-{end}"));
            }
            (Some(page), None) => {
                mutool.arg(format!("{page}-N"));
            }
            _ => {}
        }
        let out = mutool.output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    #[test]
    fn meeting_filter_emits_no_preamble_only_commands() {
        // The Lua filter runs after \begin{document}, so a preamble-only
        // command like \AtBeginDocument aborts older TeX kernels. PDF Info
        // Title already comes from pandoc's title metadata.
        assert!(
            !crate::native::highlights_ref::meeting_profile::MEETING_FILTER
                .contains("\\AtBeginDocument"),
            "meeting filter must not emit preamble-only LaTeX"
        );
    }

    #[test]
    fn duration_and_day_helpers_cover_the_title_card_strip() {
        assert_eq!(format_meeting_day("2026-11-24"), "Tue 2026-11-24");
        assert_eq!(duration_label("14:00–14:30").as_deref(), Some("30m"));
        assert_eq!(duration_label("15:00-16:30").as_deref(), Some("1h 30m"));
        assert_eq!(duration_label("09:00–10:00").as_deref(), Some("1h"));
        assert_eq!(accent_hex(None), "3B6EA8");
        assert_eq!(accent_hex(Some("#a1b2c3")), "A1B2C3");
        assert_eq!(accent_hex(Some("not-hex")), "3B6EA8");
    }

    #[test]
    fn fake_commands_write_meeting_assets_without_a_toc() {
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
                "#!/bin/sh\necho \"pandoc-call cwd=$(pwd)\" >> \"{}\"\nfor a in \"$@\"; do echo \"PANDOC-ARG:$a\" >> \"{}\"; done\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\nprintf '\\\\documentclass{{article}}\\\\begin{{document}}Hi\\\\end{{document}}\\n' > \"$out\"\nexit 0\n",
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
        let filters = write_filters(temp.path());
        let _guard = TestEnvGuard::set(&[
            ("BOB_PANDOC_COMMAND", Some(fake_pandoc.as_os_str())),
            ("BOB_XELATEX_COMMAND", Some(fake_xelatex.as_os_str())),
        ]);
        let notes = normalize(&fixture("wispr_plain.txt"));
        let dossier = render_dossier(&notes, &dossier_meta(false));
        let outcome = render_meeting_pdf(
            &dossier,
            &scratch,
            &filters,
            &test_print_meta(),
        )
        .expect("fake render");
        assert_eq!(outcome.pdf, scratch.join("note.pdf"));
        assert!(scratch.join("projected.md").is_file());
        assert!(scratch.join("meeting-profile.tex").is_file());
        assert!(scratch.join("meeting.lua").is_file());
        assert!(scratch.join("fonts").is_dir());
        assert!(scratch.join("glyphs.lua").is_file());
        let log = fs::read_to_string(&calls).expect("calls log");
        assert!(log.contains("pandoc-call"), "pandoc ran: {log}");
        assert!(
            log.contains("commonmark_x-tex_math_dollars-emoji"),
            "reader flags: {log}"
        );
        assert!(
            !log.lines().any(|line| line == "PANDOC-ARG:--toc"),
            "no TOC: {log}"
        );
        assert!(
            !log.lines()
                .any(|line| line == "PANDOC-ARG:--number-sections"),
            "no numbering: {log}"
        );
        assert!(
            log.contains("meeting.lua") || log.contains("scratch/meeting.lua"),
            "meeting filter: {log}"
        );
        let xelatex_calls: Vec<&str> = log
            .lines()
            .filter(|line| line.starts_with("xelatex-call"))
            .collect();
        assert_eq!(xelatex_calls.len(), 2, "two passes: {log}");
    }

    #[test]
    fn wispr_summary_renders_one_or_two_pages_with_stable_words() {
        if skip_if_missing_renderer("wispr meeting render") {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let filters = write_filters(&scratch);
        let notes = normalize(&fixture("wispr_plain.txt"));
        let dossier = render_dossier(&notes, &dossier_meta(false));
        let outcome = render_meeting_pdf(
            &dossier,
            &scratch,
            &filters,
            &test_print_meta(),
        )
        .expect("real meeting render");
        dump_meeting_pdf("wispr.pdf", &outcome.pdf);
        assert!(
            outcome.warnings.is_empty(),
            "zero missing glyphs and no >20pt overfull: {:?}",
            outcome.warnings
        );
        let document = lopdf::Document::load(&outcome.pdf).expect("load PDF");
        let pages = document.get_pages().len();
        assert!(
            (1..=2).contains(&pages),
            "Wispr-sized summary is 1-2 pages, got {pages}"
        );
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
        let keywords = lopdf::decode_text_string(
            info_dict.get(b"Keywords").expect("Keywords"),
        )
        .expect("decode Keywords");
        assert!(
            title.contains("Meeting Notes"),
            "Title is the reference title: {title}"
        );
        assert!(
            subject.contains("meeting notes · 20261124#^cedar-driftwood"),
            "Subject: {subject}"
        );
        assert!(keywords.contains("meetings"), "Keywords: {keywords}");
        let Some(text) = pdf_text(&outcome.pdf, None, None) else {
            eprintln!("skipping pdftotext checks: no pdftotext or mutool");
            return;
        };
        assert!(
            !text.to_ascii_lowercase().contains("contents"),
            "no TOC: {text}"
        );
        // The dossier print profile letter-spaces the kicker
        // (meeting.tex uses `LetterSpace=12.0`), so extractors expand it to
        // `M E E T I N G N OT E S`. Compare without whitespace instead of
        // depending on the extractor's glyph-spacing heuristics.
        let squashed: String =
            text.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(squashed.contains("MEETINGNOTES"), "kicker: {text}");
        assert!(
            text.contains("TL;DR") || text.contains("TLDR"),
            "tldr: {text}"
        );
        assert!(text.contains("Owner"), "next-steps owner column: {text}");
        assert!(text.contains("Action"), "next-steps action column: {text}");
        assert!(
            text.contains("structure normalized, wording unchanged"),
            "provenance: {text}"
        );
        let expected = notes_words("wispr_plain.txt", false);
        let haystack = word_tokens(&text);
        let mut idx = 0;
        for word in &expected {
            match haystack[idx..].iter().position(|found| found == word) {
                Some(pos) => idx += pos + 1,
                None => panic!(
                    "missing notes word {word:?} after {idx} in {haystack:?}"
                ),
            }
        }
    }

    #[test]
    fn transcript_appendix_starts_on_its_own_page() {
        if skip_if_missing_renderer("transcript meeting render") {
            return;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        let filters = write_filters(&scratch);
        let notes = normalize(&fixture("markdown_paste.md"));
        let dossier = render_dossier(&notes, &dossier_meta(true));
        let outcome = render_meeting_pdf(
            &dossier,
            &scratch,
            &filters,
            &MeetingMeta {
                title: "Project Seaglass planning — Meeting Notes (Mon 2026-11-24, 14:00–14:30)".to_string(),
                event_ref: "20261124#^seaglass".to_string(),
                accent: Some("2E7D4F".to_string()),
            },
        )
        .expect("transcript render");
        dump_meeting_pdf("transcript.pdf", &outcome.pdf);
        assert!(
            outcome.warnings.is_empty(),
            "transcript warnings: {:?}",
            outcome.warnings
        );
        let document = lopdf::Document::load(&outcome.pdf).expect("load PDF");
        assert!(
            document.get_pages().len() >= 2,
            "transcript starts on a new page"
        );
        let Some(first_text) = pdf_text(&outcome.pdf, Some(1), Some(1)) else {
            eprintln!("skipping page text checks: no pdftotext or mutool");
            return;
        };
        assert!(
            !first_text.contains("The schedule sounded like late June"),
            "transcript is not on page 1: {first_text}"
        );
        let Some(rest_text) = pdf_text(&outcome.pdf, Some(2), None) else {
            eprintln!("skipping page text checks: no pdftotext or mutool");
            return;
        };
        assert!(
            rest_text.contains("The schedule sounded like late June"),
            "transcript appendix: {rest_text}"
        );
        assert!(
            rest_text.contains("Transcript"),
            "appendix heading: {rest_text}"
        );
    }
}
