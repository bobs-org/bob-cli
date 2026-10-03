//! Vault source matching, markdown discovery, and result path extraction.

use super::*;
use serde_json::Value;
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PathExtraction {
    pub(super) paths: Vec<String>,
    pub(super) warnings: Vec<String>,
}

pub(super) fn extract_source_paths(
    paths: &[String],
    strict: bool,
) -> Result<PathExtraction, DataviewError> {
    let mut collector = PathCollector::default();
    for (index, path) in paths.iter().enumerate() {
        let context = format!("source path {}", index + 1);
        collector.add_raw_path(path, &context);
    }
    collector.finish(strict)
}

pub(super) fn extract_dql_paths(
    result: &Value,
    strict: bool,
) -> Result<PathExtraction, DataviewError> {
    let mut collector = PathCollector::default();
    match result.get("type").and_then(Value::as_str) {
        Some("list") => collect_list_paths(result, &mut collector),
        Some("table") => collect_table_paths(result, &mut collector),
        Some("task") => collect_task_paths(result, &mut collector),
        Some("calendar") => collect_calendar_paths(result, &mut collector),
        Some(other) => collector.warn(format!(
            "DQL {other} results do not have supported path extraction"
        )),
        None => collect_unknown_result_paths(result, &mut collector),
    }
    collector.finish(strict)
}
pub(super) fn source_order_key(path: &str) -> (String, String) {
    let name = note_stem(path).unwrap_or_else(|| path.to_string());
    (name.to_ascii_lowercase(), path.to_ascii_lowercase())
}

pub(super) fn normalize_source_tag(tag: &str) -> String {
    let tag = tag.trim();
    if tag.starts_with('#') {
        tag.to_string()
    } else {
        format!("#{tag}")
    }
}

pub(super) fn tag_matches_source(page_tag: &str, source_tag: &str) -> bool {
    page_tag == source_tag
        || page_tag
            .strip_prefix(source_tag)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub(super) fn page_tags(page: &index::DataviewPage) -> Vec<String> {
    page.source_tags.clone()
}

pub(super) fn page_outlink_paths(page: &index::DataviewPage) -> Vec<String> {
    page_file_array(page, "outlinks")
        .into_iter()
        .filter_map(|value| match value {
            DataviewValue::Link(link) => Some(link.path),
            _ => None,
        })
        .collect()
}

pub(super) fn page_file_array(
    page: &index::DataviewPage,
    field: &str,
) -> Vec<DataviewValue> {
    let Some(DataviewValue::Object(file)) = page.fields.get("file") else {
        return Vec::new();
    };
    let Some(DataviewValue::Array(values)) = file.get(field) else {
        return Vec::new();
    };
    values.as_ref().clone()
}

pub(super) fn source_link_base(path: &str) -> &str {
    path.split_once('#').map_or(path, |(base, _)| base)
}

pub(super) fn collect_native_markdown_paths(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), DataviewError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        }
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.clone(),
                error,
            }
        })?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if !is_hidden_path_component(&name)
                && name != OsStr::new("_conflicts")
            {
                collect_native_markdown_paths(&path, paths)?;
            }
        } else if file_type.is_file() && has_markdown_extension(&path) {
            paths.push(path);
        }
    }

    Ok(())
}

pub(super) fn is_hidden_path_component(component: &OsStr) -> bool {
    component.to_string_lossy().starts_with('.')
}

pub(super) fn has_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

pub(super) fn native_frontmatter_block(contents: &str) -> Option<&str> {
    let marker_len = if contents.starts_with("---\r\n") {
        5
    } else if contents.starts_with("---\n") {
        4
    } else {
        return None;
    };

    let rest = &contents[marker_len..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let line_content = line.trim_end_matches(['\r', '\n']);
        if line_content == "---" {
            return Some(&rest[..offset]);
        }
        offset += line.len();
    }

    None
}

pub(super) fn unquote_native_scalar(value: &str) -> String {
    if value.len() < 2 {
        return value.to_string();
    }

    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let Some(last) = value.chars().last() else {
        return String::new();
    };
    if !matches!(first, '"' | '\'') || first != last {
        return value.to_string();
    }

    let inner = &value[first.len_utf8()..value.len() - last.len_utf8()];
    if first == '\'' {
        return inner.replace("''", "'");
    }

    let mut output = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let Some(escaped) = chars.next() else {
                output.push(ch);
                break;
            };
            output.push(match escaped {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
        } else {
            output.push(ch);
        }
    }
    output
}

pub(super) fn normalize_native_source_folder(
    source: &str,
) -> Result<String, String> {
    let mut folder = source.trim().replace('\\', "/");
    while let Some(stripped) = folder.strip_prefix("./") {
        folder = stripped.to_string();
    }
    folder = folder.trim_matches('/').to_string();

    if folder.is_empty() {
        return Err("native folder source must not be empty".to_string());
    }
    if folder.contains('\0') {
        return Err("native folder source contains a NUL byte".to_string());
    }
    for segment in folder.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!(
                "native folder source {source:?} is not a clean \
                 vault-relative folder"
            ));
        }
    }

    Ok(folder)
}

