//! Non-fatal SVG-image fallback for generic Markdown reference PDFs.
//!
//! [`FILTER`] replaces SVG embeds with a same-stem raster sibling when one is
//! available, or with visible placeholder text otherwise. Its report is
//! emitted alongside the other optional render-pass reports.

use std::{fs, path::Path};

use serde::Deserialize;

use crate::native::style::Styler;

/// Pandoc Lua filter embedded in the bob binary.
pub(crate) const FILTER: &str = include_str!("image_guard.lua");

/// Metadata key for the original Markdown source directory.
pub(crate) const BASE_METADATA_KEY: &str = "bob-image-base";

/// Metadata key for the JSON report path written by [`FILTER`].
pub(crate) const REPORT_METADATA_KEY: &str = "bob-image-guard-report";

/// One SVG image handled by the filter.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct HandledImage {
    pub(crate) target: String,
    pub(crate) outcome: String,
    pub(crate) replacement: String,
}

/// The JSON report written by the filter (contract version 1).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Report {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) images: Vec<HandledImage>,
}

/// Result of reading the optional image-guard report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReportOutcome {
    Missing,
    Invalid(String),
    Report(Report),
}

/// Read the image-guard report from the render scratch directory.
pub(crate) fn read_report(path: &Path) -> ReportOutcome {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ReportOutcome::Missing;
        }
        Err(error) => return ReportOutcome::Invalid(error.to_string()),
    };
    match serde_json::from_str::<Report>(&text) {
        Err(error) => ReportOutcome::Invalid(error.to_string()),
        Ok(report) if report.version != 1 => ReportOutcome::Invalid(format!(
            "unsupported image-guard report version: {}",
            report.version
        )),
        Ok(report) => ReportOutcome::Report(report),
    }
}

/// Return the single stdout summary line for a non-empty report.
pub(crate) fn images_line(report: &Report) -> Option<String> {
    let total = report.images.len();
    if total == 0 {
        return None;
    }
    let siblings = report
        .images
        .iter()
        .filter(|image| image.outcome == "sibling")
        .count();
    let placeholders = report
        .images
        .iter()
        .filter(|image| image.outcome == "placeholder")
        .count();

    if total == 1 {
        let image = &report.images[0];
        return Some(match image.outcome.as_str() {
            "sibling" => {
                let extension = Path::new(&image.replacement)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("raster")
                    .to_ascii_uppercase();
                format!(
                    "images: 1 SVG image printed from its sibling {extension}"
                )
            }
            _ => "images: 1 SVG image replaced with an Image not printable placeholder".to_string(),
        });
    }

    if placeholders == 0 {
        let extensions: std::collections::BTreeSet<_> = report
            .images
            .iter()
            .filter(|image| image.outcome == "sibling")
            .filter_map(|image| {
                Path::new(&image.replacement)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase)
            })
            .collect();
        let suffix = if extensions.len() == 1 {
            format!(
                " sibling {} files",
                extensions.iter().next().unwrap().to_ascii_uppercase()
            )
        } else {
            " sibling raster files".to_string()
        };
        return Some(format!(
            "images: {siblings} SVG images printed from{suffix}"
        ));
    }

    if siblings == 0 {
        return Some(format!(
            "images: {placeholders} SVG images replaced with Image not printable placeholders"
        ));
    }

    Some(format!(
        "images: {total} SVG images · {siblings} printed from sibling rasters · {placeholders} placeholders"
    ))
}

/// Return one warning for each placeholder image.
pub(crate) fn warning_lines(report: &Report) -> Vec<String> {
    report
        .images
        .iter()
        .filter(|image| image.outcome == "placeholder")
        .map(|image| {
            format!(
                "SVG image {} is not printable; add a same-stem PNG next to it, or use a Mermaid fence",
                image.target
            )
        })
        .collect()
}

