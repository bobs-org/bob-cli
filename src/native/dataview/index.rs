use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
    sync::LazyLock,
    time::SystemTime,
};

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use regex::Regex;
use serde_json::Number;

use super::value::{DataviewLink, DataviewValue};
use super::{
    collect_native_markdown_paths, native_frontmatter_block,
    normalize_note_path, note_stem, unquote_native_scalar, DataviewError,
};

static LINE_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Za-z][A-Za-z0-9_-]*)::\s*(.*?)\s*$")
        .expect("valid inline field regex")
});
static BRACKET_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[([A-Za-z][A-Za-z0-9_-]*)::\s*([^\]]+)\]")
        .expect("valid bracket inline field regex")
});
static WIKILINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!?\[\[([^\]\n]+)\]\]").expect("valid wikilink regex")
});
static MARKDOWN_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[\s(\[{>])(#(?:[A-Za-z0-9][A-Za-z0-9_/-]*))")
        .expect("valid tag regex")
});
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*#{1,6}\s+(.+?)\s*$").expect("valid heading regex")
});
static LIST_MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\s*)[-*+]\s+(?:\[([^\]])\]\s+)?(.*)$")
        .expect("valid markdown list regex")
});
static BLOCK_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\s)\^([A-Za-z0-9_-]+)(?:\s*$)")
        .expect("valid block id regex")
});
static CLEAN_INLINE_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*\[[A-Za-z][A-Za-z0-9_-]*::\s*[^\]]+\]")
        .expect("valid inline field cleanup regex")
});
static CLEAN_BLOCK_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s+\^[A-Za-z0-9_-]+(?:\s*$)")
        .expect("valid block id cleanup regex")
});

#[derive(Debug)]
pub(super) struct DataviewIndex {
    pub(super) pages: Vec<DataviewPage>,
    pub(super) by_path: HashMap<String, usize>,
    pub(super) by_stem: HashMap<String, Vec<usize>>,
    pub(super) by_alias: HashMap<String, Vec<usize>>,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug)]
pub(super) struct DataviewPage {
    pub(super) path: String,
    pub(super) fields: HashMap<String, DataviewValue>,
    pub(super) source_tags: Vec<String>,
}

#[derive(Debug)]
struct PageDraft {
    path: String,
    size: u64,
    ctime: Option<SystemTime>,
    mtime: Option<SystemTime>,
    fields: HashMap<String, DataviewValue>,
    frontmatter: BTreeMap<String, DataviewValue>,
    aliases: Vec<String>,
    explicit_tags: Vec<String>,
    tags: Vec<String>,
    source_tags: Vec<String>,
    outlinks: Vec<DataviewLink>,
    tasks: Vec<DataviewValue>,
    lists: Vec<DataviewValue>,
}

#[derive(Debug, Clone)]
struct LinkLookup {
    by_path: HashMap<String, usize>,
    by_stem: HashMap<String, Vec<usize>>,
    by_alias: HashMap<String, Vec<usize>>,
    page_paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct ListDraft {
    indent: usize,
    line: usize,
    text: String,
    visual: String,
    task_status: Option<char>,
    inline_fields: BTreeMap<String, DataviewValue>,
    tags: Vec<String>,
    outlinks: Vec<DataviewLink>,
    block_id: Option<String>,
    section: Option<DataviewLink>,
    parent: Option<usize>,
    children: Vec<usize>,
}

impl DataviewIndex {
    pub(super) fn read(bob_dir: &Path) -> Result<Self, DataviewError> {
        let mut paths = Vec::new();
        collect_native_markdown_paths(bob_dir, &mut paths)?;
        paths.sort();

        let starred = read_starred_paths(bob_dir);
        let mut drafts = Vec::new();
        for path in paths {
            drafts.push(PageDraft::read(bob_dir, &path)?);
        }

        let lookup = LinkLookup::from_drafts(&drafts);
        let mut warnings = Vec::new();
        for draft in &mut drafts {
            draft.canonicalize_links(&lookup, &mut warnings);
        }
        let warnings = dedup_strings(warnings);

        let inlinks = collect_inlinks(&drafts, &lookup);
        let pages = drafts
            .into_iter()
            .map(|draft| {
                let path = draft.path.clone();
                let starred = starred.contains(&path);
                draft.into_page(inlinks.get(&path), starred)
            })
            .collect::<Vec<_>>();

        let by_path = pages
            .iter()
            .enumerate()
            .map(|(index, page)| (page.path.clone(), index))
            .collect::<HashMap<_, _>>();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_alias: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, page) in pages.iter().enumerate() {
            if let Some(stem) = note_stem(&page.path) {
                by_stem.entry(stem).or_default().push(index);
            }
            for alias in page_aliases(page) {
                by_alias.entry(alias).or_default().push(index);
            }
        }

