//! One already-captured refusal for every duplicate site.
//!
//! Every duplicate route (URL dedupe, same-reference stem walk, library
//! PDF as TARGET, collections) renders through [`AlreadyCaptured`] so all
//! refusals list every copy and print aligned hint rows, including the
//! selected in-place overwrite remedy.

use std::path::Path;

use super::marker::{parse_marker, read_pdf_marker};
use super::{CommandError, Config};
use crate::native::ref_library::ParsedFrontmatter;
use crate::native::style::Styler;

/// One existing copy of an already-captured source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedCopy {
    /// Vault-relative path (`ref/papers/foo.md`, `xlib/blogs/foo.pdf`).
    pub(crate) path: String,
    /// Filename stem for hint text.
    pub(crate) stem: String,
    /// Computed reading number from the note's `earlier:` lineage, when
    /// the copy is a filed family member. `None` for queued PDFs, stamp
    /// occupants, and notes outside a multi-reading family.
    pub(crate) reading: Option<u32>,
    /// Ref-note `status` or marker status, when known.
    pub(crate) status: Option<String>,
    /// Highlight count for ref notes; queued PDFs report 0.
    pub(crate) highlights: u64,
    /// Whether the highlights count was read (ref notes) or defaulted.
    pub(crate) has_highlights: bool,
    /// Ref-note `captured` or marker `captured`, when known.
    pub(crate) captured: Option<String>,
    /// True for queued intake PDFs.
    pub(crate) is_queued: bool,
}

/// One remedy row: a flag plus what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Remedy {
    /// Flag text, e.g. `-L`, `-N <name>`, `-f`.
    pub(crate) flag: String,
    /// Human description of the remedy.
    pub(crate) description: String,
}

impl Remedy {
    pub(crate) fn new(
        flag: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            flag: flag.into(),
            description: description.into(),
        }
    }
}

/// Every copy plus the remedies that apply to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlreadyCaptured {
    /// Reference title for the first line.
    pub(crate) title: String,
    /// Every existing copy, newest reading first.
    pub(crate) copies: Vec<CapturedCopy>,
    /// Remedy rows printed as `hint:` lines.
    pub(crate) remedies: Vec<Remedy>,
    /// Allocated `<root>__vN` stem for the next reread, when a family was
    /// selected. Carried for the reread remedy and the overwrite interim
    /// refusal; never printed here.
    pub(crate) next_stem: Option<String>,
}

impl AlreadyCaptured {
    pub(crate) fn new(
        title: impl Into<String>,
        copies: Vec<CapturedCopy>,
        remedies: Vec<Remedy>,
    ) -> Self {
        let mut copies = copies;
        sort_copies(&mut copies);
        Self {
            title: title.into(),
            copies,
            remedies,
            next_stem: None,
        }
    }

    /// Carry the allocated next-reread stem for the reread remedy and
    /// the overwrite interim refusal.
    pub(crate) fn with_next_stem(mut self, stem: impl Into<String>) -> Self {
        self.next_stem = Some(stem.into());
        self
    }

    /// Add the in-place overwrite remedy for the member the route would
    /// select by default. The note is replaced at the same path, while
    /// authored prose moves to `ref_history/`.
    pub(crate) fn with_overwrite_target(
        mut self,
        path: impl AsRef<str>,
    ) -> Self {
        self.remedies.push(Remedy::new(
            "-f overwrite",
            format!(
                "replace {} in place; authored notes move to ref_history/",
                path.as_ref()
            ),
        ));
        self
    }

    fn queued_only(&self) -> bool {
        !self.copies.is_empty() && self.copies.iter().all(|copy| copy.is_queued)
    }

    /// Render the full message body (without the `bob ref: error:` prefix).
    pub(crate) fn render(&self, styler: &Styler) -> String {
        let mut out = String::new();
        if self.queued_only() {
            out.push_str(&format!("already queued: {}", self.title));
        } else {
            out.push_str(&format!("already captured: {}", self.title));
        }
        for (index, copy) in self.copies.iter().enumerate() {
            out.push('\n');
            if index == 0 {
                out.push_str("  ");
            } else {
                out.push_str(&format!("  {} ", styler.dim("also:")));
            }
            out.push_str(&copy.render(styler));
        }
        if !self.remedies.is_empty() {
            let width = self
                .remedies
                .iter()
                .map(|remedy| remedy.flag.chars().count())
                .max()
                .unwrap_or(0);
            for remedy in &self.remedies {
                out.push('\n');
                let padded = pad_flag(&remedy.flag, width);
                out.push_str(&format!(
                    "{} {padded}  {}",
                    styler.dim("hint:"),
                    remedy.description
                ));
            }
        }
        out
    }

