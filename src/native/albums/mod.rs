//! Album declaration parsing, vault registry, and read-only reports.

pub(crate) mod catalog;
pub(crate) mod cli;
pub(crate) mod create;
pub(crate) mod create_run;
pub(crate) mod grouping;
pub(crate) mod index;
pub(crate) mod output;
pub(crate) mod registry;
pub(crate) mod report;
pub(crate) mod sync;
pub(crate) mod write;

pub(crate) use cli::{
    build_cli_check, build_cli_create, build_cli_list, build_cli_sync,
};

use std::{collections::HashMap, path::Path};

use serde_yaml::{Mapping, Value};

use crate::native::{
    capture_language::is_route_token,
    notebooks::{scan::derive_title, snapshot::first_alias},
    projects::{
        frontmatter_is_album, frontmatter_top_level_value, parse_frontmatter,
        trim_yaml_scalar, Frontmatter,
    },
    records::schema::{
        parse_album_record_schema, AlbumRecordSchemaParse, RecordDiagnostic,
        RecordType,
    },
};

const SUMMARY_LIMIT: usize = 140;
const RESERVED_ALBUM_STEMS: &[&str] = &["done", "ref"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlbumIssueSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumFinding {
    pub(crate) code: &'static str,
    pub(crate) severity: AlbumIssueSeverity,
    pub(crate) path: String,
    pub(crate) line_number: Option<usize>,
    pub(crate) key_path: Option<String>,
    pub(crate) message: String,
    pub(crate) suggestions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AlbumDeclaration {
    pub(crate) path: String,
    pub(crate) stem: String,
    pub(crate) title: String,
    pub(crate) emoji: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) created: Option<String>,
    pub(crate) index_by: Option<String>,
    pub(crate) index_facets: Vec<String>,
    /// Physical 1-based line of the `record:` key, when present.
    pub(crate) record_line: Option<usize>,
    /// Noun and folder claims survive schema failure so discovery can still
    /// suppress a matching legacy config kind.
    pub(crate) noun_claim: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) plural: Option<String>,
    pub(crate) record_type: Option<RecordType>,
    pub(crate) schema_diagnostic: Option<RecordDiagnostic>,
    pub(crate) findings: Vec<AlbumFinding>,
}