/// Emit the summary and non-fatal placeholder warnings.
pub(crate) fn emit_report(report: &Report, styler: &Styler) {
    if let Some(line) = images_line(report) {
        println!("{line}");
    }
    for warning in warning_lines(report) {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
}

/// Emit an optional Lua report. Missing reports are normal for test doubles;
/// malformed reports warn without changing the render result.
pub(crate) fn emit_outcome(outcome: &ReportOutcome, styler: &Styler) {
    match outcome {
        ReportOutcome::Missing => {}
        ReportOutcome::Invalid(reason) => eprintln!(
            "{}: image-guard report unreadable: {reason}",
            styler.warning_prefix()
        ),
        ReportOutcome::Report(report) => emit_report(report, styler),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs, path::PathBuf, process::Command};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "bob-cli-image-guard-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct FilterRun {
        output: String,
        report: Report,
    }

    fn run_filter(name: &str, source: &str) -> Option<FilterRun> {
        let Some(pandoc) = super::super::create::pandoc_command() else {
            eprintln!("skipping {name}: pandoc is required");
            return None;
        };
        let temp = TempDir::new(name);
        let source_path = temp.path.join("doc.md");
        let filter_path = temp.path.join("image-guard.lua");
        let report_path = temp.path.join("image-guard.json");
        fs::write(&source_path, source).expect("write source");
        fs::write(&filter_path, FILTER).expect("write filter");

        let output = Command::new(pandoc)
            .arg("-f")
            .arg("markdown")
            .arg("-t")
            .arg("native")
            .arg("--lua-filter")
            .arg(&filter_path)
            .arg("-M")
            .arg(format!("{BASE_METADATA_KEY}={}", temp.path.display()))
            .arg("-M")
            .arg(format!("{REPORT_METADATA_KEY}={}", report_path.display()))
            .arg(&source_path)
            .output()
            .expect("run pandoc");
        assert!(
            output.status.success(),
            "pandoc failed: status={:?} stderr={} stdout={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let ReportOutcome::Report(report) = read_report(&report_path) else {
            panic!("filter must write an image report");
        };
        Some(FilterRun {
            output: String::from_utf8_lossy(&output.stdout).into_owned(),
            report,
        })
    }

    #[test]
    fn filter_uses_sibling_png_and_keeps_figure_caption_and_width() {
        let Some(pandoc) = super::super::create::pandoc_command() else {
            eprintln!("skipping sibling test: pandoc is required");
            return;
        };
        let temp = TempDir::new("sibling-figure");
        fs::write(temp.path.join("figure.png"), b"png").expect("write sibling");
        fs::write(
            temp.path.join("doc.md"),
            "![Figure caption](figure.svg){#fig width=50%}\n",
        )
        .expect("write source");
        let filter_path = temp.path.join("image-guard.lua");
        let report_path = temp.path.join("image-guard.json");
        fs::write(&filter_path, FILTER).expect("write filter");
        let output = Command::new(pandoc)
            .arg("-f")
            .arg("markdown")
            .arg("-t")
            .arg("native")
            .arg("--lua-filter")
            .arg(&filter_path)
            .arg("-M")
            .arg(format!("{BASE_METADATA_KEY}={}", temp.path.display()))
            .arg("-M")
            .arg(format!("{REPORT_METADATA_KEY}={}", report_path.display()))
            .arg(temp.path.join("doc.md"))
            .output()
            .expect("run pandoc");
        assert!(
            output.status.success(),
            "pandoc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8_lossy(&output.stdout);
        assert!(output.contains("figure.png"), "{output}");
        assert!(output.contains("Caption"), "Figure caption lost: {output}");
        assert!(output.contains("Str \"Figure\""), "alt text lost: {output}");
        assert!(
            output.contains("Str \"caption\""),
            "caption text lost: {output}"
        );
        assert!(output.contains("50%"), "width lost: {output}");
        assert!(output.contains("fig"), "identifier lost: {output}");
        let ReportOutcome::Report(report) = read_report(&report_path) else {
            panic!("filter must write report");
        };
        assert_eq!(report.images.len(), 1);
        assert_eq!(report.images[0].target, "figure.svg");
        assert_eq!(report.images[0].outcome, "sibling");
        assert_eq!(report.images[0].replacement, "figure.png");
    }

    #[test]
    fn filter_uses_placeholder_for_missing_and_remote_images_and_keeps_caption()
    {
        let Some(run) = run_filter(
            "placeholders",
            "![Missing caption](missing.svg)\n\n![Remote](https://example.test/remote.svg)\n\n![Data](data:image/svg+xml;base64,PHN2Zy8+)\n",
        ) else {
            return;
        };
        assert!(run.output.contains("Image not printable"), "{}", run.output);
        assert!(run.output.contains("missing.svg"), "{}", run.output);
        assert!(
            run.output.contains("https://example.test/remote.svg"),
            "{}",
            run.output
        );
        assert!(
            run.output.contains("data:image/svg+xml"),
            "data URI image must become a placeholder: {}",
            run.output
        );
        assert!(
            run.output.contains("Caption")
                && run.output.contains("Str \"Missing\"")
                && run.output.contains("Str \"caption\""),
            "Figure caption lost: {}",
            run.output
        );
        assert_eq!(run.report.images.len(), 3);
        assert!(run
            .report
            .images
            .iter()
            .all(|image| image.outcome == "placeholder"));
    }

    #[test]
    fn filter_strips_query_before_sibling_lookup_and_leaves_non_svg_images_alone(
    ) {
        let Some(pandoc) = super::super::create::pandoc_command() else {
            eprintln!("skipping query test: pandoc is required");
            return;
        };
        let temp = TempDir::new("query-and-non-svg");
        fs::write(temp.path.join("query.png"), b"png").expect("write sibling");
        fs::write(temp.path.join("space name.png"), b"png")
            .expect("write decoded sibling");
        fs::write(
            temp.path.join("doc.md"),
            "![Query](query.svg?download=1)\n\n![Decoded](space%20name.svg)\n\n![Raster](plain.png)\n",
        )
        .expect("write source");
        let filter_path = temp.path.join("image-guard.lua");
        let report_path = temp.path.join("image-guard.json");
        fs::write(&filter_path, FILTER).expect("write filter");
        let output = Command::new(pandoc)
            .arg("-f")
            .arg("markdown")
            .arg("-t")
            .arg("native")
            .arg("--lua-filter")
            .arg(&filter_path)
            .arg("-M")
            .arg(format!("{BASE_METADATA_KEY}={}", temp.path.display()))
            .arg("-M")
            .arg(format!("{REPORT_METADATA_KEY}={}", report_path.display()))
            .arg(temp.path.join("doc.md"))
            .output()
            .expect("run pandoc");
        assert!(
            output.status.success(),
            "pandoc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.contains("query.png"),
            "query was not stripped: {output}"
        );
        assert!(
            output.contains("space name.png"),
            "percent-decoded target was not checked: {output}"
        );
        assert!(
            output.contains("plain.png"),
            "non-SVG image changed: {output}"
        );
        let ReportOutcome::Report(report) = read_report(&report_path) else {
            panic!("filter must write report");
        };
        assert_eq!(report.images.len(), 2);
        assert_eq!(report.images[0].target, "query.svg?download=1");
        assert_eq!(report.images[0].replacement, "query.png");
        assert_eq!(report.images[1].target, "space%20name.svg");
        assert_eq!(report.images[1].replacement, "space name.png");
    }

    #[test]
    fn report_output_lines_cover_summary_and_per_placeholder_warning() {
        let report = Report {
            version: 1,
            images: vec![
                HandledImage {
                    target: "one.svg".to_string(),
                    outcome: "sibling".to_string(),
                    replacement: "one.png".to_string(),
                },
                HandledImage {
                    target: "https://example.test/two.svg".to_string(),
                    outcome: "placeholder".to_string(),
                    replacement:
                        "Image not printable · https://example.test/two.svg"
                            .to_string(),
                },
            ],
        };
        assert_eq!(
            images_line(&report).as_deref(),
            Some("images: 2 SVG images · 1 printed from sibling rasters · 1 placeholders")
        );
        assert_eq!(
            warning_lines(&report),
            vec!["SVG image https://example.test/two.svg is not printable; add a same-stem PNG next to it, or use a Mermaid fence"]
        );
        let one = Report {
            version: 1,
            images: vec![report.images[0].clone()],
        };
        assert_eq!(
            images_line(&one).as_deref(),
            Some("images: 1 SVG image printed from its sibling PNG")
        );
    }

    #[test]
    fn report_reader_rejects_bad_json_and_wrong_version() {
        let temp = TempDir::new("bad-report");
        let path = temp.path.join("report.json");
        fs::write(&path, "not json").expect("write bad report");
        assert!(matches!(read_report(&path), ReportOutcome::Invalid(_)));
        fs::write(&path, r#"{"version":2,"images":[]}"#)
            .expect("write wrong version");
        assert!(matches!(read_report(&path), ReportOutcome::Invalid(_)));
    }
}
