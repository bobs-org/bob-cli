//! Collect and swap Mermaid code blocks for `bob ref create` PDFs.
//!
//! This phase wires the Lua filters through [`DiagramFilter`] on
//! [`super::create::RenderFilters`], but every production caller passes
//! `diagrams: None`, so PDF output stays byte-for-byte unchanged until
//! the wire phase turns the hook on. The kill switch, fence pre-scan,
//! collect pass, and results writer live here so that later phase can
//! orchestrate without growing a second copy of the pandoc contracts.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

use super::{CommandError, Result};

/// Env kill switch (A6). Values `off`, `0`, `false`, or `no`
/// (case-insensitive) disable the diagram pass.
pub(crate) const ENV_DIAGRAMS: &str = "BOB_HIGHLIGHTS_DIAGRAMS";

/// Pandoc metadata key carrying the collect-pass manifest path.
pub(crate) const MANIFEST_METADATA_KEY: &str = "bob-diagrams-manifest";

/// Pandoc metadata key carrying the swap-filter results path.
pub(crate) const RESULTS_METADATA_KEY: &str = "bob-diagrams-results";

/// Throwaway collect filter. Embedded like [`super::return_links::FILTER`].
pub(crate) const COLLECT_FILTER: &str = include_str!("diagrams_collect.lua");

/// First-position swap filter for both Markdown PDF routes.
pub(crate) const SWAP_FILTER: &str = include_str!("diagrams.lua");

/// Paths the swap filter needs when [`super::create::RenderFilters::diagrams`]
/// is `Some`. Production callers pass `None` until the wire phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagramFilter {
    pub(crate) filter: PathBuf,
    pub(crate) results: PathBuf,
}

/// `--lua-filter` then `--metadata bob-diagrams-results=…`. `filter` and
/// `results` are the paths the renderer should pass (already resolved
/// for cwd=scratch on the snapshot route).
pub(crate) fn apply_swap_filter_args(
    cmd: &mut Command,
    filter: &Path,
    results: &Path,
) {
    cmd.arg("--lua-filter")
        .arg(filter)
        .arg("--metadata")
        .arg(format!("{}={}", RESULTS_METADATA_KEY, results.display()));
}

/// Reader flags for the collect pass: the same flags the route's main
/// pandoc invocation uses, so keys always agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagramReader {
    /// Generic Markdown route: no `-f`, pandoc infers from the path.
    Inferred,
    /// Snapshot route: [`super::note_snapshot::profile::SNAPSHOT_PANDOC_READER`].
    Snapshot,
}

/// One unique Mermaid source, in first-seen document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollectedDiagram {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) occurrences: usize,
}

/// Per-id swap entry: a rendered PDF or a visible error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum DiagramOutcome {
    Pdf { pdf: String },
    Error { error: String },
}

/// `results.json` the swap filter reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DiagramResults {
    pub(crate) diagrams: BTreeMap<String, DiagramOutcome>,
    pub(crate) fallback_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    id: String,
    text: String,
}

/// `false` when `BOB_HIGHLIGHTS_DIAGRAMS` is `off`/`0`/`false`/`no`
/// (case-insensitive). Missing or any other value leaves the pass on.
pub(crate) fn enabled() -> bool {
    match crate::native::env::var(ENV_DIAGRAMS) {
        Ok(value) => {
            let folded = value.trim().to_ascii_lowercase();
            !matches!(folded.as_str(), "off" | "0" | "false" | "no")
        }
        Err(_) => true,
    }
}

/// Count candidate Mermaid fence openers. May over-match (a nested
/// example inside a `markdown` fence) and must never under-match.
///
/// Strips leading whitespace, `>` quote markers, and list markers, then
/// looks for three or more backticks or tildes whose remaining line
/// contains `mermaid`.
pub(crate) fn prescan(markdown: &str) -> usize {
    markdown
        .lines()
        .filter(|line| is_mermaid_fence_opener(line))
        .count()
}