        Ok(Self {
            pages,
            by_path,
            by_stem,
            by_alias,
            warnings,
        })
    }

    pub(super) fn resolve_link_path(&self, raw: &str) -> Option<String> {
        let target = super::native_link_target(raw)?;
        self.resolve_target_path(&target)
    }

    pub(super) fn resolve_target_path(&self, target: &str) -> Option<String> {
        let (base_target, subpath) = split_link_subpath(target);
        let base = normalize_link_base(base_target).ok()?;
        if self.by_path.contains_key(&base) {
            return Some(append_subpath(base, subpath));
        }

        if base_target.contains('/') || base_target.contains('\\') {
            return None;
        }

        let stem = base.strip_suffix(".md").unwrap_or(&base);
        if let Some([index]) = self.by_stem.get(stem).map(Vec::as_slice) {
            let path = self.pages[*index].path.clone();
            return Some(append_subpath(path, subpath));
        }
        if let Some([index]) = self.by_alias.get(base_target).map(Vec::as_slice)
        {
            let path = self.pages[*index].path.clone();
            return Some(append_subpath(path, subpath));
        }

        None
    }
}

impl PageDraft {
    fn read(bob_dir: &Path, path: &Path) -> Result<Self, DataviewError> {
        let contents = fs::read_to_string(path).map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.to_path_buf(),
                error,
            }
        })?;
        let metadata = fs::metadata(path).map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.to_path_buf(),
                error,
            }
        })?;
        let relative_path = path
            .strip_prefix(bob_dir)
            .map_err(|error| DataviewError::NativeQuery {
                message: format!(
                    "vault path {} is outside {}: {error}",
                    path.display(),
                    bob_dir.display()
                ),
            })?
            .to_string_lossy()
            .replace('\\', "/");

        let mut fields = HashMap::new();
        let mut frontmatter = BTreeMap::new();
        if let Some(block) = native_frontmatter_block(&contents) {
            for (key, value) in parse_yaml_frontmatter(block, path)? {
                insert_field(&mut fields, &key, value.clone());
                frontmatter.insert(key, value);
            }
        }

        let (body, body_start_line) = markdown_body(&contents);
        parse_page_inline_fields(body, &mut fields);

        let aliases = extract_aliases(fields.get("aliases"));
        let frontmatter_tags = explicit_file_tags(fields.get("tags"))
            .into_iter()
            .chain(explicit_file_tags(fields.get("tag")))
            .collect::<Vec<_>>();
        let explicit_tags = dedup_strings(
            frontmatter_tags
                .iter()
                .cloned()
                .chain(markdown_tags(body))
                .collect(),
        );
        let tags = expand_file_tags(&explicit_tags);
        let source_tags = expand_file_tags(&dedup_strings(
            frontmatter_tags
                .into_iter()
                .chain(markdown_page_tags(body))
                .collect(),
        ));
        let outlinks = dedup_links(markdown_links(&contents));
        let (tasks, lists) =
            markdown_lists(&relative_path, body, body_start_line, &fields);

        Ok(Self {
            path: relative_path,
            size: metadata.len(),
            ctime: metadata.created().ok(),
            mtime: metadata.modified().ok(),
            fields,
            frontmatter,
            aliases,
            explicit_tags,
            tags,
            source_tags,
            outlinks,
            tasks,
            lists,
        })
    }

    fn canonicalize_links(
        &mut self,
        lookup: &LinkLookup,
        warnings: &mut Vec<String>,
    ) {
        for value in self.fields.values_mut() {
            canonicalize_value_links(value, lookup, warnings);
        }
        for value in self.frontmatter.values_mut() {
            canonicalize_value_links(value, lookup, warnings);
        }
        for link in &mut self.outlinks {
            lookup.canonicalize_link(link, warnings);
        }
        for value in &mut self.tasks {
            canonicalize_value_links(value, lookup, warnings);
        }
        for value in &mut self.lists {
            canonicalize_value_links(value, lookup, warnings);
        }
    }

    fn into_page(
        mut self,
        inlinks: Option<&Vec<DataviewLink>>,
        starred: bool,
    ) -> DataviewPage {
        let inlinks = inlinks.cloned().unwrap_or_default();
        let file = self.file_object(&inlinks, starred);
        self.fields
            .insert("file".to_string(), DataviewValue::Object(file));

        DataviewPage {
            path: self.path,
            fields: self.fields,
            source_tags: self.source_tags,
        }
    }

    fn file_object(
        &self,
        inlinks: &[DataviewLink],
        starred: bool,
    ) -> BTreeMap<String, DataviewValue> {
        let mut file = BTreeMap::new();
        file.insert(
            "name".to_string(),
            DataviewValue::String(page_name(&self.path)),
        );
        file.insert(
            "folder".to_string(),
            DataviewValue::String(page_folder(&self.path)),
        );
        file.insert(
            "path".to_string(),
            DataviewValue::String(self.path.clone()),
        );
        file.insert("ext".to_string(), DataviewValue::String("md".into()));
        file.insert(
            "link".to_string(),
            DataviewValue::Link(DataviewLink::page(&self.path)),
        );
        file.insert(
            "size".to_string(),
            DataviewValue::Number(Number::from(self.size)),
        );
        file.insert("ctime".to_string(), system_datetime(self.ctime));
        file.insert("cday".to_string(), system_date(self.ctime));
        file.insert("mtime".to_string(), system_datetime(self.mtime));
        file.insert("mday".to_string(), system_date(self.mtime));
        file.insert(
            "tags".to_string(),
            DataviewValue::Array(
                self.tags
                    .iter()
                    .cloned()
                    .map(DataviewValue::String)
                    .collect(),
            ),
        );
        file.insert(
            "etags".to_string(),
            DataviewValue::Array(
                self.explicit_tags
                    .iter()
                    .cloned()
                    .map(DataviewValue::String)
                    .collect(),
            ),
        );
        file.insert(
            "inlinks".to_string(),
            DataviewValue::Array(
                inlinks.iter().cloned().map(DataviewValue::Link).collect(),
            ),
        );
        file.insert(
            "outlinks".to_string(),
            DataviewValue::Array(
                self.outlinks
                    .iter()
                    .cloned()
                    .map(DataviewValue::Link)
                    .collect(),
            ),
        );
        file.insert(
            "aliases".to_string(),
            DataviewValue::Array(
                self.aliases
                    .iter()
                    .cloned()
                    .map(DataviewValue::String)
                    .collect(),
            ),
        );
        file.insert(
            "tasks".to_string(),
            DataviewValue::Array(self.tasks.clone()),
        );
        file.insert(
            "lists".to_string(),
            DataviewValue::Array(self.lists.clone()),
        );
        file.insert(
            "frontmatter".to_string(),
            DataviewValue::Object(self.frontmatter.clone()),
        );
        file.insert("day".to_string(), page_day(&self.path));
        file.insert("starred".to_string(), DataviewValue::Bool(starred));
        file
    }
}