/// Parse one type-recognized album note. `relative_path` is the vault-relative
/// path, while every schema diagnostic still carries that source path and its
/// physical 1-based line.
pub(crate) fn parse_album_declaration(
    relative_path: &Path,
    contents: &str,
) -> Option<AlbumDeclaration> {
    let frontmatter = parse_frontmatter(contents)?;
    if !frontmatter_is_album(&frontmatter) {
        return None;
    }

    let path = relative_path.to_string_lossy().replace('\\', "/");
    let stem = relative_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned();
    let mut findings = Vec::new();
    if !valid_album_stem(&stem) {
        findings.push(finding(
            "album_stem_invalid",
            AlbumIssueSeverity::Error,
            &path,
            None,
            None,
            format!(
                "album stem {stem:?} must be a lower-case capture route and cannot be a reserved folder"
            ),
        ));
    }

    let parent = top_level_string(&frontmatter, "parent");
    let emoji = top_level_string(&frontmatter, "emoji");
    let created = top_level_string(&frontmatter, "created");
    let index_by = top_level_string(&frontmatter, "index_by");
    let summary = decode_summary(&frontmatter);
    let aliases = first_alias(contents);
    let title = derive_title(contents, aliases.as_deref(), &stem);
    for (code, message, line) in
        summary_findings(summary.as_deref(), &frontmatter)
    {
        findings.push(finding(
            code,
            AlbumIssueSeverity::Warning,
            &path,
            line,
            Some("summary".to_owned()),
            message,
        ));
    }

    let line_by_path = key_line_map(&frontmatter);
    let record_section = record_section(&frontmatter);
    let record_section_line =
        record_section.as_ref().map(|section| section.line_number);
    let record_line = record_section_line.unwrap_or(2);
    let (record_value, record_parse_error, recovered_noun) =
        match record_section {
            None => (None, None, None),
            Some(section) => match serde_yaml::from_str::<Value>(&section.yaml)
            {
                Ok(Value::Mapping(root)) => (
                    mapping_get(&root, "record").cloned(),
                    None,
                    recover_noun_from_yaml(&section.yaml),
                ),
                Ok(value) => {
                    (Some(value), None, recover_noun_from_yaml(&section.yaml))
                }
                Err(error) => {
                    let line = error
                        .location()
                        .map(|location| {
                            section.line_number + location.line() - 1
                        })
                        .unwrap_or(section.line_number);
                    let diagnostic = RecordDiagnostic::album(
                    relative_path,
                    line,
                    "record",
                    None,
                    crate::native::records::schema::RecordDiagnosticCode::InvalidYaml,
                    format!("could not parse record schema YAML: {error}"),
                );
                    (
                        None,
                        Some(diagnostic),
                        recover_noun_from_yaml(&section.yaml),
                    )
                }
            },
        };

    let parsed_schema = if let Some(diagnostic) = record_parse_error {
        AlbumRecordSchemaParse {
            noun_claim: recovered_noun,
            label: None,
            plural: None,
            record_type: None,
            diagnostic: Some(diagnostic),
        }
    } else {
        parse_album_record_schema(
            record_value.as_ref(),
            relative_path,
            &stem,
            emoji.clone(),
            &line_by_path,
            record_line,
        )
    };

    if frontmatter_top_level_value(&frontmatter, "index_by").is_some()
        && index_by.is_none()
    {
        findings.push(finding(
            "album_schema_invalid",
            AlbumIssueSeverity::Error,
            &path,
            line_by_path.get("index_by").copied(),
            Some("index_by".to_owned()),
            "index_by must be a non-empty field name".to_owned(),
        ));
    }
    let mut index_facets =
        match parse_top_level_list(&frontmatter, "index_facets") {
            Ok(Some(facets)) => facets,
            Ok(None) => Vec::new(),
            Err(message) => {
                findings.push(finding(
                    "album_schema_invalid",
                    AlbumIssueSeverity::Error,
                    &path,
                    line_by_path.get("index_facets").copied(),
                    Some("index_facets".to_owned()),
                    message,
                ));
                Vec::new()
            }
        };
    if index_facets.len() > 3 {
        findings.push(finding(
            "album_schema_invalid",
            AlbumIssueSeverity::Error,
            &path,
            line_by_path.get("index_facets").copied(),
            Some("index_facets".to_owned()),
            "index_facets may contain at most three field names".to_owned(),
        ));
        index_facets.truncate(3);
    }
    if let Some(diagnostic) = parsed_schema.diagnostic.as_ref() {
        findings.push(schema_finding(&path, diagnostic));
    }
    if let Some(record_type) = parsed_schema.record_type.as_ref() {
        if let Some(index_by) = index_by.as_deref()
            && record_type.field(index_by).is_none()
        {
            findings.push(finding(
                "album_index_field_unknown",
                AlbumIssueSeverity::Error,
                &path,
                line_by_path.get("index_by").copied(),
                Some("index_by".to_owned()),
                format!("index_by names undeclared field {index_by:?}"),
            ));
        }
        for facet in &index_facets {
            if record_type.field(facet).is_none() {
                findings.push(finding(
                    "album_index_field_unknown",
                    AlbumIssueSeverity::Error,
                    &path,
                    line_by_path
                        .get(&format!("index_facets.{facet}"))
                        .copied()
                        .or_else(|| line_by_path.get("index_facets").copied()),
                    Some(format!("index_facets.{facet}")),
                    format!("index_facets names undeclared field {facet:?}"),
                ));
            }
        }
    }

    Some(AlbumDeclaration {
        path,
        stem,
        title,
        emoji,
        summary,
        parent,
        created,
        index_by,
        index_facets,
        record_line: record_section_line,
        noun_claim: parsed_schema.noun_claim,
        label: parsed_schema.label,
        plural: parsed_schema.plural,
        record_type: parsed_schema.record_type,
        schema_diagnostic: parsed_schema.diagnostic,
        findings,
    })
}

