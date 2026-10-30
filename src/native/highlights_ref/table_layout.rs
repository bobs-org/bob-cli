//! Table-layout Lua pass for `bob ref create` PDFs.
//!
//! The filter ([`FILTER`]) classifies each table and rewrites prose tables
//! into single-column row records; other tables stay grids. The TeX macros
//! ([`HEADER_INCLUDES`]) draw the record environment. This module holds both
//! embedded assets, the JSON report the filter writes for bob, and the
//! `tables:` summary and warning formatting. The pass is not wired into
//! `bob ref create` yet.

use std::{fs, path::Path};

use serde::Deserialize;

use crate::native::style::Styler;

/// First pandoc `--lua-filter` once the pandoc-wiring phase lands.
pub(crate) const FILTER: &str = include_str!("table_layout.lua");

/// TeX macros for record environments, labels, and captions.
pub(crate) const HEADER_INCLUDES: &str = include_str!("table_layout.tex");

/// Frontmatter / document override: `auto | records | grid`.
pub(crate) const TABLES_METADATA_KEY: &str = "bob-tables";

/// Global default passed from config; must differ from
/// [`TABLES_METADATA_KEY`] so `--metadata` cannot clobber frontmatter.
pub(crate) const TABLES_DEFAULT_METADATA_KEY: &str = "bob-tables-default";

/// Word threshold for the auto prose rule.
pub(crate) const PROSE_WORDS_METADATA_KEY: &str = "bob-table-prose-words";

/// Pandoc metadata key carrying the JSON report path for the filter.
pub(crate) const REPORT_METADATA_KEY: &str = "bob-table-layout-report";

/// One table that stayed a grid even though records were requested.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct KeptGrid {
    pub(crate) table: u32,
    #[serde(default)]
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) requested: String,
}

/// The JSON report the filter writes (contract version 1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Report {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) tables: u32,
    #[serde(default)]
    pub(crate) records: u32,
    #[serde(default)]
    pub(crate) grids: u32,
    #[serde(default)]
    pub(crate) kept_grid: Vec<KeptGrid>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

/// How the render's table-layout report turned out.
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
            "unsupported table-layout report version: {}",
            report.version
        )),
        Ok(report) => ReportOutcome::Report(report),
    }
}

/// The `tables:` success line for a report, if the document has tables.
pub(crate) fn tables_line(report: &Report) -> Option<String> {
    if report.tables == 0 {
        return None;
    }
    Some(format!(
        "tables: {} {} · {} {}",
        report.records,
        if report.records == 1 {
            "record"
        } else {
            "records"
        },
        report.grids,
        if report.grids == 1 { "grid" } else { "grids" },
    ))
}