impl LinkLookup {
    fn from_drafts(drafts: &[PageDraft]) -> Self {
        let mut by_path = HashMap::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_alias: HashMap<String, Vec<usize>> = HashMap::new();
        let page_paths = drafts
            .iter()
            .map(|draft| draft.path.clone())
            .collect::<Vec<_>>();

        for (index, draft) in drafts.iter().enumerate() {
            by_path.insert(draft.path.clone(), index);
            if let Some(stem) = note_stem(&draft.path) {
                by_stem.entry(stem).or_default().push(index);
            }
            for alias in &draft.aliases {
                by_alias.entry(alias.clone()).or_default().push(index);
            }
        }

        Self {
            by_path,
            by_stem,
            by_alias,
            page_paths,
        }
    }

    fn canonicalize_link(
        &self,
        link: &mut DataviewLink,
        warnings: &mut Vec<String>,
    ) {
        if let Some(path) = self.resolve_target_path(&link.raw_target, warnings)
        {
            link.path = path;
        }
    }

    fn resolve_target_path(
        &self,
        target: &str,
        warnings: &mut Vec<String>,
    ) -> Option<String> {
        let (base_target, subpath) = split_link_subpath(target);
        let base = normalize_link_base(base_target).ok()?;
        if self.by_path.contains_key(&base) {
            return Some(append_subpath(base, subpath));
        }

        if base_target.contains('/') || base_target.contains('\\') {
            return None;
        }

        let stem = base.strip_suffix(".md").unwrap_or(&base);
        if let Some(indices) = self.by_stem.get(stem) {
            if let [index] = indices.as_slice() {
                let path = self.page_paths[*index].clone();
                return Some(append_subpath(path, subpath));
            }
            warnings.push(format!(
                "ambiguous Dataview link target {target:?}; matched {} notes",
                indices.len()
            ));
            return None;
        }

        if let Some(indices) = self.by_alias.get(base_target) {
            if let [index] = indices.as_slice() {
                let path = self.page_paths[*index].clone();
                return Some(append_subpath(path, subpath));
            }
            warnings.push(format!(
                "ambiguous Dataview alias target {target:?}; matched {} notes",
                indices.len()
            ));
        }

        None
    }
}

