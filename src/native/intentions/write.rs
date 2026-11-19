//! Pure, verified line-span YAML edits for the five intention-owned keys.

use serde_yaml::{Mapping, Value};

use crate::native::yaml_spans::{self, Line};

const OWNED: [&str; 5] = [
    "intentions",
    "summary",
    "summary_digest",
    "summary_drafted",
    "reflection",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Patch<T> {
    #[default]
    Untouched,
    Set(T),
    Unset,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OwnedPatch {
    pub(crate) intentions: Patch<Vec<String>>,
    pub(crate) summary: Patch<String>,
    pub(crate) summary_digest: Patch<String>,
    pub(crate) summary_drafted: Patch<String>,
    pub(crate) reflection: Patch<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}]: {}", self.code, self.message)
    }
}

/// Apply an explicit field patch without filesystem effects. The original
/// frontmatter and the complete proposed mapping are reparsed before success.
pub(crate) fn apply(
    original: &str,
    patch: &OwnedPatch,
) -> Result<String, WriteError> {
    if is_noop(patch) {
        return Ok(original.to_owned());
    }
    let (mut lines, open, close, had_frontmatter) = split_document(original)?;
    let newline = lines
        .iter()
        .find(|line| !line.ending.is_empty())
        .map(|line| line.ending.clone())
        .unwrap_or_else(|| "\n".to_owned());
    let original_map = if had_frontmatter {
        parse_mapping(&lines[open + 1..close])?
    } else {
        Mapping::new()
    };
    let mut expected = original_map.clone();
    let patches = patch_values(patch);
    for (name, value) in &patches {
        let key = Value::String((*name).to_owned());
        match value {
            Desired::Untouched => {}
            Desired::Unset => {
                expected.remove(&key);
            }
            Desired::Set(value) => {
                expected.insert(key, value.clone());
            }
        }
    }
    if expected == original_map {
        return Ok(original.to_owned());
    }

    if !had_frontmatter {
        let body = yaml_spans::join_lines(&lines);
        let mut header = vec![Line {
            text: "---".to_owned(),
            ending: newline.clone(),
        }];
        for name in OWNED {
            if let Some((_, Desired::Set(value))) =
                patches.iter().find(|(key, _)| *key == name)
            {
                render_field(name, value, &newline, &mut header);
            }
        }
        header.push(Line {
            text: "---".to_owned(),
            ending: if body.is_empty() {
                String::new()
            } else {
                newline.clone()
            },
        });
        let mut output = yaml_spans::join_lines(&header);
        output.push_str(&body);
        return verify(original, &output, &expected, &patches);
    }

    let spans =
        yaml_spans::collect_key_spans(&lines[open + 1..close], open + 1)
            .map_err(|message| error("duplicate_key", message))?;
    let mut edits: Vec<_> = patches
        .iter()
        .filter_map(|(name, desired)| {
            if matches!(desired, Desired::Untouched) {
                return None;
            }
            spans
                .iter()
                .find(|span| span.name == *name)
                .map(|span| (*name, desired, span.clone()))
        })
        .collect();
    edits.sort_by_key(|(_, _, span)| std::cmp::Reverse(span.start));
    for (name, desired, span) in edits {
        let global_start = span.start;
        let global_end = span.end;
        let preserved = collect_comments(
            &lines[global_start..=global_end],
            !matches!(desired, Desired::Set(_)),
        );
        match desired {
            Desired::Unset => {
                yaml_spans::apply_frontmatter_edits(
                    &mut lines,
                    global_start,
                    global_end,
                    preserved,
                );
            }
            Desired::Set(value) => {
                let colon = span.colon - global_start;
                let old_line = &lines[global_start].text;
                let after = &old_line[colon + 1..];
                let (_, inline_comment) = yaml_spans::split_comment(after);
                let prefix = format!(
                    "{}:{}",
                    &old_line[..colon],
                    yaml_spans::gap_after_colon(after)
                );
                let mut replacement = Vec::new();
                let comment = if inline_comment.is_empty() {
                    String::new()
                } else {
                    let spacing = yaml_spans::whitespace_before_comment(after);
                    format!(
                        "{}{inline_comment}",
                        if spacing.is_empty() { " " } else { spacing }
                    )
                };
                render_field_with_prefix(
                    name,
                    value,
                    &newline,
                    &prefix,
                    &comment,
                    &mut replacement,
                );
                replacement.extend(preserved);
                yaml_spans::apply_frontmatter_edits(
                    &mut lines,
                    global_start,
                    global_end,
                    replacement,
                );
            }
            Desired::Untouched => unreachable!(),
        }
    }

    // Rescan after replacements and append absent keys in the contract order.
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| {
            (line.text.trim() == "---"
                && line.text.trim_start_matches('\u{feff}').trim() == "---")
                .then_some(index)
        })
        .ok_or_else(|| {
            error("frontmatter_invalid", "closing delimiter was lost")
        })?;
    let now = yaml_spans::collect_key_spans(&lines[open + 1..close], open + 1)
        .map_err(|message| error("duplicate_key", message))?;
    let existing: std::collections::BTreeSet<_> =
        now.iter().map(|span| span.name.as_str()).collect();
    let mut added = Vec::new();
    for name in OWNED {
        if existing.contains(name) {
            continue;
        }
        if let Some((_, Desired::Set(value))) =
            patches.iter().find(|(key, _)| *key == name)
        {
            render_field(name, value, &newline, &mut added);
        }
    }
    if !added.is_empty() {
        if close > 0 && lines[close - 1].ending.is_empty() {
            lines[close - 1].ending = newline.clone();
        }
        for line in &mut added {
            if line.ending.is_empty() {
                line.ending = newline.clone();
            }
        }
        lines.splice(close..close, added);
    }
    let output = yaml_spans::join_lines(&lines);
    verify(original, &output, &expected, &patches)
}