pub(super) fn native_link_target(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let link = if let Some(rest) = trimmed.strip_prefix("[[") {
        let end = rest.find("]]")?;
        &rest[..end]
    } else {
        trimmed
    };
    let before_alias = link.split_once('|').map_or(link, |(target, _)| target);
    let before_subpath = before_alias
        .split_once('#')
        .map_or(before_alias, |(target, _)| target);
    let target = before_subpath.trim().replace('\\', "/");
    (!target.is_empty()).then_some(target)
}

pub(super) fn native_expression_link(raw: &str) -> Option<DataviewLink> {
    let (target, display) = raw
        .split_once('|')
        .map_or((raw, None), |(target, display)| {
            (target, Some(display.trim().to_string()))
        });
    let target = target.trim();
    if target.is_empty() {
        return None;
    }

    Some(DataviewLink::new(
        normalized_link_literal_path(target),
        display.filter(|display| !display.is_empty()),
        false,
        target.to_string(),
    ))
}

pub(super) fn normalized_link_literal_path(target: &str) -> String {
    let (base, subpath) = target
        .split_once('#')
        .map_or((target, None), |(base, subpath)| (base, Some(subpath)));
    let mut path = normalize_note_path(base.trim())
        .unwrap_or_else(|_| target.trim().replace('\\', "/"));
    if let Some(subpath) = subpath.filter(|subpath| !subpath.is_empty()) {
        path.push('#');
        path.push_str(subpath);
    }
    path
}

pub(super) fn comparable_link_path(raw: &str) -> Option<String> {
    native_link_target(raw).and_then(|target| normalize_note_path(&target).ok())
}

pub(super) fn note_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}
pub(super) fn collect_list_paths(
    result: &Value,
    collector: &mut PathCollector,
) {
    let Some(values) = result_array(result) else {
        collector.warn("DQL list result missing values array".to_string());
        return;
    };

    if identifier_is_grouped(result.get("primaryMeaning")) {
        warn_grouped_rows("DQL list row", values.len(), collector);
        return;
    }

    for (index, value) in values.iter().enumerate() {
        let context = format!("DQL list row {}", index + 1);
        if !collector.add_identity(value, &context) {
            collector.warn(format!("{context} has no source note identity"));
        }
    }
}

pub(super) fn collect_table_paths(
    result: &Value,
    collector: &mut PathCollector,
) {
    let Some(rows) = result_array(result) else {
        collector.warn("DQL table result missing values array".to_string());
        return;
    };

    if identifier_is_grouped(result.get("idMeaning")) {
        warn_grouped_rows("DQL table row", rows.len(), collector);
        return;
    }

    for (index, row) in rows.iter().enumerate() {
        let context = format!("DQL table row {}", index + 1);
        match table_row_identity(row) {
            Some(identity) if collector.add_identity(identity, &context) => {}
            _ => {
                collector.warn(format!("{context} has no source note identity"))
            }
        }
    }
}

pub(super) fn collect_task_paths(
    result: &Value,
    collector: &mut PathCollector,
) {
    let Some(values) = result_array(result) else {
        collector.warn("DQL task result missing values array".to_string());
        return;
    };

    let mut row_number = 0;
    for value in values {
        collect_task_grouping(value, collector, &mut row_number);
    }
}

pub(super) fn collect_calendar_paths(
    result: &Value,
    collector: &mut PathCollector,
) {
    let Some(values) = result_array(result) else {
        collector.warn("DQL calendar result missing values array".to_string());
        return;
    };

    for (index, row) in values.iter().enumerate() {
        let context = format!("DQL calendar row {}", index + 1);
        let identity =
            row.get("link").or_else(|| row.get("path")).unwrap_or(row);
        if !collector.add_identity(identity, &context) {
            collector.warn(format!("{context} has no source note identity"));
        }
    }
}

pub(super) fn collect_unknown_result_paths(
    result: &Value,
    collector: &mut PathCollector,
) {
    if let Some(rows) = result_array(result) {
        for (index, row) in rows.iter().enumerate() {
            let context = format!("DQL row {}", index + 1);
            if !collector.add_identity(row, &context) {
                collector
                    .warn(format!("{context} has no source note identity"));
            }
        }
    } else {
        collector.warn(
            "DQL result missing a recognized type and values array".to_string(),
        );
    }
}