fn parse_yaml_frontmatter(
    frontmatter: &str,
    path: &Path,
) -> Result<BTreeMap<String, DataviewValue>, DataviewError> {
    if frontmatter.trim().is_empty() {
        return Ok(BTreeMap::new());
    }

    let parsed: serde_yaml::Value =
        serde_yaml::from_str(frontmatter).map_err(|error| {
            DataviewError::NativeQuery {
                message: format!(
                    "failed to parse YAML frontmatter in {}: {error}",
                    path.display()
                ),
            }
        })?;
    let serde_yaml::Value::Mapping(mapping) = parsed else {
        return Ok(BTreeMap::new());
    };

    let mut fields = BTreeMap::new();
    for (key, value) in mapping {
        let Some(key) = yaml_key_to_string(&key) else {
            continue;
        };
        fields.insert(key, yaml_value_to_dataview(value));
    }
    Ok(fields)
}

fn yaml_key_to_string(key: &serde_yaml::Value) -> Option<String> {
    match key {
        serde_yaml::Value::String(value) => Some(value.clone()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        serde_yaml::Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn yaml_value_to_dataview(value: serde_yaml::Value) -> DataviewValue {
    match value {
        serde_yaml::Value::Null => DataviewValue::Null,
        serde_yaml::Value::Bool(value) => DataviewValue::Bool(value),
        serde_yaml::Value::Number(value) => yaml_number_to_dataview(value),
        serde_yaml::Value::String(value) => parse_dataview_scalar(&value),
        serde_yaml::Value::Sequence(values) => DataviewValue::Array(
            values.into_iter().map(yaml_value_to_dataview).collect(),
        ),
        serde_yaml::Value::Mapping(values) => {
            let mut object = BTreeMap::new();
            for (key, value) in values {
                if let Some(key) = yaml_key_to_string(&key) {
                    object.insert(key, yaml_value_to_dataview(value));
                }
            }
            DataviewValue::Object(object)
        }
        serde_yaml::Value::Tagged(value) => yaml_value_to_dataview(value.value),
    }
}

fn yaml_number_to_dataview(value: serde_yaml::Number) -> DataviewValue {
    if let Some(value) = value.as_i64() {
        return DataviewValue::Number(Number::from(value));
    }
    if let Some(value) = value.as_u64() {
        return DataviewValue::Number(Number::from(value));
    }
    if let Some(value) = value.as_f64().and_then(Number::from_f64) {
        return DataviewValue::Number(value);
    }
    DataviewValue::String(value.to_string())
}

fn parse_dataview_scalar(raw: &str) -> DataviewValue {
    let value = unquote_native_scalar(raw.trim());
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("null") || trimmed == "~" {
        return DataviewValue::Null;
    }
    if trimmed.eq_ignore_ascii_case("true") {
        return DataviewValue::Bool(true);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return DataviewValue::Bool(false);
    }
    if let Some(link) = parse_wikilink_literal(trimmed) {
        return DataviewValue::Link(link);
    }
    if let Some(value) = parse_inline_number(trimmed) {
        return value;
    }
    if is_datetime(trimmed) {
        return DataviewValue::DateTime(trimmed.to_string());
    }
    if is_date(trimmed) {
        return DataviewValue::Date(trimmed.to_string());
    }
    if let Some(duration) = duration_to_iso(trimmed) {
        return DataviewValue::Duration(duration);
    }

    DataviewValue::String(value)
}

fn parse_inline_number(value: &str) -> Option<DataviewValue> {
    if let Ok(value) = value.parse::<i64>() {
        return Some(DataviewValue::Number(Number::from(value)));
    }
    if let Ok(value) = value.parse::<u64>() {
        return Some(DataviewValue::Number(Number::from(value)));
    }
    value
        .parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .map(DataviewValue::Number)
}

fn is_date(value: &str) -> bool {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

fn is_datetime(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value).is_ok()
        || NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").is_ok()
}

fn duration_to_iso(value: &str) -> Option<String> {
    let words = value.split_whitespace().collect::<Vec<_>>();
    let [amount, unit] = words.as_slice() else {
        return None;
    };
    let amount = amount.parse::<u64>().ok()?;
    let unit = unit.to_ascii_lowercase();
    let designator = match unit.as_str() {
        "millisecond" | "milliseconds" | "ms" => {
            return Some(format!("PT{amount}MS"));
        }
        "second" | "seconds" | "s" => "S",
        "minute" | "minutes" | "m" => "M",
        "hour" | "hours" | "h" => "H",
        "day" | "days" | "d" => return Some(format!("P{amount}D")),
        "week" | "weeks" | "w" => return Some(format!("P{}D", amount * 7)),
        _ => return None,
    };
    Some(format!("PT{amount}{designator}"))
}

fn parse_wikilink_literal(value: &str) -> Option<DataviewLink> {
    let trimmed = value.trim();
    let (embed, rest) = trimmed
        .strip_prefix("![[")
        .map(|rest| (true, rest))
        .or_else(|| trimmed.strip_prefix("[[").map(|rest| (false, rest)))?;
    let end = rest.find("]]")?;
    if !rest[end + 2..].trim().is_empty() {
        return None;
    }
    link_from_inner(&rest[..end], embed)
}

fn link_from_inner(inner: &str, embed: bool) -> Option<DataviewLink> {
    let (target, display) = inner
        .split_once('|')
        .map_or((inner, None), |(target, display)| {
            (target, Some(display.trim().to_string()))
        });
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    Some(DataviewLink::new(
        normalize_link_target(target),
        display.filter(|display| !display.is_empty()),
        embed,
        target.to_string(),
    ))
}

fn normalize_link_target(target: &str) -> String {
    let (base, subpath) = split_link_subpath(target);
    normalize_link_base(base)
        .map(|base| append_subpath(base, subpath))
        .unwrap_or_else(|_| target.trim().replace('\\', "/"))
}

fn normalize_link_base(target: &str) -> Result<String, String> {
    normalize_note_path(&target.trim().replace('\\', "/"))
}

fn split_link_subpath(target: &str) -> (&str, Option<&str>) {
    target
        .split_once('#')
        .map_or((target, None), |(base, subpath)| (base, Some(subpath)))
}

fn append_subpath(mut path: String, subpath: Option<&str>) -> String {
    if let Some(subpath) = subpath.filter(|subpath| !subpath.is_empty()) {
        path.push('#');
        path.push_str(subpath);
    }
    path
}

fn canonicalize_value_links(
    value: &mut DataviewValue,
    lookup: &LinkLookup,
    warnings: &mut Vec<String>,
) {
    match value {
        DataviewValue::Link(link) => lookup.canonicalize_link(link, warnings),
        DataviewValue::Array(values) => {
            for value in values {
                canonicalize_value_links(value, lookup, warnings);
            }
        }
        DataviewValue::Object(values) => {
            for value in values.values_mut() {
                canonicalize_value_links(value, lookup, warnings);
            }
        }
        DataviewValue::Null
        | DataviewValue::Bool(_)
        | DataviewValue::Number(_)
        | DataviewValue::String(_)
        | DataviewValue::Date(_)
        | DataviewValue::DateTime(_)
        | DataviewValue::Duration(_) => {}
    }
}

fn parse_page_inline_fields(
    body: &str,
    fields: &mut HashMap<String, DataviewValue>,
) {
    for line in body.lines() {
        if markdown_list_line(line).is_some() {
            continue;
        }
        if let Some(captures) = LINE_FIELD_RE.captures(line) {
            insert_field(
                fields,
                &captures[1],
                parse_dataview_scalar(captures[2].trim()),
            );
            continue;
        }
        for captures in BRACKET_FIELD_RE.captures_iter(line) {
            insert_field(
                fields,
                &captures[1],
                parse_dataview_scalar(captures[2].trim()),
            );
        }
    }
}

fn insert_field(
    fields: &mut HashMap<String, DataviewValue>,
    key: &str,
    value: DataviewValue,
) {
    match fields.remove(key) {
        None => {
            fields.insert(key.to_string(), value);
        }
        Some(DataviewValue::Array(mut values)) => {
            values.push(value);
            fields.insert(key.to_string(), DataviewValue::Array(values));
        }
        Some(existing) => {
            fields.insert(
                key.to_string(),
                DataviewValue::Array(vec![existing, value]),
            );
        }
    }
}

fn markdown_body(contents: &str) -> (&str, usize) {
    let Some(frontmatter) = native_frontmatter_block(contents) else {
        return (contents, 0);
    };
    let marker_and_frontmatter = frontmatter.len()
        + if contents.starts_with("---\r\n") {
            5
        } else {
            4
        };
    let after_frontmatter = &contents[marker_and_frontmatter..];
    let closing_len = after_frontmatter
        .strip_prefix("---\r\n")
        .map(|_| 5)
        .or_else(|| after_frontmatter.strip_prefix("---\n").map(|_| 4))
        .unwrap_or(0);
    let body_start = marker_and_frontmatter + closing_len;
    let body_start_line = contents[..body_start].lines().count();
    (&contents[body_start..], body_start_line)
}

fn markdown_links(contents: &str) -> Vec<DataviewLink> {
    WIKILINK_RE
        .find_iter(contents)
        .filter_map(|match_| {
            let raw = match_.as_str();
            let embed = raw.starts_with('!');
            let start = if embed { 3 } else { 2 };
            let inner = &raw[start..raw.len() - 2];
            link_from_inner(inner, embed)
        })
        .collect()
}

fn markdown_tags(contents: &str) -> Vec<String> {
    MARKDOWN_TAG_RE
        .captures_iter(contents)
        .map(|captures| normalize_tag(&captures[2]))
        .collect()
}

fn markdown_page_tags(contents: &str) -> Vec<String> {
    contents
        .lines()
        .filter(|line| markdown_list_line(line).is_none())
        .flat_map(markdown_tags)
        .collect()
}

fn markdown_lists(
    page_path: &str,
    body: &str,
    body_start_line: usize,
    page_fields: &HashMap<String, DataviewValue>,
) -> (Vec<DataviewValue>, Vec<DataviewValue>) {
    let mut drafts: Vec<ListDraft> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut current_section = None;

    for (offset, line) in body.lines().enumerate() {
        if let Some(captures) = HEADING_RE.captures(line) {
            current_section = Some(section_link(page_path, captures[1].trim()));
        }

        let Some((indent, task_status, raw_text)) = markdown_list_line(line)
        else {
            continue;
        };
        while stack
            .last()
            .is_some_and(|index| drafts[*index].indent >= indent)
        {
            stack.pop();
        }
        let parent = stack.last().copied();
        let index = drafts.len();
        if let Some(parent) = parent {
            drafts[parent].children.push(index);
        }
        stack.push(index);

        let inline_fields = inline_fields_in_text(raw_text);
        let block_id = block_id(raw_text);
        let visual = clean_list_text(raw_text);
        let tags = markdown_tags(raw_text);
        let outlinks = markdown_links(raw_text);
        drafts.push(ListDraft {
            indent,
            line: body_start_line + offset,
            text: visual.clone(),
            visual,
            task_status,
            inline_fields,
            tags,
            outlinks,
            block_id,
            section: current_section.clone(),
            parent,
            children: Vec::new(),
        });
    }

    let task_values = drafts
        .iter()
        .enumerate()
        .filter(|(_, draft)| draft.task_status.is_some())
        .map(|(index, _)| list_object(index, &drafts, page_path, page_fields))
        .collect::<Vec<_>>();
    let list_values = drafts
        .iter()
        .enumerate()
        .map(|(index, _)| list_object(index, &drafts, page_path, page_fields))
        .collect::<Vec<_>>();

    (task_values, list_values)
}

fn markdown_list_line(line: &str) -> Option<(usize, Option<char>, &str)> {
    let captures = LIST_MARKER_RE.captures(line)?;
    let indent = captures.get(1).map_or(0, |value| value.as_str().len());
    let task_status = captures
        .get(2)
        .and_then(|value| value.as_str().chars().next());
    let text = captures.get(3).map_or("", |value| value.as_str());
    Some((indent, task_status, text))
}

fn inline_fields_in_text(text: &str) -> BTreeMap<String, DataviewValue> {
    let mut fields = BTreeMap::new();
    for captures in BRACKET_FIELD_RE.captures_iter(text) {
        fields.insert(
            captures[1].to_string(),
            parse_dataview_scalar(captures[2].trim()),
        );
    }
    fields
}

fn block_id(text: &str) -> Option<String> {
    BLOCK_ID_RE
        .captures(text)
        .map(|captures| captures[1].to_string())
}

fn clean_list_text(text: &str) -> String {
    CLEAN_BLOCK_ID_RE
        .replace_all(&CLEAN_INLINE_FIELD_RE.replace_all(text, ""), "")
        .trim()
        .to_string()
}

fn section_link(page_path: &str, heading: &str) -> DataviewLink {
    let subpath = heading.split_whitespace().collect::<Vec<_>>().join(" ");
    DataviewLink::new(
        format!("{page_path}#{subpath}"),
        Some(heading.to_string()),
        false,
        format!("{page_path}#{subpath}"),
    )
}

fn list_object(
    index: usize,
    drafts: &[ListDraft],
    page_path: &str,
    page_fields: &HashMap<String, DataviewValue>,
) -> DataviewValue {
    let draft = &drafts[index];
    let mut object = BTreeMap::new();
    for (key, value) in page_fields {
        object.insert(key.clone(), value.clone());
    }
    for (key, value) in &draft.inline_fields {
        object.insert(key.clone(), value.clone());
    }

    let task = draft.task_status.is_some();
    let status = draft.task_status.unwrap_or(' ').to_string();
    let checked = task && draft.task_status != Some(' ');
    let completed = matches!(draft.task_status, Some('x' | 'X'));
    let fully_completed = completed
        && draft
            .children
            .iter()
            .all(|child| list_fully_completed(*child, drafts));
    let children = draft
        .children
        .iter()
        .map(|child| list_object(*child, drafts, page_path, page_fields))
        .collect::<Vec<_>>();

    object.insert("status".to_string(), DataviewValue::String(status));
    object.insert("checked".to_string(), DataviewValue::Bool(checked));
    object.insert("completed".to_string(), DataviewValue::Bool(completed));
    object.insert(
        "fullyCompleted".to_string(),
        DataviewValue::Bool(fully_completed),
    );
    object.insert(
        "text".to_string(),
        DataviewValue::String(draft.text.clone()),
    );
    object.insert(
        "visual".to_string(),
        DataviewValue::String(draft.visual.clone()),
    );
    object.insert(
        "line".to_string(),
        DataviewValue::Number(Number::from(draft.line as u64)),
    );
    object.insert(
        "lineCount".to_string(),
        DataviewValue::Number(Number::from(1)),
    );
    object.insert(
        "path".to_string(),
        DataviewValue::String(page_path.to_string()),
    );
    object.insert(
        "section".to_string(),
        draft
            .section
            .clone()
            .map(DataviewValue::Link)
            .unwrap_or(DataviewValue::Null),
    );
    object.insert(
        "tags".to_string(),
        DataviewValue::Array(
            draft
                .tags
                .iter()
                .cloned()
                .map(DataviewValue::String)
                .collect(),
        ),
    );
    object.insert(
        "outlinks".to_string(),
        DataviewValue::Array(
            draft
                .outlinks
                .iter()
                .cloned()
                .map(DataviewValue::Link)
                .collect(),
        ),
    );
    object.insert(
        "link".to_string(),
        DataviewValue::Link(task_link(draft, page_path)),
    );
    object.insert("children".to_string(), DataviewValue::Array(children));
    object.insert("task".to_string(), DataviewValue::Bool(task));
    object.insert(
        "annotated".to_string(),
        DataviewValue::Bool(!draft.inline_fields.is_empty()),
    );
    object.insert(
        "parent".to_string(),
        draft
            .parent
            .map(|parent| DataviewValue::Number(Number::from(parent as u64)))
            .unwrap_or(DataviewValue::Null),
    );
    object.insert(
        "blockId".to_string(),
        draft
            .block_id
            .clone()
            .map(DataviewValue::String)
            .unwrap_or(DataviewValue::Null),
    );

    DataviewValue::Object(object)
}

fn list_fully_completed(index: usize, drafts: &[ListDraft]) -> bool {
    let draft = &drafts[index];
    matches!(draft.task_status, Some('x' | 'X'))
        && draft
            .children
            .iter()
            .all(|child| list_fully_completed(*child, drafts))
}

fn task_link(draft: &ListDraft, page_path: &str) -> DataviewLink {
    let path = draft.block_id.as_ref().map_or_else(
        || page_path.to_string(),
        |block_id| format!("{page_path}#^{block_id}"),
    );
    DataviewLink::new(path.clone(), None, false, path)
}

fn collect_inlinks(
    drafts: &[PageDraft],
    lookup: &LinkLookup,
) -> HashMap<String, Vec<DataviewLink>> {
    let mut inlinks: HashMap<String, Vec<DataviewLink>> = HashMap::new();
    let mut seen = HashSet::new();
    for draft in drafts {
        for link in &draft.outlinks {
            let target = strip_link_subpath(&link.path);
            if !lookup.by_path.contains_key(target) {
                continue;
            }
            if seen.insert((target.to_string(), draft.path.clone())) {
                inlinks
                    .entry(target.to_string())
                    .or_default()
                    .push(DataviewLink::page(&draft.path));
            }
        }
    }
    inlinks
}

fn dedup_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            deduped.push(value);
        }
    }
    deduped
}

