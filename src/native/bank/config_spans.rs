//! Style-preserving, entry-granular editor for the hub's `bank:` mapping.
//!
//! Untouched entries stay as their original byte slices. New and replaced
//! entries copy neighbour style: flow maps stay flow maps, scalar
//! categories stay scalars while they hold only a label, and digit
//! hotkeys are always quoted. Restore spans name exact before/after bytes.

use serde_yaml::Value as YamlValue;

use crate::native::yaml_spans::{
    blank_or_comment, first_content_indent, flow_is_closed, has_tab_indent,
    has_yaml_feature, indentation, join_lines, parse_key, render_scalar,
    split_comment, split_lines, validate_flow_map_keys,
    whitespace_before_comment, yaml_text, FlowMapError, Line,
};

const BANK_ORDER: &[&str] = &["accounts", "categories", "rules", "stale_after"];
const CATEGORY_KEYS: &[&str] = &["label", "hotkey", "group", "hidden"];
const ACCOUNT_KEYS: &[&str] = &["label", "kind", "start"];
const RULE_KEYS: &[&str] = &["match", "payee", "cat", "link"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpanError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) line: Option<usize>,
    pub(crate) key_path: Option<String>,
}

impl std::fmt::Display for SpanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "[{}]: {}", self.code, self.message)
    }
}

impl std::error::Error for SpanError {}