fn valid_album_stem(stem: &str) -> bool {
    stem == stem.to_ascii_lowercase()
        && is_route_token(stem)
        && !RESERVED_ALBUM_STEMS.contains(&stem)
}

/// One vault-root album route: a capture token with no separators, dots, or
/// reserved folder names. The returned stem is lower-cased.
pub(crate) fn confined_album_route(route: &str) -> Option<String> {
    if route.is_empty() || Path::new(route).is_absolute() {
        return None;
    }
    if !is_route_token(route) {
        return None;
    }
    let stem = route.to_ascii_lowercase();
    (!RESERVED_ALBUM_STEMS.contains(&stem.as_str())).then_some(stem)
}

fn top_level_string(
    frontmatter: &Frontmatter<'_>,
    key: &str,
) -> Option<String> {
    let raw = frontmatter_top_level_value(frontmatter, key)?.trim();
    if raw.is_empty() {
        return None;
    }
    let value = trim_yaml_scalar(raw).trim();
    if value.is_empty()
        || (value.starts_with(['[', '{', '|', '>'])
            && !(key == "parent" && value.starts_with("[[")))
    {
        None
    } else {
        Some(strip_yaml_comment(value).to_owned())
    }
}

fn decode_summary(frontmatter: &Frontmatter<'_>) -> Option<String> {
    let index = frontmatter.lines.iter().position(|line| {
        !line.starts_with([' ', '\t'])
            && line
                .strip_prefix("summary")
                .is_some_and(|rest| rest.starts_with(':'))
    })?;
    let raw = frontmatter.lines[index]
        .strip_prefix("summary:")
        .unwrap_or("")
        .trim();
    if raw.starts_with(['|', '>']) {
        let mut lines = Vec::new();
        for line in &frontmatter.lines[index + 1..] {
            if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
                break;
            }
            lines.push(line.trim());
        }
        let value = lines.join("\n");
        return Some(value.trim().to_owned());
    }
    if raw.is_empty() {
        return Some(String::new());
    }
    if let Ok(Value::String(value)) = serde_yaml::from_str::<Value>(raw) {
        return Some(value);
    }
    Some(strip_yaml_comment(trim_yaml_scalar(raw)).to_owned())
}

fn strip_yaml_comment(raw: &str) -> &str {
    raw.find(" #").map_or(raw, |index| &raw[..index]).trim_end()
}

fn summary_findings(
    summary: Option<&str>,
    frontmatter: &Frontmatter<'_>,
) -> Vec<(&'static str, String, Option<usize>)> {
    let line = frontmatter
        .lines
        .iter()
        .position(|entry| {
            !entry.starts_with([' ', '\t'])
                && entry
                    .strip_prefix("summary")
                    .is_some_and(|rest| rest.starts_with(':'))
        })
        .map(|index| index + 2);
    let Some(summary) = summary.filter(|summary| !summary.trim().is_empty())
    else {
        return vec![(
            "album_summary_missing",
            "add a one-line `summary:` describing this album".to_owned(),
            line,
        )];
    };
    let mut findings = Vec::new();
    if summary.contains('\n') {
        findings.push((
            "album_summary_multiline",
            "keep `summary:` to one line".to_owned(),
            line,
        ));
    }
    let first_line = summary.lines().next().unwrap_or(summary);
    if first_line.chars().count() > SUMMARY_LIMIT {
        findings.push((
            "album_summary_long",
            "keep `summary:` to at most 140 characters".to_owned(),
            line,
        ));
    }
    findings
}

