//! Lossless source inventory for YAML frontmatter.

use std::ops::Range;

use chrono::{NaiveDate, NaiveTime};
use serde::Serialize;
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

use crate::native::yaml_spans::{
    self, blank_or_comment, indentation, mapping_colon, Line,
};

use super::fingerprint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrontmatterState {
    Ok,
    Absent,
    Invalid,
    Unclosed,
}

pub(crate) type FrontmatterError = super::PropsError;

/// Reported value category. `tags` and `aliases` are key-aware list kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PropertyKind {
    Text,
    Number,
    Checkbox,
    Date,
    Datetime,
    List,
    Tags,
    Aliases,
    Map,
    Null,
    Opaque,
}

/// Source style needed by the future writer to retain the user's YAML form.
/// `datetime_offset` is the exact suffix (`Z`, `-0500`, `-05:00`, or null);
/// `datetime_separator` is `T`, a space, or null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ValueStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quoted: Option<String>,
    pub(crate) flow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) datetime_separator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) datetime_offset: Option<String>,
}

/// One top-level source entry, in the order it appeared in the note.
///
/// `line`/`end_line` cover the key and value. `source_range` includes the
/// entry's attached leading blank/comment trivia and is byte-exact; the key
/// and value ranges are separate so the writer can replace only one token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PropertyEntry {
    pub(crate) key: String,
    pub(crate) semantic_key: YamlValue,
    pub(crate) line: Option<usize>,
    pub(crate) end_line: Option<usize>,
    pub(crate) value: JsonValue,
    pub(crate) raw_value: Option<YamlValue>,
    pub(crate) kind: PropertyKind,
    pub(crate) style: ValueStyle,
    pub(crate) duplicate: bool,
    pub(crate) opaque: bool,
    pub(crate) diagnostic: Option<String>,
    pub(crate) source_range: Range<usize>,
    pub(crate) leading_range: Range<usize>,
    pub(crate) key_range: Range<usize>,
    pub(crate) colon_range: Range<usize>,
    pub(crate) value_range: Range<usize>,
    pub(crate) inline_comment_range: Option<Range<usize>>,
}

impl PropertyEntry {
    pub(crate) fn raw<'a>(&self, source: &'a str) -> &'a str {
        &source[self.source_range.clone()]
    }
}

/// A parsed frontmatter view that retains all original Markdown bytes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PropsDocument {
    pub(crate) source: String,
    pub(crate) state: FrontmatterState,
    pub(crate) error: Option<FrontmatterError>,
    pub(crate) start_line: Option<usize>,
    pub(crate) end_line: Option<usize>,
    pub(crate) entries: Vec<PropertyEntry>,
    pub(crate) fingerprint: Option<String>,
    pub(crate) semantic_mapping: Option<YamlValue>,
    /// The complete closed delimiter/YAML prefix, excluding BOM and body.
    /// Its exact bytes let the writer place new frontmatter without rebuilding
    /// the rest of the note.
    pub(crate) frontmatter_range: Option<Range<usize>>,
    pub(crate) bom_range: Option<Range<usize>>,
    pub(crate) body_range: Range<usize>,
}