fn is_mermaid_fence_opener(line: &str) -> bool {
    let rest = strip_quote_and_list_prefix(line);
    let Some(info) = fence_info(rest) else {
        return false;
    };
    info.to_ascii_lowercase().contains("mermaid")
}

fn strip_quote_and_list_prefix(line: &str) -> &str {
    let mut rest = line;
    loop {
        rest = rest.trim_start();
        if let Some(stripped) = rest.strip_prefix('>') {
            rest = stripped;
            continue;
        }
        if let Some(stripped) = strip_list_marker(rest) {
            rest = stripped;
            continue;
        }
        break;
    }
    rest
}

fn strip_list_marker(rest: &str) -> Option<&str> {
    let bytes = rest.as_bytes();
    if bytes.len() >= 2
        && matches!(bytes[0], b'-' | b'*' | b'+')
        && bytes[1].is_ascii_whitespace()
    {
        return Some(&rest[1..]);
    }
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let after = &rest[digits..];
    let mut chars = after.chars();
    match (chars.next(), chars.next()) {
        (Some(mark), Some(space))
            if matches!(mark, '.' | ')') && space.is_whitespace() =>
        {
            Some(&after[1..])
        }
        _ => None,
    }
}

fn fence_info(rest: &str) -> Option<&str> {
    let bytes = rest.as_bytes();
    let opener = *bytes.first()?;
    if opener != b'`' && opener != b'~' {
        return None;
    }
    let n = bytes.iter().take_while(|byte| **byte == opener).count();
    (n >= 3).then_some(&rest[n..])
}

