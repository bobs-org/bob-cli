//! Paired return links for `bob ref create` Markdown PDFs.
//!
//! The second pandoc Lua filter ([`FILTER`]) tags every eligible
//! same-document link with a sequential letter and pairs it with a return
//! pill row at its target; the TeX macros ([`HEADER_INCLUDES`]) draw the tags
//! and pills. This module holds both embedded assets, the JSON report the
//! filter writes for bob, and the `links:` summary and warning formatting.
//! See `docs/highlights-create.md` (`Local links and return pills`).

use std::{collections::BTreeMap, fs, path::Path};

use serde::Deserialize;

use crate::native::style::Styler;

/// Second pandoc `--lua-filter`, run after the code-break/listen filter.
pub(crate) const FILTER: &str = include_str!("return_links.lua");

/// TeX macros for tags, pills, and return destinations.
pub(crate) const HEADER_INCLUDES: &str = include_str!("return_links.tex");

/// Pandoc metadata key carrying the JSON report path for the filter.
pub(crate) const REPORT_METADATA_KEY: &str = "bob-return-links-report";

fn default_true() -> bool {
    true
}

/// One link the filter could not resolve, rendered as plain text.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct DeadLink {
    #[serde(default)]
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) reason: String,
}

/// One duplicated id that at least one link resolved to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct DuplicateId {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) count: usize,
}

/// The JSON report the filter writes (contract version 1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Report {
    pub(crate) version: u32,
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) prefix: String,
    #[serde(default)]
    pub(crate) paired: u32,
    #[serde(default)]
    pub(crate) targets: u32,
    #[serde(default)]
    pub(crate) untagged: u32,
    #[serde(default)]
    pub(crate) github: u32,
    #[serde(default)]
    pub(crate) dead: Vec<DeadLink>,
    #[serde(default)]
    pub(crate) duplicates: Vec<DuplicateId>,
}

/// How the render's return-link report turned out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReportOutcome {
    /// No report file. Test-double pandoc wrappers never run the filter.
    Missing,
    /// Unreadable, bad JSON, or the wrong contract version.
    Invalid(String),
    /// The filter wrote a version-1 report.
    Report(Report),
}

/// Read the filter's JSON report from the scratch path.
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
            "unsupported return-link report version: {}",
            report.version
        )),
        Ok(report) => ReportOutcome::Report(report),
    }
}

/// The `links:` success line for a report, if one applies.
pub(crate) fn links_line(report: &Report) -> Option<String> {
    if !report.enabled {
        return Some("links: off (bob-return-links: false)".to_string());
    }
    if report.paired == 0
        && report.targets == 0
        && report.untagged == 0
        && report.github == 0
        && report.dead.is_empty()
    {
        return Some("links: none".to_string());
    }
    let mut line = format!(
        "links: {} paired ({} {})",
        report.paired,
        report.targets,
        if report.targets == 1 {
            "target"
        } else {
            "targets"
        }
    );
    if report.github > 0 {
        line.push_str(&format!(
            " · {} via GitHub-style {}",
            report.github,
            if report.github == 1 { "slug" } else { "slugs" }
        ));
    }
    if report.untagged > 0 {
        line.push_str(&format!(" · {} untagged", report.untagged));
    }
    if !report.dead.is_empty() {
        line.push_str(&format!(" · {} dead", report.dead.len()));
    }
    Some(line)
}