#[derive(Debug, Clone)]
enum Desired {
    Untouched,
    Set(Value),
    Unset,
}

fn patch_values(patch: &OwnedPatch) -> Vec<(&'static str, Desired)> {
    vec![
        (
            "intentions",
            match &patch.intentions {
                Patch::Untouched => Desired::Untouched,
                Patch::Unset => Desired::Unset,
                Patch::Set(values) if values.is_empty() => Desired::Unset,
                Patch::Set(values) => Desired::Set(Value::Sequence(
                    values.iter().cloned().map(Value::String).collect(),
                )),
            },
        ),
        ("summary", desired_text(&patch.summary)),
        ("summary_digest", desired_text(&patch.summary_digest)),
        ("summary_drafted", desired_text(&patch.summary_drafted)),
        ("reflection", desired_text(&patch.reflection)),
    ]
}

fn desired_text(patch: &Patch<String>) -> Desired {
    match patch {
        Patch::Untouched => Desired::Untouched,
        Patch::Unset => Desired::Unset,
        Patch::Set(value) if value.is_empty() => Desired::Unset,
        Patch::Set(value) => Desired::Set(Value::String(value.clone())),
    }
}

fn is_noop(patch: &OwnedPatch) -> bool {
    matches!(patch.intentions, Patch::Untouched)
        && matches!(patch.summary, Patch::Untouched)
        && matches!(patch.summary_digest, Patch::Untouched)
        && matches!(patch.summary_drafted, Patch::Untouched)
        && matches!(patch.reflection, Patch::Untouched)
}

fn split_document(
    original: &str,
) -> Result<(Vec<Line>, usize, usize, bool), WriteError> {
    let lines = yaml_spans::split_lines(original);
    let Some(first) = lines.first() else {
        return Ok((lines, 0, 0, false));
    };
    if first
        .text
        .strip_prefix('\u{feff}')
        .unwrap_or(&first.text)
        .trim()
        != "---"
    {
        return Ok((lines, 0, 0, false));
    }
    let Some(close) =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            (line.text.trim() == "---").then_some(index)
        })
    else {
        return Err(error(
            "frontmatter_invalid",
            "cannot patch an unclosed frontmatter block",
        ));
    };
    Ok((lines, 0, close, true))
}

fn parse_mapping(lines: &[Line]) -> Result<Mapping, WriteError> {
    yaml_spans::collect_key_spans(lines, 0)
        .map_err(|message| error("duplicate_key", message))?;
    let yaml = yaml_spans::yaml_text(lines);
    if yaml.lines().all(|line| {
        line.trim().is_empty() || line.trim_start().starts_with('#')
    }) {
        return Ok(Mapping::new());
    }
    if yaml_spans::validate_flow_map_keys(&yaml)
        == Err(yaml_spans::FlowMapError::DuplicateKey)
    {
        return Err(error("duplicate_key", "duplicate key in flow mapping"));
    }
    let value: Value = serde_yaml::from_str(&yaml).map_err(|parse_error| {
        error(
            "frontmatter_invalid",
            format!("invalid frontmatter YAML: {parse_error}"),
        )
    })?;
    match value {
        Value::Mapping(mapping) => Ok(mapping),
        _ => Err(error(
            "frontmatter_invalid",
            "frontmatter YAML must be a mapping",
        )),
    }
}