impl PropsDocument {
    pub(crate) fn parse(source: &str) -> Self {
        let lines = yaml_spans::split_lines(source);
        let has_bom = source.starts_with('\u{feff}');
        let opening = lines.first().is_some_and(|line| {
            line.text.strip_prefix('\u{feff}').unwrap_or(&line.text) == "---"
        });
        if !opening {
            let mapping = empty_mapping();
            return Self {
                source: source.to_owned(),
                state: FrontmatterState::Absent,
                error: None,
                start_line: None,
                end_line: None,
                entries: Vec::new(),
                fingerprint: Some(fingerprint::empty_mapping_fingerprint()),
                semantic_mapping: Some(mapping),
                frontmatter_range: None,
                bom_range: has_bom.then_some(0..bom_offset(true)),
                body_range: bom_offset(has_bom)..source.len(),
            };
        }

        let close_index =
            lines.iter().enumerate().skip(1).find_map(|(index, line)| {
                (line.text == "---").then_some(index)
            });
        let Some(close_index) = close_index else {
            let entries =
                inventory_block_entries(source, &lines, 1, lines.len());
            return Self {
                source: source.to_owned(),
                state: FrontmatterState::Unclosed,
                error: Some(FrontmatterError {
                    code: "prop_invalid_yaml".to_owned(),
                    message: "frontmatter block is not closed".to_owned(),
                    hint: "Add a closing `---` before the note body."
                        .to_owned(),
                    key: None,
                    line: Some(1),
                    op_index: None,
                }),
                start_line: Some(1),
                end_line: None,
                entries,
                fingerprint: None,
                semantic_mapping: None,
                frontmatter_range: None,
                bom_range: has_bom.then_some(0..bom_offset(true)),
                body_range: source.len()..source.len(),
            };
        };

        let offsets = line_offsets(&lines);
        let inner_lines = &lines[1..close_index];
        let yaml = yaml_spans::yaml_text(inner_lines);
        let mut entries = if starts_with_flow_mapping(inner_lines) {
            inventory_flow_entries(source, inner_lines, offsets[1])
        } else {
            inventory_block_entries(source, &lines, 1, close_index)
        };
        mark_duplicates(&mut entries);

        let parsed = parse_root_mapping(&yaml);
        let duplicate = entries.iter().find(|entry| entry.duplicate);
        let (state, error, mapping) = match parsed {
            Err(parse_error) => (
                FrontmatterState::Invalid,
                Some(yaml_error(&parse_error, &yaml)),
                None,
            ),
            Ok(None) => (
                FrontmatterState::Invalid,
                Some(FrontmatterError {
                    code: "prop_invalid_yaml".to_owned(),
                    message: "frontmatter YAML must be a mapping".to_owned(),
                    hint:
                        "Use a YAML mapping between the frontmatter delimiters."
                            .to_owned(),
                    key: None,
                    line: inner_lines
                        .iter()
                        .position(|line| !root_trivia(&line.text))
                        .map(|index| index + 2),
                    op_index: None,
                }),
                None,
            ),
            Ok(Some(mapping)) => {
                if let Some(entry) = duplicate {
                    (
                        FrontmatterState::Invalid,
                        Some(FrontmatterError {
                            code: "prop_duplicate_key".to_owned(),
                            message: format!(
                                "duplicate frontmatter key {:?}",
                                entry.key
                            ),
                            hint: "Remove or rename duplicate keys before editing properties."
                                .to_owned(),
                            key: Some(entry.key.clone()),
                            line: entry.line,
                            op_index: None,
                        }),
                        None,
                    )
                } else {
                    (FrontmatterState::Ok, None, Some(mapping))
                }
            }
        };
        for entry in &mut entries {
            if entry.duplicate && entry.diagnostic.is_none() {
                entry.diagnostic =
                    Some(format!("duplicate frontmatter key {:?}", entry.key));
            }
        }

        let semantic_mapping = if state == FrontmatterState::Ok {
            mapping
        } else {
            None
        };
        let fingerprint =
            semantic_mapping.as_ref().map(fingerprint::fingerprint);
        let start = bom_offset(has_bom);
        let end = offsets[close_index] + lines[close_index].text.len();
        let close_ending = lines[close_index].ending.len();
        Self {
            source: source.to_owned(),
            state,
            error,
            start_line: Some(1),
            end_line: Some(close_index + 1),
            entries,
            fingerprint,
            semantic_mapping,
            frontmatter_range: Some(start..end + close_ending),
            bom_range: has_bom.then_some(0..bom_offset(true)),
            body_range: end + close_ending..source.len(),
        }
    }
}

fn empty_mapping() -> YamlValue {
    YamlValue::Mapping(serde_yaml::Mapping::new())
}

fn bom_offset(has_bom: bool) -> usize {
    if has_bom {
        '\u{feff}'.len_utf8()
    } else {
        0
    }
}