fn dedup_links(values: Vec<DataviewLink>) -> Vec<DataviewLink> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for value in values {
        if seen.insert((value.path.clone(), value.display.clone(), value.embed))
        {
            deduped.push(value);
        }
    }
    deduped
}

fn strip_link_subpath(path: &str) -> &str {
    path.split_once('#').map_or(path, |(path, _)| path)
}

fn extract_aliases(value: Option<&DataviewValue>) -> Vec<String> {
    match value {
        Some(DataviewValue::String(value)) => vec![value.clone()],
        Some(DataviewValue::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn explicit_file_tags(value: Option<&DataviewValue>) -> Vec<String> {
    match value {
        Some(DataviewValue::String(value)) => vec![normalize_tag(value)],
        Some(DataviewValue::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(normalize_tag))
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_tag(tag: &str) -> String {
    let trimmed = tag.trim().trim_start_matches('#');
    format!("#{trimmed}")
}

fn expand_file_tags(explicit: &[String]) -> Vec<String> {
    let mut tags = Vec::new();
    let mut seen = HashSet::new();
    for tag in explicit {
        let tag = normalize_tag(tag);
        let bare = tag.trim_start_matches('#');
        let mut current = String::new();
        for segment in bare.split('/') {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            let expanded = format!("#{current}");
            if seen.insert(expanded.clone()) {
                tags.push(expanded);
            }
        }
    }
    tags
}

fn page_aliases(page: &DataviewPage) -> Vec<String> {
    page.fields
        .get("file")
        .and_then(|value| match value {
            DataviewValue::Object(file) => file.get("aliases"),
            _ => None,
        })
        .and_then(|value| match value {
            DataviewValue::Array(values) => Some(values),
            _ => None,
        })
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

fn page_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn page_folder(path: &str) -> String {
    Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn page_day(path: &str) -> DataviewValue {
    let name = page_name(path);
    if is_date(&name) {
        DataviewValue::Date(name)
    } else {
        DataviewValue::Null
    }
}

fn system_datetime(value: Option<SystemTime>) -> DataviewValue {
    value.map_or(DataviewValue::Null, |value| {
        let datetime = DateTime::<Utc>::from(value);
        DataviewValue::DateTime(
            datetime.to_rfc3339_opts(SecondsFormat::Secs, true),
        )
    })
}

fn system_date(value: Option<SystemTime>) -> DataviewValue {
    value.map_or(DataviewValue::Null, |value| {
        let datetime = DateTime::<Utc>::from(value);
        DataviewValue::Date(datetime.date_naive().to_string())
    })
}

fn read_starred_paths(bob_dir: &Path) -> HashSet<String> {
    let mut starred = HashSet::new();
    read_starred_json(&bob_dir.join(".obsidian/starred.json"), &mut starred);
    read_bookmarks_json(
        &bob_dir.join(".obsidian/bookmarks.json"),
        &mut starred,
    );
    starred
}

fn read_starred_json(path: &Path, starred: &mut HashSet<String>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return;
    };
    for item in value
        .get("items")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(path) = item.get("path").and_then(serde_json::Value::as_str)
            && let Ok(path) = normalize_note_path(path)
        {
            starred.insert(path);
        }
    }
}

fn read_bookmarks_json(path: &Path, starred: &mut HashSet<String>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return;
    };
    collect_bookmarked_files(&value, starred);
}

fn collect_bookmarked_files(
    value: &serde_json::Value,
    starred: &mut HashSet<String>,
) {
    if let Some(path) = value.get("path").and_then(serde_json::Value::as_str)
        && value.get("type").and_then(serde_json::Value::as_str) == Some("file")
        && let Ok(path) = normalize_note_path(path)
    {
        starred.insert(path);
    }
    for child in value
        .get("items")
        .or_else(|| value.get("children"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_bookmarked_files(child, starred);
    }
}