/// Exact before/after bytes of one touched span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreSpan {
    pub(crate) key_path: String,
    pub(crate) before: Option<String>,
    pub(crate) after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BankSpanEdit {
    pub(crate) contents: String,
    pub(crate) spans: Vec<RestoreSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place<'a> {
    Before(&'a str),
    After(&'a str),
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeqPlace {
    Before(usize),
    After(usize),
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BankContainer {
    Accounts,
    Categories,
    Rules,
    StaleAfter,
}

impl BankContainer {
    fn key(self) -> &'static str {
        match self {
            Self::Accounts => "accounts",
            Self::Categories => "categories",
            Self::Rules => "rules",
            Self::StaleAfter => "stale_after",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CategoryDraft {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) hotkey: Option<char>,
    pub(crate) group: Option<String>,
    pub(crate) hidden: Option<bool>,
}

impl CategoryDraft {
    fn has_mapping_fields(&self) -> bool {
        self.hotkey.is_some() || self.group.is_some() || self.hidden.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountDraft {
    pub(crate) alias: String,
    pub(crate) label: Option<String>,
    pub(crate) kind: String,
    pub(crate) start: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleDraft {
    pub(crate) match_text: String,
    pub(crate) payee: Option<String>,
    pub(crate) cat: Option<String>,
    pub(crate) link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FlowSpacing {
    after_open: String,
    after_colon: String,
    after_comma: String,
    before_close: String,
}

impl FlowSpacing {
    fn padded() -> Self {
        Self {
            after_open: " ".to_owned(),
            after_colon: " ".to_owned(),
            after_comma: " ".to_owned(),
            before_close: " ".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Style {
    Scalar,
    Flow(FlowSpacing),
    Block,
    EmptyFlow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    key: String,
    start: usize,
    key_line: usize,
    end: usize,
    indent: usize,
    colon: usize,
    style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeqItem {
    start: usize,
    dash_line: usize,
    end: usize,
    indent: usize,
    style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MapContainer {
    item: Item,
    entries: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeqContainer {
    item: Item,
    entries: Vec<SeqItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Child {
    Accounts(MapContainer),
    Categories(MapContainer),
    Rules(SeqContainer),
    StaleAfter(Item),
    Other(Item),
}

impl Child {
    fn item(&self) -> &Item {
        match self {
            Self::Accounts(container) | Self::Categories(container) => {
                &container.item
            }
            Self::Rules(container) => &container.item,
            Self::StaleAfter(item) | Self::Other(item) => item,
        }
    }

    fn key(&self) -> &str {
        &self.item().key
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BankDocument {
    lines: Vec<Line>,
    newline: String,
    bank: Item,
    children: Vec<Child>,
}

pub(crate) fn replace_category(
    markdown: &str,
    draft: &CategoryDraft,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = categories(&doc)?;
    let index = map_index(&container.entries, &draft.key, "bank.categories")?;
    let existing = &container.entries[index];
    let existing_keys = entry_key_order(&doc.lines, existing);
    let style = style_for_replace(
        &existing.style,
        draft.has_mapping_fields(),
        &container.entries,
        index,
        Style::Flow(FlowSpacing::padded()),
    );
    let comment =
        trailing_comment(&doc.lines, existing.key_line, existing.colon);
    let replacement = emit_category(
        draft,
        &existing_keys,
        &style,
        existing.indent,
        &doc.newline,
        &comment,
    );
    splice_item(
        markdown,
        &doc,
        existing,
        &replacement,
        &format!("bank.categories.{}", draft.key),
    )
}

pub(crate) fn insert_category(
    markdown: &str,
    draft: &CategoryDraft,
    place: Place<'_>,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = categories(&doc)?;
    if container.entries.iter().any(|item| item.key == draft.key) {
        return Err(error(
            "duplicate_entry",
            format!("category {} already exists", draft.key),
            Some(container.item.key_line + 1),
            Some(format!("bank.categories.{}", draft.key)),
        ));
    }
    let at = map_place(&container.entries, place, "bank.categories")?;
    let style = style_for_insert(
        &container.entries,
        at,
        draft.has_mapping_fields(),
        Style::Flow(FlowSpacing::padded()),
    );
    let indent = sibling_indent(container);
    let replacement =
        emit_category(draft, &[], &style, indent, &doc.newline, "");
    insert_map_entry(
        markdown,
        &doc,
        container,
        at,
        replacement,
        &format!("bank.categories.{}", draft.key),
    )
}

pub(crate) fn remove_category(
    markdown: &str,
    key: &str,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = categories(&doc)?;
    let index = map_index(&container.entries, key, "bank.categories")?;
    remove_map_entry(
        markdown,
        &doc,
        &container.entries[index],
        &format!("bank.categories.{key}"),
    )
}

pub(crate) fn move_category(
    markdown: &str,
    key: &str,
    place: Place<'_>,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = categories(&doc)?;
    let from = map_index(&container.entries, key, "bank.categories")?;
    let mut to = map_place(&container.entries, place, "bank.categories")?;
    if to > from {
        to -= 1;
    }
    if to == from {
        return Ok(noop(markdown));
    }
    move_map_entry(
        markdown,
        &doc,
        container,
        from,
        to,
        &format!("bank.categories.{key}"),
    )
}

pub(crate) fn replace_account(
    markdown: &str,
    draft: &AccountDraft,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = accounts(&doc)?;
    let index = map_index(&container.entries, &draft.alias, "bank.accounts")?;
    let existing = &container.entries[index];
    let existing_keys = entry_key_order(&doc.lines, existing);
    let style = keep_or_block(&existing.style);
    let comment =
        trailing_comment(&doc.lines, existing.key_line, existing.colon);
    let replacement = emit_account(
        draft,
        &existing_keys,
        &style,
        existing.indent,
        &doc.newline,
        &comment,
    );
    splice_item(
        markdown,
        &doc,
        existing,
        &replacement,
        &format!("bank.accounts.{}", draft.alias),
    )
}

pub(crate) fn insert_account(
    markdown: &str,
    draft: &AccountDraft,
    place: Place<'_>,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = accounts(&doc)?;
    if container.entries.iter().any(|item| item.key == draft.alias) {
        return Err(error(
            "duplicate_entry",
            format!("account {} already exists", draft.alias),
            Some(container.item.key_line + 1),
            Some(format!("bank.accounts.{}", draft.alias)),
        ));
    }
    let at = map_place(&container.entries, place, "bank.accounts")?;
    let style = style_for_insert(&container.entries, at, true, Style::Block);
    let indent = sibling_indent(container);
    let replacement =
        emit_account(draft, &[], &style, indent, &doc.newline, "");
    insert_map_entry(
        markdown,
        &doc,
        container,
        at,
        replacement,
        &format!("bank.accounts.{}", draft.alias),
    )
}

pub(crate) fn remove_account(
    markdown: &str,
    alias: &str,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = accounts(&doc)?;
    let index = map_index(&container.entries, alias, "bank.accounts")?;
    remove_map_entry(
        markdown,
        &doc,
        &container.entries[index],
        &format!("bank.accounts.{alias}"),
    )
}

pub(crate) fn move_account(
    markdown: &str,
    alias: &str,
    place: Place<'_>,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = accounts(&doc)?;
    let from = map_index(&container.entries, alias, "bank.accounts")?;
    let mut to = map_place(&container.entries, place, "bank.accounts")?;
    if to > from {
        to -= 1;
    }
    if to == from {
        return Ok(noop(markdown));
    }
    move_map_entry(
        markdown,
        &doc,
        container,
        from,
        to,
        &format!("bank.accounts.{alias}"),
    )
}

pub(crate) fn replace_rule(
    markdown: &str,
    index: usize,
    draft: &RuleDraft,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = rules(&doc)?;
    let existing = seq_get(&container.entries, index)?;
    let existing_keys = seq_key_order(&doc.lines, existing);
    let style = keep_or_block(&existing.style);
    let comment = seq_trailing_comment(&doc.lines, existing);
    let replacement = emit_rule(
        draft,
        &existing_keys,
        &style,
        existing.indent,
        &doc.newline,
        &comment,
    );
    splice_seq(
        markdown,
        &doc,
        existing,
        replacement,
        &format!("bank.rules.{index}"),
    )
}

pub(crate) fn insert_rule(
    markdown: &str,
    draft: &RuleDraft,
    place: SeqPlace,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = rules(&doc)?;
    let at = seq_place(&container.entries, place)?;
    let style = seq_style_for_insert(&container.entries, at);
    let indent = seq_sibling_indent(container);
    let replacement = emit_rule(draft, &[], &style, indent, &doc.newline, "");
    insert_seq_entry(
        markdown,
        &doc,
        container,
        at,
        replacement,
        &format!("bank.rules.{at}"),
    )
}

pub(crate) fn remove_rule(
    markdown: &str,
    index: usize,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = rules(&doc)?;
    let existing = seq_get(&container.entries, index)?;
    if container.entries.len() == 1 {
        let comment = trailing_comment(
            &doc.lines,
            container.item.key_line,
            container.item.colon,
        );
        let replacement = vec![Line {
            text: format!(
                "{}rules: []{comment}",
                " ".repeat(container.item.indent)
            ),
            ending: doc.newline.clone(),
        }];
        return splice_item(
            markdown,
            &doc,
            &container.item,
            &replacement,
            "bank.rules",
        );
    }
    remove_seq_entry(markdown, &doc, existing, &format!("bank.rules.{index}"))
}

pub(crate) fn move_rule(
    markdown: &str,
    index: usize,
    place: SeqPlace,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = rules(&doc)?;
    let _ = seq_get(&container.entries, index)?;
    let mut to = seq_place(&container.entries, place)?;
    if to > index {
        to -= 1;
    }
    if to == index {
        return Ok(noop(markdown));
    }
    move_seq_entry(
        markdown,
        &doc,
        container,
        index,
        to,
        &format!("bank.rules.{to}"),
    )
}

pub(crate) fn set_account_label(
    markdown: &str,
    alias: &str,
    label: Option<&str>,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    let container = accounts(&doc)?;
    let index = map_index(&container.entries, alias, "bank.accounts")?;
    let account = &container.entries[index];
    match &account.style {
        Style::Flow(_) | Style::EmptyFlow => {
            let mut draft = account_draft_from(&doc.lines, account)?;
            draft.label = label.map(str::to_owned);
            replace_account(markdown, &draft)
        }
        Style::Block | Style::Scalar => set_block_child(
            markdown,
            &doc,
            account,
            "label",
            label.map(|label| YamlValue::String(label.to_owned())),
            ACCOUNT_KEYS,
            &format!("bank.accounts.{alias}.label"),
        ),
    }
}

pub(crate) fn set_stale_after(
    markdown: &str,
    days: Option<u32>,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    match (stale_child(&doc), days) {
        (None, None) => Ok(noop(markdown)),
        (Some(item), None) => {
            remove_child_item(markdown, &doc, item, "bank.stale_after")
        }
        (Some(item), Some(days)) => {
            let comment =
                trailing_comment(&doc.lines, item.key_line, item.colon);
            let replacement = emit_scalar_key(
                "stale_after",
                &YamlValue::Number(days.into()),
                item.indent,
                &doc.newline,
                &comment,
            );
            splice_item(markdown, &doc, item, &replacement, "bank.stale_after")
        }
        (None, Some(days)) => {
            let indent = bank_child_indent(&doc);
            let replacement = emit_scalar_key(
                "stale_after",
                &YamlValue::Number(days.into()),
                indent,
                &doc.newline,
                "",
            );
            insert_bank_child(
                markdown,
                &doc,
                BankContainer::StaleAfter,
                replacement,
                "bank.stale_after",
            )
        }
    }
}

pub(crate) fn ensure_container(
    markdown: &str,
    container: BankContainer,
) -> Result<BankSpanEdit, SpanError> {
    let doc = parse_bank_document(markdown)?;
    if doc
        .children
        .iter()
        .any(|child| child.key() == container.key())
    {
        return Ok(noop(markdown));
    }
    let indent = bank_child_indent(&doc);
    let empty = match container {
        BankContainer::Rules => "[]",
        BankContainer::StaleAfter => "10",
        _ => "{}",
    };
    let replacement = vec![Line {
        text: format!("{}{}: {empty}", " ".repeat(indent), container.key()),
        ending: doc.newline.clone(),
    }];
    insert_bank_child(
        markdown,
        &doc,
        container,
        replacement,
        &format!("bank.{}", container.key()),
    )
}

pub(crate) fn restore(
    markdown: &str,
    spans: &[RestoreSpan],
) -> Result<String, SpanError> {
    let mut text = markdown.to_owned();
    for span in spans.iter().rev() {
        text = restore_one(&text, span)?;
    }
    Ok(text)
}

pub(crate) fn parse_bank_document(
    markdown: &str,
) -> Result<BankDocument, SpanError> {
    let lines = split_lines(markdown);
    let opening = lines
        .first()
        .is_some_and(|line| line.text.trim_start_matches('\u{feff}') == "---");
    if !opening {
        return Err(unsupported(
            "note has no opening frontmatter delimiter",
            Some(1),
            None,
        ));
    }
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.text == "---").then_some(index))
        .ok_or_else(|| {
            unsupported("note has no closing frontmatter delimiter", None, None)
        })?;
    let newline = lines
        .iter()
        .find(|line| !line.ending.is_empty())
        .map(|line| line.ending.clone())
        .unwrap_or_else(|| "\n".to_owned());
    let yaml = yaml_text(&lines[1..close]);
    let root = collect_map_items(&lines, 1, close, 0, "")?;
    let bank = root
        .into_iter()
        .find(|item| item.key == "bank")
        .ok_or_else(|| {
            unsupported(
                "bank mapping has no unambiguous source entry",
                None,
                Some("bank".to_owned()),
            )
        })?;
    refuse_features(&lines, &bank, "bank")?;
    refuse_flow_container(&bank, "bank")?;
    let child_start = bank.key_line + 1;
    let child_indent = first_content_indent(&lines, child_start, bank.end + 1)
        .unwrap_or(bank.indent + 2);
    let child_items = if matches!(bank.style, Style::EmptyFlow | Style::Scalar)
    {
        Vec::new()
    } else {
        collect_map_items(
            &lines,
            child_start,
            bank.end + 1,
            child_indent,
            "bank",
        )?
    };
    let mut children = Vec::new();
    for item in child_items {
        refuse_features(&lines, &item, &format!("bank.{}", item.key))?;
        if has_tab_indent(&lines[item.key_line].text)
            || (item.start..=item.end)
                .any(|index| has_tab_indent(&lines[index].text))
        {
            return Err(unsupported(
                "tab indentation cannot be edited safely",
                Some(item.key_line + 1),
                Some(format!("bank.{}", item.key)),
            ));
        }
        let child = match item.key.as_str() {
            "accounts" => {
                refuse_flow_container(&item, "bank.accounts")?;
                Child::Accounts(collect_map_container(
                    &lines,
                    item,
                    "bank.accounts",
                )?)
            }
            "categories" => {
                refuse_flow_container(&item, "bank.categories")?;
                Child::Categories(collect_map_container(
                    &lines,
                    item,
                    "bank.categories",
                )?)
            }
            "rules" => {
                refuse_flow_container(&item, "bank.rules")?;
                Child::Rules(collect_seq_container(&lines, item, "bank.rules")?)
            }
            "stale_after" => Child::StaleAfter(item),
            _ => Child::Other(item),
        };
        children.push(child);
    }
    serde_yaml::from_str::<YamlValue>(&yaml).map_err(|yaml_error| {
        let line = yaml_error.location().map(|location| location.line() + 1);
        unsupported(
            format!("frontmatter is not valid YAML: {yaml_error}"),
            line,
            Some("bank".to_owned()),
        )
    })?;
    Ok(BankDocument {
        lines,
        newline,
        bank,
        children,
    })
}

fn collect_map_container(
    lines: &[Line],
    item: Item,
    path: &str,
) -> Result<MapContainer, SpanError> {
    let entries = match item.style {
        Style::EmptyFlow | Style::Scalar => Vec::new(),
        Style::Flow(_) => {
            return Err(unsupported(
                format!("{path} is a flow-style container and cannot be edited safely"),
                Some(item.key_line + 1),
                Some(path.to_owned()),
            ));
        }
        Style::Block => {
            let start = item.key_line + 1;
            let indent = first_content_indent(lines, start, item.end + 1)
                .unwrap_or(item.indent + 2);
            collect_map_items(lines, start, item.end + 1, indent, path)?
        }
    };
    for entry in &entries {
        refuse_features(lines, entry, &format!("{path}.{}", entry.key))?;
        refuse_multiline_flow(lines, entry, &format!("{path}.{}", entry.key))?;
        if let Style::Flow(ref _spacing) = entry.style {
            let inline = inline_value(lines, entry);
            validate_flow_at(
                &inline,
                &format!("{path}.{}", entry.key),
                entry.key_line,
            )?;
        }
    }
    Ok(MapContainer { item, entries })
}

fn collect_seq_container(
    lines: &[Line],
    item: Item,
    path: &str,
) -> Result<SeqContainer, SpanError> {
    let entries = match item.style {
        Style::EmptyFlow | Style::Scalar => Vec::new(),
        Style::Flow(_) => {
            return Err(unsupported(
                format!("{path} is a flow-style container and cannot be edited safely"),
                Some(item.key_line + 1),
                Some(path.to_owned()),
            ));
        }
        Style::Block => {
            let start = item.key_line + 1;
            let indent = first_content_indent(lines, start, item.end + 1)
                .unwrap_or(item.indent + 2);
            collect_seq_items(lines, start, item.end + 1, indent, path)?
        }
    };
    for (index, entry) in entries.iter().enumerate() {
        let key_path = format!("{path}.{index}");
        if has_yaml_feature(&lines[entry.start..=entry.end]) {
            return Err(unsupported(
                "anchors, aliases, and tags inside bank: are not safe to edit",
                Some(entry.dash_line + 1),
                Some(key_path),
            ));
        }
        refuse_seq_multiline_flow(lines, entry, &key_path)?;
        if let Style::Flow(_) = entry.style {
            let inline = seq_inline(lines, entry);
            validate_flow_at(&inline, &key_path, entry.dash_line)?;
        }
    }
    Ok(SeqContainer { item, entries })
}

fn collect_map_items(
    lines: &[Line],
    start: usize,
    end: usize,
    indent: usize,
    parent: &str,
) -> Result<Vec<Item>, SpanError> {
    let mut found = Vec::<(usize, String, usize)>::new();
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(end.min(lines.len()))
        .skip(start)
    {
        if blank_or_comment(&line.text) {
            continue;
        }
        if has_tab_indent(&line.text) {
            return Err(unsupported(
                "tab indentation cannot be edited safely",
                Some(index + 1),
                none_path(parent),
            ));
        }
        let actual = indentation(&line.text);
        if actual != indent {
            continue;
        }
        let Some((key, colon)) = parse_key(&line.text, indent) else {
            return Err(unsupported(
                "a block mapping line has no unambiguous key span",
                Some(index + 1),
                none_path(parent),
            ));
        };
        if found.iter().any(|(_, prior, _)| prior == &key) {
            return Err(unsupported(
                format!("duplicate YAML key {key:?} cannot be edited safely"),
                Some(index + 1),
                Some(join_path(parent, &key)),
            ));
        }
        found.push((index, key, colon));
    }
    let comment_starts = comment_starts(lines, start, &found);
    let mut items = Vec::with_capacity(found.len());
    for (at, (key_line, key, colon)) in found.iter().enumerate() {
        let next_start = found
            .get(at + 1)
            .map(|_| comment_starts[at + 1])
            .unwrap_or(end);
        let item_start = comment_starts[at];
        let value_end = map_value_end(lines, *key_line, indent, next_start);
        let mut item = Item {
            key: key.clone(),
            start: item_start,
            key_line: *key_line,
            end: value_end,
            indent,
            colon: *colon,
            style: Style::Scalar,
        };
        item.style = classify_style(lines, &item, parent)?;
        items.push(item);
    }
    Ok(items)
}

fn collect_seq_items(
    lines: &[Line],
    start: usize,
    end: usize,
    indent: usize,
    parent: &str,
) -> Result<Vec<SeqItem>, SpanError> {
    let mut dashes = Vec::new();
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(end.min(lines.len()))
        .skip(start)
    {
        if blank_or_comment(&line.text) {
            continue;
        }
        if has_tab_indent(&line.text) {
            return Err(unsupported(
                "tab indentation cannot be edited safely",
                Some(index + 1),
                none_path(parent),
            ));
        }
        if indentation(&line.text) != indent {
            continue;
        }
        if !is_dash_line(&line.text, indent) {
            return Err(unsupported(
                "a sequence line has no unambiguous item span",
                Some(index + 1),
                none_path(parent),
            ));
        }
        dashes.push(index);
    }
    let comment_starts = seq_comment_starts(lines, start, &dashes);
    let mut items = Vec::with_capacity(dashes.len());
    for (at, dash_line) in dashes.iter().enumerate() {
        let next_start = dashes
            .get(at + 1)
            .map(|_| comment_starts[at + 1])
            .unwrap_or(end);
        let value_end = seq_value_end(lines, *dash_line, indent, next_start);
        let mut item = SeqItem {
            start: comment_starts[at],
            dash_line: *dash_line,
            end: value_end,
            indent,
            style: Style::Scalar,
        };
        item.style = classify_seq_style(lines, &item, parent)?;
        items.push(item);
    }
    Ok(items)
}

fn comment_starts(
    lines: &[Line],
    range_start: usize,
    found: &[(usize, String, usize)],
) -> Vec<usize> {
    found
        .iter()
        .map(|(key_line, _, _)| {
            attach_comments_above(lines, range_start, *key_line)
        })
        .collect()
}

fn seq_comment_starts(
    lines: &[Line],
    range_start: usize,
    dashes: &[usize],
) -> Vec<usize> {
    dashes
        .iter()
        .map(|dash| attach_comments_above(lines, range_start, *dash))
        .collect()
}

fn attach_comments_above(
    lines: &[Line],
    range_start: usize,
    key_line: usize,
) -> usize {
    let mut start = key_line;
    while start > range_start {
        let prev = start - 1;
        let text = &lines[prev].text;
        if text.trim().is_empty() {
            break;
        }
        if text.trim().starts_with('#') {
            start = prev;
            continue;
        }
        break;
    }
    start
}

fn map_value_end(
    lines: &[Line],
    key_line: usize,
    indent: usize,
    stop_before: usize,
) -> usize {
    let mut end = key_line;
    for index in (key_line + 1)..stop_before.min(lines.len()) {
        let text = &lines[index].text;
        if blank_or_comment(text) {
            continue;
        }
        if indentation(text) <= indent {
            break;
        }
        end = index;
    }
    end
}

fn seq_value_end(
    lines: &[Line],
    dash_line: usize,
    indent: usize,
    stop_before: usize,
) -> usize {
    map_value_end(lines, dash_line, indent, stop_before)
}

fn classify_style(
    lines: &[Line],
    item: &Item,
    parent: &str,
) -> Result<Style, SpanError> {
    let inline = inline_value(lines, item);
    classify_inline(&inline, item.key_line, item.end, parent, &item.key)
}

fn classify_seq_style(
    lines: &[Line],
    item: &SeqItem,
    parent: &str,
) -> Result<Style, SpanError> {
    let inline = seq_inline(lines, item);
    classify_inline(&inline, item.dash_line, item.end, parent, "")
}

fn classify_inline(
    inline: &str,
    key_line: usize,
    end: usize,
    parent: &str,
    key: &str,
) -> Result<Style, SpanError> {
    let path = join_path(parent, key);
    if inline.is_empty() {
        return Ok(if key_line == end {
            Style::Scalar
        } else {
            Style::Block
        });
    }
    if inline == "{}" || inline == "[]" {
        if key_line != end {
            return Err(unsupported(
                "multi-line flow values cannot be edited safely",
                Some(key_line + 1),
                Some(path),
            ));
        }
        return Ok(Style::EmptyFlow);
    }
    if inline.starts_with('{') || inline.starts_with('[') {
        if key_line != end || !flow_is_closed(inline) {
            return Err(unsupported(
                "multi-line flow values cannot be edited safely",
                Some(key_line + 1),
                Some(path),
            ));
        }
        validate_flow_at(inline, &path, key_line)?;
        return Ok(Style::Flow(detect_flow_spacing(inline)));
    }
    if key_line != end {
        // Compact block mapping: `- match: VENMO` plus nested keys.
        return Ok(Style::Block);
    }
    Ok(Style::Scalar)
}

fn inline_value(lines: &[Line], item: &Item) -> String {
    let rest = &lines[item.key_line].text[item.colon + 1..];
    let (value, _) = split_comment(rest);
    value.trim().to_owned()
}

fn seq_inline(lines: &[Line], item: &SeqItem) -> String {
    let line = &lines[item.dash_line].text;
    let rest = dash_rest(line, item.indent);
    let (value, _) = split_comment(rest);
    value.trim().to_owned()
}

fn is_dash_line(line: &str, indent: usize) -> bool {
    let rest = &line[indent..];
    rest.starts_with("- ") || rest == "-" || rest.starts_with("-{")
}

fn dash_rest(line: &str, indent: usize) -> &str {
    let rest = &line[indent..];
    if let Some(stripped) = rest.strip_prefix("- ") {
        stripped
    } else if let Some(stripped) = rest.strip_prefix('-') {
        stripped
    } else {
        rest
    }
}

fn detect_flow_spacing(flow: &str) -> FlowSpacing {
    let trimmed = flow.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') || trimmed.len() < 2
    {
        return FlowSpacing::padded();
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let after_open = leading_ws(inner).to_owned();
    let before_close = trailing_ws(inner).to_owned();
    let mut after_colon = if after_open.is_empty() {
        String::new()
    } else {
        " ".to_owned()
    };
    let mut after_comma = after_colon.clone();
    let bytes = inner.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut saw_colon = false;
    let mut saw_comma = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            continue;
        }
        if byte == b':' && !saw_colon {
            saw_colon = true;
            after_colon = ws_after(inner, index + 1).to_owned();
        }
        if byte == b',' && !saw_comma {
            saw_comma = true;
            after_comma = ws_after(inner, index + 1).to_owned();
        }
    }
    FlowSpacing {
        after_open,
        after_colon,
        after_comma,
        before_close,
    }
}

fn leading_ws(text: &str) -> &str {
    &text[..text.len() - text.trim_start().len()]
}

fn trailing_ws(text: &str) -> &str {
    &text[text.trim_end().len()..]
}

fn ws_after(text: &str, start: usize) -> &str {
    if start >= text.len() {
        return "";
    }
    let rest = &text[start..];
    &rest[..rest.len() - rest.trim_start().len()]
}

fn refuse_features(
    lines: &[Line],
    item: &Item,
    path: &str,
) -> Result<(), SpanError> {
    if has_yaml_feature(&lines[item.start..=item.end]) {
        return Err(unsupported(
            "anchors, aliases, and tags inside bank: are not safe to edit",
            Some(item.key_line + 1),
            Some(path.to_owned()),
        ));
    }
    Ok(())
}

fn refuse_flow_container(item: &Item, path: &str) -> Result<(), SpanError> {
    if matches!(item.style, Style::Flow(_)) {
        return Err(unsupported(
            format!(
                "{path} is a flow-style container and cannot be edited safely"
            ),
            Some(item.key_line + 1),
            Some(path.to_owned()),
        ));
    }
    Ok(())
}

fn refuse_multiline_flow(
    lines: &[Line],
    item: &Item,
    path: &str,
) -> Result<(), SpanError> {
    let inline = inline_value(lines, item);
    if (inline.starts_with('{') || inline.starts_with('['))
        && (item.key_line != item.end || !flow_is_closed(&inline))
    {
        return Err(unsupported(
            "multi-line flow values cannot be edited safely",
            Some(item.key_line + 1),
            Some(path.to_owned()),
        ));
    }
    Ok(())
}

fn refuse_seq_multiline_flow(
    lines: &[Line],
    item: &SeqItem,
    path: &str,
) -> Result<(), SpanError> {
    let inline = seq_inline(lines, item);
    if (inline.starts_with('{') || inline.starts_with('['))
        && (item.dash_line != item.end || !flow_is_closed(&inline))
    {
        return Err(unsupported(
            "multi-line flow values cannot be edited safely",
            Some(item.dash_line + 1),
            Some(path.to_owned()),
        ));
    }
    Ok(())
}

fn validate_flow_at(
    flow: &str,
    path: &str,
    line: usize,
) -> Result<(), SpanError> {
    validate_flow_map_keys(flow).map_err(|err| {
        let message = match err {
            FlowMapError::UnresolvableKey => {
                "flow mapping has a key that cannot be resolved safely"
            }
            FlowMapError::DuplicateKey => {
                "duplicate key in flow YAML cannot be edited safely"
            }
            FlowMapError::CommentInside => {
                "comments inside a flow value cannot be preserved safely"
            }
        };
        unsupported(message, Some(line + 1), Some(path.to_owned()))
    })
}

fn categories(doc: &BankDocument) -> Result<&MapContainer, SpanError> {
    doc.children
        .iter()
        .find_map(|child| match child {
            Child::Categories(container) => Some(container),
            _ => None,
        })
        .ok_or_else(|| {
            error(
                "missing_container",
                "bank.categories is missing",
                Some(doc.bank.key_line + 1),
                Some("bank.categories".to_owned()),
            )
        })
}

fn accounts(doc: &BankDocument) -> Result<&MapContainer, SpanError> {
    doc.children
        .iter()
        .find_map(|child| match child {
            Child::Accounts(container) => Some(container),
            _ => None,
        })
        .ok_or_else(|| {
            error(
                "missing_container",
                "bank.accounts is missing",
                Some(doc.bank.key_line + 1),
                Some("bank.accounts".to_owned()),
            )
        })
}

fn rules(doc: &BankDocument) -> Result<&SeqContainer, SpanError> {
    doc.children
        .iter()
        .find_map(|child| match child {
            Child::Rules(container) => Some(container),
            _ => None,
        })
        .ok_or_else(|| {
            error(
                "missing_container",
                "bank.rules is missing",
                Some(doc.bank.key_line + 1),
                Some("bank.rules".to_owned()),
            )
        })
}

fn stale_child(doc: &BankDocument) -> Option<&Item> {
    doc.children.iter().find_map(|child| match child {
        Child::StaleAfter(item) => Some(item),
        _ => None,
    })
}

fn map_index(
    entries: &[Item],
    key: &str,
    parent: &str,
) -> Result<usize, SpanError> {
    entries
        .iter()
        .position(|item| item.key == key)
        .ok_or_else(|| {
            error(
                "missing_entry",
                format!("{parent}.{key} has no source span"),
                None,
                Some(format!("{parent}.{key}")),
            )
        })
}

fn map_place(
    entries: &[Item],
    place: Place<'_>,
    parent: &str,
) -> Result<usize, SpanError> {
    match place {
        Place::End => Ok(entries.len()),
        Place::Before(key) => map_index(entries, key, parent),
        Place::After(key) => Ok(map_index(entries, key, parent)? + 1),
    }
}

fn seq_get(entries: &[SeqItem], index: usize) -> Result<&SeqItem, SpanError> {
    entries.get(index).ok_or_else(|| {
        error(
            "missing_entry",
            format!("bank.rules.{index} has no source span"),
            None,
            Some(format!("bank.rules.{index}")),
        )
    })
}

fn seq_place(entries: &[SeqItem], place: SeqPlace) -> Result<usize, SpanError> {
    match place {
        SeqPlace::End => Ok(entries.len()),
        SeqPlace::Before(index) => {
            seq_get(entries, index)?;
            Ok(index)
        }
        SeqPlace::After(index) => {
            seq_get(entries, index)?;
            Ok(index + 1)
        }
    }
}

fn sibling_indent(container: &MapContainer) -> usize {
    container
        .entries
        .first()
        .map(|item| item.indent)
        .unwrap_or(container.item.indent + 2)
}

fn seq_sibling_indent(container: &SeqContainer) -> usize {
    container
        .entries
        .first()
        .map(|item| item.indent)
        .unwrap_or(container.item.indent + 2)
}

fn bank_child_indent(doc: &BankDocument) -> usize {
    doc.children
        .first()
        .map(|child| child.item().indent)
        .unwrap_or(doc.bank.indent + 2)
}

fn style_for_replace(
    existing: &Style,
    mapping_fields: bool,
    siblings: &[Item],
    index: usize,
    default_flow: Style,
) -> Style {
    match existing {
        Style::Scalar if !mapping_fields => Style::Scalar,
        Style::Scalar => promote_style(siblings, index, default_flow),
        Style::EmptyFlow => match &default_flow {
            Style::Flow(spacing) => Style::Flow(spacing.clone()),
            other => other.clone(),
        },
        other => other.clone(),
    }
}

fn style_for_insert(
    siblings: &[Item],
    at: usize,
    mapping_fields: bool,
    default: Style,
) -> Style {
    let nearest = nearest_style(siblings, at).unwrap_or(default);
    match nearest {
        Style::Scalar if mapping_fields => promote_style(
            siblings,
            at.saturating_sub(1),
            Style::Flow(FlowSpacing::padded()),
        ),
        Style::EmptyFlow => Style::Flow(FlowSpacing::padded()),
        other => other,
    }
}

fn seq_style_for_insert(siblings: &[SeqItem], at: usize) -> Style {
    nearest_seq_style(siblings, at).unwrap_or(Style::Block)
}

fn nearest_style(siblings: &[Item], at: usize) -> Option<Style> {
    if at > 0 {
        return siblings.get(at - 1).map(|item| item.style.clone());
    }
    siblings.first().map(|item| item.style.clone())
}

fn nearest_seq_style(siblings: &[SeqItem], at: usize) -> Option<Style> {
    if at > 0 {
        return siblings.get(at - 1).map(|item| item.style.clone());
    }
    siblings.first().map(|item| item.style.clone())
}

fn promote_style(
    siblings: &[Item],
    index: usize,
    default_flow: Style,
) -> Style {
    let _ = index;
    if let Some(spacing) = siblings.iter().find_map(|item| match &item.style {
        Style::Flow(spacing) => Some(spacing.clone()),
        _ => None,
    }) {
        return Style::Flow(spacing);
    }
    if siblings
        .iter()
        .any(|item| matches!(item.style, Style::Block))
    {
        return Style::Block;
    }
    default_flow
}

fn keep_or_block(existing: &Style) -> Style {
    match existing {
        Style::EmptyFlow => Style::Block,
        Style::Scalar => Style::Block,
        other => other.clone(),
    }
}

fn entry_key_order(lines: &[Line], item: &Item) -> Vec<String> {
    match &item.style {
        Style::Flow(_) | Style::EmptyFlow => {
            flow_key_order(&inline_value(lines, item))
        }
        Style::Block => nested_keys(lines, item),
        Style::Scalar => Vec::new(),
    }
}

fn seq_key_order(lines: &[Line], item: &SeqItem) -> Vec<String> {
    match &item.style {
        Style::Flow(_) | Style::EmptyFlow => {
            flow_key_order(&seq_inline(lines, item))
        }
        Style::Block => seq_nested_keys(lines, item),
        Style::Scalar => Vec::new(),
    }
}

fn flow_key_order(flow: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let bytes = flow.as_bytes();
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0isize;
    let mut expecting_key = false;
    let mut key_start = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(current) = quote {
            if current == b'"' && escaped {
                escaped = false;
            } else if current == b'"' && byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            index += 1;
            continue;
        }
        match byte {
            b'{' => {
                depth += 1;
                if depth == 1 {
                    expecting_key = true;
                    key_start = index + 1;
                }
            }
            b'}' | b']' => depth -= 1,
            b'[' => depth += 1,
            b',' if depth == 1 => {
                expecting_key = true;
                key_start = index + 1;
            }
            b':' if depth == 1 && expecting_key => {
                let raw = flow[key_start..index].trim();
                if let Ok(YamlValue::String(key)) = serde_yaml::from_str(raw) {
                    keys.push(key);
                } else if !raw.is_empty() {
                    keys.push(raw.trim_matches('"').to_owned());
                }
                expecting_key = false;
            }
            _ => {}
        }
        index += 1;
    }
    keys
}

fn nested_keys(lines: &[Line], item: &Item) -> Vec<String> {
    collect_map_items(
        lines,
        item.key_line + 1,
        item.end + 1,
        item.indent + 2,
        &item.key,
    )
    .map(|items| items.into_iter().map(|item| item.key).collect())
    .unwrap_or_default()
}

fn seq_nested_keys(lines: &[Line], item: &SeqItem) -> Vec<String> {
    let inline = seq_inline(lines, item);
    let mut keys = Vec::new();
    if !inline.is_empty()
        && !inline.starts_with('{')
        && let Some((key, _)) = parse_key(
            &format!("{}{inline}", " ".repeat(item.indent + 2)),
            item.indent + 2,
        )
    {
        keys.push(key);
    }
    if item.dash_line < item.end
        && let Ok(items) = collect_map_items(
            lines,
            item.dash_line + 1,
            item.end + 1,
            item.indent + 2,
            "",
        )
    {
        keys.extend(items.into_iter().map(|item| item.key));
    }
    keys
}

fn ordered_pairs(
    existing: &[String],
    canonical: &[&str],
    values: &[(&str, Option<String>)],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for key in existing {
        if let Some((_, value)) = values
            .iter()
            .find(|(name, value)| *name == key && value.is_some())
        {
            out.push((key.clone(), value.clone().expect("checked")));
            seen.push(key.as_str());
        }
    }
    for key in canonical {
        if seen.contains(key) {
            continue;
        }
        if let Some((_, value)) = values
            .iter()
            .find(|(name, value)| name == key && value.is_some())
        {
            out.push(((*key).to_owned(), value.clone().expect("checked")));
        }
    }
    out
}

fn emit_category(
    draft: &CategoryDraft,
    existing_keys: &[String],
    style: &Style,
    indent: usize,
    newline: &str,
    comment: &str,
) -> Vec<Line> {
    let key = crate::native::records::yaml_string_scalar(&draft.key);
    match style {
        Style::Scalar => emit_scalar_key(
            &draft.key,
            &YamlValue::String(draft.label.clone()),
            indent,
            newline,
            comment,
        ),
        Style::Flow(_) | Style::EmptyFlow => {
            let spacing = match style {
                Style::Flow(spacing) => spacing.clone(),
                _ => FlowSpacing::padded(),
            };
            let pairs = category_pairs(draft, existing_keys);
            let flow = emit_flow(&pairs, &spacing);
            vec![Line {
                text: format!("{}{key}: {flow}{comment}", " ".repeat(indent)),
                ending: newline.to_owned(),
            }]
        }
        Style::Block => {
            let pairs = category_pairs(draft, existing_keys);
            emit_block_map(&key, &pairs, indent, newline, comment)
        }
    }
}

fn category_pairs(
    draft: &CategoryDraft,
    existing_keys: &[String],
) -> Vec<(String, String)> {
    ordered_pairs(
        existing_keys,
        CATEGORY_KEYS,
        &[
            (
                "label",
                Some(render_scalar(&YamlValue::String(draft.label.clone()))),
            ),
            (
                "hotkey",
                draft.hotkey.map(|hotkey| {
                    render_scalar(&YamlValue::String(hotkey.to_string()))
                }),
            ),
            (
                "group",
                draft.group.as_ref().map(|group| {
                    render_scalar(&YamlValue::String(group.clone()))
                }),
            ),
            (
                "hidden",
                draft
                    .hidden
                    .map(|hidden| render_scalar(&YamlValue::Bool(hidden))),
            ),
        ],
    )
}

fn emit_account(
    draft: &AccountDraft,
    existing_keys: &[String],
    style: &Style,
    indent: usize,
    newline: &str,
    comment: &str,
) -> Vec<Line> {
    let key = crate::native::records::yaml_string_scalar(&draft.alias);
    let pairs = ordered_pairs(
        existing_keys,
        ACCOUNT_KEYS,
        &[
            (
                "label",
                draft.label.as_ref().map(|label| {
                    render_scalar(&YamlValue::String(label.clone()))
                }),
            ),
            (
                "kind",
                Some(render_scalar(&YamlValue::String(draft.kind.clone()))),
            ),
            (
                "start",
                draft.start.as_ref().map(|start| {
                    render_scalar(&YamlValue::String(start.clone()))
                }),
            ),
        ],
    );
    match style {
        Style::Flow(spacing) => {
            let flow = emit_flow(&pairs, spacing);
            vec![Line {
                text: format!("{}{key}: {flow}{comment}", " ".repeat(indent)),
                ending: newline.to_owned(),
            }]
        }
        _ => emit_block_map(&key, &pairs, indent, newline, comment),
    }
}

fn emit_rule(
    draft: &RuleDraft,
    existing_keys: &[String],
    style: &Style,
    indent: usize,
    newline: &str,
    comment: &str,
) -> Vec<Line> {
    let pairs = ordered_pairs(
        existing_keys,
        RULE_KEYS,
        &[
            (
                "match",
                Some(render_scalar(&YamlValue::String(
                    draft.match_text.clone(),
                ))),
            ),
            (
                "payee",
                draft.payee.as_ref().map(|payee| {
                    render_scalar(&YamlValue::String(payee.clone()))
                }),
            ),
            (
                "cat",
                draft
                    .cat
                    .as_ref()
                    .map(|cat| render_scalar(&YamlValue::String(cat.clone()))),
            ),
            (
                "link",
                draft.link.as_ref().map(|link| {
                    render_scalar(&YamlValue::String(link.clone()))
                }),
            ),
        ],
    );
    match style {
        Style::Flow(_) | Style::EmptyFlow => {
            let spacing = match style {
                Style::Flow(spacing) => spacing.clone(),
                _ => FlowSpacing::padded(),
            };
            let flow = emit_flow(&pairs, &spacing);
            vec![Line {
                text: format!("{}- {flow}{comment}", " ".repeat(indent)),
                ending: newline.to_owned(),
            }]
        }
        _ => emit_block_rule(&pairs, indent, newline, comment),
    }
}

fn emit_flow(pairs: &[(String, String)], spacing: &FlowSpacing) -> String {
    let mut out = String::from("{");
    out.push_str(&spacing.after_open);
    for (index, (key, value)) in pairs.iter().enumerate() {
        if index > 0 {
            out.push(',');
            out.push_str(&spacing.after_comma);
        }
        out.push_str(key);
        out.push(':');
        out.push_str(&spacing.after_colon);
        out.push_str(value);
    }
    out.push_str(&spacing.before_close);
    out.push('}');
    out
}

fn emit_block_map(
    key: &str,
    pairs: &[(String, String)],
    indent: usize,
    newline: &str,
    comment: &str,
) -> Vec<Line> {
    let spaces = " ".repeat(indent);
    let mut lines = vec![Line {
        text: format!("{spaces}{key}:{comment}"),
        ending: newline.to_owned(),
    }];
    let child = " ".repeat(indent + 2);
    for (key, value) in pairs {
        lines.push(Line {
            text: format!("{child}{key}: {value}"),
            ending: newline.to_owned(),
        });
    }
    lines
}

fn emit_block_rule(
    pairs: &[(String, String)],
    indent: usize,
    newline: &str,
    comment: &str,
) -> Vec<Line> {
    let spaces = " ".repeat(indent);
    let mut lines = Vec::new();
    if let Some((first_key, first_value)) = pairs.first() {
        lines.push(Line {
            text: format!("{spaces}- {first_key}: {first_value}{comment}"),
            ending: newline.to_owned(),
        });
        let child = " ".repeat(indent + 2);
        for (key, value) in pairs.iter().skip(1) {
            lines.push(Line {
                text: format!("{child}{key}: {value}"),
                ending: newline.to_owned(),
            });
        }
    }
    lines
}

fn emit_scalar_key(
    key: &str,
    value: &YamlValue,
    indent: usize,
    newline: &str,
    comment: &str,
) -> Vec<Line> {
    let key = crate::native::records::yaml_string_scalar(key);
    vec![Line {
        text: format!(
            "{}{key}: {}{comment}",
            " ".repeat(indent),
            render_scalar(value)
        ),
        ending: newline.to_owned(),
    }]
}

fn trailing_comment(lines: &[Line], key_line: usize, colon: usize) -> String {
    let rest = &lines[key_line].text[colon + 1..];
    let (_, comment) = split_comment(rest);
    if comment.is_empty() {
        String::new()
    } else {
        format!("{}{comment}", whitespace_before_comment(rest))
    }
}

fn seq_trailing_comment(lines: &[Line], item: &SeqItem) -> String {
    let rest = dash_rest(&lines[item.dash_line].text, item.indent);
    let (_, comment) = split_comment(rest);
    if comment.is_empty() {
        String::new()
    } else {
        format!("{}{comment}", whitespace_before_comment(rest))
    }
}

fn account_draft_from(
    lines: &[Line],
    item: &Item,
) -> Result<AccountDraft, SpanError> {
    let inline = inline_value(lines, item);
    let mapping: YamlValue = serde_yaml::from_str(&inline).map_err(|_| {
        unsupported(
            "account flow mapping cannot be parsed",
            Some(item.key_line + 1),
            Some(format!("bank.accounts.{}", item.key)),
        )
    })?;
    let map = mapping.as_mapping().ok_or_else(|| {
        unsupported(
            "account value must be a mapping",
            Some(item.key_line + 1),
            Some(format!("bank.accounts.{}", item.key)),
        )
    })?;
    Ok(AccountDraft {
        alias: item.key.clone(),
        label: map
            .get(YamlValue::String("label".to_owned()))
            .and_then(YamlValue::as_str)
            .map(str::to_owned),
        kind: map
            .get(YamlValue::String("kind".to_owned()))
            .and_then(YamlValue::as_str)
            .unwrap_or("checking")
            .to_owned(),
        start: map
            .get(YamlValue::String("start".to_owned()))
            .and_then(YamlValue::as_str)
            .map(str::to_owned),
    })
}

fn splice_item(
    markdown: &str,
    doc: &BankDocument,
    item: &Item,
    replacement: &[Line],
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let before = Some(bytes_of(&doc.lines, item.key_line, item.end));
    let mut lines = doc.lines.clone();
    replace_range(
        &mut lines,
        item.key_line,
        item.end,
        replacement,
        &doc.newline,
    );
    finish(markdown, lines, key_path, before)
}

fn splice_seq(
    markdown: &str,
    doc: &BankDocument,
    item: &SeqItem,
    replacement: Vec<Line>,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let before = Some(bytes_of(&doc.lines, item.dash_line, item.end));
    let mut lines = doc.lines.clone();
    replace_range(
        &mut lines,
        item.dash_line,
        item.end,
        &replacement,
        &doc.newline,
    );
    finish(markdown, lines, key_path, before)
}

fn insert_map_entry(
    markdown: &str,
    doc: &BankDocument,
    container: &MapContainer,
    at: usize,
    replacement: Vec<Line>,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    if container.entries.is_empty() {
        return insert_first_child(
            markdown,
            doc,
            &container.item,
            replacement,
            key_path,
        );
    }
    let at_line = if at >= container.entries.len() {
        container.entries.last().expect("checked").end + 1
    } else {
        container.entries[at].start
    };
    insert_lines(markdown, doc, at_line, replacement, key_path)
}

fn insert_seq_entry(
    markdown: &str,
    doc: &BankDocument,
    container: &SeqContainer,
    at: usize,
    replacement: Vec<Line>,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    if container.entries.is_empty() {
        return insert_first_child(
            markdown,
            doc,
            &container.item,
            replacement,
            key_path,
        );
    }
    let at_line = if at >= container.entries.len() {
        container.entries.last().expect("checked").end + 1
    } else {
        container.entries[at].start
    };
    insert_lines(markdown, doc, at_line, replacement, key_path)
}

fn insert_first_child(
    markdown: &str,
    doc: &BankDocument,
    parent: &Item,
    children: Vec<Line>,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let before = Some(bytes_of(&doc.lines, parent.start, parent.end));
    let mut key_line = doc.lines[parent.key_line].clone();
    let prefix = &key_line.text[..=parent.colon];
    let rest = &key_line.text[parent.colon + 1..];
    let (_, comment) = split_comment(rest);
    let trailing = whitespace_before_comment(rest);
    key_line.text = format!("{prefix}{trailing}{comment}");
    if key_line.ending.is_empty() {
        key_line.ending = doc.newline.clone();
    }
    let mut replacement = vec![key_line];
    replacement.extend(children);
    let mut lines = doc.lines.clone();
    replace_range(
        &mut lines,
        parent.key_line,
        parent.end,
        &replacement,
        &doc.newline,
    );
    finish(markdown, lines, key_path, before)
}

fn insert_lines(
    markdown: &str,
    doc: &BankDocument,
    at: usize,
    replacement: Vec<Line>,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let mut lines = doc.lines.clone();
    let at = at.min(lines.len());
    insert_range(&mut lines, at, replacement, &doc.newline);
    finish(markdown, lines, key_path, None)
}

fn remove_map_entry(
    markdown: &str,
    doc: &BankDocument,
    item: &Item,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let before = Some(bytes_of(&doc.lines, item.start, item.end));
    let mut lines = doc.lines.clone();
    replace_range(&mut lines, item.start, item.end, &[], &doc.newline);
    finish(markdown, lines, key_path, before)
}

fn remove_seq_entry(
    markdown: &str,
    doc: &BankDocument,
    item: &SeqItem,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let before = Some(bytes_of(&doc.lines, item.start, item.end));
    let mut lines = doc.lines.clone();
    replace_range(&mut lines, item.start, item.end, &[], &doc.newline);
    finish(markdown, lines, key_path, before)
}

fn remove_child_item(
    markdown: &str,
    doc: &BankDocument,
    item: &Item,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    remove_map_entry(markdown, doc, item, key_path)
}

fn move_map_entry(
    markdown: &str,
    doc: &BankDocument,
    container: &MapContainer,
    from: usize,
    to: usize,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let item = &container.entries[from];
    let slice = doc.lines[item.start..=item.end].to_vec();
    let before = Some(bytes_of(
        &doc.lines,
        container.item.start,
        container.item.end,
    ));
    let mut lines = doc.lines.clone();
    replace_range(&mut lines, item.start, item.end, &[], &doc.newline);
    let shifted: Vec<Item> = container
        .entries
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != from)
        .map(|(_, item)| item.clone())
        .collect();
    let at = if to >= shifted.len() {
        shifted
            .last()
            .map(|item| adjust_after_remove(item.end + 1, item.start, item.end))
            .unwrap_or(container.item.key_line + 1)
    } else {
        adjust_after_remove(shifted[to].start, item.start, item.end)
    };
    insert_range(&mut lines, at, slice, &doc.newline);
    finish(markdown, lines, key_path, before)
}

fn move_seq_entry(
    markdown: &str,
    doc: &BankDocument,
    container: &SeqContainer,
    from: usize,
    to: usize,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let item = &container.entries[from];
    let slice = doc.lines[item.start..=item.end].to_vec();
    let before = Some(bytes_of(
        &doc.lines,
        container.item.start,
        container.item.end,
    ));
    let mut lines = doc.lines.clone();
    replace_range(&mut lines, item.start, item.end, &[], &doc.newline);
    let shifted: Vec<SeqItem> = container
        .entries
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != from)
        .map(|(_, item)| item.clone())
        .collect();
    let at = if to >= shifted.len() {
        shifted
            .last()
            .map(|item| adjust_after_remove(item.end + 1, item.start, item.end))
            .unwrap_or(container.item.key_line + 1)
    } else {
        adjust_after_remove(shifted[to].start, item.start, item.end)
    };
    insert_range(&mut lines, at, slice, &doc.newline);
    finish(markdown, lines, key_path, before)
}

fn adjust_after_remove(at: usize, start: usize, end: usize) -> usize {
    if at > end {
        at - (end - start + 1)
    } else if at > start {
        start
    } else {
        at
    }
}

fn insert_bank_child(
    markdown: &str,
    doc: &BankDocument,
    container: BankContainer,
    replacement: Vec<Line>,
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let order = BANK_ORDER
        .iter()
        .position(|key| *key == container.key())
        .unwrap_or(BANK_ORDER.len());
    let mut at = None;
    for child in &doc.children {
        let child_order = BANK_ORDER
            .iter()
            .position(|key| *key == child.key())
            .unwrap_or(BANK_ORDER.len());
        if child_order < order {
            at = Some(child.item().end + 1);
        } else {
            at = Some(at.unwrap_or(child.item().start));
            break;
        }
    }
    let at = at.unwrap_or(doc.bank.key_line + 1);
    if doc.children.is_empty() {
        return insert_first_child(
            markdown,
            doc,
            &doc.bank,
            replacement,
            key_path,
        );
    }
    insert_lines(markdown, doc, at, replacement, key_path)
}

fn set_block_child(
    markdown: &str,
    doc: &BankDocument,
    parent: &Item,
    field: &str,
    value: Option<YamlValue>,
    canonical: &[&str],
    key_path: &str,
) -> Result<BankSpanEdit, SpanError> {
    let nested = collect_map_items(
        &doc.lines,
        parent.key_line + 1,
        parent.end + 1,
        parent.indent + 2,
        &parent.key,
    )
    .unwrap_or_default();
    match (nested.iter().find(|item| item.key == field), value) {
        (None, None) => Ok(noop(markdown)),
        (Some(item), None) => remove_map_entry(markdown, doc, item, key_path),
        (Some(item), Some(value)) => {
            let comment =
                trailing_comment(&doc.lines, item.key_line, item.colon);
            let replacement = emit_scalar_key(
                field,
                &value,
                item.indent,
                &doc.newline,
                &comment,
            );
            splice_item(markdown, doc, item, &replacement, key_path)
        }
        (None, Some(value)) => {
            let indent = nested
                .first()
                .map(|item| item.indent)
                .unwrap_or(parent.indent + 2);
            let replacement =
                emit_scalar_key(field, &value, indent, &doc.newline, "");
            let field_order = canonical.iter().position(|key| *key == field);
            let mut at = parent.key_line + 1;
            for item in &nested {
                let item_order =
                    canonical.iter().position(|key| *key == item.key);
                if field_order.is_some()
                    && item_order
                        .is_some_and(|order| order < field_order.unwrap_or(0))
                {
                    at = item.end + 1;
                } else {
                    at = item.start;
                    break;
                }
            }
            if nested.is_empty() {
                at = parent.key_line + 1;
            }
            insert_lines(markdown, doc, at, replacement, key_path)
        }
    }
}

fn insert_range(
    lines: &mut Vec<Line>,
    at: usize,
    replacement: Vec<Line>,
    newline: &str,
) {
    let mut replacement = replacement;
    for line in &mut replacement {
        if line.ending.is_empty() {
            line.ending = newline.to_owned();
        }
    }
    let at = at.min(lines.len());
    lines.splice(at..at, replacement);
}

fn replace_range(
    lines: &mut Vec<Line>,
    start: usize,
    end: usize,
    replacement: &[Line],
    newline: &str,
) {
    let mut replacement = replacement.to_vec();
    for line in &mut replacement {
        if line.ending.is_empty() {
            line.ending = newline.to_owned();
        }
    }
    if start >= lines.len() {
        lines.extend(replacement);
        return;
    }
    let end = end.min(lines.len() - 1);
    let start = start.min(end);
    lines.splice(start..=end, replacement);
}

fn bytes_of(lines: &[Line], start: usize, end: usize) -> String {
    if start > end || start >= lines.len() {
        return String::new();
    }
    join_lines(&lines[start..=end.min(lines.len() - 1)])
}

fn finish(
    original: &str,
    lines: Vec<Line>,
    key_path: &str,
    _before: Option<String>,
) -> Result<BankSpanEdit, SpanError> {
    let contents = normalize_ending(original, join_lines(&lines));
    if contents == original {
        return Ok(noop(original));
    }
    let (before, after) = diff_spans(original, &contents);
    Ok(BankSpanEdit {
        contents,
        spans: vec![RestoreSpan {
            key_path: key_path.to_owned(),
            before,
            after,
        }],
    })
}

fn diff_spans(
    original: &str,
    edited: &str,
) -> (Option<String>, Option<String>) {
    let mut prefix = original
        .bytes()
        .zip(edited.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    let mut suffix = original[prefix..]
        .bytes()
        .rev()
        .zip(edited[prefix..].bytes().rev())
        .take_while(|(left, right)| left == right)
        .count()
        .min(original.len() - prefix)
        .min(edited.len() - prefix);
    prefix = char_floor(original, prefix);
    suffix =
        char_start_len(original, original.len() - suffix.min(original.len()));
    while !after_is_unique(edited, prefix, suffix) {
        if prefix > 0 {
            prefix = char_floor(original, prefix - 1);
            continue;
        }
        if suffix > 0 {
            suffix = shrink_suffix(original, suffix);
            continue;
        }
        break;
    }
    let before = original[prefix..original.len() - suffix].to_owned();
    let after = edited[prefix..edited.len() - suffix].to_owned();
    (
        (!before.is_empty()).then_some(before),
        (!after.is_empty()).then_some(after),
    )
}

fn after_is_unique(edited: &str, prefix: usize, suffix: usize) -> bool {
    let end = edited.len() - suffix;
    if prefix > end {
        return false;
    }
    let after = &edited[prefix..end];
    if after.is_empty() {
        return false;
    }
    match edited.find(after) {
        Some(index) => !edited[index + after.len()..].contains(after),
        None => false,
    }
}

fn char_floor(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn char_start_len(text: &str, index: usize) -> usize {
    text.len() - char_floor(text, index.min(text.len()))
}

fn shrink_suffix(text: &str, suffix: usize) -> usize {
    if suffix == 0 {
        return 0;
    }
    let start = text.len() - suffix.min(text.len());
    let start = char_floor(text, start);
    match text[start..].chars().next() {
        Some(ch) => suffix.saturating_sub(ch.len_utf8()),
        None => 0,
    }
}

fn normalize_ending(original: &str, mut text: String) -> String {
    let original_nl = original.ends_with('\n');
    if original_nl && !text.ends_with('\n') {
        if original.ends_with("\r\n") {
            text.push_str("\r\n");
        } else {
            text.push('\n');
        }
    }
    if !original_nl && text.ends_with("\r\n") {
        text.truncate(text.len() - 2);
    } else if !original_nl && text.ends_with('\n') {
        text.pop();
    }
    text
}

fn restore_one(
    markdown: &str,
    span: &RestoreSpan,
) -> Result<String, SpanError> {
    match (&span.before, &span.after) {
        (None, None) => Ok(markdown.to_owned()),
        (before, Some(after)) => replace_once(
            markdown,
            after,
            before.as_deref().unwrap_or(""),
            &span.key_path,
        ),
        (Some(before), None) => {
            // Deletion restore: `after_bytes` recorded a unique neighbour
            // as `after` whenever it could. A true empty after means the
            // deleted span was the whole remainder of the file.
            if markdown.is_empty() {
                return Ok(before.clone());
            }
            Err(error(
                "restore_stale",
                format!(
                    "restore span {} is missing its after bytes",
                    span.key_path
                ),
                None,
                Some(span.key_path.clone()),
            ))
        }
    }
}

fn replace_once(
    markdown: &str,
    needle: &str,
    put: &str,
    key_path: &str,
) -> Result<String, SpanError> {
    if needle.is_empty() {
        return Ok(format!("{markdown}{put}"));
    }
    let Some(index) = markdown.find(needle) else {
        return Err(error(
            "restore_stale",
            format!(
                "restore span {key_path} no longer matches its after bytes"
            ),
            None,
            Some(key_path.to_owned()),
        ));
    };
    if markdown[index + needle.len()..].contains(needle) {
        return Err(error(
            "restore_stale",
            format!("restore span {key_path} matches more than once"),
            None,
            Some(key_path.to_owned()),
        ));
    }
    let mut text = String::new();
    text.push_str(&markdown[..index]);
    text.push_str(put);
    text.push_str(&markdown[index + needle.len()..]);
    Ok(text)
}

fn noop(markdown: &str) -> BankSpanEdit {
    BankSpanEdit {
        contents: markdown.to_owned(),
        spans: Vec::new(),
    }
}

fn join_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_owned()
    } else if key.is_empty() {
        parent.to_owned()
    } else {
        format!("{parent}.{key}")
    }
}

fn none_path(parent: &str) -> Option<String> {
    (!parent.is_empty()).then(|| parent.to_owned())
}

fn unsupported(
    message: impl Into<String>,
    line: Option<usize>,
    key_path: Option<String>,
) -> SpanError {
    error("unsupported_yaml", message, line, key_path)
}

fn error(
    code: &'static str,
    message: impl Into<String>,
    line: Option<usize>,
    key_path: Option<String>,
) -> SpanError {
    SpanError {
        code,
        message: message.into(),
        line,
        key_path,
    }
}

#[cfg(test)]
#[path = "config_spans_tests.rs"]
mod tests;