    /// Build the refusal for URL-dedupe hits: every PDF-backed ref note
    /// sharing the key, plus queued intake PDFs, grouped by path.
    pub(crate) fn from_recorded(
        config: &Config,
        hits: &[super::sources::RecordedSource],
        title_fallback: &str,
    ) -> Self {
        let mut seen = std::collections::BTreeSet::new();
        let mut copies = Vec::new();
        let mut title = title_fallback.to_string();
        for hit in hits {
            if !seen.insert(hit.path.clone()) {
                continue;
            }
            if hit.is_ref_note {
                let copy = copy_from_ref_note(config, &hit.path);
                if title == title_fallback || title.is_empty() {
                    if let Some(note_title) = note_title(&hit.path) {
                        title = note_title;
                    }
                }
                copies.push(copy);
            } else {
                let copy = copy_from_queued(config, &hit.path);
                if title == title_fallback || title.is_empty() {
                    if let Some(marker_title) = queued_title(&hit.path) {
                        title = marker_title;
                    }
                }
                copies.push(copy);
            }
        }
        let readings = super::recapture::family_readings_for(config);
        for copy in &mut copies {
            if copy.reading.is_none()
                && let Some(number) = readings.get(&copy.path)
            {
                copy.reading = Some(*number);
            }
        }
        Self::new(title, copies, Vec::new())
    }
}

fn pad_flag(flag: &str, width: usize) -> String {
    let mut padded = flag.to_string();
    let len = flag.chars().count();
    if len < width {
        padded.push_str(&" ".repeat(width - len));
    }
    padded
}

impl CapturedCopy {
    fn render(&self, styler: &Styler) -> String {
        let sep = styler.dim("·");
        let mut parts = vec![self.path.clone()];
        if let Some(reading) = self.reading {
            parts.push(format!("reading v{reading}"));
        }
        if let Some(status) = &self.status {
            parts.push(paint_status(styler, status));
        }
        if self.has_highlights || !self.is_queued {
            let label = if self.highlights == 1 {
                "1 highlight".to_string()
            } else {
                format!("{} highlights", self.highlights)
            };
            parts.push(label);
        }
        if let Some(captured) = &self.captured {
            parts.push(format!("captured {captured}"));
        }
        parts.join(&format!(" {sep} "))
    }
}

fn paint_status(styler: &Styler, status: &str) -> String {
    match status.to_lowercase().as_str() {
        "read" => styler.green(status),
        "wip" => styler.yellow(status),
        "next" | "ready" => styler.cyan(status),
        "abandoned" | "legacy" => styler.dim(status),
        _ => status.to_string(),
    }
}