/// Print the `tables:` line on stdout and the report warnings on stderr.
pub(crate) fn emit_outcome(outcome: &ReportOutcome, styler: &Styler) {
    match outcome {
        ReportOutcome::Missing => {}
        ReportOutcome::Invalid(reason) => {
            eprintln!(
                "{}: table-layout report unreadable: {reason}",
                styler.warning_prefix()
            );
        }
        ReportOutcome::Report(report) => {
            if let Some(line) = tables_line(report) {
                println!("{line}");
            }
            for kept in &report.kept_grid {
                eprintln!(
                    "{}: table {} stayed a grid: {}",
                    styler.warning_prefix(),
                    kept.table,
                    kept.reason
                );
            }
            for warning in &report.warnings {
                eprintln!("{}: {warning}", styler.warning_prefix());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs, process::Command};

    const TWELVE: &str =
        "one two three four five six seven eight nine ten eleven twelve";
    const ELEVEN: &str =
        "one two three four five six seven eight nine ten eleven";

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "bob-cli-table-layout-{name}-{}",
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

    fn skip_pandoc(test_name: &str) -> Option<std::ffi::OsString> {
        let Some(pandoc) = super::super::create::pandoc_command() else {
            eprintln!("skipping {test_name}: pandoc is required");
            return None;
        };
        Some(pandoc)
    }

    struct FilterRun {
        output: String,
        report: Report,
    }

    fn run_filter(
        name: &str,
        source: &str,
        to: &str,
        extra: &[(&str, &str)],
    ) -> Option<FilterRun> {
        run_filter_from(
            name,
            source,
            super::super::code_catalog::MARKDOWN_PANDOC_READER,
            to,
            extra,
        )
    }

    fn run_filter_from(
        name: &str,
        source: &str,
        from: &str,
        to: &str,
        extra: &[(&str, &str)],
    ) -> Option<FilterRun> {
        let pandoc = skip_pandoc(name)?;
        let temp = TempDir::new(name);
        let filter_path = temp.path.join("table-layout.lua");
        let report_path = temp.path.join("table-layout.json");
        let ext = if from == "html" { "html" } else { "md" };
        let source_path = temp.path.join(format!("doc.{ext}"));
        fs::write(&filter_path, FILTER).expect("write filter");
        fs::write(&source_path, source).expect("write source");
        let mut cmd = Command::new(&pandoc);
        cmd.arg("-f")
            .arg(from)
            .arg("-t")
            .arg(to)
            .arg("--lua-filter")
            .arg(&filter_path)
            .arg("-M")
            .arg(format!("{REPORT_METADATA_KEY}={}", report_path.display()));
        for (key, value) in extra {
            cmd.arg("-M").arg(format!("{key}={value}"));
        }
        let output = cmd.arg(&source_path).output().expect("run pandoc");
        assert!(
            output.status.success(),
            "pandoc failed ({from}->{to}): status={:?} stderr={} stdout={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let ReportOutcome::Report(report) = read_report(&report_path) else {
            panic!(
                "filter must write a report: {:?}",
                read_report(&report_path)
            );
        };
        Some(FilterRun {
            output: stdout,
            report,
        })
    }

    fn prose_table(col2: &str) -> String {
        format!(
            "| Kind | Now | Later |\n\
             |---|---|---|\n\
             | Alpha | {col2} | extra |\n\
             | Beta | short | more |\n"
        )
    }

    fn numeric_table() -> &'static str {
        "| Phases | Count |\n\
         |---|---|\n\
         | 1-3 | 55 |\n\
         | 4-5 | 98 |\n"
    }

    fn count(haystack: &str, needle: &str) -> usize {
        haystack.match_indices(needle).count()
    }

    fn collapsed(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn find_in_order(haystack: &str, needles: &[&str]) {
        let mut rest = haystack;
        for needle in needles {
            let Some(pos) = rest.find(needle) else {
                panic!("missing {needle:?} in order: {haystack}");
            };
            rest = &rest[pos + needle.len()..];
        }
    }

    #[test]
    fn filter_declares_improve_grid_hook() {
        assert!(
            FILTER.contains("local function improve_grid"),
            "data-grid fills improve_grid; the hook must exist"
        );
        assert!(
            HEADER_INCLUDES.contains("BobTableRecords"),
            "TeX macros must define BobTableRecords"
        );
        assert!(
            HEADER_INCLUDES.contains("needspace"),
            "records use needspace, which is already declared"
        );
    }

    #[test]
    fn report_parses_full_contract() {
        let report = parse_report(
            r#"{
                "version": 1,
                "tables": 5,
                "records": 3,
                "grids": 2,
                "kept_grid": [{
                    "table": 4,
                    "reason": "row or column spans",
                    "requested": "auto"
                }],
                "warnings": ["bob-tables: unknown value 'rows' (use auto, records, or grid)"]
            }"#,
        );
        assert_eq!(report.version, 1);
        assert_eq!(report.tables, 5);
        assert_eq!(report.records, 3);
        assert_eq!(report.grids, 2);
        assert_eq!(
            report.kept_grid,
            vec![KeptGrid {
                table: 4,
                reason: "row or column spans".to_string(),
                requested: "auto".to_string(),
            }]
        );
        assert_eq!(
            report.warnings,
            vec![
                "bob-tables: unknown value 'rows' (use auto, records, or grid)"
                    .to_string()
            ]
        );
    }

    #[test]
    fn report_defaults_missing_lists() {
        let report =
            parse_report(r#"{"version": 1, "tables": 1, "records": 1}"#);
        assert!(report.kept_grid.is_empty());
        assert!(report.warnings.is_empty());
        assert_eq!(report.grids, 0);
    }

    #[test]
    fn read_report_rejects_wrong_version_and_bad_json() {
        let temp = TempDir::new("report-invalid");
        let bad_version = temp.path.join("version.json");
        fs::write(&bad_version, r#"{"version": 2, "tables": 1}"#)
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

    fn report_with(tables: u32, records: u32, grids: u32) -> Report {
        Report {
            version: 1,
            tables,
            records,
            grids,
            kept_grid: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn tables_line_covers_every_variant() {
        assert_eq!(tables_line(&report_with(0, 0, 0)), None);
        assert_eq!(
            tables_line(&report_with(5, 3, 2)),
            Some("tables: 3 records · 2 grids".to_string())
        );
        assert_eq!(
            tables_line(&report_with(2, 1, 1)),
            Some("tables: 1 record · 1 grid".to_string())
        );
        assert_eq!(
            tables_line(&report_with(1, 0, 1)),
            Some("tables: 0 records · 1 grid".to_string())
        );
    }

    #[test]
    fn emit_outcome_is_silent_when_missing() {
        emit_outcome(&ReportOutcome::Missing, &Styler::plain());
    }

    #[test]
    fn classification_prose_table_becomes_records() {
        let Some(run) = run_filter("prose", &prose_table(TWELVE), "latex", &[])
        else {
            return;
        };
        assert!(
            !run.output.contains("longtable"),
            "prose table must not stay a longtable: {}",
            run.output
        );
        assert!(
            run.output.contains("BobTableRecords"),
            "prose table must use BobTableRecords: {}",
            run.output
        );
        find_in_order(
            &collapsed(&run.output),
            &["Alpha", "Now", TWELVE, "Later", "extra", "Beta"],
        );
        assert_eq!(run.report.tables, 1);
        assert_eq!(run.report.records, 1);
        assert_eq!(run.report.grids, 0);
        assert!(run.report.kept_grid.is_empty());
    }

    #[test]
    fn classification_numeric_table_stays_grid() {
        let Some(run) = run_filter("numeric", numeric_table(), "latex", &[])
        else {
            return;
        };
        assert!(
            run.output.contains("longtable"),
            "numeric table stays a longtable: {}",
            run.output
        );
        assert!(
            !run.output.contains("BobTableRecords"),
            "numeric table must not become records: {}",
            run.output
        );
        assert_eq!(run.report.tables, 1);
        assert_eq!(run.report.records, 0);
        assert_eq!(run.report.grids, 1);
    }

    #[test]
    fn classification_honors_twelve_word_boundary() {
        let Some(eleven) =
            run_filter("eleven", &prose_table(ELEVEN), "latex", &[])
        else {
            return;
        };
        assert!(
            eleven.output.contains("longtable"),
            "11 words stay a grid: {}",
            eleven.output
        );
        assert_eq!(eleven.report.records, 0);
        let Some(twelve) =
            run_filter("twelve", &prose_table(TWELVE), "latex", &[])
        else {
            return;
        };
        assert!(
            twelve.output.contains("BobTableRecords"),
            "12 words become records: {}",
            twelve.output
        );
        assert_eq!(twelve.report.records, 1);
    }

    #[test]
    fn classification_prose_words_metadata_moves_boundary() {
        let extra = [(PROSE_WORDS_METADATA_KEY, "5")];
        let five = "one two three four five";
        let four = "one two three four";
        let Some(at) =
            run_filter("prose-5", &prose_table(five), "latex", &extra)
        else {
            return;
        };
        assert!(
            at.output.contains("BobTableRecords"),
            "5 words at threshold 5 are records: {}",
            at.output
        );
        let Some(below) =
            run_filter("prose-4", &prose_table(four), "latex", &extra)
        else {
            return;
        };
        assert!(
            below.output.contains("longtable"),
            "4 words at threshold 5 stay a grid: {}",
            below.output
        );
    }

    #[test]
    fn override_bob_records_forces_data_table() {
        let source =
            format!("::: {{.bob-records}}\n\n{}\n:::\n", numeric_table());
        let Some(run) = run_filter("force-records", &source, "latex", &[])
        else {
            return;
        };
        assert!(
            run.output.contains("BobTableRecords"),
            ".bob-records forces records: {}",
            run.output
        );
        assert!(
            !run.output.contains("longtable"),
            ".bob-records must drop the grid: {}",
            run.output
        );
        assert_eq!(run.report.records, 1);
    }

    #[test]
    fn override_bob_grid_keeps_prose_table() {
        let source =
            format!("::: {{.bob-grid}}\n\n{}\n:::\n", prose_table(TWELVE));
        let Some(run) = run_filter("force-grid", &source, "latex", &[]) else {
            return;
        };
        assert!(
            run.output.contains("longtable"),
            ".bob-grid keeps a grid: {}",
            run.output
        );
        assert!(
            !run.output.contains("BobTableRecords"),
            ".bob-grid must not rewrite: {}",
            run.output
        );
        assert_eq!(run.report.grids, 1);
        assert!(run.report.kept_grid.is_empty());
    }

    #[test]
    fn override_frontmatter_beats_default() {
        let source =
            format!("---\nbob-tables: grid\n---\n\n{}", prose_table(TWELVE));
        let extra = [(TABLES_DEFAULT_METADATA_KEY, "records")];
        let Some(run) =
            run_filter("fm-beats-default", &source, "latex", &extra)
        else {
            return;
        };
        assert!(
            run.output.contains("longtable"),
            "frontmatter grid beats default records: {}",
            run.output
        );
        assert_eq!(run.report.grids, 1);
        assert_eq!(run.report.records, 0);
    }

    #[test]
    fn override_both_classes_warns_and_falls_through() {
        let source = format!(
            "::: {{.bob-records .bob-grid}}\n\n{}\n:::\n",
            numeric_table()
        );
        let Some(run) = run_filter("both-classes", &source, "latex", &[])
        else {
            return;
        };
        assert!(
            run.output.contains("longtable"),
            "both classes are ignored; data table stays a grid: {}",
            run.output
        );
        assert_eq!(run.report.grids, 1);
        assert!(
            run.report.warnings.iter().any(|w| {
                w.contains("bob-records") && w.contains("bob-grid")
            }),
            "both-classes must warn: {:?}",
            run.report.warnings
        );
    }

    fn html_rowspan_prose() -> String {
        format!(
            "<table>\n\
             <thead><tr><th>K</th><th>V</th></tr></thead>\n\
             <tbody>\n\
             <tr><td rowspan=\"2\">Alpha</td>\
             <td>{TWELVE}</td></tr>\n\
             <tr><td>{TWELVE}</td></tr>\n\
             </tbody>\n\
             </table>\n"
        )
    }

    fn html_foot_prose() -> String {
        format!(
            "<table>\n\
             <thead><tr><th>K</th><th>V</th></tr></thead>\n\
             <tbody><tr><td>Alpha</td><td>{TWELVE}</td></tr></tbody>\n\
             <tfoot><tr><td>T</td><td>foot</td></tr></tfoot>\n\
             </table>\n"
        )
    }

    fn html_header_link_prose() -> String {
        format!(
            "<table>\n\
             <thead><tr><th>Feature</th>\
             <th><a href=\"https://example.com\">Docs</a></th></tr></thead>\n\
             <tbody><tr><td>Alpha</td><td>{TWELVE}</td></tr></tbody>\n\
             </table>\n"
        )
    }

    #[test]
    fn complex_tables_stay_grid_and_are_reported() {
        let source = format!(
            "{}\n\n{}\n\n{}",
            html_rowspan_prose(),
            html_foot_prose(),
            html_header_link_prose()
        );
        let Some(run) =
            run_filter_from("complex", &source, "html", "latex", &[])
        else {
            return;
        };
        assert!(
            run.output.contains("longtable"),
            "complex tables stay grids: {}",
            run.output
        );
        assert!(
            !run.output.contains("BobTableRecords"),
            "complex tables must not become records: {}",
            run.output
        );
        assert_eq!(run.report.tables, 3);
        assert_eq!(run.report.records, 0);
        assert_eq!(run.report.grids, 3);
        assert_eq!(run.report.kept_grid.len(), 3, "{:?}", run.report.kept_grid);
        assert_eq!(run.report.kept_grid[0].table, 1);
        assert_eq!(run.report.kept_grid[0].reason, "row or column spans");
        assert_eq!(run.report.kept_grid[0].requested, "auto");
        assert_eq!(run.report.kept_grid[1].table, 2);
        assert_eq!(run.report.kept_grid[1].reason, "footer row");
        assert_eq!(run.report.kept_grid[2].table, 3);
        assert!(
            run.report.kept_grid[2].reason.contains("link"),
            "header link reason: {:?}",
            run.report.kept_grid[2]
        );
    }

    #[test]
    fn fidelity_native_keeps_nodes_once_in_order() {
        let source = "\
            | Name | Notes |\n\
            |---|---|\n\
            | Alpha | see [site](https://example.com/x) and `code` and *em*[^1] plus several extra words in this cell now |\n\
            | Beta | |\n\
            \n\
            : A shared caption\n\
            \n\
            [^1]: a footnote\n";
        let Some(run) = run_filter("fidelity", source, "native", &[]) else {
            return;
        };
        assert!(
            run.output.contains("bob-table-records"),
            "wrapper class: {}",
            run.output
        );
        let html = format!(
            "<table id=\"mytbl\"><caption>A shared caption</caption>\
             <thead><tr><th>Name</th><th>Notes</th></tr></thead>\
             <tbody><tr><td>Alpha</td><td>{TWELVE}</td></tr></tbody></table>"
        );
        let Some(id_run) =
            run_filter_from("fidelity-id", &html, "html", "native", &[])
        else {
            return;
        };
        assert!(
            id_run.output.contains("mytbl"),
            "table id survives on the Div: {}",
            id_run.output
        );
        assert!(
            run.output.contains("BobTableCaption"),
            "caption macro: {}",
            run.output
        );
        find_in_order(
            &run.output,
            &["Str \"A\"", "Str \"shared\"", "Str \"caption\""],
        );
        assert!(
            run.output.contains("Link")
                && run.output.contains("https://example.com/x"),
            "link node survives: {}",
            run.output
        );
        assert!(
            run.output.contains("Code") && run.output.contains("code"),
            "inline code survives: {}",
            run.output
        );
        assert!(
            run.output.contains("Emph"),
            "emphasis survives: {}",
            run.output
        );
        assert!(
            run.output.contains("Note"),
            "footnote survives: {}",
            run.output
        );
        assert!(
            run.output.contains("—") || run.output.contains("\\8212"),
            "empty cell is an em dash: {}",
            run.output
        );
        assert_eq!(count(&run.output, "Alpha"), 1, "{}", run.output);
        assert_eq!(count(&run.output, "Beta"), 1, "{}", run.output);
        assert_eq!(
            count(&run.output, "https://example.com/x"),
            1,
            "{}",
            run.output
        );
        find_in_order(&run.output, &["Alpha", "https://example.com/x", "Beta"]);
        assert_eq!(run.report.records, 1);
    }

    #[test]
    fn lead_in_nine_word_cell_is_labeled() {
        let nine = "one two three four five six seven eight nine";
        let source = format!(
            "| Key | Value |\n\
             |---|---|\n\
             | {nine} | {TWELVE} |\n"
        );
        let Some(run) = run_filter("lead-nine", &source, "latex", &[]) else {
            return;
        };
        assert!(
            run.output.contains("BobTableRecords"),
            "forced by prose col2: {}",
            run.output
        );
        assert!(
            run.output.contains("BobRecordLabel"),
            "9-word col1 is a labeled field: {}",
            run.output
        );
        assert!(
            !run.output.contains(&format!("\\textbf{{{nine}}}")),
            "9-word col1 is not a bold lead-in: {}",
            run.output
        );
        assert!(run.output.contains(nine), "{}", run.output);
    }

    #[test]
    fn lead_in_two_column_short_keys_are_unlabeled_values() {
        let source = format!(
            "| Key | Value |\n\
             |---|---|\n\
             | Alpha | {TWELVE} |\n\
             | Beta | {TWELVE} |\n"
        );
        let Some(run) = run_filter("lead-two-col", &source, "latex", &[])
        else {
            return;
        };
        assert!(run.output.contains("BobTableRecords"), "{}", run.output);
        assert!(
            run.output.contains("\\textbf{Alpha}"),
            "short col1 is a bold lead-in: {}",
            run.output
        );
        assert!(
            run.output.contains("BobRecordValue"),
            "2-col value is indented: {}",
            run.output
        );
        assert!(
            !run.output.contains("BobRecordLabel"),
            "2-col lead-in must not label the value: {}",
            run.output
        );
    }

    #[test]
    fn robustness_malformed_prose_words_falls_back() {
        let extra = [(PROSE_WORDS_METADATA_KEY, "nope")];
        let Some(run) =
            run_filter("bad-words", &prose_table(TWELVE), "latex", &extra)
        else {
            return;
        };
        assert!(
            run.output.contains("BobTableRecords"),
            "fallback 12 still classifies prose: {}",
            run.output
        );
        assert!(
            run.report.warnings.iter().any(|w| {
                w.contains("bob-table-prose-words") && w.contains("nope")
            }),
            "malformed threshold must warn: {:?}",
            run.report.warnings
        );
        let Some(eleven) =
            run_filter("bad-words-11", &prose_table(ELEVEN), "latex", &extra)
        else {
            return;
        };
        assert!(
            eleven.output.contains("longtable"),
            "fallback 12 keeps 11-word cells as a grid: {}",
            eleven.output
        );
    }

    #[test]
    fn robustness_nested_table_is_complex() {
        let source = format!(
            "<table>\n\
             <thead><tr><th>K</th><th>V</th></tr></thead>\n\
             <tbody><tr><td>Alpha</td><td>{TWELVE}\
             <table><tr><td>inner</td></tr></table></td></tr></tbody>\n\
             </table>\n"
        );
        let Some(run) =
            run_filter_from("nested", &source, "html", "latex", &[])
        else {
            return;
        };
        assert!(
            run.output.contains("longtable"),
            "nested table stays a grid: {}",
            run.output
        );
        assert!(
            run.report
                .kept_grid
                .iter()
                .any(|k| k.reason == "nested table"),
            "nested table is complex: {:?}",
            run.report.kept_grid
        );
    }

    #[test]
    fn tables_in_lists_and_quotes_are_rewritten() {
        let source = format!(
            "> {TWELVE} quoted intro.\n\
             >\n\
             > | Key | Value |\n\
             > |---|---|\n\
             > | Alpha | {TWELVE} |\n\
             \n\
             - outer\n\
             \n\
               | Key | Value |\n\
               |---|---|\n\
               | Beta | {TWELVE} |\n"
        );
        let Some(run) = run_filter("nested-blocks", &source, "latex", &[])
        else {
            return;
        };
        assert_eq!(run.report.tables, 2, "{:?}", run.report);
        assert_eq!(run.report.records, 2, "{:?}", run.report);
        assert!(
            !run.output.contains("longtable"),
            "quoted and listed prose tables become records: {}",
            run.output
        );
    }

    #[test]
    fn xelatex_pdf_contains_labels_and_cell_text() {
        let Some(pandoc) = skip_pandoc("pdf") else {
            return;
        };
        if Command::new("xelatex").arg("--version").output().is_err() {
            eprintln!("skipping pdf: xelatex is required");
            return;
        }
        if Command::new("pdftotext").arg("-v").output().is_err() {
            eprintln!("skipping pdf: pdftotext is required");
            return;
        }
        let temp = TempDir::new("pdf");
        let filter_path = temp.path.join("table-layout.lua");
        let header_path = temp.path.join("header.tex");
        let source_path = temp.path.join("doc.md");
        let pdf_path = temp.path.join("doc.pdf");
        let report_path = temp.path.join("table-layout.json");
        fs::write(&filter_path, FILTER).expect("write filter");
        fs::write(&header_path, HEADER_INCLUDES).expect("write header");
        fs::write(&source_path, prose_table(TWELVE)).expect("write source");
        let output = Command::new(&pandoc)
            .arg("-f")
            .arg(super::super::code_catalog::MARKDOWN_PANDOC_READER)
            .arg("--lua-filter")
            .arg(&filter_path)
            .arg("--include-in-header")
            .arg(&header_path)
            .arg("--pdf-engine=xelatex")
            .arg("-M")
            .arg(format!("{REPORT_METADATA_KEY}={}", report_path.display()))
            .arg("-o")
            .arg(&pdf_path)
            .arg(&source_path)
            .output()
            .expect("run pandoc pdf");
        assert!(
            output.status.success(),
            "pandoc pdf failed: stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let text = Command::new("pdftotext")
            .arg("-layout")
            .arg(&pdf_path)
            .arg("-")
            .output()
            .expect("pdftotext");
        assert!(text.status.success(), "pdftotext failed");
        let haystack = String::from_utf8_lossy(&text.stdout);
        assert!(
            haystack.contains("Now") && haystack.contains("Later"),
            "PDF text must hold labels: {haystack}"
        );
        assert!(
            haystack.contains("Alpha")
                && haystack.contains("one two three four five"),
            "PDF text must hold cell text: {haystack}"
        );
    }
}