fn collect_comments(lines: &[Line], include_inline: bool) -> Vec<Line> {
    let mut comments = Vec::new();
    let (value, inline) = lines
        .first()
        .map(|line| yaml_spans::split_comment(line.text.as_str()))
        .unwrap_or(("", ""));
    if include_inline && !inline.is_empty() {
        let spacing = &value[value.trim_end().len()..];
        comments.push(Line {
            text: format!("{spacing}{inline}"),
            ending: lines[0].ending.clone(),
        });
    }
    for line in lines.iter().skip(1) {
        let trimmed = line.text.trim_start();
        if trimmed.starts_with('#')
            || (trimmed.is_empty() && !comments.is_empty())
        {
            comments.push(line.clone());
        }
    }
    comments
}

fn render_field(
    name: &str,
    value: &Value,
    newline: &str,
    output: &mut Vec<Line>,
) {
    let prefix = format!("{name}: ");
    render_field_with_prefix(name, value, newline, &prefix, "", output);
}

fn render_field_with_prefix(
    name: &str,
    value: &Value,
    newline: &str,
    prefix: &str,
    inline_comment: &str,
    output: &mut Vec<Line>,
) {
    match (name, value) {
        ("intentions", Value::Sequence(items)) => {
            if items.is_empty() {
                return;
            }
            let key_prefix = prefix.trim_end();
            output.push(Line {
                text: format!("{key_prefix}{inline_comment}"),
                ending: newline.to_owned(),
            });
            for item in items {
                let text = item.as_str().unwrap_or_default();
                output.push(Line {
                    text: format!("  - {}", quote_yaml(text)),
                    ending: newline.to_owned(),
                });
            }
        }
        ("reflection", Value::String(text))
            if text.contains(['\n', '\r']) && block_scalar_safe(text) =>
        {
            output.push(Line {
                text: format!("{prefix}|-{inline_comment}"),
                ending: newline.to_owned(),
            });
            for line in text.split('\n') {
                output.push(Line {
                    text: format!("  {line}"),
                    ending: newline.to_owned(),
                });
            }
            if text.ends_with('\n') {
                output.push(Line {
                    text: "  ".to_owned(),
                    ending: newline.to_owned(),
                });
            }
        }
        (_, Value::String(text)) => {
            output.push(Line {
                text: format!("{prefix}{}{inline_comment}", quote_yaml(text)),
                ending: newline.to_owned(),
            });
        }
        _ => {}
    }
}

fn quote_yaml(text: &str) -> String {
    if text.contains("[[")
        || text.starts_with([
            '-', '?', ':', '[', ']', '{', '}', ',', '#', '&', '*', '!', '|',
            '>', '%', '@', '`',
        ])
        || text.contains(": ")
        || text.contains(" #")
    {
        serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
    } else {
        yaml_spans::yaml_string_scalar(text)
    }
}

fn block_scalar_safe(text: &str) -> bool {
    if text.starts_with([' ', '\t', '\n', '\r'])
        || text.ends_with([' ', '\t', '\n', '\r'])
    {
        return false;
    }
    text.split('\n').all(|line| {
        !line.ends_with([' ', '\t']) && !line.starts_with([' ', '\t'])
    })
}