fn parse_root_mapping(
    yaml: &str,
) -> Result<Option<YamlValue>, serde_yaml::Error> {
    if yaml.lines().all(|line| {
        let trimmed = line.trim();
        trimmed.is_empty() || trimmed.starts_with('#')
    }) {
        return Ok(Some(empty_mapping()));
    }
    let value = serde_yaml::from_str::<YamlValue>(yaml)?;
    if value.as_mapping().is_some() {
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

fn yaml_error(error: &serde_yaml::Error, yaml: &str) -> FrontmatterError {
    let line = error.location().map(|location| location.line() + 1);
    let mut message = format!("invalid frontmatter YAML: {error}");
    if let Some(location) = error.location() {
        let snippet = yaml
            .lines()
            .nth(location.line().saturating_sub(1))
            .unwrap_or_default()
            .trim();
        if !snippet.is_empty() {
            message.push_str(&format!(" (near {snippet:?})"));
        }
    }
    FrontmatterError {
        code: "prop_invalid_yaml".to_owned(),
        message,
        hint: "Repair the frontmatter YAML before editing properties."
            .to_owned(),
        key: None,
        line,
        op_index: None,
    }
}

#[derive(Debug)]
struct RootKey {
    index: usize,
    key: YamlValue,
    name: String,
    key_start: usize,
    colon: usize,
    opaque_key: bool,
}

fn inventory_block_entries(
    source: &str,
    lines: &[Line],
    inner_start: usize,
    inner_end: usize,
) -> Vec<PropertyEntry> {
    let mut roots = Vec::new();
    for index in inner_start..inner_end.min(lines.len()) {
        let line = &lines[index].text;
        if indentation(line) != 0 || blank_or_comment(line) {
            continue;
        }
        let Some(colon) = mapping_colon(line, 0) else {
            continue;
        };
        let key_start = line[..colon].len() - line[..colon].trim_start().len();
        let raw_key = line[key_start..colon].trim_end();
        let Ok(key) = serde_yaml::from_str::<YamlValue>(raw_key) else {
            continue;
        };
        let (name, opaque_key) = normalized_key(&key, raw_key);
        roots.push(RootKey {
            index,
            key,
            name,
            key_start,
            colon,
            opaque_key,
        });
    }
    let offsets = line_offsets(lines);
    let leading_starts: Vec<usize> = roots
        .iter()
        .map(|root| {
            let mut start = root.index;
            while start > inner_start && root_trivia(&lines[start - 1].text) {
                start -= 1;
            }
            start
        })
        .collect();
    let mut entries = Vec::with_capacity(roots.len());
    for (index, root) in roots.iter().enumerate() {
        let next_key = roots
            .get(index + 1)
            .map(|next| next.index)
            .unwrap_or(inner_end);
        let end_exclusive = roots
            .get(index + 1)
            .map(|_| leading_starts[index + 1])
            .unwrap_or(inner_end);
        let value_lines = &lines[root.index..next_key.min(inner_end)];
        let end_line_index = value_lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !root_trivia(&line.text))
            .map(|(offset, _)| root.index + offset)
            .last()
            .unwrap_or(root.index);
        let fragment = yaml_spans::yaml_text(value_lines);
        let parsed = serde_yaml::from_str::<YamlValue>(&fragment).ok();
        let value = parsed.as_ref().and_then(|value| {
            value
                .as_mapping()
                .and_then(|mapping| mapping.get(&root.key))
                .cloned()
        });
        let source_value_start = offsets[root.index] + root.colon + 1;
        let value_text = &lines[root.index].text[root.colon + 1..];
        let (value_before_comment, inline_comment) =
            yaml_spans::split_comment(value_text);
        let value_token = value_before_comment.trim();
        let token_start = value_before_comment.len()
            - value_before_comment.trim_start().len();
        let has_child_value =
            (root.index + 1..next_key.min(inner_end)).any(|line_index| {
                !root_trivia(&lines[line_index].text)
                    && indentation(&lines[line_index].text) > 0
            });
        let last_value_line = value_lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !root_trivia(&line.text))
            .map(|(offset, _)| root.index + offset)
            .last()
            .unwrap_or(root.index);
        let value_range = if has_child_value {
            let first_child =
                (root.index + 1..=last_value_line).find(|line_index| {
                    !root_trivia(&lines[*line_index].text)
                        && indentation(&lines[*line_index].text) > 0
                });
            let start = if value_token.is_empty() {
                first_child.map_or(source_value_start, |line_index| {
                    offsets[line_index]
                })
            } else {
                source_value_start + token_start
            };
            start..offsets[last_value_line] + lines[last_value_line].text.len()
        } else {
            let start = source_value_start + token_start;
            start..start + value_token.len()
        };
        let source_entry_start = offsets[leading_starts[index]];
        let source_entry_end =
            offsets.get(end_exclusive).copied().unwrap_or(source.len());
        let key_end = root.colon
            - (lines[root.index].text[..root.colon].len()
                - lines[root.index].text[..root.colon].trim_end().len());
        let key_range =
            offsets[root.index] + root.key_start..offsets[root.index] + key_end;
        let colon_range = offsets[root.index] + root.colon
            ..offsets[root.index] + root.colon + 1;
        let inline_comment_range = if inline_comment.is_empty() {
            None
        } else {
            let comment_start = value_text.len() - inline_comment.len();
            Some(
                source_value_start + comment_start
                    ..source_value_start + value_text.len(),
            )
        };
        let leading_range = source_entry_start..key_range.start;
        entries.push(build_entry(
            root.name.clone(),
            root.key.clone(),
            root.index + 1,
            end_line_index + 1,
            value,
            value_token,
            value_lines,
            root.opaque_key,
            source_entry_start..source_entry_end,
            leading_range,
            key_range,
            colon_range,
            value_range,
            inline_comment_range,
        ));
    }
    entries
}