/// One stderr warning per distinct dead target, then per duplicated anchor.
pub(crate) fn warnings(report: &Report) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut first: BTreeMap<&str, &DeadLink> = BTreeMap::new();
    for dead in &report.dead {
        counts
            .entry(dead.target.as_str())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        if !first.contains_key(dead.target.as_str()) {
            first.insert(dead.target.as_str(), dead);
            seen.push(dead.target.as_str());
        }
    }
    for target in seen {
        let entry = first[target];
        if entry.reason == "ambiguous" {
            out.push(format!(
                "local link {target} (\"{}\") matches more than one heading by GitHub-style slug; rendered as plain text; give the heading an explicit {{#id}}",
                entry.text
            ));
        } else {
            let mut message = format!(
                "local link {target} (\"{}\") has no target; rendered as plain text",
                entry.text
            );
            if counts[target] > 1 {
                message.push_str(&format!(" ({} links)", counts[target]));
            }
            out.push(message);
        }
    }
    for duplicate in &report.duplicates {
        out.push(format!(
            "anchor #{} is defined {} times; its links still jump but get no return pills",
            duplicate.id, duplicate.count
        ));
    }
    out
}

/// Print the `links:` line on stdout and the report warnings on stderr.
pub(crate) fn emit_outcome(outcome: &ReportOutcome, styler: &Styler) {
    match outcome {
        ReportOutcome::Missing => {}
        ReportOutcome::Invalid(reason) => {
            eprintln!(
                "{}: return-link report unreadable: {reason}",
                styler.warning_prefix()
            );
        }
        ReportOutcome::Report(report) => {
            if let Some(line) = links_line(report) {
                println!("{line}");
            }
            for warning in warnings(report) {
                eprintln!("{}: {warning}", styler.warning_prefix());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs, process::Command};

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "bob-cli-return-links-{name}-{}",
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

    fn parse_report(json: &str) -> Report {
        serde_json::from_str(json).expect("parse report fixture")
    }

    #[test]
    fn report_parses_full_contract() {
        let report = parse_report(
            r##"{
                "version": 1, "enabled": true, "prefix": "bob:ret:",
                "paired": 27, "targets": 11, "untagged": 2, "github": 4,
                "dead": [{"target": "#nope", "text": "the old ranking", "reason": "missing"}],
                "duplicates": [{"id": "setup", "count": 2}]
            }"##,
        );
        assert_eq!(report.version, 1);
        assert!(report.enabled);
        assert_eq!(report.prefix, "bob:ret:");
        assert_eq!(report.paired, 27);
        assert_eq!(report.targets, 11);
        assert_eq!(report.untagged, 2);
        assert_eq!(report.github, 4);
        assert_eq!(
            report.dead,
            vec![DeadLink {
                target: "#nope".to_string(),
                text: "the old ranking".to_string(),
                reason: "missing".to_string(),
            }]
        );
        assert_eq!(
            report.duplicates,
            vec![DuplicateId {
                id: "setup".to_string(),
                count: 2,
            }]
        );
    }

    #[test]
    fn report_defaults_missing_lists_and_enabled() {
        let report =
            parse_report(r#"{"version": 1, "paired": 2, "targets": 1}"#);
        assert!(report.enabled);
        assert!(report.dead.is_empty());
        assert!(report.duplicates.is_empty());
        assert_eq!(report.prefix, "");
        assert_eq!(report.untagged, 0);
        assert_eq!(report.github, 0);
    }

    #[test]
    fn report_parses_opt_out() {
        let report = parse_report(r#"{"version": 1, "enabled": false}"#);
        assert!(!report.enabled);
    }

    #[test]
    fn read_report_rejects_wrong_version_and_bad_json() {
        let temp = TempDir::new("report-invalid");
        let bad_version = temp.path.join("version.json");
        fs::write(&bad_version, r#"{"version": 2, "enabled": true}"#)
            .expect("write fixture");
        assert!(
            matches!(read_report(&bad_version), ReportOutcome::Invalid(_)),
            "wrong version must be invalid"
        );
        let bad_json = temp.path.join("bad.json");
        fs::write(&bad_json, "{not json").expect("write fixture");
        assert!(
            matches!(read_report(&bad_json), ReportOutcome::Invalid(_)),
            "bad JSON must be invalid"
        );
        assert_eq!(
            read_report(&temp.path.join("absent.json")),
            ReportOutcome::Missing
        );
    }

    fn report_with(
        paired: u32,
        targets: u32,
        untagged: u32,
        github: u32,
        dead: Vec<DeadLink>,
    ) -> Report {
        Report {
            version: 1,
            enabled: true,
            prefix: "bob:ret:".to_string(),
            paired,
            targets,
            untagged,
            github,
            dead,
            duplicates: Vec::new(),
        }
    }

    fn dead(target: &str, text: &str, reason: &str) -> DeadLink {
        DeadLink {
            target: target.to_string(),
            text: text.to_string(),
            reason: reason.to_string(),
        }
    }

    #[test]
    fn links_line_covers_every_variant() {
        let mut off = report_with(0, 0, 0, 0, Vec::new());
        off.enabled = false;
        assert_eq!(
            links_line(&off),
            Some("links: off (bob-return-links: false)".to_string())
        );
        assert_eq!(
            links_line(&report_with(0, 0, 0, 0, Vec::new())),
            Some("links: none".to_string())
        );
        assert_eq!(
            links_line(&report_with(
                2,
                1,
                0,
                0,
                vec![dead("#nope", "old", "missing")]
            )),
            Some("links: 2 paired (1 target) · 1 dead".to_string())
        );
        assert_eq!(
            links_line(&report_with(27, 11, 2, 4, Vec::new())),
            Some(
                "links: 27 paired (11 targets) · 4 via GitHub-style slugs · 2 untagged"
                    .to_string()
            )
        );
        assert_eq!(
            links_line(&report_with(1, 1, 0, 1, Vec::new())),
            Some(
                "links: 1 paired (1 target) · 1 via GitHub-style slug"
                    .to_string()
            )
        );
    }

    #[test]
    fn warnings_cover_every_variant() {
        assert!(warnings(&report_with(0, 0, 0, 0, Vec::new())).is_empty());
        assert_eq!(
            warnings(&report_with(
                0,
                0,
                0,
                0,
                vec![dead("#nope", "the old ranking", "missing")]
            )),
            vec![
                "local link #nope (\"the old ranking\") has no target; rendered as plain text"
                    .to_string()
            ]
        );
        assert_eq!(
            warnings(&report_with(
                0,
                0,
                0,
                0,
                vec![
                    dead("#nope", "first", "missing"),
                    dead("#nope", "second", "missing"),
                    dead("#gone", "third", "missing"),
                ]
            )),
            vec![
                "local link #nope (\"first\") has no target; rendered as plain text (2 links)"
                    .to_string(),
                "local link #gone (\"third\") has no target; rendered as plain text"
                    .to_string(),
            ]
        );
        assert_eq!(
            warnings(&report_with(
                0,
                0,
                0,
                0,
                vec![dead("#setup", "setup", "ambiguous")]
            )),
            vec![
                "local link #setup (\"setup\") matches more than one heading by GitHub-style slug; rendered as plain text; give the heading an explicit {#id}"
                    .to_string()
            ]
        );
        let mut duplicated = report_with(0, 0, 0, 0, Vec::new());
        duplicated.duplicates = vec![DuplicateId {
            id: "setup".to_string(),
            count: 2,
        }];
        assert_eq!(
            warnings(&duplicated),
            vec![
                "anchor #setup is defined 2 times; its links still jump but get no return pills"
                    .to_string()
            ]
        );
    }

    struct FilterRender {
        latex: String,
        report: Report,
    }

    /// Run pandoc `--to=latex` with both filters in bob's order and a report
    /// path, like `render_temp_pdf` does. Skips when pandoc is missing.
    fn render_latex(name: &str, source: &str) -> Option<FilterRender> {
        let Some(pandoc) = super::super::create::pandoc_command() else {
            eprintln!("skipping return-link filter test: pandoc is required");
            return None;
        };
        let temp = TempDir::new(name);
        let code_break = temp.path.join("filter.lua");
        let return_filter = temp.path.join("return-links.lua");
        let report_path = temp.path.join("return-links.json");
        let source_path = temp.path.join("doc.md");
        fs::write(&code_break, super::super::create::PANDOC_CODE_BREAK_FILTER)
            .expect("write filter");
        fs::write(&return_filter, FILTER).expect("write return filter");
        fs::write(&source_path, source).expect("write source");

        let output = Command::new(&pandoc)
            .arg(&source_path)
            .arg("--to=latex")
            .arg("--lua-filter")
            .arg(&code_break)
            .arg("--lua-filter")
            .arg(&return_filter)
            .arg("-M")
            .arg(format!("{REPORT_METADATA_KEY}={}", report_path.display()))
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        let latex = String::from_utf8_lossy(&output.stdout).into_owned();
        let outcome = read_report(&report_path);
        let ReportOutcome::Report(report) = outcome else {
            panic!("filter must write a report: {outcome:?}");
        };
        Some(FilterRender { latex, report })
    }

    fn count_occurrences(haystack: &str, needle: &str) -> usize {
        haystack.match_indices(needle).count()
    }

    /// Names inside `MARKER{...}` in document order.
    fn braced_names(haystack: &str, marker: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = haystack;
        while let Some(position) = rest.find(marker) {
            let tail = &rest[position + marker.len()..];
            if let Some(end) = tail.find('}') {
                out.push(tail[..end].to_string());
                rest = &tail[end + 1..];
            } else {
                break;
            }
        }
        out
    }

    #[test]
    fn filter_pairs_links_from_prose_list_cell_and_footnote() {
        let Some(render) = render_latex(
            "pairing",
            "# Target\n\nSee [first](#target) in prose.\n\n- [second](#target) in a list\n\n| col |\n|-----|\n| [third](#target) in a cell |\n\nFootnote[^1].\n\n[^1]: [fourth](#target) in a footnote.\n",
        ) else {
            return;
        };
        for (anchor, tag) in [
            ("bob:ret:1", "ᵃ"),
            ("bob:ret:2", "ᵇ"),
            ("bob:ret:3", "ᶜ"),
            ("bob:ret:4", "ᵈ"),
        ] {
            assert!(
                render
                    .latex
                    .contains(&format!("\\BobReturnAnchor{{{anchor}}}")),
                "missing anchor {anchor}: {}",
                render.latex
            );
            assert!(
                render.latex.contains(&format!("\\BobTag{{{tag}}}")),
                "missing tag {tag}: {}",
                render.latex
            );
        }
        assert_eq!(
            count_occurrences(&render.latex, "\\BobBacklinks"),
            1,
            "exactly one pill row: {}",
            render.latex
        );
        let pills = braced_names(&render.latex, "\\BobBack{");
        assert_eq!(
            pills,
            vec![
                "bob:ret:1".to_string(),
                "bob:ret:2".to_string(),
                "bob:ret:3".to_string(),
                "bob:ret:4".to_string(),
            ],
            "pills stay in source order: {}",
            render.latex
        );
        assert_eq!(render.report.paired, 4);
        assert_eq!(render.report.targets, 1);
        assert!(render.report.dead.is_empty());
    }

    #[test]
    fn filter_leaves_skip_context_links_untagged() {
        let Some(render) = render_latex(
            "skip-contexts",
            "# Target\n\n## See [in-heading](#target)\n\nTable: caption with [in-caption](#target)\n\n| [in-head](#target) |\n|---|\n| body cell |\n\n![figure caption with [in-figcap](#target)](img.png)\n\n- [in-nav-one](#target)\n- [in-nav-two](#target)\n",
        ) else {
            return;
        };
        assert!(
            !render.latex.contains("\\BobTag"),
            "skip contexts gain no tags: {}",
            render.latex
        );
        assert!(
            !render.latex.contains("\\BobReturnAnchor"),
            "skip contexts gain no anchors: {}",
            render.latex
        );
        assert!(
            !render.latex.contains("\\BobBacklinks"),
            "untagged target gets no row: {}",
            render.latex
        );
        assert!(
            render.latex.contains("\\hyperref[target]"),
            "skip links stay working forward links: {}",
            render.latex
        );
        assert_eq!(render.report.paired, 0);
        assert_eq!(render.report.targets, 0);
        // Seven, not six: pandoc duplicates the figure-caption link into both
        // the Figure caption and the Image description, so the AST holds one
        // extra skip-context node. Both copies stay untagged forward links.
        assert_eq!(render.report.untagged, 7);
    }

    #[test]
    fn filter_resolves_decoded_and_github_aliases() {
        let Some(render) = render_latex(
            "resolution",
            "# Doc\n\n[d](#a%2Db) [g](#7-ranked-recommendations) [h](#__init__-method)\n\n# A B\n\n## 7. Ranked recommendations\n\n## `__init__` method\n",
        ) else {
            return;
        };
        assert!(
            render.latex.contains("\\hyperref[a-b]"),
            "percent-decoded id resolves: {}",
            render.latex
        );
        assert!(
            render.latex.contains("\\hyperref[ranked-recommendations]"),
            "GitHub slug resolves to the canonical id: {}",
            render.latex
        );
        assert!(
            render.latex.contains("\\hyperref[init__-method]"),
            "code-span slug resolves to the canonical id: {}",
            render.latex
        );
        assert_eq!(count_occurrences(&render.latex, "\\BobTag"), 3);
        assert_eq!(render.report.paired, 3);
        assert_eq!(render.report.targets, 3);
        assert_eq!(
            render.report.github, 2,
            "only alias routes count: {:?}",
            render.report
        );
        assert!(render.report.dead.is_empty());
    }

    #[test]
    fn filter_numbers_github_duplicates_and_rejects_ambiguous_aliases() {
        let Some(render) = render_latex(
            "aliases",
            "# Doc\n\n[n](#same-1) [x](#dup-1) [c](#ccc)\n\n## Same {#one}\n\n## Same {#two}\n\n## Dup {#aaa}\n\n## Dup {#bbb}\n\n## Dup-1 {#ccc}\n",
        ) else {
            return;
        };
        assert!(
            render.latex.contains("\\hyperref[two]"),
            "GitHub -1 numbering resolves: {}",
            render.latex
        );
        assert!(
            render.latex.contains("\\hyperref[ccc]"),
            "real id resolves: {}",
            render.latex
        );
        assert!(
            !render.latex.contains("\\hyperref[dup-1]"),
            "ambiguous alias must not stay a link: {}",
            render.latex
        );
        assert!(
            render.latex.contains("{x}"),
            "ambiguous link becomes plain text: {}",
            render.latex
        );
        assert_eq!(render.report.paired, 2);
        assert_eq!(render.report.github, 1);
        assert_eq!(
            render.report.dead,
            vec![DeadLink {
                target: "#dup-1".to_string(),
                text: "x".to_string(),
                reason: "ambiguous".to_string(),
            }]
        );
    }

    #[test]
    fn filter_prefers_real_ids_over_aliases() {
        let Some(render) = render_latex(
            "real-beats-alias",
            "# Doc\n\n[a](#dup-1)\n\n## Dup\n\n## Dup\n\n## Dup-1\n",
        ) else {
            return;
        };
        assert!(
            render.latex.contains("\\hyperref[dup-1]"),
            "real id beats the ambiguous alias: {}",
            render.latex
        );
        assert!(render.report.dead.is_empty());
        assert_eq!(render.report.paired, 1);
    }

    #[test]
    fn filter_renders_dead_links_as_plain_text() {
        let Some(render) =
            render_latex("dead", "# Target\n\nSee [gone *emph*](#nope).\n")
        else {
            return;
        };
        assert!(
            !render.latex.contains("\\hyperref[nope]"),
            "dead link must not stay a link: {}",
            render.latex
        );
        assert!(
            render.latex.contains("\\emph{emph}"),
            "dead link keeps its formatting: {}",
            render.latex
        );
        assert_eq!(
            render.report.dead,
            vec![DeadLink {
                target: "#nope".to_string(),
                text: "gone emph".to_string(),
                reason: "missing".to_string(),
            }]
        );
    }

    #[test]
    fn filter_leaves_non_capable_targets_untagged() {
        let Some(render) = render_latex(
            "non-capable",
            "# Doc\n\n[c](#mycode) [f](#myfig) [s](#setup) [h](#innerspan)\n\n``` {#mycode}\ncode\n```\n\n![alt](img.png){#myfig}\n\n::: {#setup}\nFirst.\n:::\n\n::: {#setup}\nSecond.\n:::\n\n# A [b]{#innerspan} c\n",
        ) else {
            return;
        };
        for id in ["mycode", "myfig", "setup", "innerspan"] {
            assert!(
                render.latex.contains(&format!("\\hyperref[{id}]")),
                "non-capable target {id} keeps its forward link: {}",
                render.latex
            );
        }
        assert!(
            !render.latex.contains("\\BobTag"),
            "non-capable targets gain no tags: {}",
            render.latex
        );
        assert!(
            !render.latex.contains("\\BobBacklinks")
                && !render.latex.contains("\\BobBackInline"),
            "non-capable targets get no rows: {}",
            render.latex
        );
        assert_eq!(render.report.paired, 0);
        assert_eq!(render.report.untagged, 4);
        assert_eq!(
            render.report.duplicates,
            vec![DuplicateId {
                id: "setup".to_string(),
                count: 2,
            }]
        );
    }

    #[test]
    fn filter_places_div_span_and_needspace_rows() {
        let Some(render) = render_latex(
            "placement",
            "# Doc\n\nSee [t](#tblsec) and [d](#mydiv) and [s](#myspan).\n\n## Tblsec head {#tblsec}\n\n| a |\n|---|\n| b |\n\n::: {#mydiv}\nDiv content.\n:::\n\n[target span]{#myspan} here.\n",
        ) else {
            return;
        };
        let latex = &render.latex;
        let div_label = latex.find("\\label{mydiv}").expect("div label");
        let div_row = latex[div_label..]
            .find("\\BobBacklinks")
            .map(|offset| div_label + offset)
            .expect("div row");
        let div_text = latex.find("Div content.").expect("div text");
        assert!(
            div_label < div_row && div_row < div_text,
            "Div row is the Div's first block: {latex}"
        );
        assert!(
            latex.contains("\\BobBackInline{\\BobBackCompact{bob:ret:3}{ᶜ}}"),
            "Span target gets a compact inline row: {latex}"
        );
        let needspace = latex
            .find("\\needspace{16\\baselineskip}")
            .expect("needspace guard");
        let heading = latex.find("\\subsection{Tblsec head}").expect("heading");
        assert!(
            needspace < heading,
            "needspace goes before a pill heading followed by a table: {latex}"
        );
        assert_eq!(render.report.paired, 3);
        assert_eq!(render.report.targets, 3);
    }

    #[test]
    fn filter_opt_out_returns_the_document_unchanged() {
        let Some(pandoc) = super::super::create::pandoc_command() else {
            eprintln!("skipping return-link filter test: pandoc is required");
            return;
        };
        let temp = TempDir::new("opt-out");
        let code_break = temp.path.join("filter.lua");
        let return_filter = temp.path.join("return-links.lua");
        let report_path = temp.path.join("return-links.json");
        let source_path = temp.path.join("doc.md");
        fs::write(&code_break, super::super::create::PANDOC_CODE_BREAK_FILTER)
            .expect("write filter");
        fs::write(&return_filter, FILTER).expect("write return filter");
        fs::write(
            &source_path,
            "---\nbob-return-links: false\n---\n\n# Target\n\nSee [x](#target).\n",
        )
        .expect("write source");

        let both = Command::new(&pandoc)
            .arg(&source_path)
            .arg("--to=latex")
            .arg("--lua-filter")
            .arg(&code_break)
            .arg("--lua-filter")
            .arg(&return_filter)
            .arg("-M")
            .arg(format!("{REPORT_METADATA_KEY}={}", report_path.display()))
            .output()
            .expect("run pandoc");
        assert!(both.status.success(), "{both:?}");
        let solo = Command::new(&pandoc)
            .arg(&source_path)
            .arg("--to=latex")
            .arg("--lua-filter")
            .arg(&code_break)
            .output()
            .expect("run pandoc");
        assert!(solo.status.success(), "{solo:?}");
        assert_eq!(
            String::from_utf8_lossy(&both.stdout),
            String::from_utf8_lossy(&solo.stdout),
            "opt-out output is byte-identical to the code-break filter alone"
        );
        let ReportOutcome::Report(report) = read_report(&report_path) else {
            panic!("opt-out must still write a report");
        };
        assert!(!report.enabled);
    }

    #[test]
    fn filter_leaves_listen_cards_untouched() {
        let Some(render) = render_latex(
            "listen",
            "# Target\n\n<div class=\"listen\">\n\n[script](#target) and more\n\n</div>\n",
        ) else {
            return;
        };
        assert!(
            render.latex.contains("\\BobListenCard"),
            "listen card still renders: {}",
            render.latex
        );
        assert!(
            !render.latex.contains("\\BobTag"),
            "no tag inside the listen card: {}",
            render.latex
        );
        assert_eq!(render.report.paired, 0);
        assert!(render.report.dead.is_empty());
    }

    #[test]
    fn filter_renumbers_anchors_on_prefix_collision() {
        let Some(render) = render_latex(
            "prefix-collision",
            "# T\n\n[a](#bob:ret:1) [b](#t)\n\n::: {#bob:ret:1}\nD.\n:::\n",
        ) else {
            return;
        };
        assert!(
            render.latex.contains("\\BobReturnAnchor{bob:ret1:1}"),
            "anchors move off the colliding prefix: {}",
            render.latex
        );
        assert_eq!(render.report.prefix, "bob:ret1:");
        assert_eq!(render.report.paired, 2);
    }

    #[test]
    fn filter_counts_tags_in_bijective_base_23() {
        let mut source = "# T\n\n".to_string();
        for number in 1..=24 {
            source.push_str(&format!("[l{number}](#t) "));
        }
        let Some(render) = render_latex("tag-sequence", &source) else {
            return;
        };
        assert!(
            render.latex.contains("\\BobTag{ᶻ}"),
            "23rd tag is z: {}",
            render.latex
        );
        assert!(
            render.latex.contains("\\BobTag{ᵃᵃ}"),
            "24th tag is aa: {}",
            render.latex
        );
        assert_eq!(render.report.paired, 24);
        assert_eq!(render.report.targets, 1);
    }

    #[test]
    fn filter_keeps_every_anchor_paired_with_exactly_one_pill() {
        let Some(render) = render_latex(
            "invariant",
            "# Doc\n\n[a](#alpha) [b](#alpha) [dead](#nope) [skip](#alpha)\n\n## Alpha\n\n## Skip {#skip}\n\nSee [c](#skip).\n\n::: {#skip}\nDup.\n:::\n\n::: {#skip}\nDup.\n:::\n\n[span link](#myspan) and [target span]{#myspan}.\n",
        ) else {
            return;
        };
        let anchors = braced_names(&render.latex, "\\BobReturnAnchor{");
        let mut pills = braced_names(&render.latex, "\\BobBack{");
        pills.extend(braced_names(&render.latex, "\\BobBackCompact{"));
        assert_eq!(
            anchors.len(),
            pills.len(),
            "every anchor has exactly one pill: {}",
            render.latex
        );
        let mut anchors_sorted = anchors.clone();
        let mut pills_sorted = pills.clone();
        anchors_sorted.sort();
        pills_sorted.sort();
        assert_eq!(
            anchors_sorted, pills_sorted,
            "anchor names match pill names one to one: {}",
            render.latex
        );
        assert_eq!(anchors.len(), render.report.paired as usize);
    }
}