/// Newest reading first (copies with a computed reading number sort
/// before copies without one), then the existing captured-date order.
/// Copies without readings keep their previous relative order.
fn sort_copies(copies: &mut [CapturedCopy]) {
    copies.sort_by(|a, b| match (a.reading, b.reading) {
        (Some(left), Some(right)) => right
            .cmp(&left)
            .then_with(|| compare_captured(a, b))
            .then_with(|| a.path.cmp(&b.path)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => {
            compare_captured(a, b).then_with(|| a.path.cmp(&b.path))
        }
    });
}

fn compare_captured(a: &CapturedCopy, b: &CapturedCopy) -> std::cmp::Ordering {
    match (&a.captured, &b.captured) {
        (Some(left), Some(right)) => right.cmp(left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Vault-relative display path for reports.
pub(crate) fn vault_display(config: &Config, path: &Path) -> String {
    super::io::vault_relative_path_value(config, path)
}

fn copy_from_ref_note(config: &Config, path: &Path) -> CapturedCopy {
    let display = vault_display(config, path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    let (status, highlights, captured) = ref_note_details(path);
    CapturedCopy {
        path: display,
        stem,
        reading: None,
        status,
        highlights: highlights.unwrap_or(0),
        has_highlights: highlights.is_some(),
        captured,
        is_queued: false,
    }
}

fn copy_from_queued(config: &Config, path: &Path) -> CapturedCopy {
    let display = vault_display(config, path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    let (status, captured) = queued_details(path);
    CapturedCopy {
        path: display,
        stem,
        reading: None,
        status,
        highlights: 0,
        has_highlights: false,
        captured,
        is_queued: true,
    }
}

/// Copy for a stamp occupant that may be a PDF, sidecar, bundle, note,
/// or id reference. Ref notes and PDFs carry details; other occupants
/// list the path alone.
pub(crate) fn copy_from_occupant(config: &Config, path: &Path) -> CapturedCopy {
    let display = vault_display(config, path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    {
        return copy_from_ref_note(config, path);
    }
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        let (status, captured) = queued_details(path);
        let highlights = ref_note_highlights_for_pdf(config, path);
        return CapturedCopy {
            path: display,
            stem,
            reading: None,
            status,
            highlights: highlights.unwrap_or(0),
            has_highlights: highlights.is_some(),
            captured,
            is_queued: is_queued_path(config, path),
        };
    }
    CapturedCopy {
        path: display,
        stem,
        reading: None,
        status: None,
        highlights: 0,
        has_highlights: false,
        captured: None,
        is_queued: false,
    }
}

fn is_queued_path(config: &Config, path: &Path) -> bool {
    super::stamp::relative_inside(path, &config.xlib_dir).is_some()
}

fn ref_note_highlights_for_pdf(_config: &Config, _path: &Path) -> Option<u64> {
    None
}

fn ref_note_details(
    path: &Path,
) -> (Option<String>, Option<u64>, Option<String>) {
    let contents = match std::fs::read_to_string(path).ok() {
        Some(contents) => contents,
        None => return (None, None, None),
    };
    let (frontmatter, _) =
        match super::frontmatter::split_frontmatter(&contents) {
            Some(split) => split,
            None => return (None, None, None),
        };
    let parsed = ParsedFrontmatter::parse(&frontmatter);
    let status = parsed
        .get_str("status")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let highlights = parsed
        .get_str("highlights_count")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let captured = parsed
        .get_str("captured")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    (status, highlights, captured)
}

fn note_title(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let split = super::frontmatter::split_frontmatter(&contents)?;
    let parsed = ParsedFrontmatter::parse(&split.0);
    parsed
        .get_str("title")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn queued_details(path: &Path) -> (Option<String>, Option<String>) {
    let marker = match read_pdf_marker(path).ok() {
        Some(marker) => marker,
        None => return (None, None),
    };
    let projection = match parse_marker(&marker.contents).ok() {
        Some(projection) => projection,
        None => return (None, None),
    };
    let status = projection
        .get("status")
        .and_then(|value| value.as_string())
        .map(str::to_string);
    let captured = projection
        .get("captured")
        .and_then(|value| value.as_string())
        .map(str::to_string);
    (status, captured)
}

fn queued_title(path: &Path) -> Option<String> {
    let marker = read_pdf_marker(path).ok()?;
    let projection = parse_marker(&marker.contents).ok()?;
    projection
        .get("title")
        .and_then(|value| value.as_string())
        .map(str::to_string)
}

/// Print a create/collection error: the first line after `error:`, then
/// indented detail rows, then every `hint:` row. Returns the exit code.
pub(crate) fn report_error(
    command: &str,
    error: CommandError,
    styler: &Styler,
) -> i32 {
    eprintln!("{}", format_error(command, &error.message, styler));
    error.exit_code.unwrap_or(1)
}

/// Format an error body for stderr, preserving every detail row and every
/// `hint:` row.
pub(crate) fn format_error(
    command: &str,
    message: &str,
    styler: &Styler,
) -> String {
    let mut lines = message.lines();
    let first = lines.next().unwrap_or_default();
    let mut out = vec![format!("{command}: {}: {first}", styler.red("error"))];
    for line in lines {
        if line.starts_with("hint:") {
            let body = line.strip_prefix("hint:").unwrap_or("").trim_start();
            out.push(format!("{} {body}", styler.dim("hint:")));
        } else if let Some(styled) = style_indexed_row(line, styler) {
            out.push(styled);
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

/// Style collection-style indexed rows (`  1  example.com/…`): dim the
/// index so the URL column lines up. Other detail rows pass through.
fn style_indexed_row(line: &str, styler: &Styler) -> Option<String> {
    let row = line.strip_prefix("  ")?;
    let digits = row.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || row.get(digits..)?.get(..2)? != "  " {
        return None;
    }
    Some(format!(
        "  {}  {}",
        styler.dim(&row[..digits]),
        &row[digits + 2..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(
        path: &str,
        status: Option<&str>,
        highlights: u64,
        captured: Option<&str>,
        is_queued: bool,
    ) -> CapturedCopy {
        copy_with_reading(path, None, status, highlights, captured, is_queued)
    }

    fn copy_with_reading(
        path: &str,
        reading: Option<u32>,
        status: Option<&str>,
        highlights: u64,
        captured: Option<&str>,
        is_queued: bool,
    ) -> CapturedCopy {
        CapturedCopy {
            path: path.to_string(),
            stem: Path::new(path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string(),
            reading,
            status: status.map(str::to_string),
            highlights,
            has_highlights: true,
            captured: captured.map(str::to_string),
            is_queued,
        }
    }

    #[test]
    fn single_captured_copy_renders_detail_and_hints() {
        let refusal = AlreadyCaptured::new(
            "Attention Is All You Need",
            vec![copy(
                "ref/papers/attention_is_all_you_need.md",
                Some("wip"),
                3,
                Some("2026-09-01"),
                false,
            )],
            vec![
                Remedy::new("-N <name>", "capture under a different name"),
                Remedy::new("-L", "narrate it instead"),
            ],
        );
        let plain = refusal.render(&Styler::plain());
        assert!(
            plain.starts_with("already captured: Attention Is All You Need"),
            "{plain}"
        );
        assert!(
            plain.contains(
                "  ref/papers/attention_is_all_you_need.md · wip · 3 highlights · captured 2026-09-01"
            ),
            "{plain}"
        );
        assert!(
            plain.contains("hint: -N <name>  capture under a different name"),
            "{plain}"
        );
        assert!(
            plain.contains("hint: -L         narrate it instead"),
            "{plain}"
        );
    }

    #[test]
    fn several_copies_list_every_path_with_also_rows() {
        let refusal = AlreadyCaptured::new(
            "Series",
            vec![
                copy(
                    "ref/blogs/series.md",
                    Some("read"),
                    41,
                    Some("2026-03-02"),
                    false,
                ),
                copy(
                    "ref/blogs/series_other.md",
                    Some("wip"),
                    0,
                    Some("2026-09-01"),
                    false,
                ),
            ],
            vec![Remedy::new("-N <name>", "capture under a different name")],
        );
        let plain = refusal.render(&Styler::plain());
        assert!(
            plain.contains("  ref/blogs/series_other.md"),
            "newest capture first:\n{plain}",
        );
        assert!(plain.contains("  also: ref/blogs/series.md"), "{plain}");
    }

    #[test]
    fn queued_only_renders_already_queued_with_force_remedy() {
        let refusal = AlreadyCaptured::new(
            "Queued",
            vec![CapturedCopy {
                path: "xlib/blogs/queued.pdf".to_string(),
                stem: "queued".to_string(),
                reading: None,
                status: None,
                highlights: 0,
                has_highlights: false,
                captured: None,
                is_queued: true,
            }],
            vec![Remedy::new("-f", "replace the queued PDF")],
        );
        let plain = refusal.render(&Styler::plain());
        assert!(plain.starts_with("already queued: Queued"), "{plain}");
        assert!(
            plain.contains("hint: -f  replace the queued PDF"),
            "{plain}"
        );
    }

    #[test]
    fn colored_output_dims_separators_and_hints() {
        let refusal = AlreadyCaptured::new(
            "Title",
            vec![copy(
                "ref/papers/a.md",
                Some("read"),
                1,
                Some("2026-01-01"),
                false,
            )],
            vec![Remedy::new("-L", "narrate it")],
        );
        let colored = refusal.render(&Styler::colored());
        assert!(colored.contains("\u{1b}[2m·\u{1b}[0m"), "{colored:?}");
        assert!(colored.contains("\u{1b}[2mhint:\u{1b}[0m"), "{colored:?}");
        let plain = refusal.render(&Styler::plain());
        assert!(!plain.contains("\u{1b}["), "{plain:?}");
    }

    #[test]
    fn family_copies_render_reading_numbers_newest_first() {
        let refusal = AlreadyCaptured::new(
            "Attention Is All You Need",
            vec![
                copy_with_reading(
                    "ref/papers/attention_is_all_you_need.md",
                    Some(1),
                    Some("read"),
                    41,
                    Some("2026-03-02"),
                    false,
                ),
                copy_with_reading(
                    "ref/papers/attention_is_all_you_need__v2.md",
                    Some(2),
                    Some("wip"),
                    3,
                    Some("2026-09-01"),
                    false,
                ),
            ],
            vec![Remedy::new("-N <name>", "capture under a different name")],
        );
        let plain = refusal.render(&Styler::plain());
        assert!(
            plain.starts_with("already captured: Attention Is All You Need"),
            "{plain}"
        );
        let v2 = plain
            .find("ref/papers/attention_is_all_you_need__v2.md")
            .expect("v2 row");
        let v1 = plain
            .find("ref/papers/attention_is_all_you_need.md ·")
            .expect("v1 row");
        assert!(v2 < v1, "newest reading first:\n{plain}");
        assert!(
            plain.contains(
                "  ref/papers/attention_is_all_you_need__v2.md · reading v2 · wip · 3 highlights · captured 2026-09-01"
            ),
            "{plain}"
        );
        assert!(
            plain.contains(
                "  also: ref/papers/attention_is_all_you_need.md · reading v1 · read · 41 highlights · captured 2026-03-02"
            ),
            "{plain}"
        );
    }

    #[test]
    fn readings_sort_before_queued_copies() {
        let refusal = AlreadyCaptured::new(
            "Mixed",
            vec![
                copy("xlib/blogs/mixed.pdf", None, 0, None, true),
                copy_with_reading(
                    "ref/blogs/mixed.md",
                    Some(1),
                    Some("ready"),
                    0,
                    Some("2026-09-01"),
                    false,
                ),
            ],
            vec![Remedy::new("-f", "replace the queued PDF")],
        );
        let plain = refusal.render(&Styler::plain());
        let filed = plain.find("ref/blogs/mixed.md").expect("filed row");
        let queued = plain.find("xlib/blogs/mixed.pdf").expect("queued row");
        assert!(filed < queued, "readings first:\n{plain}");
        assert!(plain.contains("reading v1"), "{plain}");
    }

    #[test]
    fn next_stem_is_carried_but_never_rendered() {
        let refusal = AlreadyCaptured::new(
            "Title",
            vec![copy(
                "ref/papers/a.md",
                Some("read"),
                1,
                Some("2026-01-01"),
                false,
            )],
            vec![Remedy::new("-L", "narrate it")],
        )
        .with_next_stem("a__v2");
        assert_eq!(refusal.next_stem.as_deref(), Some("a__v2"));
        let plain = refusal.render(&Styler::plain());
        assert!(!plain.contains("a__v2"), "{plain}");
    }

    #[test]
    fn from_recorded_enriches_family_readings_newest_first() {
        let root = std::env::temp_dir().join(format!(
            "bob-duplicate-{}-{}-family",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(root.join("ref/papers")).expect("ref dir");
        for (rel, earlier) in [
            ("ref/papers/enriched.md", String::new()),
            (
                "ref/papers/enriched__v2.md",
                "earlier:\n  - \"[[ref/papers/enriched|v1 · 2026-01-01]]\"\n"
                    .to_string(),
            ),
        ] {
            std::fs::write(
                root.join(rel),
                format!(
                    "---\ntitle: Enriched\nsource_url: https://example.com/enriched\nsource_pdf: lib/papers/backing.pdf\nstatus: ready\nhighlights_count: 7\ncaptured: 2026-09-01\n{earlier}---\n# Enriched\n",
                ),
            )
            .expect("write fixture note");
        }
        let config = Config {
            bob_dir: root.clone(),
            lib_dir: root.join("lib"),
            ref_dir: root.join("ref"),
            xlib_dir: root.join("xlib"),
        };
        let url_key = super::super::clip_url::validate_and_clean(
            "https://example.com/enriched",
        )
        .expect("fixture url")
        .dedupe_key;
        let hits: Vec<super::super::sources::RecordedSource> =
            ["ref/papers/enriched.md", "ref/papers/enriched__v2.md"]
                .iter()
                .map(|rel| super::super::sources::RecordedSource {
                    dedupe_key: url_key.clone(),
                    path: root.join(rel),
                    is_ref_note: true,
                    source_pdf: Some("lib/papers/backing.pdf".to_string()),
                    has_audio: false,
                })
                .collect();
        let refusal =
            AlreadyCaptured::from_recorded(&config, &hits, "Enriched");
        let readings: Vec<Option<u32>> =
            refusal.copies.iter().map(|copy| copy.reading).collect();
        assert_eq!(readings, vec![Some(2), Some(1)], "{readings:?}");
        let plain = refusal.render(&Styler::plain());
        assert!(plain.contains("enriched__v2.md · reading v2"), "{plain}");
        assert!(
            plain.contains("also: ref/papers/enriched.md · reading v1"),
            "{plain}"
        );
    }

    #[test]
    fn format_error_keeps_every_hint_row() {
        let message = "already captured: Title\n  ref/papers/a.md · read\nhint: -N <name>  capture under a different name\nhint: -L  narrate it";
        let formatted =
            format_error("bob ref create", message, &Styler::plain());
        assert!(
            formatted
                .contains("bob ref create: error: already captured: Title"),
            "{formatted}"
        );
        assert!(
            formatted.contains("  ref/papers/a.md · read"),
            "{formatted}"
        );
        assert_eq!(formatted.matches("hint:").count(), 2, "{formatted}");
    }
}