fn starts_with_flow_mapping(lines: &[Line]) -> bool {
    lines
        .iter()
        .map(|line| line.text.trim())
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .is_some_and(|line| line.starts_with('{'))
}

/// Inventory a root flow mapping without relying on serde_yaml's map order.
/// Nested flow collections and quoted punctuation are skipped by depth.
fn inventory_flow_entries(
    source: &str,
    lines: &[Line],
    source_start: usize,
) -> Vec<PropertyEntry> {
    let yaml = yaml_spans::yaml_text(lines);
    let Some(open) = yaml.find('{') else {
        return Vec::new();
    };
    let bytes = yaml.as_bytes();
    let mut entries = Vec::new();
    let mut item_start = open + 1;
    let mut index = item_start;
    let mut curly = 1usize;
    let mut square = 0usize;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if current == byte {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'{' => curly += 1,
            b'}' => {
                curly = curly.saturating_sub(1);
                if curly == 0 {
                    push_flow_entry(
                        source,
                        lines,
                        source_start,
                        &yaml,
                        item_start,
                        index,
                        &mut entries,
                    );
                    break;
                }
            }
            b'[' => square += 1,
            b']' => square = square.saturating_sub(1),
            b',' if curly == 1 && square == 0 => {
                push_flow_entry(
                    source,
                    lines,
                    source_start,
                    &yaml,
                    item_start,
                    index,
                    &mut entries,
                );
                item_start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    entries
}

fn push_flow_entry(
    source: &str,
    lines: &[Line],
    source_start: usize,
    yaml: &str,
    item_start: usize,
    item_end: usize,
    entries: &mut Vec<PropertyEntry>,
) {
    let raw = &yaml[item_start..item_end];
    let Some(colon) = flow_item_colon(raw) else {
        return;
    };
    let leading = raw.len() - raw.trim_start().len();
    let key_text = raw[leading..colon].trim_end();
    let Ok(key_value) = serde_yaml::from_str::<YamlValue>(key_text) else {
        return;
    };
    let (key, opaque_key) = normalized_key(&key_value, key_text);
    let value_after_colon = &raw[colon + 1..];
    let (value_before_comment, comment) =
        yaml_spans::split_comment(value_after_colon);
    let value_text = value_before_comment.trim();
    let parsed_value = if value_text.is_empty() {
        Some(YamlValue::Null)
    } else {
        serde_yaml::from_str::<YamlValue>(value_text).ok()
    };
    let key_start = item_start + leading;
    let key_end = item_start + colon
        - (raw[..colon].len() - raw[..colon].trim_end().len());
    let value_start = item_start + colon + 1 + value_before_comment.len()
        - value_before_comment.trim_start().len();
    let value_end = value_start + value_text.len();
    let source_range =
        map_normalized_range(source, lines, source_start, item_start, item_end);
    let leading_range = map_normalized_range(
        source,
        lines,
        source_start,
        item_start,
        key_start,
    );
    let key_range =
        map_normalized_range(source, lines, source_start, key_start, key_end);
    let colon_range = map_normalized_range(
        source,
        lines,
        source_start,
        item_start + colon,
        item_start + colon + 1,
    );
    let value_range = map_normalized_range(
        source,
        lines,
        source_start,
        value_start,
        value_end,
    );
    let inline_comment_range = if comment.is_empty() {
        None
    } else {
        let comment_start = value_after_colon.len() - comment.len();
        Some(map_normalized_range(
            source,
            lines,
            source_start,
            item_start + colon + 1 + comment_start,
            item_start + colon + 1 + value_after_colon.len(),
        ))
    };
    let (line, end_line) = (
        normalized_line_number(yaml, item_start, source_start, source, lines),
        normalized_line_number(
            yaml,
            item_end.saturating_sub(1),
            source_start,
            source,
            lines,
        ),
    );
    let value_lines = vec![Line {
        text: value_text.to_owned(),
        ending: String::new(),
    }];
    entries.push(build_entry(
        key,
        key_value,
        line,
        end_line,
        parsed_value,
        value_text,
        &value_lines,
        opaque_key,
        source_range,
        leading_range,
        key_range,
        colon_range,
        value_range,
        inline_comment_range,
    ));
}

fn flow_item_colon(item: &str) -> Option<usize> {
    let bytes = item.as_bytes();
    let mut curly = 0usize;
    let mut square = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if current == byte {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'{' => curly += 1,
            b'}' => curly = curly.saturating_sub(1),
            b'[' => square += 1,
            b']' => square = square.saturating_sub(1),
            b':' if curly == 0 && square == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn map_normalized_range(
    source: &str,
    lines: &[Line],
    source_start: usize,
    start: usize,
    end: usize,
) -> Range<usize> {
    let yaml = yaml_spans::yaml_text(lines);
    let source_line_offsets = line_offsets_for_source(source);
    let normalized_start = normalized_offset_to_line_col(&yaml, start);
    let normalized_end = normalized_offset_to_line_col(&yaml, end);
    let first_inner_line = source
        .get(..source_start)
        .map(|prefix| prefix.bytes().filter(|byte| *byte == b'\n').count())
        .unwrap_or(0);
    let start_line = first_inner_line + normalized_start.0;
    let end_line = first_inner_line + normalized_end.0;
    let start_byte = source_line_offsets
        .get(start_line)
        .copied()
        .unwrap_or(source.len())
        + normalized_start.1;
    let end_byte = source_line_offsets
        .get(end_line)
        .copied()
        .unwrap_or(source.len())
        + normalized_end.1;
    start_byte.min(source.len())..end_byte.min(source.len())
}

fn normalized_line_number(
    yaml: &str,
    offset: usize,
    source_start: usize,
    source: &str,
    _lines: &[Line],
) -> usize {
    let inner_line = yaml[..offset.min(yaml.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    source[..source_start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + inner_line
        + 1
}

fn normalized_offset_to_line_col(yaml: &str, offset: usize) -> (usize, usize) {
    let bounded = offset.min(yaml.len());
    let before = &yaml[..bounded];
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let column = before
        .rfind('\n')
        .map_or(before.len(), |newline| before.len() - newline - 1);
    (line, column)
}

fn line_offsets_for_source(source: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            offsets.push(index + 1);
        }
    }
    offsets
}

fn normalized_key(key: &YamlValue, raw: &str) -> (String, bool) {
    match key {
        YamlValue::String(value) => (value.clone(), false),
        YamlValue::Null => ("null".to_owned(), true),
        YamlValue::Bool(value) => (value.to_string(), true),
        YamlValue::Number(value) => (value.to_string(), true),
        YamlValue::Sequence(_)
        | YamlValue::Mapping(_)
        | YamlValue::Tagged(_) => (raw.trim().to_owned(), true),
    }
}

fn mark_duplicates(entries: &mut [PropertyEntry]) {
    let duplicate: Vec<bool> = (0..entries.len())
        .map(|index| {
            entries.iter().enumerate().any(|(other, entry)| {
                other != index
                    && (entry.key == entries[index].key
                        || entry.semantic_key == entries[index].semantic_key)
            })
        })
        .collect();
    for (entry, duplicate) in entries.iter_mut().zip(duplicate) {
        entry.duplicate = duplicate;
    }
}

fn root_trivia(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || (indentation(line) == 0 && trimmed.starts_with('#'))
}

fn build_entry(
    key: String,
    semantic_key: YamlValue,
    line: usize,
    end_line: usize,
    raw_value: Option<YamlValue>,
    value_token: &str,
    source_lines: &[Line],
    opaque_key: bool,
    source_range: Range<usize>,
    leading_range: Range<usize>,
    key_range: Range<usize>,
    colon_range: Range<usize>,
    value_range: Range<usize>,
    inline_comment_range: Option<Range<usize>>,
) -> PropertyEntry {
    let value_is_opaque = raw_value
        .as_ref()
        .is_some_and(|value| !value_is_json_safe(value));
    let raw_features = yaml_spans::has_yaml_feature(source_lines);
    let opaque =
        opaque_key || raw_value.is_none() || value_is_opaque || raw_features;
    let kind = if opaque {
        PropertyKind::Opaque
    } else {
        classify_kind(&key, raw_value.as_ref().expect("nonopaque has value"))
    };
    let style = value_style(value_token, raw_value.as_ref());
    let diagnostic = opaque.then(|| {
        if raw_features {
            "anchors, aliases, and custom YAML tags are shown as opaque"
                .to_owned()
        } else if opaque_key {
            "non-string YAML property keys are not editable properties"
                .to_owned()
        } else if value_is_opaque {
            "this YAML value cannot be represented safely as JSON".to_owned()
        } else {
            "this property value could not be parsed safely".to_owned()
        }
    });
    let value = if opaque {
        JsonValue::Null
    } else {
        crate::native::records::fingerprint::yaml_to_canonical_json(
            raw_value.as_ref().expect("nonopaque has value"),
        )
    };
    PropertyEntry {
        key,
        semantic_key,
        line: Some(line),
        end_line: Some(end_line.max(line)),
        value,
        raw_value,
        kind,
        style,
        duplicate: false,
        opaque,
        diagnostic,
        source_range,
        leading_range,
        key_range,
        colon_range,
        value_range,
        inline_comment_range,
    }
}

fn value_is_json_safe(value: &YamlValue) -> bool {
    match value {
        YamlValue::Mapping(mapping) => mapping.iter().all(|(key, value)| {
            key.as_str().is_some() && value_is_json_safe(value)
        }),
        YamlValue::Sequence(values) => values.iter().all(value_is_json_safe),
        YamlValue::Tagged(_) => false,
        YamlValue::Number(number) => {
            number.as_i64().is_some()
                || number.as_u64().is_some()
                || number.as_f64().is_some_and(f64::is_finite)
        }
        YamlValue::Null | YamlValue::Bool(_) | YamlValue::String(_) => true,
    }
}

fn classify_kind(key: &str, value: &YamlValue) -> PropertyKind {
    match value {
        YamlValue::Null => PropertyKind::Null,
        YamlValue::Bool(_) => PropertyKind::Checkbox,
        YamlValue::Number(_) => PropertyKind::Number,
        YamlValue::String(text) => {
            if key == "tags" {
                PropertyKind::Tags
            } else if key == "aliases" {
                PropertyKind::Aliases
            } else if is_date(text) {
                PropertyKind::Date
            } else if is_datetime(text) {
                PropertyKind::Datetime
            } else {
                PropertyKind::Text
            }
        }
        YamlValue::Sequence(_) => match key {
            "tags" => PropertyKind::Tags,
            "aliases" => PropertyKind::Aliases,
            _ => PropertyKind::List,
        },
        YamlValue::Mapping(_) => PropertyKind::Map,
        YamlValue::Tagged(_) => PropertyKind::Opaque,
    }
}

fn is_date(text: &str) -> bool {
    text.len() == 10
        && NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == text)
}

fn datetime_parts(text: &str) -> Option<(String, String)> {
    let separator_index = text.char_indices().find_map(|(index, ch)| {
        matches!(ch, 'T' | ' ').then_some((index, ch))
    })?;
    let (index, separator) = separator_index;
    let date = &text[..index];
    if !is_date(date) {
        return None;
    }
    let time_tail = &text[index + 1..];
    let (time, offset) =
        if let Some(prefix) = time_tail.strip_suffix('Z') {
            (prefix, "Z")
        } else {
            let offset_start = time_tail.char_indices().skip(1).find_map(
                |(offset_index, ch)| {
                    matches!(ch, '+' | '-').then_some(offset_index)
                },
            );
            if let Some(offset_start) = offset_start {
                (&time_tail[..offset_start], &time_tail[offset_start..])
            } else {
                (time_tail, "")
            }
        };
    if !valid_time(time) || !valid_offset(offset) {
        return None;
    }
    Some((separator.to_string(), offset.to_owned()))
}

fn valid_time(time: &str) -> bool {
    ["%H:%M:%S%.f", "%H:%M:%S", "%H:%M"]
        .iter()
        .any(|format| NaiveTime::parse_from_str(time, format).is_ok())
}

fn valid_offset(offset: &str) -> bool {
    if offset.is_empty() || offset == "Z" {
        return true;
    }
    let Some(signless) = offset
        .strip_prefix('+')
        .or_else(|| offset.strip_prefix('-'))
    else {
        return false;
    };
    let (hours, minutes) =
        if let Some((hours, minutes)) = signless.split_once(':') {
            if hours.len() != 2
                || minutes.len() != 2
                || !hours.bytes().all(|byte| byte.is_ascii_digit())
                || !minutes.bytes().all(|byte| byte.is_ascii_digit())
            {
                return false;
            }
            (hours, minutes)
        } else {
            if signless.len() != 4
                || !signless.bytes().all(|byte| byte.is_ascii_digit())
            {
                return false;
            }
            (&signless[..2], &signless[2..])
        };
    if !hours.bytes().all(|byte| byte.is_ascii_digit())
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let hours = hours.parse::<u32>().ok();
    let minutes = minutes.parse::<u32>().ok();
    hours.is_some_and(|hours| hours <= 23)
        && minutes.is_some_and(|minutes| minutes <= 59)
}

fn is_datetime(text: &str) -> bool {
    datetime_parts(text).is_some()
}

fn value_style(value_token: &str, value: Option<&YamlValue>) -> ValueStyle {
    let trimmed = value_token.trim_start();
    let quoted = match trimmed.as_bytes().first() {
        Some(b'\'') => Some("single".to_owned()),
        Some(b'"') => Some("double".to_owned()),
        _ => None,
    };
    let flow = trimmed.starts_with('{') || trimmed.starts_with('[');
    let datetime = value.and_then(YamlValue::as_str).and_then(datetime_parts);
    ValueStyle {
        quoted,
        flow,
        datetime_separator: datetime
            .as_ref()
            .map(|(separator, _)| separator.clone()),
        datetime_offset: datetime.map(|(_, offset)| offset),
    }
}

fn line_offsets(lines: &[Line]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut offset = 0;
    for line in lines {
        offsets.push(offset);
        offset += line.text.len() + line.ending.len();
    }
    offsets.push(offset);
    offsets
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{FrontmatterState, PropertyKind, PropsDocument};

    #[test]
    fn inventory_keeps_source_order_numeric_looking_names_and_bytes() {
        let note =
            "---\r\n'10': ten # keep\r\n'2': two\r\n\r\n---\r\n# body\r\n";
        let document = PropsDocument::parse(note);
        assert_eq!(document.state, FrontmatterState::Ok);
        assert_eq!(
            document
                .entries
                .iter()
                .map(|item| item.key.as_str())
                .collect::<Vec<_>>(),
            ["10", "2"]
        );
        assert_eq!(document.entries[0].value, json!("ten"));
        assert_eq!(document.entries[0].line, Some(2));
        assert!(document.entries[0]
            .raw(&document.source)
            .contains("'10': ten # keep\r\n"));
        assert_eq!(
            &document.source[document.entries[0].key_range.clone()],
            "'10'"
        );
        assert_eq!(
            &document.source[document.entries[0].colon_range.clone()],
            ":"
        );
        assert_eq!(
            &document.source[document.entries[0].value_range.clone()],
            "ten"
        );
        assert_eq!(
            document.entries[0]
                .inline_comment_range
                .as_ref()
                .map(|range| &document.source[range.clone()]),
            Some("# keep")
        );
        assert_eq!(document.end_line, Some(5));
    }

    #[test]
    fn null_empty_false_zero_and_empty_list_remain_distinct() {
        let document = PropsDocument::parse(
            "---\na: null\nb: ''\nc: false\nd: 0\ne: []\n---\n",
        );
        assert_eq!(document.state, FrontmatterState::Ok);
        assert_eq!(document.entries[0].kind, PropertyKind::Null);
        assert_eq!(document.entries[1].value, json!(""));
        assert_eq!(document.entries[2].kind, PropertyKind::Checkbox);
        assert_eq!(document.entries[3].value, json!(0));
        assert_eq!(document.entries[4].kind, PropertyKind::List);
        assert_ne!(document.entries[0].value, document.entries[1].value);
        assert_ne!(document.entries[2].value, document.entries[3].value);
    }

    #[test]
    fn duplicate_keys_are_visible_and_disable_the_fingerprint() {
        let document =
            PropsDocument::parse("---\nname: first\n'name': second\n---\n");
        assert_eq!(document.state, FrontmatterState::Invalid);
        assert!(document.fingerprint.is_none());
        assert_eq!(document.entries.len(), 2);
        assert!(document.entries.iter().all(|entry| entry.duplicate));
        assert_eq!(document.entries[1].line, Some(3));
    }

    #[test]
    fn invalid_and_unclosed_frontmatter_return_recovery_rows() {
        let invalid = PropsDocument::parse("---\nname: [\n---\nbody\n");
        assert_eq!(invalid.state, FrontmatterState::Invalid);
        assert!(invalid.error.as_ref().unwrap().line.is_some());
        assert_eq!(invalid.entries.len(), 1);
        let unclosed = PropsDocument::parse("---\nname: visible\n");
        assert_eq!(unclosed.state, FrontmatterState::Unclosed);
        assert_eq!(unclosed.entries.len(), 1);
        assert_eq!(unclosed.entries[0].value, json!("visible"));
    }

    #[test]
    fn absent_frontmatter_has_an_empty_mapping_fingerprint_and_keeps_bom() {
        let document = PropsDocument::parse("\u{feff}# Body\n");
        assert_eq!(document.state, FrontmatterState::Absent);
        assert_eq!(document.entries.len(), 0);
        assert!(document
            .fingerprint
            .as_ref()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(document.body_range.start, 3);
        assert_eq!(document.bom_range, Some(0..3));
    }

    #[test]
    fn comment_only_frontmatter_is_an_empty_mapping() {
        let absent = PropsDocument::parse("# Body\n");
        let commented =
            PropsDocument::parse("---\n# authored empty block\n\n---\n");
        assert_eq!(commented.state, FrontmatterState::Ok);
        assert!(commented.entries.is_empty());
        assert_eq!(commented.fingerprint, absent.fingerprint);
    }

    #[test]
    fn opaque_nested_mapping_values_keep_the_document_fingerprint() {
        let document = PropsDocument::parse("---\nconfig: {1: number}\n---\n");
        assert_eq!(document.state, FrontmatterState::Ok);
        assert!(document.fingerprint.is_some());
        assert!(document.entries[0].opaque);
        assert_eq!(document.entries[0].kind, PropertyKind::Opaque);
        assert_eq!(document.entries[0].value, serde_json::Value::Null);
        assert!(document.entries[0].diagnostic.is_some());
    }

    #[test]
    fn anchor_and_alias_values_are_reported_as_opaque() {
        let document = PropsDocument::parse(
            "---\ndefaults: &base {x: 1}\ncopy: *base\n---\n",
        );
        assert_eq!(document.state, FrontmatterState::Ok);
        assert!(document.entries.iter().all(|entry| entry.opaque));
        assert!(document
            .entries
            .iter()
            .all(|entry| entry.diagnostic.is_some()));
    }

    #[test]
    fn frontmatter_span_leaves_the_bom_outside() {
        let document =
            PropsDocument::parse("\u{feff}---\nname: value\n---\nbody");
        assert_eq!(document.bom_range, Some(0..3));
        let range = document.frontmatter_range.unwrap();
        assert_eq!(&document.source[range], "---\nname: value\n---\n");
    }

    #[test]
    fn root_flow_mappings_retain_their_source_order() {
        let document =
            PropsDocument::parse("---\n{ '10': ten, '2': two }\n---\n");
        assert_eq!(document.state, FrontmatterState::Ok);
        assert_eq!(
            document
                .entries
                .iter()
                .map(|item| item.key.as_str())
                .collect::<Vec<_>>(),
            ["10", "2"]
        );
        assert_eq!(document.entries[0].value, json!("ten"));
        assert_eq!(document.entries[1].value, json!("two"));
    }

    #[test]
    fn datetime_style_reports_separator_and_offset_form() {
        let document = PropsDocument::parse(
            "---\nat: 2026-11-22 09:41:00-0500\nother: 2026-11-22T09:41:00-05:00\n---\n",
        );
        assert_eq!(document.entries[0].kind, PropertyKind::Datetime);
        assert_eq!(
            document.entries[0].style.datetime_separator.as_deref(),
            Some(" ")
        );
        assert_eq!(
            document.entries[0].style.datetime_offset.as_deref(),
            Some("-0500")
        );
        assert_eq!(
            document.entries[1].style.datetime_offset.as_deref(),
            Some("-05:00")
        );
    }
}