/// Run pandoc with the collect filter and the route's reader flags.
/// Returns unique blocks in first-seen order, with occurrence counts.
pub(crate) fn collect(
    pandoc: &OsStr,
    source: &Path,
    reader: DiagramReader,
    workdir: &Path,
) -> Result<Vec<CollectedDiagram>> {
    fs::create_dir_all(workdir).map_err(|error| {
        CommandError::new(format!(
            "create diagrams workdir {}: {error}",
            workdir.display()
        ))
    })?;
    let filter_path = workdir.join("diagrams-collect.lua");
    fs::write(&filter_path, COLLECT_FILTER).map_err(|error| {
        CommandError::new(format!(
            "write collect filter {}: {error}",
            filter_path.display()
        ))
    })?;
    let manifest_path = workdir.join("manifest.json");
    let throwaway = workdir.join("collect.out");
    let mut cmd = Command::new(pandoc);
    cmd.arg(source);
    if let DiagramReader::Snapshot = reader {
        cmd.arg("-f")
            .arg(super::note_snapshot::profile::SNAPSHOT_PANDOC_READER);
    }
    let output = cmd
        .arg("-o")
        .arg(&throwaway)
        .arg("--lua-filter")
        .arg(&filter_path)
        .arg("--metadata")
        .arg(format!(
            "{}={}",
            MANIFEST_METADATA_KEY,
            manifest_path.display()
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            CommandError::new(format!(
                "run pandoc collect {}: {error}",
                Path::new(pandoc).display()
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = match (stderr.is_empty(), stdout.is_empty()) {
            (false, false) => format!("{stderr}\n{stdout}"),
            (false, true) => stderr,
            (true, false) => stdout,
            (true, true) => "pandoc produced no diagnostic output".to_string(),
        };
        return Err(CommandError::new(format!(
            "pandoc collect failed while reading {} (exit {}):\n{}",
            source.display(),
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            detail
        )));
    }
    let raw = fs::read_to_string(&manifest_path).map_err(|error| {
        CommandError::new(format!(
            "read diagrams manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    parse_manifest(&raw)
}

fn parse_manifest(raw: &str) -> Result<Vec<CollectedDiagram>> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| {
            CommandError::new(format!("parse diagrams manifest: {error}"))
        })?;
    let entries: Vec<ManifestEntry> = match value {
        serde_json::Value::Array(_) => {
            serde_json::from_value(value).map_err(|error| {
                CommandError::new(format!(
                    "parse diagrams manifest entries: {error}"
                ))
            })?
        }
        serde_json::Value::Object(map) if map.is_empty() => Vec::new(),
        other => {
            return Err(CommandError::new(format!(
                "diagrams manifest must be a JSON array, got {other}"
            )));
        }
    };
    let mut ordered: Vec<CollectedDiagram> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        if let Some(&i) = index.get(&entry.id) {
            ordered[i].occurrences += 1;
            continue;
        }
        index.insert(entry.id.clone(), ordered.len());
        ordered.push(CollectedDiagram {
            id: entry.id,
            text: entry.text,
            occurrences: 1,
        });
    }
    Ok(ordered)
}

/// Write `results.json` with absolute PDF paths per the shared contract.
pub(crate) fn write_results(
    path: &Path,
    results: &DiagramResults,
) -> Result<()> {
    let json = serde_json::to_string(results).map_err(|error| {
        CommandError::new(format!(
            "encode diagrams results {}: {error}",
            path.display()
        ))
    })?;
    fs::write(path, json).map_err(|error| {
        CommandError::new(format!(
            "write diagrams results {}: {error}",
            path.display()
        ))
    })
}

/// One-page PDF xdvipdfmx can include. Hand-written 7-bit ASCII so tests
/// do not depend on lopdf's empty-stream output, which XeLaTeX rejects.
#[cfg(test)]
pub(crate) fn write_one_page_pdf(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create diagram PDF parent");
    }
    fs::write(path, include_bytes!("diagrams_blank.pdf"))
        .expect("write one-page diagram PDF");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, ffi::OsStr, process::Command};

    use crate::native::env::TestEnvGuard;
    use crate::native::highlights_ref::create::pandoc_command;

    const FLOW: &str = "flowchart LR\n  A --> B\n";
    const SEQUENCE: &str = "sequenceDiagram\n  A->>B: hi\n";

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "bob-cli-diagrams-{name}-{}",
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

    fn skip_pandoc(test_name: &str) -> Option<std::ffi::OsString> {
        let Some(pandoc) = pandoc_command() else {
            eprintln!("skipping {test_name}: pandoc/xelatex is required");
            return None;
        };
        Some(pandoc)
    }

    fn collect_source() -> String {
        format!(
            "# Diagrams\n\n\
             ```text\n\
             flowchart LR\n\
               skip --> me\n\
             ```\n\n\
             ```\n\
             flowchart LR\n\
               also --> skip\n\
             ```\n\n\
             ~~~ {{.mermaid}}\n{FLOW}~~~\n\n\
             - nested\n\n\
               ```mermaid\n{FLOW}```\n\n\
             ```mermaid\n{SEQUENCE}```\n"
        )
    }

    #[test]
    fn kill_switch_reads_off_values_through_native_env() {
        for value in ["off", "OFF", "0", "false", "No", " nO "] {
            let _guard =
                TestEnvGuard::set(&[(ENV_DIAGRAMS, Some(OsStr::new(value)))]);
            assert!(!enabled(), "{value} must disable");
        }
        let _guard =
            TestEnvGuard::set(&[(ENV_DIAGRAMS, Some(OsStr::new("on")))]);
        assert!(enabled());
        let _guard = TestEnvGuard::set(&[(ENV_DIAGRAMS, None)]);
        assert!(enabled(), "unset leaves the pass on");
    }

    #[test]
    fn prescan_counts_backtick_tilde_attribute_quoted_and_list_fences() {
        let markdown = "\
```mermaid\nflowchart LR\n```\n\
~~~mermaid\nflowchart TD\n~~~\n\
``` {.mermaid}\nflowchart TB\n```\n\
> ```mermaid\n> flowchart LR\n> ```\n\
- ```mermaid\n  flowchart LR\n  ```\n\
  1. ```mermaid\n     flowchart LR\n     ```\n\
    ```mermaid\n    flowchart LR\n    ```\n\
````markdown\n```mermaid\nflowchart LR\n```\n````\n\
```text\nflowchart LR\n```\n\
```\nflowchart LR\n```\n\
plain flowchart LR in a paragraph\n";
        // 3 explicit + 1 quote + 1 list + 1 ordered + 1 indented + 1
        // nested-in-markdown over-match = 8. text/untagged/prose = 0.
        assert_eq!(prescan(markdown), 8);
        assert_eq!(prescan("# No diagrams\n\nHello.\n"), 0);
    }

    #[test]
    fn collect_finds_attribute_and_nested_fences_on_both_readers() {
        let Some(pandoc) = skip_pandoc("collect") else {
            return;
        };
        let temp = TempDir::new("collect");
        let source = temp.path.join("doc.md");
        fs::write(&source, collect_source()).expect("write source");
        for reader in [DiagramReader::Inferred, DiagramReader::Snapshot] {
            let workdir = temp.path.join(format!("{reader:?}"));
            let blocks =
                collect(&pandoc, &source, reader, &workdir).expect("collect");
            assert_eq!(
                blocks.len(),
                2,
                "{reader:?}: unique sources, untagged ignored: {blocks:?}"
            );
            let flow = blocks
                .iter()
                .find(|block| block.text.contains("flowchart LR"))
                .expect("flowchart");
            assert_eq!(flow.occurrences, 2, "{reader:?} de-duplicates");
            assert_eq!(flow.id.len(), 40, "sha1 hex: {}", flow.id);
            assert!(
                flow.id.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                flow.id
            );
            let seq = blocks
                .iter()
                .find(|block| block.text.contains("sequenceDiagram"))
                .expect("sequence");
            assert_eq!(seq.occurrences, 1);
            assert_ne!(flow.id, seq.id);
        }
    }

    #[test]
    fn write_results_emits_absolute_pdf_or_error_and_null_fallback() {
        let temp = TempDir::new("results");
        let pdf = temp.path.join("abcd.pdf");
        write_one_page_pdf(&pdf);
        let path = temp.path.join("results.json");
        let mut diagrams = BTreeMap::new();
        diagrams.insert(
            "a".repeat(40),
            DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        diagrams.insert(
            "b".repeat(40),
            DiagramOutcome::Error {
                error: "Parse error on line 4".to_string(),
            },
        );
        write_results(
            &path,
            &DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write");
        let raw = fs::read_to_string(&path).expect("read");
        let parsed: DiagramResults = serde_json::from_str(&raw).expect("parse");
        assert!(parsed.fallback_error.is_none());
        match parsed.diagrams.get(&"a".repeat(40)) {
            Some(DiagramOutcome::Pdf { pdf: written }) => {
                assert!(Path::new(written).is_absolute(), "{written}");
                assert_eq!(written, &pdf.display().to_string());
            }
            other => panic!("pdf entry: {other:?}"),
        }
        match parsed.diagrams.get(&"b".repeat(40)) {
            Some(DiagramOutcome::Error { error }) => {
                assert!(error.contains("Parse error"));
            }
            other => panic!("error entry: {other:?}"),
        }
        assert!(raw.contains("\"fallback_error\":null"), "{raw}");
    }

    fn swap_native(source: &str, results: &DiagramResults) -> Option<String> {
        let pandoc = skip_pandoc("swap native")?;
        let temp = TempDir::new("swap-native");
        let md = temp.path.join("doc.md");
        fs::write(&md, source).expect("write source");
        let filter = temp.path.join("diagrams.lua");
        fs::write(&filter, SWAP_FILTER).expect("write swap filter");
        let results_path = temp.path.join("results.json");
        write_results(&results_path, results).expect("write results");
        let output = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("native")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                results_path.display()
            ))
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    #[test]
    fn swap_preserves_identifier_and_drops_source_in_native() {
        let temp = TempDir::new("id-pdf");
        let pdf = temp.path.join("fig.pdf");
        write_one_page_pdf(&pdf);
        let Some(pandoc) = skip_pandoc("swap native collect") else {
            return;
        };
        let md = temp.path.join("doc.md");
        fs::write(&md, format!("``` {{#fig-flow .mermaid}}\n{FLOW}```\n"))
            .expect("write source");
        let workdir = temp.path.join("collect");
        let blocks = collect(&pandoc, &md, DiagramReader::Inferred, &workdir)
            .expect("collect");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let mut diagrams = BTreeMap::new();
        diagrams.insert(
            blocks[0].id.clone(),
            DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        let Some(native) = swap_native(
            &fs::read_to_string(&md).expect("read"),
            &DiagramResults {
                diagrams,
                fallback_error: None,
            },
        ) else {
            return;
        };
        assert!(
            native.contains("fig-flow"),
            "identifier preserved: {native}"
        );
        assert!(native.contains("Image"), "swapped to Image: {native}");
        assert!(!native.contains("flowchart LR"), "source gone: {native}");
    }

    #[test]
    fn swap_error_emits_italic_note_then_source() {
        let Some(pandoc) = skip_pandoc("swap error") else {
            return;
        };
        let temp = TempDir::new("swap-error");
        let md = temp.path.join("doc.md");
        fs::write(&md, format!("```mermaid\n{FLOW}```\n"))
            .expect("write source");
        let workdir = temp.path.join("collect");
        let blocks = collect(&pandoc, &md, DiagramReader::Inferred, &workdir)
            .expect("collect");
        let mut diagrams = BTreeMap::new();
        diagrams.insert(
            blocks[0].id.clone(),
            DiagramOutcome::Error {
                error: "Parse error on line 4: bad".to_string(),
            },
        );
        let filter = temp.path.join("diagrams.lua");
        fs::write(&filter, SWAP_FILTER).expect("write filter");
        let results_path = temp.path.join("results.json");
        write_results(
            &results_path,
            &DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let output = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                results_path.display()
            ))
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        let latex = String::from_utf8_lossy(&output.stdout);
        let note_at = latex
            .find("Mermaid diagram not rendered: Parse error on line 4")
            .unwrap_or_else(|| panic!("note: {latex}"));
        let source_at = latex
            .find("flowchart LR")
            .unwrap_or_else(|| panic!("source: {latex}"));
        assert!(note_at < source_at, "note before source: {latex}");
        assert!(
            latex.contains("\\emph{") || latex.contains("\\textit{"),
            "italic note: {latex}"
        );
    }

    #[test]
    fn absent_or_corrupt_results_leave_latex_unchanged() {
        let Some(pandoc) = skip_pandoc("swap corrupt") else {
            return;
        };
        let temp = TempDir::new("swap-corrupt");
        let md = temp.path.join("doc.md");
        let source = format!("```mermaid\n{FLOW}```\n");
        fs::write(&md, &source).expect("write source");
        let filter = temp.path.join("diagrams.lua");
        fs::write(&filter, SWAP_FILTER).expect("write filter");

        let baseline = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .output()
            .expect("baseline");
        assert!(baseline.status.success(), "{baseline:?}");
        let baseline_tex = String::from_utf8_lossy(&baseline.stdout);

        let missing = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                temp.path.join("missing.json").display()
            ))
            .output()
            .expect("missing results");
        assert!(missing.status.success(), "{missing:?}");
        assert_eq!(String::from_utf8_lossy(&missing.stdout), baseline_tex);

        let corrupt_path = temp.path.join("results.json");
        fs::write(&corrupt_path, "{not json").expect("corrupt");
        let corrupt = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                corrupt_path.display()
            ))
            .output()
            .expect("corrupt results");
        assert!(corrupt.status.success(), "{corrupt:?}");
        assert_eq!(String::from_utf8_lossy(&corrupt.stdout), baseline_tex);
    }
}