fn parse_top_level_list(
    frontmatter: &Frontmatter<'_>,
    key: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(index) = frontmatter.lines.iter().position(|line| {
        !line.starts_with([' ', '\t'])
            && line
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(':'))
    }) else {
        return Ok(None);
    };
    let raw = frontmatter.lines[index]
        .strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or("")
        .trim();
    if raw.starts_with('[') {
        let Ok(Value::Sequence(items)) = serde_yaml::from_str::<Value>(raw)
        else {
            return Err("index_facets must be an ordered list of field names"
                .to_owned());
        };
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            let Some(value) = item.as_str() else {
                return Err(
                    "index_facets entries must be non-empty strings".to_owned()
                );
            };
            if value.trim().is_empty() {
                return Err(
                    "index_facets entries must be non-empty strings".to_owned()
                );
            }
            values.push(value.to_owned());
        }
        return Ok(Some(values));
    }
    if !raw.is_empty() {
        return Err(
            "index_facets must be an ordered list of field names".to_owned()
        );
    }
    let mut values = Vec::new();
    for line in &frontmatter.lines[index + 1..] {
        if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
            break;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some(item) = line.trim().strip_prefix('-') else {
            return Err(
                "index_facets must use a YAML list of field names".to_owned()
            );
        };
        let item = item.trim();
        if let Some(value) =
            item.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        {
            values.push(value.to_owned());
        } else if let Some(value) =
            item.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))
        {
            values.push(value.to_owned());
        } else if !item.is_empty() {
            let value = strip_yaml_comment(item).trim();
            if value.is_empty() {
                return Err(
                    "index_facets entries must be non-empty strings".to_owned()
                );
            }
            values.push(value.to_owned());
        } else {
            return Err(
                "index_facets entries must be non-empty strings".to_owned()
            );
        }
    }
    Ok(Some(values))
}

struct RecordSection {
    yaml: String,
    line_number: usize,
}

fn record_section(frontmatter: &Frontmatter<'_>) -> Option<RecordSection> {
    let start = frontmatter.lines.iter().position(|line| {
        !line.starts_with([' ', '\t'])
            && line
                .strip_prefix("record")
                .is_some_and(|rest| rest.starts_with(':'))
    })?;
    let mut end = start + 1;
    while end < frontmatter.lines.len() {
        let line = frontmatter.lines[end];
        if !line.starts_with([' ', '\t'])
            && !line.trim().is_empty()
            && !line.trim_start().starts_with('#')
        {
            break;
        }
        end += 1;
    }
    Some(RecordSection {
        yaml: frontmatter.lines[start..end].join("\n"),
        line_number: start + 2,
    })
}

/// Indentation-aware YAML key → 1-based physical line map for nested
/// frontmatter diagnostics. Bank reuses this walk for `bank:` key paths.
pub(crate) fn key_line_map(
    frontmatter: &Frontmatter<'_>,
) -> HashMap<String, usize> {
    let mut lines_by_path = HashMap::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (index, line) in frontmatter.lines.iter().enumerate() {
        let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
        let trimmed = line.trim_start();
        if trimmed.is_empty()
            || trimmed.starts_with(['#', '-'])
            || line.contains(['{', '}'])
        {
            continue;
        }
        let Some((raw_key, after_colon)) = trimmed.split_once(':') else {
            continue;
        };
        if !after_colon.is_empty() && !after_colon.starts_with([' ', '\t', '#'])
        {
            continue;
        }
        let key = raw_key.trim().trim_matches(['\'', '"']);
        if key.is_empty() {
            continue;
        }
        while stack
            .last()
            .is_some_and(|(indent, _)| *indent >= indentation)
        {
            stack.pop();
        }
        let mut path = stack
            .iter()
            .map(|(_, key)| key.as_str())
            .collect::<Vec<_>>();
        path.push(key);
        lines_by_path.insert(path.join("."), index + 2);
        stack.push((indentation, key.to_owned()));
    }
    lines_by_path
}

fn recover_noun_from_yaml(yaml: &str) -> Option<String> {
    let record = yaml.strip_prefix("record:")?.trim_start();
    if record.starts_with('{') {
        return recover_noun_from_flow(record);
    }
    recover_noun_from_block(yaml)
}

fn recover_noun_from_flow(record: &str) -> Option<String> {
    let inner = record.strip_prefix('{')?;
    let inner = inner.strip_suffix('}').unwrap_or(inner);
    for item in split_flow_items(inner) {
        let Some((key, value)) = item.split_once(':') else {
            continue;
        };
        if yaml_key_name(key) == "noun" {
            return recover_yaml_scalar(value);
        }
    }
    None
}

