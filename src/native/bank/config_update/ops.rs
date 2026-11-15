//! Category and group ops: semantic apply, consequences, and inverses.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::protocol::{
    Anchor, Op, Restoration, RestoreSpan as ProtocolSpan, UnsetField,
};
use crate::native::albums::output::split_icon;
use crate::native::bank::config::{
    self, BankConfig, Category, ConfigParse, HOTKEY_ORDER, RESERVED_HOTKEY,
};
use crate::native::bank::config_spans::{
    self, BankContainer, CategoryDraft, Place, RestoreSpan,
};
use crate::native::bank::model;
use crate::native::bank::scan::BankScan;

#[derive(Debug, Clone)]
pub(crate) struct OpError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) op_index: Option<usize>,
    pub(crate) key_path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) affected: Option<usize>,
}

impl OpError {
    pub(crate) fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint,
            op_index: None,
            key_path: None,
            line: None,
            affected: None,
        }
    }

    fn at(mut self, index: usize) -> Self {
        self.op_index = Some(index);
        self
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OpResult {
    pub(crate) op_index: usize,
    pub(crate) op_name: &'static str,
    pub(crate) summary: String,
    pub(crate) key: Option<String>,
    pub(crate) hotkey: Option<char>,
    pub(crate) rows: Option<usize>,
    pub(crate) rule_refs: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct InverseOp {
    pub(crate) op: Op,
    pub(crate) restore: Option<Restoration>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageIndex {
    rows: HashMap<String, usize>,
    rule_refs: HashMap<String, usize>,
}

impl UsageIndex {
    pub(crate) fn from_scan(scan: &BankScan, config: &BankConfig) -> Self {
        let rows = model::collect_rows(scan, Some(config), None);
        let mut row_counts = HashMap::new();
        for row in &rows {
            if let Some(key) = &row.category {
                *row_counts.entry(key.clone()).or_insert(0) += 1;
            }
        }
        let mut rule_refs = HashMap::new();
        for rule in &config.rules {
            if let Some(key) = &rule.cat {
                *rule_refs.entry(key.clone()).or_insert(0) += 1;
            }
        }
        Self {
            rows: row_counts,
            rule_refs,
        }
    }

    fn rows(&self, key: &str) -> usize {
        self.rows.get(key).copied().unwrap_or(0)
    }

    fn rule_refs(&self, key: &str) -> usize {
        self.rule_refs.get(key).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
struct AuthoredCategory {
    key: String,
    label: String,
    hotkey: Option<char>,
    group: Option<String>,
    hidden: Option<bool>,
}

impl AuthoredCategory {
    fn draft(&self) -> CategoryDraft {
        CategoryDraft {
            key: self.key.clone(),
            label: self.label.clone(),
            hotkey: self.hotkey,
            group: self.group.clone(),
            hidden: self.hidden,
        }
    }
}

pub(crate) fn apply_ops(
    markdown: &str,
    ops: &[Op],
    restorations: &[Option<Restoration>],
    usage: &UsageIndex,
    op_offset: usize,
) -> Result<(String, Vec<OpResult>, Vec<InverseOp>), OpError> {
    let mut contents = markdown.to_owned();
    let mut results = Vec::new();
    let mut inverses = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let restoration = restorations.get(index).and_then(Option::as_ref);
        let outcome =
            apply_one(&contents, op, restoration, usage, op_offset + index)?;
        contents = outcome.contents;
        results.push(outcome.result);
        inverses.push(outcome.inverse);
    }
    inverses.reverse();
    Ok((contents, results, inverses))
}

struct OneOutcome {
    contents: String,
    result: OpResult,
    inverse: InverseOp,
}

fn apply_one(
    markdown: &str,
    op: &Op,
    restoration: Option<&Restoration>,
    usage: &UsageIndex,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let config = parse_ok(markdown).map_err(|error| error.at(index))?;
    if let Some(restoration) = restoration {
        validate_remove_usage(op, usage, &config, index)?;
        let spans = restoration
            .spans
            .iter()
            .map(|span| RestoreSpan {
                key_path: span.key_path.clone(),
                before: span.prior.clone(),
                after: span.expected.clone(),
            })
            .collect::<Vec<_>>();
        let contents = config_spans::restore(markdown, &spans)
            .map_err(|error| span_error(&error).at(index))?;
        let result = restored_result(op, index, &config, usage);
        return Ok(OneOutcome {
            contents,
            result,
            inverse: InverseOp {
                op: op.clone(),
                restore: None,
            },
        });
    }
    match op {
        Op::CategoryAdd {
            label,
            group,
            hotkey,
            hidden,
            place,
            restore_key,
        } => add_category(
            markdown,
            &config,
            label,
            group.as_deref(),
            *hotkey,
            *hidden,
            place.as_ref(),
            restore_key.as_deref(),
            index,
        ),
        Op::CategorySet {
            key,
            label,
            group,
            hotkey,
            hidden,
            unset,
        } => set_category(
            markdown,
            &config,
            usage,
            key,
            label.as_deref(),
            group.as_deref(),
            *hotkey,
            *hidden,
            unset,
            index,
        ),
        Op::CategoryReset { key } => {
            reset_category(markdown, &config, usage, key, index)
        }
        Op::CategoryMove { key, place } => {
            move_category(markdown, &config, key, place, index)
        }
        Op::CategoryRemove { key } => {
            remove_category(markdown, &config, usage, key, index)
        }
        Op::GroupRename { group, to } => {
            rename_group(markdown, &config, group, to, index)
        }
        Op::GroupMove { group, place } => {
            move_group(markdown, &config, group, place, index)
        }
    }
}

fn validate_remove_usage(
    op: &Op,
    usage: &UsageIndex,
    config: &BankConfig,
    index: usize,
) -> Result<(), OpError> {
    let Op::CategoryRemove { key } = op else {
        return Ok(());
    };
    let rows = usage.rows(key);
    let rules = usage.rule_refs(key);
    if rows == 0 && rules == 0 {
        return Ok(());
    }
    let label = config
        .category(key)
        .map(full_label)
        .unwrap_or_else(|| key.to_owned());
    let mut parts = Vec::new();
    if rows > 0 {
        parts.push(format!("{rows} row{}", if rows == 1 { "" } else { "s" }));
    }
    if rules > 0 {
        parts
            .push(format!("{rules} rule{}", if rules == 1 { "" } else { "s" }));
    }
    let mut error = OpError::new(
        "undo_blocked",
        format!("{} now use {label} · cannot undo", parts.join(" and ")),
        Some(
            "hide it instead, or wait until those rows no longer depend on it"
                .to_owned(),
        ),
    );
    error.op_index = Some(index);
    error.affected = Some(rows + rules);
    error.key_path = Some(format!("bank.categories.{key}"));
    Err(error)
}

fn restored_result(
    op: &Op,
    index: usize,
    config: &BankConfig,
    usage: &UsageIndex,
) -> OpResult {
    let (key, hotkey) = match op {
        Op::CategoryAdd {
            restore_key,
            hotkey,
            ..
        } => (restore_key.clone(), *hotkey),
        Op::CategorySet { key, hotkey, .. } => (Some(key.clone()), *hotkey),
        Op::CategoryReset { key }
        | Op::CategoryMove { key, .. }
        | Op::CategoryRemove { key } => {
            let hotkey =
                config.category(key).and_then(|category| category.hotkey);
            (Some(key.clone()), hotkey)
        }
        Op::GroupRename { .. } | Op::GroupMove { .. } => (None, None),
    };
    let rows = key.as_deref().map(|key| usage.rows(key));
    let rule_refs = key.as_deref().map(|key| usage.rule_refs(key));
    OpResult {
        op_index: index,
        op_name: op.name(),
        summary: format!("restores {}", op.name()),
        key,
        hotkey,
        rows,
        rule_refs,
    }
}

#[allow(clippy::too_many_arguments)]
fn add_category(
    markdown: &str,
    config: &BankConfig,
    label: &str,
    group: Option<&str>,
    hotkey: Option<char>,
    hidden: Option<bool>,
    place: Option<&Anchor>,
    restore_key: Option<&str>,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let key = match restore_key {
        Some(key) => key.to_owned(),
        None => derive_stored_key(label, config, index)?,
    };
    if let Some(existing) = config.category(&key) {
        if existing.declared {
            return Err(exists_error(existing, index));
        }
        if restore_key.is_none() {
            let derived = derive_stored_key(label, config, index)?;
            if derived == key {
                return Err(exists_error(existing, index));
            }
        }
    }
    let claimed = claimed_hotkeys(config);
    let pinned = match hotkey {
        Some(hotkey) => {
            refuse_taken(hotkey, None, config, index)?;
            hotkey
        }
        None => suggested_hotkey(&key, label, &claimed).ok_or_else(|| {
            OpError::new(
                "hotkey_taken",
                "no free hotkey remains to pin",
                Some("unset another key, or pass hotkey: none is not valid on add; pick a free character".to_owned()),
            )
            .at(index)
        })?,
    };
    let draft = CategoryDraft {
        key: key.clone(),
        label: label.to_owned(),
        hotkey: Some(pinned),
        group: group.map(str::to_owned),
        hidden,
    };
    let mut text = markdown.to_owned();
    if categories_missing(&text) {
        let ensured =
            config_spans::ensure_container(&text, BankContainer::Categories)
                .map_err(|error| span_error(&error).at(index))?;
        text = ensured.contents;
    }
    let span_place = resolve_add_place(&text, config, group, place, index)?;
    let place_key;
    let place_ref = match &span_place {
        None => Place::End,
        Some(Anchor::Before(key)) => {
            place_key = key.clone();
            Place::Before(&place_key)
        }
        Some(Anchor::After(key)) => {
            place_key = key.clone();
            Place::After(&place_key)
        }
    };
    let edit = config_spans::insert_category(&text, &draft, place_ref)
        .map_err(|error| span_error(&error).at(index))?;
    let contents = edit.contents;
    let group_text = group.unwrap_or("Ungrouped");
    let summary = format!(
        "adds {label} to {group_text} as [{}] · stored as {key} · no other keys move",
        pinned.to_ascii_uppercase()
    );
    Ok(OneOutcome {
        contents,
        result: OpResult {
            op_index: index,
            op_name: "category.add",
            summary,
            key: Some(key.clone()),
            hotkey: Some(pinned),
            rows: Some(0),
            rule_refs: Some(0),
        },
        inverse: InverseOp {
            op: Op::CategoryRemove { key },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn exists_error(existing: &Category, index: usize) -> OpError {
    let mut error = OpError::new(
        "category_exists",
        format!("category {} already exists", existing.key),
        if existing.hidden {
            Some(format!(
                "unhide {} instead of adding a duplicate",
                existing.key
            ))
        } else {
            Some(
                "pick a different label, or edit the existing category"
                    .to_owned(),
            )
        },
    );
    error.op_index = Some(index);
    error.key_path = Some(format!("bank.categories.{}", existing.key));
    error
}

fn derive_stored_key(
    label: &str,
    config: &BankConfig,
    index: usize,
) -> Result<String, OpError> {
    let base = slug_from_label(label).ok_or_else(|| {
        OpError::new(
            "envelope_wrong_type",
            format!(
                "could not derive a stored key matching [a-z][a-z0-9_]* from {label:?}"
            ),
            Some("start the label with a letter after any emoji".to_owned()),
        )
        .at(index)
    })?;
    if let Some(existing) = config.category(&base) {
        if same_label(existing, label) {
            return Err(exists_error(existing, index));
        }
        let mut suffix = 2u32;
        loop {
            let candidate = format!("{base}_{suffix}");
            if config.category(&candidate).is_none() {
                return Ok(candidate);
            }
            suffix = suffix.saturating_add(1);
            if suffix > 99 {
                return Err(OpError::new(
                    "category_exists",
                    format!("too many keys collide with {base}"),
                    None,
                )
                .at(index));
            }
        }
    }
    Ok(base)
}

fn slug_from_label(label: &str) -> Option<String> {
    let (_, rest) = split_icon(label);
    let source = if rest.is_empty() { label } else { rest };
    let mut out = String::new();
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let slug = out.trim_matches('_').to_owned();
    config::is_alias(&slug).then_some(slug)
}

fn same_label(category: &Category, label: &str) -> bool {
    normalize_label(&full_label(category)) == normalize_label(label)
}

fn normalize_label(label: &str) -> String {
    let (_, rest) = split_icon(label);
    let text = if rest.is_empty() { label } else { rest };
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn full_label(category: &Category) -> String {
    if category.emoji.is_empty() {
        category.label.clone()
    } else {
        format!("{} {}", category.emoji, category.label)
    }
}

fn suggested_hotkey(
    key: &str,
    label: &str,
    claimed: &HashSet<char>,
) -> Option<char> {
    if let Some(hotkey) = config::derive_hotkey(key, label, claimed) {
        return Some(hotkey);
    }
    HOTKEY_ORDER.chars().find(|ch| {
        *ch != RESERVED_HOTKEY
            && config::is_hotkey_char(*ch)
            && !claimed.contains(ch)
    })
}

fn claimed_hotkeys(config: &BankConfig) -> HashSet<char> {
    config
        .categories
        .iter()
        .filter_map(|category| category.hotkey)
        .collect()
}

fn refuse_taken(
    hotkey: char,
    except: Option<&str>,
    config: &BankConfig,
    index: usize,
) -> Result<(), OpError> {
    if hotkey == RESERVED_HOTKEY {
        return Err(OpError::new(
            "hotkey_reserved",
            "hotkey \"q\" is reserved",
            None,
        )
        .at(index));
    }
    if !config::is_hotkey_char(hotkey) {
        return Err(OpError::new(
            "hotkey_invalid",
            format!("hotkey {hotkey:?} must be one character in [a-z0-9]"),
            None,
        )
        .at(index));
    }
    if let Some(owner) = config.categories.iter().find(|category| {
        category.hotkey == Some(hotkey)
            && except.is_none_or(|key| category.key != key)
    }) {
        let mut error = OpError::new(
            "hotkey_taken",
            format!("hotkey {hotkey:?} already claimed by {}", owner.key),
            Some(format!("pick a free key, or change {} first", owner.key)),
        );
        error.op_index = Some(index);
        error.key_path = Some(format!("bank.categories.{}", owner.key));
        return Err(error);
    }
    Ok(())
}

fn categories_missing(markdown: &str) -> bool {
    let Some(frontmatter) =
        crate::native::projects::parse_frontmatter(markdown)
    else {
        return false;
    };
    let yaml = frontmatter.lines.join("\n");
    let Ok(root) = serde_yaml::from_str::<serde_yaml::Value>(&yaml) else {
        return false;
    };
    let Some(bank) = root.as_mapping().and_then(|mapping| {
        mapping.get(serde_yaml::Value::String("bank".to_owned()))
    }) else {
        return false;
    };
    !bank.as_mapping().is_some_and(|mapping| {
        mapping.contains_key(serde_yaml::Value::String("categories".to_owned()))
    })
}

fn resolve_add_place(
    markdown: &str,
    _config: &BankConfig,
    group: Option<&str>,
    place: Option<&Anchor>,
    index: usize,
) -> Result<Option<Anchor>, OpError> {
    if let Some(place) = place {
        declared_anchor(markdown, place, index)?;
        return Ok(Some(place.clone()));
    }
    let Some(group) = group else {
        return Ok(None);
    };
    let authored = authored_categories(markdown);
    let last = authored
        .iter()
        .rev()
        .find(|category| category.group.as_deref() == Some(group))
        .map(|category| category.key.clone());
    if last.is_none() {
        let last_declared =
            authored.last().map(|category| category.key.clone());
        return Ok(last_declared.map(Anchor::After));
    }
    Ok(last.map(Anchor::After))
}

fn declared_anchor(
    markdown: &str,
    place: &Anchor,
    index: usize,
) -> Result<(), OpError> {
    let key = match place {
        Anchor::Before(key) | Anchor::After(key) => key.as_str(),
    };
    if authored_categories(markdown)
        .iter()
        .any(|item| item.key == key)
    {
        return Ok(());
    }
    Err(OpError::new(
        "unknown_category",
        format!("category {key} is not a declared bank: entry"),
        Some("position before/after a declared category key".to_owned()),
    )
    .at(index))
}

#[allow(clippy::too_many_arguments)]
fn set_category(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    key: &str,
    label: Option<&str>,
    group: Option<&str>,
    hotkey: Option<char>,
    hidden: Option<bool>,
    unset: &[UnsetField],
    index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = config.category(key).ok_or_else(|| {
        OpError::new(
            "unknown_category",
            format!("category {key} does not exist"),
            None,
        )
        .at(index)
    })?;
    if let Some(hotkey) = hotkey {
        refuse_taken(hotkey, Some(key), config, index)?;
    }
    let authored = authored_categories(markdown);
    let mut draft = authored
        .iter()
        .find(|item| item.key == key)
        .cloned()
        .unwrap_or_else(|| AuthoredCategory {
            key: key.to_owned(),
            label: full_label(existing),
            hotkey: None,
            group: None,
            hidden: None,
        });
    let prior = draft.clone();
    if let Some(label) = label {
        draft.label = label.to_owned();
    }
    if let Some(group) = group {
        draft.group = Some(group.to_owned());
    }
    if let Some(hotkey) = hotkey {
        draft.hotkey = Some(hotkey);
    }
    if let Some(hidden) = hidden {
        draft.hidden = Some(hidden);
    }
    for field in unset {
        match field {
            UnsetField::Hotkey => draft.hotkey = None,
            UnsetField::Group => draft.group = None,
        }
    }
    let creating = !existing.declared;
    let mut text = markdown.to_owned();
    let edit = if creating {
        if categories_missing(&text) {
            let ensured = config_spans::ensure_container(
                &text,
                BankContainer::Categories,
            )
            .map_err(|error| span_error(&error).at(index))?;
            text = ensured.contents;
        }
        config_spans::insert_category(&text, &draft.draft(), Place::End)
            .map_err(|error| span_error(&error).at(index))?
    } else {
        config_spans::replace_category(markdown, &draft.draft())
            .map_err(|error| span_error(&error).at(index))?
    };
    let rows = usage.rows(key);
    let summary = set_summary(existing, &draft, prior.hotkey, rows);
    let inverse = if creating {
        InverseOp {
            op: Op::CategoryReset {
                key: key.to_owned(),
            },
            restore: Some(restoration_from(&edit.spans)),
        }
    } else {
        InverseOp {
            op: inverse_set(key, &prior, &draft),
            restore: Some(restoration_from(&edit.spans)),
        }
    };
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "category.set",
            summary,
            key: Some(key.to_owned()),
            hotkey: draft.hotkey.or(existing.hotkey),
            rows: Some(rows),
            rule_refs: Some(usage.rule_refs(key)),
        },
        inverse,
    })
}

fn inverse_set(
    key: &str,
    prior: &AuthoredCategory,
    next: &AuthoredCategory,
) -> Op {
    let mut unset = Vec::new();
    if prior.hotkey.is_none() && next.hotkey.is_some() {
        unset.push(UnsetField::Hotkey);
    }
    if prior.group.is_none() && next.group.is_some() {
        unset.push(UnsetField::Group);
    }
    Op::CategorySet {
        key: key.to_owned(),
        label: (prior.label != next.label).then(|| prior.label.clone()),
        group: match (&prior.group, &next.group) {
            (Some(group), other) if other.as_ref() != Some(group) => {
                Some(group.clone())
            }
            _ => None,
        },
        hotkey: match (prior.hotkey, next.hotkey) {
            (Some(hotkey), other) if other != Some(hotkey) => Some(hotkey),
            _ => None,
        },
        hidden: match (prior.hidden, next.hidden) {
            (Some(flag), other) if other != Some(flag) => Some(flag),
            (None, Some(_)) => Some(false),
            _ => None,
        },
        unset,
    }
}

fn set_summary(
    existing: &Category,
    draft: &AuthoredCategory,
    prior_hotkey: Option<char>,
    rows: usize,
) -> String {
    let display = full_label(existing);
    if draft.hidden == Some(true) && !existing.hidden {
        return format!(
            "hides {display} · {rows} rows keep it · never offered"
        );
    }
    if existing.hidden && draft.hidden == Some(false) {
        return format!("unhides {display}");
    }
    if draft.label != full_label(existing) && draft.label != existing.label {
        return format!(
            "renames {display} → {} · stored key stays {} · {rows} rows follow",
            draft.label, existing.key
        );
    }
    if draft.hotkey != prior_hotkey {
        if let Some(hotkey) = draft.hotkey {
            return format!(
                "pins {} as [{}] · stored key stays {}",
                display,
                hotkey.to_ascii_uppercase(),
                existing.key
            );
        }
        return format!("returns {display}'s key to derivation");
    }
    if draft.group.as_deref() != existing.group.as_deref() {
        return format!(
            "moves {display} to {}",
            draft.group.as_deref().unwrap_or("Ungrouped")
        );
    }
    format!("updates {display}")
}

fn reset_category(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    key: &str,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = config.category(key).ok_or_else(|| {
        OpError::new(
            "unknown_category",
            format!("category {key} does not exist"),
            None,
        )
        .at(index)
    })?;
    if !existing.builtin {
        return Err(OpError::new(
            "not_builtin",
            format!("{key} is not a built-in category"),
            Some("hide or remove it instead".to_owned()),
        )
        .at(index));
    }
    if !existing.declared {
        return Ok(OneOutcome {
            contents: markdown.to_owned(),
            result: OpResult {
                op_index: index,
                op_name: "category.reset",
                summary: format!(
                    "restores {}'s default appearance",
                    full_label(existing)
                ),
                key: Some(key.to_owned()),
                hotkey: existing.hotkey,
                rows: Some(usage.rows(key)),
                rule_refs: Some(usage.rule_refs(key)),
            },
            inverse: InverseOp {
                op: Op::CategoryReset {
                    key: key.to_owned(),
                },
                restore: None,
            },
        });
    }
    let authored = authored_categories(markdown)
        .into_iter()
        .find(|item| item.key == key)
        .expect("declared builtin has an authored entry");
    let edit = config_spans::remove_category(markdown, key)
        .map_err(|error| span_error(&error).at(index))?;
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "category.reset",
            summary: format!(
                "restores {}'s default appearance",
                full_label(existing)
            ),
            key: Some(key.to_owned()),
            hotkey: existing.hotkey,
            rows: Some(usage.rows(key)),
            rule_refs: Some(usage.rule_refs(key)),
        },
        inverse: InverseOp {
            op: Op::CategoryAdd {
                label: authored.label,
                group: authored.group,
                hotkey: authored.hotkey,
                hidden: authored.hidden,
                place: None,
                restore_key: Some(key.to_owned()),
            },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn move_category(
    markdown: &str,
    config: &BankConfig,
    key: &str,
    place: &Anchor,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = config.category(key).ok_or_else(|| {
        OpError::new(
            "unknown_category",
            format!("category {key} does not exist"),
            None,
        )
        .at(index)
    })?;
    if !existing.declared {
        return Err(OpError::new(
            "builtin_category",
            format!(
                "{key} is an undeclared built-in; declare it before moving"
            ),
            Some(
                "set a field on it first, or move declared categories"
                    .to_owned(),
            ),
        )
        .at(index));
    }
    declared_anchor(markdown, place, index)?;
    let authored = authored_categories(markdown);
    let inverse_place = original_neighbor(&authored, key);
    let place_key;
    let span_place = match place {
        Anchor::Before(target) => {
            place_key = target.clone();
            Place::Before(&place_key)
        }
        Anchor::After(target) => {
            place_key = target.clone();
            Place::After(&place_key)
        }
    };
    let edit = config_spans::move_category(markdown, key, span_place)
        .map_err(|error| span_error(&error).at(index))?;
    let target = match place {
        Anchor::Before(key) => format!("before {key}"),
        Anchor::After(key) => format!("after {key}"),
    };
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "category.move",
            summary: format!("moves {} {target}", full_label(existing)),
            key: Some(key.to_owned()),
            hotkey: existing.hotkey,
            rows: None,
            rule_refs: None,
        },
        inverse: InverseOp {
            op: Op::CategoryMove {
                key: key.to_owned(),
                place: inverse_place,
            },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn original_neighbor(authored: &[AuthoredCategory], key: &str) -> Anchor {
    let index = authored
        .iter()
        .position(|item| item.key == key)
        .unwrap_or(0);
    if index + 1 < authored.len() {
        Anchor::Before(authored[index + 1].key.clone())
    } else if index > 0 {
        Anchor::After(authored[index - 1].key.clone())
    } else {
        Anchor::After(key.to_owned())
    }
}

fn remove_category(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    key: &str,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = config.category(key).ok_or_else(|| {
        OpError::new(
            "unknown_category",
            format!("category {key} does not exist"),
            None,
        )
        .at(index)
    })?;
    if existing.builtin {
        return Err(OpError::new(
            "builtin_category",
            format!("{key} is a built-in category"),
            Some("reset or hide it instead".to_owned()),
        )
        .at(index));
    }
    if !existing.declared {
        return Err(OpError::new(
            "unknown_category",
            format!("category {key} is not declared"),
            None,
        )
        .at(index));
    }
    refuse_in_use(key, usage, config, index)?;
    let authored = authored_categories(markdown)
        .into_iter()
        .find(|item| item.key == key)
        .expect("declared category has authored entry");
    let neighbor = original_neighbor(&authored_categories(markdown), key);
    let edit = config_spans::remove_category(markdown, key)
        .map_err(|error| span_error(&error).at(index))?;
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "category.remove",
            summary: format!("removes {}", full_label(existing)),
            key: Some(key.to_owned()),
            hotkey: existing.hotkey,
            rows: Some(0),
            rule_refs: Some(0),
        },
        inverse: InverseOp {
            op: Op::CategoryAdd {
                label: authored.label,
                group: authored.group,
                hotkey: authored.hotkey,
                hidden: authored.hidden,
                place: Some(neighbor),
                restore_key: Some(key.to_owned()),
            },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn refuse_in_use(
    key: &str,
    usage: &UsageIndex,
    config: &BankConfig,
    index: usize,
) -> Result<(), OpError> {
    let rows = usage.rows(key);
    let rules = usage.rule_refs(key);
    if rows == 0 && rules == 0 {
        return Ok(());
    }
    let label = config
        .category(key)
        .map(full_label)
        .unwrap_or_else(|| key.to_owned());
    let mut parts = Vec::new();
    if rows > 0 {
        parts.push(format!("{rows} row{}", if rows == 1 { "" } else { "s" }));
    }
    if rules > 0 {
        parts
            .push(format!("{rules} rule{}", if rules == 1 { "" } else { "s" }));
    }
    let mut error = OpError::new(
        "category_in_use",
        format!(
            "{} use {label} · hide it instead (Ctrl+H)",
            parts.join(" and ")
        ),
        Some("hide it instead".to_owned()),
    );
    error.op_index = Some(index);
    error.affected = Some(rows + rules);
    error.key_path = Some(format!("bank.categories.{key}"));
    Err(error)
}

fn rename_group(
    markdown: &str,
    config: &BankConfig,
    group: &str,
    to: &str,
    index: usize,
) -> Result<OneOutcome, OpError> {
    refuse_builtin_group(config, group, index)?;
    let authored = authored_categories(markdown);
    let members = authored
        .iter()
        .filter(|item| item.group.as_deref() == Some(group))
        .cloned()
        .collect::<Vec<_>>();
    if members.is_empty() {
        return Err(OpError::new(
            "unknown_group",
            format!("group {group:?} has no declared categories"),
            Some(
                "rename a group that appears on a declared category".to_owned(),
            ),
        )
        .at(index));
    }
    let mut contents = markdown.to_owned();
    let mut spans = Vec::new();
    for member in &members {
        let mut draft = member.clone();
        draft.group = Some(to.to_owned());
        let edit = config_spans::replace_category(&contents, &draft.draft())
            .map_err(|error| span_error(&error).at(index))?;
        contents = edit.contents;
        spans.extend(edit.spans);
    }
    Ok(OneOutcome {
        contents,
        result: OpResult {
            op_index: index,
            op_name: "group.rename",
            summary: format!(
                "renames group {group} → {to} · {} {}",
                members.len(),
                if members.len() == 1 {
                    "category"
                } else {
                    "categories"
                }
            ),
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
        },
        inverse: InverseOp {
            op: Op::GroupRename {
                group: to.to_owned(),
                to: group.to_owned(),
            },
            restore: Some(restoration_from(&spans)),
        },
    })
}

fn move_group(
    markdown: &str,
    config: &BankConfig,
    group: &str,
    place: &Anchor,
    index: usize,
) -> Result<OneOutcome, OpError> {
    refuse_builtin_group(config, group, index)?;
    let target_name = match place {
        Anchor::Before(name) | Anchor::After(name) => name.as_str(),
    };
    if target_name == group {
        return Ok(OneOutcome {
            contents: markdown.to_owned(),
            result: OpResult {
                op_index: index,
                op_name: "group.move",
                summary: format!("group {group} stays put"),
                key: None,
                hotkey: None,
                rows: None,
                rule_refs: None,
            },
            inverse: InverseOp {
                op: Op::GroupMove {
                    group: group.to_owned(),
                    place: place.clone(),
                },
                restore: None,
            },
        });
    }
    refuse_builtin_group(config, target_name, index)?;
    let authored = authored_categories(markdown);
    let members = authored
        .iter()
        .filter(|item| item.group.as_deref() == Some(group))
        .map(|item| item.key.clone())
        .collect::<Vec<_>>();
    if members.is_empty() {
        return Err(OpError::new(
            "unknown_group",
            format!("group {group:?} has no declared categories"),
            None,
        )
        .at(index));
    }
    let target_members = authored
        .iter()
        .filter(|item| item.group.as_deref() == Some(target_name))
        .map(|item| item.key.clone())
        .collect::<Vec<_>>();
    if target_members.is_empty() {
        return Err(OpError::new(
            "unknown_group",
            format!("group {target_name:?} has no declared categories"),
            None,
        )
        .at(index));
    }
    let inverse_place = original_group_neighbor(&authored, group);
    let mut contents = markdown.to_owned();
    let mut spans = Vec::new();
    for (offset, key) in members.iter().enumerate() {
        let place_key;
        let span_place = if offset == 0 {
            match place {
                Anchor::Before(_) => {
                    place_key = target_members[0].clone();
                    Place::Before(&place_key)
                }
                Anchor::After(_) => {
                    place_key = target_members
                        .last()
                        .cloned()
                        .expect("target group has members");
                    Place::After(&place_key)
                }
            }
        } else {
            place_key = members[offset - 1].clone();
            Place::After(&place_key)
        };
        let edit = config_spans::move_category(&contents, key, span_place)
            .map_err(|error| span_error(&error).at(index))?;
        contents = edit.contents;
        spans.extend(edit.spans);
    }
    let target = match place {
        Anchor::Before(name) => format!("before {name}"),
        Anchor::After(name) => format!("after {name}"),
    };
    Ok(OneOutcome {
        contents,
        result: OpResult {
            op_index: index,
            op_name: "group.move",
            summary: format!(
                "moves {group} {target} · {} {}",
                members.len(),
                if members.len() == 1 {
                    "category"
                } else {
                    "categories"
                }
            ),
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
        },
        inverse: InverseOp {
            op: Op::GroupMove {
                group: group.to_owned(),
                place: inverse_place,
            },
            restore: Some(restoration_from(&spans)),
        },
    })
}

fn original_group_neighbor(
    authored: &[AuthoredCategory],
    group: &str,
) -> Anchor {
    let keys = authored
        .iter()
        .filter(|item| item.group.as_deref() == Some(group))
        .map(|item| item.key.as_str())
        .collect::<Vec<_>>();
    let Some(first) = keys.first() else {
        return Anchor::After(group.to_owned());
    };
    let index = authored
        .iter()
        .position(|item| item.key == *first)
        .unwrap_or(0);
    if let Some(prev_group) = authored[..index]
        .iter()
        .rev()
        .find(|item| item.group.as_deref() != Some(group))
        .and_then(|item| item.group.clone())
    {
        return Anchor::After(prev_group);
    }
    if let Some(next_group) = authored[index..]
        .iter()
        .find(|item| item.group.as_deref() != Some(group))
        .and_then(|item| item.group.clone())
    {
        return Anchor::Before(next_group);
    }
    Anchor::After(group.to_owned())
}

fn refuse_builtin_group(
    config: &BankConfig,
    group: &str,
    index: usize,
) -> Result<(), OpError> {
    let undeclared = config.categories.iter().any(|category| {
        !category.declared
            && category.builtin
            && category.group.as_deref() == Some(group)
    });
    if undeclared {
        return Err(OpError::new(
            "group_builtin",
            format!(
                "{group} is bob's built-in group of undeclared built-ins"
            ),
            Some(
                "set, hide, or reset each built-in tile instead of renaming or moving the group"
                    .to_owned(),
            ),
        )
        .at(index));
    }
    Ok(())
}

fn parse_ok(markdown: &str) -> Result<BankConfig, OpError> {
    match config::parse_bank_config(markdown) {
        ConfigParse::Ok(config) => Ok(config),
        ConfigParse::MissingMapping => Err(OpError::new(
            "invalid_config",
            "fix bank: in YAML first",
            Some("bank.md has no bank: mapping".to_owned()),
        )),
        ConfigParse::Invalid { diagnostics, .. } => {
            let first = diagnostics.first();
            let mut error = OpError::new(
                "invalid_config",
                "fix bank: in YAML first",
                first.map(|item| item.message.clone()),
            );
            error.line = first.and_then(|item| item.line);
            error.key_path = first.map(|item| item.key_path.clone());
            Err(error)
        }
    }
}

fn authored_categories(markdown: &str) -> Vec<AuthoredCategory> {
    let Some(frontmatter) =
        crate::native::projects::parse_frontmatter(markdown)
    else {
        return Vec::new();
    };
    let yaml = frontmatter.lines.join("\n");
    let Ok(root) = serde_yaml::from_str::<serde_yaml::Value>(&yaml) else {
        return Vec::new();
    };
    let Some(bank) = root.as_mapping().and_then(|mapping| {
        mapping.get(serde_yaml::Value::String("bank".to_owned()))
    }) else {
        return Vec::new();
    };
    let Some(categories) = bank.as_mapping().and_then(|mapping| {
        mapping.get(serde_yaml::Value::String("categories".to_owned()))
    }) else {
        return Vec::new();
    };
    let Some(categories) = categories.as_mapping() else {
        return Vec::new();
    };
    categories
        .iter()
        .filter_map(|(key, value)| {
            let key = key.as_str()?.to_owned();
            Some(authored_from_value(key, value))
        })
        .collect()
}

fn authored_from_value(
    key: String,
    value: &serde_yaml::Value,
) -> AuthoredCategory {
    if let Some(mapping) = value.as_mapping() {
        let label = mapping
            .get(serde_yaml::Value::String("label".to_owned()))
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or(&key)
            .to_owned();
        let hotkey = mapping
            .get(serde_yaml::Value::String("hotkey".to_owned()))
            .and_then(hotkey_from_yaml);
        let group = mapping
            .get(serde_yaml::Value::String("group".to_owned()))
            .and_then(serde_yaml::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        let hidden = mapping
            .get(serde_yaml::Value::String("hidden".to_owned()))
            .and_then(serde_yaml::Value::as_bool);
        AuthoredCategory {
            key,
            label,
            hotkey,
            group,
            hidden,
        }
    } else {
        AuthoredCategory {
            key,
            label: value.as_str().unwrap_or("").to_owned(),
            hotkey: None,
            group: None,
            hidden: None,
        }
    }
}

fn hotkey_from_yaml(value: &serde_yaml::Value) -> Option<char> {
    if let Some(text) = value.as_str() {
        let mut chars = text.trim().chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => Some(ch),
            _ => None,
        }
    } else {
        value
            .as_u64()
            .filter(|number| *number <= 9)
            .and_then(|number| char::from_digit(number as u32, 10))
    }
}

fn restoration_from(spans: &[RestoreSpan]) -> Restoration {
    Restoration {
        spans: spans
            .iter()
            .map(|span| ProtocolSpan {
                key_path: span.key_path.clone(),
                expected: span.after.clone(),
                prior: span.before.clone(),
            })
            .collect(),
    }
}

fn span_error(error: &config_spans::SpanError) -> OpError {
    let mut mapped = OpError::new(error.code, error.message.clone(), None);
    mapped.line = error.line;
    mapped.key_path = error.key_path.clone();
    mapped
}

pub(crate) fn inverse_to_json(inverse: &InverseOp) -> Value {
    let mut payload = op_to_json(&inverse.op);
    if let Some(restoration) = &inverse.restore {
        payload["restore"] = json!({
            "spans": restoration.spans.iter().map(|span| {
                json!({
                    "key_path": span.key_path,
                    "expected": span.expected.clone().map(Value::String).unwrap_or(Value::Null),
                    "prior": span.prior.clone().map(Value::String).unwrap_or(Value::Null),
                })
            }).collect::<Vec<_>>(),
        });
    }
    payload
}

pub(crate) fn op_to_json(op: &Op) -> Value {
    match op {
        Op::CategoryAdd {
            label,
            group,
            hotkey,
            hidden,
            place,
            restore_key,
        } => {
            let mut payload = json!({"op": "category.add", "label": label});
            if let Some(group) = group {
                payload["group"] = json!(group);
            }
            if let Some(hotkey) = hotkey {
                payload["hotkey"] = json!(hotkey.to_string());
            }
            if let Some(hidden) = hidden {
                payload["hidden"] = json!(hidden);
            }
            match place {
                Some(Anchor::Before(key)) => payload["before"] = json!(key),
                Some(Anchor::After(key)) => payload["after"] = json!(key),
                None => {}
            }
            if let Some(key) = restore_key {
                payload["restore_key"] = json!(key);
            }
            payload
        }
        Op::CategorySet {
            key,
            label,
            group,
            hotkey,
            hidden,
            unset,
        } => {
            let mut payload = json!({"op": "category.set", "key": key});
            let mut set = serde_json::Map::new();
            if let Some(label) = label {
                set.insert("label".to_owned(), json!(label));
            }
            if let Some(group) = group {
                set.insert("group".to_owned(), json!(group));
            }
            if let Some(hotkey) = hotkey {
                set.insert("hotkey".to_owned(), json!(hotkey.to_string()));
            }
            if let Some(hidden) = hidden {
                set.insert("hidden".to_owned(), json!(hidden));
            }
            if !set.is_empty() {
                payload["set"] = Value::Object(set);
            }
            if !unset.is_empty() {
                payload["unset"] = json!(unset
                    .iter()
                    .map(|field| field.as_str())
                    .collect::<Vec<_>>());
            }
            payload
        }
        Op::CategoryReset { key } => {
            json!({"op": "category.reset", "key": key})
        }
        Op::CategoryMove { key, place } => {
            let mut payload = json!({"op": "category.move", "key": key});
            match place {
                Anchor::Before(target) => payload["before"] = json!(target),
                Anchor::After(target) => payload["after"] = json!(target),
            }
            payload
        }
        Op::CategoryRemove { key } => {
            json!({"op": "category.remove", "key": key})
        }
        Op::GroupRename { group, to } => {
            json!({"op": "group.rename", "group": group, "to": to})
        }
        Op::GroupMove { group, place } => {
            let mut payload = json!({"op": "group.move", "group": group});
            match place {
                Anchor::Before(target) => payload["before"] = json!(target),
                Anchor::After(target) => payload["after"] = json!(target),
            }
            payload
        }
    }
}

pub(crate) fn parse_live(markdown: &str) -> Result<BankConfig, OpError> {
    parse_ok(markdown)
}