fn verify(
    original: &str,
    proposed: &str,
    expected: &Mapping,
    patches: &[(&'static str, Desired)],
) -> Result<String, WriteError> {
    let (_, _, close, had_frontmatter) = split_document(proposed)?;
    let lines = yaml_spans::split_lines(proposed);
    let proposed_map = if had_frontmatter {
        parse_mapping(&lines[1..close])?
    } else {
        Mapping::new()
    };
    if &proposed_map != expected {
        return Err(error(
            "write_verify_failed",
            "the proposed YAML mapping differs from the intended patch",
        ));
    }
    for (name, desired) in patches {
        if let Desired::Set(value) = desired {
            let got = proposed_map.get(Value::String((*name).to_owned()));
            if got != Some(value) {
                return Err(error(
                    "write_verify_failed",
                    format!("{name} did not round-trip"),
                ));
            }
        }
    }
    // A no-frontmatter insertion preserves the original body byte-for-byte;
    // existing frontmatter edits are generated exclusively by span splices.
    let _ = original;
    Ok(proposed.to_owned())
}

fn error(code: &'static str, message: impl Into<String>) -> WriteError {
    WriteError {
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_op_preserves_exact_original_bytes() {
        let original =
            "\u{feff}---\r\nkey: yes\r\n---\r\nbody without final newline";
        assert_eq!(apply(original, &OwnedPatch::default()).unwrap(), original);
    }

    #[test]
    fn absent_frontmatter_is_inserted_without_changing_the_original_body() {
        let original = "# Daily note\n\nBody without a final newline";
        let patch = OwnedPatch {
            intentions: Patch::Set(vec!["plan".to_owned()]),
            ..OwnedPatch::default()
        };
        let output = apply(original, &patch).unwrap();
        assert_eq!(output, "---\nintentions:\n  - plan\n---\n# Daily note\n\nBody without a final newline");
        assert_eq!(
            apply(
                original,
                &OwnedPatch {
                    intentions: Patch::Unset,
                    ..OwnedPatch::default()
                }
            )
            .unwrap(),
            original
        );
    }

    #[test]
    fn edits_flow_values_and_additions_preserve_bom_crlf_comments_and_body() {
        let original = "\u{feff}---\r\nother: keep # unrelated\r\nintentions: [old] # list note\r\nsummary: Old # summary note\r\ncustom: value\r\n---\r\nbody stays\r\n";
        let patch = OwnedPatch {
            intentions: Patch::Set(vec![
                "[[notes]]".to_owned(),
                "true".to_owned(),
                "x: y".to_owned(),
            ]),
            summary: Patch::Set("A better account".to_owned()),
            reflection: Patch::Set("Meaning\nsecond paragraph".to_owned()),
            ..OwnedPatch::default()
        };
        let output = apply(original, &patch).unwrap();
        assert!(output.starts_with("\u{feff}---\r\n"));
        assert!(output.contains("other: keep # unrelated\r\n"));
        assert!(output.contains("intentions: # list note\r\n  - \"[[notes]]\"\r\n  - \"true\"\r\n  - \"x: y\"\r\n"));
        assert!(output.contains("summary: A better account # summary note\r\n"));
        assert!(output
            .contains("reflection: |-\r\n  Meaning\r\n  second paragraph\r\n"));
        assert!(output.ends_with("body stays\r\n"));
    }

    #[test]
    fn empty_values_unset_and_quotes_round_trip_without_empty_placeholders() {
        let original = "---\nsummary: old\nsummary_digest: sha256:0000000000000000\nreflection: old meaning\n---\nbody\n";
        let patch = OwnedPatch {
            summary: Patch::Set(String::new()),
            summary_digest: Patch::Unset,
            reflection: Patch::Set(
                "  keep leading space\ntrailing\t".to_owned(),
            ),
            ..OwnedPatch::default()
        };
        let output = apply(original, &patch).unwrap();
        assert!(!output.contains("summary:"));
        assert!(!output.contains("summary_digest:"));
        assert!(!output.contains("intentions: []"));
        assert!(output
            .contains("reflection: \"  keep leading space\\ntrailing\\t\""));
        assert!(output.ends_with("---\nbody\n"));
    }

    #[test]
    fn malformed_or_duplicate_frontmatter_is_refused_and_noop_is_exact() {
        let malformed = "---\nintentions: [\n---\nbody\n";
        let patch = OwnedPatch {
            summary: Patch::Set("new".to_owned()),
            ..OwnedPatch::default()
        };
        assert_eq!(
            apply(malformed, &patch).unwrap_err().code,
            "frontmatter_invalid"
        );
        let duplicate = "---\nintentions: [one]\n\"intentions\": [two]\n---\n";
        assert_eq!(apply(duplicate, &patch).unwrap_err().code, "duplicate_key");
        assert_eq!(
            apply(malformed, &OwnedPatch::default()).unwrap(),
            malformed
        );
    }

    #[test]
    fn removing_every_owned_key_keeps_an_empty_frontmatter_block_valid() {
        let original = "---\nintentions: [plan]\nsummary: note\nsummary_digest: sha256:1111111111111111\nsummary_drafted: 2026-11-19\nreflection: private\n---\nbody\n";
        let output = apply(
            original,
            &OwnedPatch {
                intentions: Patch::Set(Vec::new()),
                summary: Patch::Unset,
                summary_digest: Patch::Unset,
                summary_drafted: Patch::Unset,
                reflection: Patch::Unset,
            },
        )
        .expect("empty frontmatter mapping remains valid");
        assert_eq!(output, "---\n---\nbody\n");
        let parsed = crate::native::intentions::read(
            &output,
            crate::native::intentions::NoteSubject::Day(
                chrono::NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
            ),
        );
        assert!(parsed.diagnostics.is_empty());
    }
}