fn recover_noun_from_block(yaml: &str) -> Option<String> {
    let mut child_indent = None;
    for line in yaml.lines().skip(1) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        if child_indent.is_none() {
            child_indent = Some(indent);
        }
        if Some(indent) != child_indent {
            continue;
        }
        let trimmed = line.trim_start();
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        if !value.is_empty() && !value.starts_with([' ', '\t', '#']) {
            continue;
        }
        if yaml_key_name(key) == "noun" {
            return recover_yaml_scalar(value);
        }
    }
    None
}

fn yaml_key_name(raw: &str) -> String {
    trim_yaml_scalar(raw.trim()).trim().to_owned()
}

fn recover_yaml_scalar(raw: &str) -> Option<String> {
    let stripped = strip_yaml_comment(raw).trim();
    if stripped.is_empty() {
        return None;
    }
    let value = trim_yaml_scalar(stripped).trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn split_flow_items(raw: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '{' | '[' => depth += 1,
            '}' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                items.push(raw[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if start <= raw.len() {
        items.push(raw[start..].trim());
    }
    items
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

fn schema_finding(path: &str, diagnostic: &RecordDiagnostic) -> AlbumFinding {
    AlbumFinding {
        code: "album_schema_invalid",
        severity: AlbumIssueSeverity::Error,
        path: path.to_owned(),
        line_number: diagnostic.line_number,
        key_path: diagnostic.key_path.clone(),
        message: diagnostic.message.clone(),
        suggestions: diagnostic.suggestions.clone(),
    }
}

fn finding(
    code: &'static str,
    severity: AlbumIssueSeverity,
    path: &str,
    line_number: Option<usize>,
    key_path: Option<String>,
    message: impl Into<String>,
) -> AlbumFinding {
    AlbumFinding {
        code,
        severity,
        path: path.to_owned(),
        line_number,
        key_path,
        message: message.into(),
        suggestions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{parse_album_declaration, AlbumIssueSeverity};

    fn parse(text: &str) -> super::AlbumDeclaration {
        parse_album_declaration(Path::new("eat.md"), text)
            .expect("album declaration")
    }

    #[test]
    fn album_type_spellings_match_existing_frontmatter_grammar() {
        for type_line in [
            "type: \"[[album]]\"",
            "type: [[album]]",
            "type: [\"[[area]]\", \"[[album]]\"]",
            "type:\n  - \"[[area]]\"\n  - \"[[album]]\"",
        ] {
            let text = format!(
                "---\n{type_line}\nrecord: {{noun: restaurant}}\n---\n# Eat\n"
            );
            assert!(
                parse_album_declaration(Path::new("eat.md"), &text).is_some()
            );
        }
    }

    #[test]
    fn block_and_flow_schema_preserve_field_and_choice_order() {
        let block = parse(
            "---\ntype: [[album]]\nparent: [[body]]\nsummary: Food\nrecord:\n  noun: restaurant\n  fields:\n    status:\n      values:\n        to_try: 🔖 To try\n        liked: 💚 Liked\n    location: {}\n---\n# Eat\n",
        );
        assert_eq!(block.noun_claim.as_deref(), Some("restaurant"));
        assert_eq!(
            block
                .record_type
                .as_ref()
                .unwrap()
                .fields
                .iter()
                .map(|field| field.key.as_str())
                .collect::<Vec<_>>(),
            ["status", "location"]
        );
        assert_eq!(
            match &block.record_type.as_ref().unwrap().fields[0].kind {
                crate::native::records::schema::FieldKind::Choices(choices) => {
                    choices
                        .iter()
                        .map(|choice| choice.value.as_str())
                        .collect::<Vec<_>>()
                }
                _ => panic!("ordered choices"),
            },
            ["to_try", "liked"]
        );

        let flow = parse(
            "---\ntype: [[album]]\nrecord: {noun: movie, fields: {year: {values: number, min: 1900}}}\n---\n# Movies\n",
        );
        assert!(flow.record_type.is_some());
        assert!(flow.schema_diagnostic.is_none());
    }

    #[test]
    fn optional_fields_and_display_defaults_include_acronym_plural_rule() {
        let restaurant = parse(
            "---\ntype: [[album]]\nrecord: {noun: restaurant}\n---\n# Eat\n",
        );
        assert_eq!(restaurant.label.as_deref(), Some("Restaurant"));
        assert_eq!(restaurant.plural.as_deref(), Some("restaurants"));
        assert!(restaurant.record_type.as_ref().unwrap().fields.is_empty());

        let tv = parse(
            "---\ntype: [[album]]\nrecord: {noun: tv_show, label: 'TV show'}\n---\n# TV\n",
        );
        assert_eq!(tv.plural.as_deref(), Some("TV shows"));
    }

    #[test]
    fn invalid_field_options_have_physical_line_and_full_key_path() {
        let declaration = parse(
            "---\r\ntype: [[album]]\r\nsummary: Restaurants\r\nrecord:\r\n  noun: restaurant\r\n  fields:\r\n    location:\r\n      requred: true\r\n---\r\n",
        );
        let diagnostic = declaration.schema_diagnostic.as_ref().unwrap();
        assert_eq!(diagnostic.file_path.as_deref(), Some(Path::new("eat.md")));
        assert_eq!(diagnostic.line_number, Some(8));
        assert_eq!(
            diagnostic.key_path.as_deref(),
            Some("record.fields.location.requred")
        );
        assert_eq!(diagnostic.suggestions, ["required"]);
        assert_eq!(
            declaration
                .findings
                .iter()
                .find(|finding| finding.code == "album_schema_invalid")
                .unwrap()
                .severity,
            AlbumIssueSeverity::Error
        );
    }

    #[test]
    fn nested_repeated_key_lines_follow_the_full_path() {
        let declaration = parse(
            "---\ntype: [[album]]\nrecord:\n  noun: restaurant\n  fields:\n    values:\n      values: []\n---\n",
        );
        let diagnostic = declaration.schema_diagnostic.as_ref().unwrap();
        assert_eq!(
            diagnostic.key_path.as_deref(),
            Some("record.fields.values.values")
        );
        assert_eq!(diagnostic.line_number, Some(7));
    }

    #[test]
    fn invalid_schema_keeps_the_noun_claim() {
        let declaration = parse(
            "---\ntype: [[album]]\nrecord: {noun: restaurant, fields: {title: {}}}\n---\n",
        );
        assert_eq!(declaration.noun_claim.as_deref(), Some("restaurant"));
        assert!(declaration.record_type.is_none());
    }

    #[test]
    fn all_existing_field_kinds_defaults_bounds_and_lists_are_reused() {
        let declaration = parse(
            "---\ntype: [[album]]\nrecord:\n  noun: sample\n  fields:\n    name: {required: true, default: Example}\n    status: {values: {ready: Ready, later: Later}}\n    visited: {values: date, default: today}\n    related: {values: link, list: true}\n    url: {values: url}\n    score: {values: number, min: 0, max: 5, default: 3}\n---\n",
        );
        let record_type = declaration.record_type.as_ref().unwrap();
        assert!(matches!(
            &record_type.field("name").unwrap().kind,
            super::super::records::schema::FieldKind::Text
        ));
        assert!(matches!(
            &record_type.field("status").unwrap().kind,
            super::super::records::schema::FieldKind::Choices(_)
        ));
        assert!(matches!(
            &record_type.field("visited").unwrap().kind,
            super::super::records::schema::FieldKind::Date
        ));
        assert!(matches!(
            &record_type.field("related").unwrap().kind,
            super::super::records::schema::FieldKind::Link
        ));
        assert!(matches!(
            &record_type.field("url").unwrap().kind,
            super::super::records::schema::FieldKind::Url
        ));
        assert!(matches!(
            &record_type.field("score").unwrap().kind,
            super::super::records::schema::FieldKind::Number
        ));
        assert!(record_type.field("related").unwrap().list);
        assert!(record_type.field("score").unwrap().default.is_some());
    }

    #[test]
    fn album_nouns_stems_and_index_metadata_are_validated() {
        let reserved =
            parse("---\ntype: [[album]]\nrecord: {noun: album}\n---\n");
        assert_eq!(reserved.noun_claim.as_deref(), Some("album"));
        assert!(reserved.schema_diagnostic.is_some());

        let invalid_stem = parse_album_declaration(
            Path::new("Bad Stem.md"),
            "---\ntype: [[album]]\nrecord: {noun: sample}\n---\n",
        )
        .unwrap();
        assert!(invalid_stem
            .findings
            .iter()
            .any(|finding| finding.code == "album_stem_invalid"));

        let metadata = parse_album_declaration(
            Path::new("sample.md"),
            "---\ntype: [[album]]\nsummary: Sample collection\nindex_by: status\nindex_facets:\n  - location\n  - status\nrecord:\n  noun: sample\n  fields:\n    status: {values: {ready: Ready}}\n    location: {}\n---\n",
        )
        .unwrap();
        assert_eq!(metadata.index_by.as_deref(), Some("status"));
        assert_eq!(metadata.index_facets, ["location", "status"]);
        assert!(metadata.findings.is_empty());

        let bad_index = parse_album_declaration(
            Path::new("sample.md"),
            "---\ntype: [[album]]\nindex_by: missing\nrecord: {noun: sample, fields: {status: {}}}\n---\n",
        )
        .unwrap();
        assert!(bad_index
            .findings
            .iter()
            .any(|finding| finding.code == "album_index_field_unknown"));
    }

    #[test]
    fn malformed_flow_yaml_retains_a_recoverable_noun_claim() {
        let declaration = parse(
            "---\ntype: [[album]]\nrecord: {noun: restaurant, fields: {status: [}\n---\n",
        );
        assert_eq!(declaration.noun_claim.as_deref(), Some("restaurant"));
        let diagnostic = declaration.schema_diagnostic.as_ref().unwrap();
        assert_eq!(diagnostic.file_path.as_deref(), Some(Path::new("eat.md")));
        assert!(diagnostic.line_number.is_some());
        assert_eq!(diagnostic.key_path.as_deref(), Some("record"));
    }

    #[test]
    fn noun_recovery_keeps_only_a_direct_record_noun() {
        let commented = parse(
            "---\ntype: [[album]]\nrecord:\n  noun: restaurant # comment\n  fields: [\n---\n",
        );
        assert_eq!(commented.noun_claim.as_deref(), Some("restaurant"));
        assert!(commented.record_type.is_none());

        let quoted = parse(
            "---\ntype: [[album]]\nrecord:\n  noun: \"restaurant\"\n  fields: [\n---\n",
        );
        assert_eq!(quoted.noun_claim.as_deref(), Some("restaurant"));

        let unclosed_flow = parse(
            "---\ntype: [[album]]\nrecord: {noun: restaurant, fields: [\n---\n",
        );
        assert_eq!(unclosed_flow.noun_claim.as_deref(), Some("restaurant"));

        let nested_only = parse(
            "---\ntype: [[album]]\nrecord:\n  fields:\n    thing:\n      noun: restaurant\n    broken: [\n---\n",
        );
        assert_eq!(nested_only.noun_claim, None);
        assert_eq!(nested_only.stem, "eat");
    }

    #[test]
    fn top_level_album_facts_do_not_read_nested_record_fields() {
        let declaration = parse(
            "---\ntype: [[album]]\nrecord:\n  noun: sample\n  label: Nested\n  fields:\n    summary: {}\n---\n# First H1\n",
        );
        assert_eq!(declaration.title, "First H1");
        assert_eq!(declaration.label.as_deref(), Some("Nested"));
        assert_eq!(declaration.summary, None);
        assert!(declaration
            .findings
            .iter()
            .any(|finding| finding.code == "album_summary_missing"));
    }
}