pub(super) fn collect_task_grouping(
    value: &Value,
    collector: &mut PathCollector,
    row_number: &mut usize,
) {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                collect_task_grouping(entry, collector, row_number);
            }
        }
        Value::Object(map)
            if map.contains_key("key") && map.contains_key("rows") =>
        {
            collect_task_grouping(&map["rows"], collector, row_number);
        }
        Value::Object(_) => {
            *row_number += 1;
            let context = format!("DQL task row {}", *row_number);
            let identity = value
                .get("path")
                .or_else(|| value.get("link"))
                .or_else(|| value.get("section"))
                .or_else(|| value.get("file"))
                .unwrap_or(value);
            if !collector.add_identity(identity, &context) {
                collector
                    .warn(format!("{context} has no source note identity"));
            }
        }
        _ => {
            *row_number += 1;
            collector.warn(format!(
                "DQL task row {} has no source note identity",
                *row_number
            ));
        }
    }
}

pub(super) fn result_array(result: &Value) -> Option<&Vec<Value>> {
    result
        .get("values")
        .or_else(|| result.get("rows"))
        .and_then(Value::as_array)
}

pub(super) fn table_row_identity(row: &Value) -> Option<&Value> {
    match row {
        Value::Array(cells) => cells.first(),
        Value::Object(map) => map
            .get("id")
            .or_else(|| map.get("key"))
            .or_else(|| map.get("path"))
            .or_else(|| map.get("file"))
            .or(Some(row)),
        _ => Some(row),
    }
}

pub(super) fn identifier_is_grouped(value: Option<&Value>) -> bool {
    value
        .and_then(|meaning| meaning.get("type"))
        .and_then(Value::as_str)
        == Some("group")
}

pub(super) fn warn_grouped_rows(
    context_prefix: &str,
    row_count: usize,
    collector: &mut PathCollector,
) {
    if row_count == 0 {
        collector.warn(format!(
            "{context_prefix} set uses grouped identity; cannot derive \
             source note paths"
        ));
        return;
    }

    for index in 0..row_count {
        collector.warn(format!(
            "{context_prefix} {} uses grouped identity; cannot derive a \
             source note path",
            index + 1
        ));
    }
}

#[derive(Debug, Default)]
pub(super) struct PathCollector {
    paths: Vec<String>,
    seen: HashSet<String>,
    warnings: Vec<String>,
}

impl PathCollector {
    pub(super) fn add_identity(
        &mut self,
        value: &Value,
        context: &str,
    ) -> bool {
        if let Some(identity) = list_pair_identity(value) {
            return self.add_identity(identity, context);
        }

        if let Some(raw_path) = direct_path(value) {
            self.add_raw_path(raw_path, context)
        } else {
            false
        }
    }

    pub(super) fn add_raw_path(
        &mut self,
        raw_path: &str,
        context: &str,
    ) -> bool {
        match normalize_note_path(raw_path) {
            Ok(path) => {
                if self.seen.insert(path.clone()) {
                    self.paths.push(path);
                }
                true
            }
            Err(reason) => {
                self.warn(format!("{context}: {reason}"));
                false
            }
        }
    }

    pub(super) fn warn(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    pub(super) fn finish(
        self,
        strict: bool,
    ) -> Result<PathExtraction, DataviewError> {
        if strict && !self.warnings.is_empty() {
            return Err(DataviewError::StrictPaths {
                warnings: self.warnings,
            });
        }

        Ok(PathExtraction {
            paths: self.paths,
            warnings: self.warnings,
        })
    }
}

pub(super) fn list_pair_identity(value: &Value) -> Option<&Value> {
    let map = value.as_object()?;
    let widget = map.get("$widget").and_then(Value::as_str);
    if widget == Some("dataview:list-pair") {
        return map.get("key").or_else(|| map.get("id"));
    }

    None
}

pub(super) fn direct_path(value: &Value) -> Option<&str> {
    match value {
        Value::String(path) => Some(path),
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("link")
                && let Some(path) = map.get("path").and_then(Value::as_str)
            {
                return Some(path);
            }

            map.get("path")
                .and_then(Value::as_str)
                .or_else(|| nested_path(map.get("file")))
                .or_else(|| nested_path(map.get("link")))
                .or_else(|| nested_path(map.get("section")))
        }
        _ => None,
    }
}

pub(super) fn nested_path(value: Option<&Value>) -> Option<&str> {
    value?.as_object()?.get("path").and_then(Value::as_str)
}

pub(super) fn normalize_note_path(raw_path: &str) -> Result<String, String> {
    if raw_path.is_empty() {
        return Err("empty path".to_string());
    }

    let without_subpath =
        raw_path.split_once('#').map_or(raw_path, |(path, _)| path);
    let mut path = without_subpath.replace('\\', "/");
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_string();
    }

    if path.is_empty() {
        return Err(format!("path {raw_path:?} does not name a note"));
    }
    if path.starts_with('/') {
        return Err(format!("path {raw_path:?} is not vault-relative"));
    }
    if path.contains('\0') {
        return Err(format!("path {raw_path:?} contains a NUL byte"));
    }

    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!(
                "path {raw_path:?} is not a clean vault-relative path"
            ));
        }
    }

    if !path.ends_with(".md") {
        path.push_str(".md");
    }

    Ok(path)
}
