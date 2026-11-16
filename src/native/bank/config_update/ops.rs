//! Category and group ops: semantic apply, consequences, and inverses.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::protocol::{
    AccountUnsetField, Anchor, Op, Restoration, RestoreSpan as ProtocolSpan,
    RuleUnsetField, UnsetField,
};
use crate::native::albums::output::split_icon;
use crate::native::bank::config::{
    self, Account, AccountKind, BankConfig, Category, ConfigParse, Rule,
    HOTKEY_ORDER, RESERVED_HOTKEY,
};
use crate::native::bank::config_spans::{
    self, AccountDraft, BankContainer, CategoryDraft, Place, RestoreSpan,
    RuleDraft, SeqPlace,
};
use crate::native::bank::journal::JournalSnapshot;
use crate::native::bank::model;
use crate::native::bank::month::{self, LocatedRowKind};
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
    pub(crate) journal_events: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct InverseOp {
    pub(crate) op: Op,
    pub(crate) restore: Option<Restoration>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageIndex {
    rows: HashMap<String, usize>,
    account_rows: HashMap<String, usize>,
    default_rows: usize,
    account_journal_events: HashMap<String, usize>,
}

impl UsageIndex {
    pub(crate) fn from_scan(
        scan: &BankScan,
        config: &BankConfig,
        journal: &JournalSnapshot,
    ) -> Self {
        let rows = model::collect_rows(scan, Some(config), None);
        let mut row_counts = HashMap::new();
        let mut account_rows = HashMap::new();
        for row in &rows {
            *account_rows.entry(row.account.clone()).or_insert(0) += 1;
            if let Some(key) = &row.category {
                *row_counts.entry(key.clone()).or_insert(0) += 1;
            }
        }
        let default_rows = scan
            .months
            .iter()
            .flat_map(|month| {
                let note = month::parse_month_note(
                    &month.path,
                    &month.contents,
                    month.month,
                );
                note.days
                    .into_iter()
                    .flat_map(|day| day.rows)
                    .filter(|row| {
                        matches!(
                            &row.kind,
                            LocatedRowKind::Parsed(parsed)
                                if parsed.account.is_none()
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .count();
        let mut account_journal_events = HashMap::new();
        for event in &journal.imports {
            *account_journal_events
                .entry(event.account.clone())
                .or_insert(0) += 1;
        }
        for event in journal.transactions.values() {
            *account_journal_events
                .entry(event.account.clone())
                .or_insert(0) += 1;
        }
        Self {
            rows: row_counts,
            account_rows,
            default_rows,
            account_journal_events,
        }
    }

    fn rows(&self, key: &str) -> usize {
        self.rows.get(key).copied().unwrap_or(0)
    }

    fn account_rows(&self, alias: &str) -> usize {
        self.account_rows.get(alias).copied().unwrap_or(0)
    }

    fn account_journal_events(&self, alias: &str) -> usize {
        self.account_journal_events.get(alias).copied().unwrap_or(0)
    }

    fn default_rows(&self) -> usize {
        self.default_rows
    }
}

fn rule_refs(config: &BankConfig, key: &str) -> usize {
    config
        .rules
        .iter()
        .filter(|rule| rule.cat.as_deref() == Some(key))
        .count()
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
        Op::AccountAdd {
            alias,
            label,
            kind,
            start,
        } => add_account(
            markdown,
            &config,
            alias.as_deref(),
            label,
            *kind,
            start.as_deref(),
            index,
        ),
        Op::AccountSet {
            alias,
            label,
            rename_to,
            kind,
            start,
            unset,
        } => set_account(
            markdown,
            &config,
            usage,
            alias,
            label.as_deref(),
            rename_to.as_deref(),
            *kind,
            start.as_deref(),
            unset,
            index,
        ),
        Op::AccountRename { alias, to } => {
            refuse_account_rename(markdown, &config, usage, alias, to, index)
        }
        Op::AccountDefault { alias } => {
            refuse_account_default(markdown, &config, usage, alias, index)
        }
        Op::AccountMove { alias, place } => {
            refuse_account_move(markdown, &config, usage, alias, place, index)
        }
        Op::AccountRemove { alias } => {
            remove_account(markdown, &config, usage, alias, index)
        }
        Op::RuleAdd {
            match_text,
            payee,
            cat,
            link,
            position,
        } => add_rule(
            markdown,
            &config,
            match_text,
            payee.as_deref(),
            cat.as_deref(),
            link.as_deref(),
            *position,
            index,
        ),
        Op::RuleSet {
            index: rule_index,
            match_was,
            match_text,
            payee,
            cat,
            link,
            unset,
        } => set_rule(
            markdown,
            &config,
            *rule_index,
            match_was,
            match_text.as_deref(),
            payee.as_deref(),
            cat.as_deref(),
            link.as_deref(),
            unset,
            index,
        ),
        Op::RuleMove {
            index: rule_index,
            match_was,
            position,
        } => move_rule(
            markdown,
            &config,
            *rule_index,
            match_was,
            *position,
            index,
        ),
        Op::RuleRemove {
            index: rule_index,
            match_was,
        } => remove_rule(markdown, &config, *rule_index, match_was, index),
        Op::BankSet {
            stale_after,
            unset_stale_after,
        } => set_stale_after(
            markdown,
            &config,
            *stale_after,
            *unset_stale_after,
            index,
        ),
    }
}

fn validate_remove_usage(
    op: &Op,
    usage: &UsageIndex,
    config: &BankConfig,
    index: usize,
) -> Result<(), OpError> {
    match op {
        Op::CategoryRemove { key } => {
            let rows = usage.rows(key);
            let rules = rule_refs(config, key);
            if rows == 0 && rules == 0 {
                return Ok(());
            }
            let label = config
                .category(key)
                .map(full_label)
                .unwrap_or_else(|| key.to_owned());
            let mut parts = Vec::new();
            if rows > 0 {
                parts.push(format!(
                    "{rows} row{}",
                    if rows == 1 { "" } else { "s" }
                ));
            }
            if rules > 0 {
                parts.push(format!(
                    "{rules} rule{}",
                    if rules == 1 { "" } else { "s" }
                ));
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
        Op::AccountRemove { alias } => {
            let Some(account) = config.account(alias) else {
                return Ok(());
            };
            let rows = usage.account_rows(alias);
            let events = usage.account_journal_events(alias);
            if config
                .default_account()
                .is_some_and(|default| default.alias == account.alias)
            {
                return Err(account_error(
                    "undo_blocked",
                    format!("{alias} is now the default account · cannot undo"),
                    rows,
                    events,
                    index,
                    alias,
                ));
            }
            if rows == 0 && events == 0 {
                return Ok(());
            }
            Err(account_error(
                "undo_blocked",
                format!("{rows} rows and {events} journal events now use {alias} · cannot undo"),
                rows,
                events,
                index,
                alias,
            ))
        }
        _ => Ok(()),
    }
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
        Op::AccountAdd { alias, .. } => (alias.clone(), None),
        Op::AccountSet { alias, .. }
        | Op::AccountRename { alias, .. }
        | Op::AccountDefault { alias }
        | Op::AccountMove { alias, .. }
        | Op::AccountRemove { alias } => (Some(alias.clone()), None),
        Op::RuleAdd { .. }
        | Op::RuleSet { .. }
        | Op::RuleMove { .. }
        | Op::RuleRemove { .. }
        | Op::BankSet { .. }
        | Op::GroupRename { .. }
        | Op::GroupMove { .. } => (None, None),
    };
    let rows = key.as_deref().map(|key| usage.rows(key));
    let rule_refs = key.as_deref().map(|key| rule_refs(config, key));
    let journal_events = key.as_deref().and_then(|key| {
        config
            .account(key)
            .map(|_| usage.account_journal_events(key))
    });
    OpResult {
        op_index: index,
        op_name: op.name(),
        summary: format!("restores {}", op.name()),
        key,
        hotkey,
        rows,
        rule_refs,
        journal_events,
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
            journal_events: None,
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
    bank_container_missing(markdown, "categories")
}

fn bank_container_missing(markdown: &str, key: &str) -> bool {
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
        mapping.contains_key(serde_yaml::Value::String(key.to_owned()))
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
            rule_refs: Some(rule_refs(config, key)),
            journal_events: None,
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
                rule_refs: Some(rule_refs(config, key)),
                journal_events: None,
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
            rule_refs: Some(rule_refs(config, key)),
            journal_events: None,
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
            journal_events: None,
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
            journal_events: None,
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
    let rules = rule_refs(config, key);
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
            journal_events: None,
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
                journal_events: None,
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
            journal_events: None,
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

fn add_account(
    markdown: &str,
    config: &BankConfig,
    alias: Option<&str>,
    label: &str,
    kind: AccountKind,
    start: Option<&str>,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let alias = match alias {
        Some(alias) => alias.to_owned(),
        None => derive_account_alias(label).ok_or_else(|| {
            OpError::new(
                "account_alias_invalid",
                format!("could not derive an account alias from {label:?}"),
                Some("supply alias matching [a-z][a-z0-9_]*".to_owned()),
            )
            .at(index)
        })?,
    };
    if config.account(&alias).is_some() {
        return Err(OpError::new(
            "account_exists",
            format!("account alias {alias} already exists"),
            Some(
                "choose a unique alias or edit the existing account".to_owned(),
            ),
        )
        .at(index));
    }
    let draft = AccountDraft {
        alias: alias.clone(),
        label: Some(label.to_owned()),
        kind: kind.as_str().to_owned(),
        start: start.map(str::to_owned),
    };
    let edit = config_spans::insert_account(markdown, &draft, Place::End)
        .map_err(|error| span_error(&error).at(index))?;
    let start_text =
        start.map(|day| format!(", from {day}")).unwrap_or_default();
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "account.add",
            summary: format!(
                "adds {label} as {alias} ({}){start_text} · default account stays put",
                kind.as_str()
            ),
            key: Some(alias.clone()),
            hotkey: None,
            rows: Some(0),
            rule_refs: None,
            journal_events: Some(0),
        },
        inverse: InverseOp {
            op: Op::AccountRemove {
                alias: alias.clone(),
            },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn derive_account_alias(label: &str) -> Option<String> {
    let mut alias = String::new();
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            alias.push(character.to_ascii_lowercase());
        } else if !alias.is_empty() && !alias.ends_with('_') {
            alias.push('_');
        }
    }
    let alias = alias.trim_matches('_').to_owned();
    config::is_alias(&alias).then_some(alias)
}

fn set_account(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    alias: &str,
    label: Option<&str>,
    rename_to: Option<&str>,
    kind: Option<AccountKind>,
    start: Option<&str>,
    unset: &[AccountUnsetField],
    index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = config.account(alias).ok_or_else(|| {
        OpError::new(
            "unknown_account",
            format!("account {alias} does not exist"),
            None,
        )
        .at(index)
    })?;
    if unset.contains(&AccountUnsetField::Alias)
        || rename_to.is_some_and(|target| target != alias)
    {
        return Err(account_migration_error(
            "account_alias_immutable",
            format!("account alias {alias} cannot be renamed"),
            usage,
            alias,
            index,
            "account aliases identify existing rows and journal history; edit bank.md in YAML or wait for a migration flow",
        ));
    }
    let current_start =
        existing.start.map(|day| day.format("%Y-%m-%d").to_string());
    let changes_kind = kind.is_some_and(|next| next != existing.kind);
    let changes_start =
        start.is_some_and(|next| Some(next) != current_start.as_deref());
    if changes_kind
        || changes_start
        || unset.contains(&AccountUnsetField::Kind)
        || unset.contains(&AccountUnsetField::Start)
    {
        return Err(account_migration_error(
            "account_migration",
            format!("account {alias} kind or start cannot change without a row migration"),
            usage,
            alias,
            index,
            "edit bank.md in YAML or wait for a migration flow; existing rows keep their account attribution",
        ));
    }

    let prior_label = existing.label.clone();
    let prior_label_authored = existing.label_authored;
    let next_label = if unset.contains(&AccountUnsetField::Label) {
        None
    } else if let Some(label) = label {
        Some(label.to_owned())
    } else if prior_label_authored {
        Some(prior_label.clone())
    } else {
        None
    };
    let label_changes = if unset.contains(&AccountUnsetField::Label) {
        prior_label_authored
    } else {
        label.is_some_and(|label| !prior_label_authored || label != prior_label)
    };
    if !label_changes {
        return Ok(OneOutcome {
            contents: markdown.to_owned(),
            result: OpResult {
                op_index: index,
                op_name: "account.set",
                summary: format!("account {alias} is already current"),
                key: Some(alias.to_owned()),
                hotkey: None,
                rows: Some(usage.account_rows(alias)),
                rule_refs: None,
                journal_events: Some(usage.account_journal_events(alias)),
            },
            inverse: InverseOp {
                op: Op::AccountSet {
                    alias: alias.to_owned(),
                    label: label.map(str::to_owned),
                    rename_to: rename_to.map(str::to_owned),
                    kind,
                    start: start.map(str::to_owned),
                    unset: unset.to_vec(),
                },
                restore: None,
            },
        });
    }
    let edit =
        config_spans::set_account_label(markdown, alias, next_label.as_deref())
            .map_err(|error| span_error(&error).at(index))?;
    let rows = usage.account_rows(alias);
    let events = usage.account_journal_events(alias);
    let summary = match next_label.as_deref() {
        Some(next) if next != prior_label => format!(
            "renames account {prior_label} → {next} · {rows} rows and {events} journal events stay under {alias}"
        ),
        None if prior_label_authored => format!(
            "clears {alias}'s custom label · {rows} rows and {events} journal events keep the alias"
        ),
        _ => format!("keeps {alias} unchanged"),
    };
    let inverse = Op::AccountSet {
        alias: alias.to_owned(),
        label: prior_label_authored.then_some(prior_label),
        rename_to: None,
        kind: None,
        start: None,
        unset: if prior_label_authored {
            Vec::new()
        } else {
            vec![AccountUnsetField::Label]
        },
    };
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "account.set",
            summary,
            key: Some(alias.to_owned()),
            hotkey: None,
            rows: Some(rows),
            rule_refs: None,
            journal_events: Some(events),
        },
        inverse: InverseOp {
            op: inverse,
            restore: (!edit.spans.is_empty())
                .then(|| restoration_from(&edit.spans)),
        },
    })
}

fn refuse_account_rename(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    alias: &str,
    to: &str,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let account = config.account(alias).ok_or_else(|| {
        OpError::new(
            "unknown_account",
            format!("account {alias} does not exist"),
            None,
        )
        .at(index)
    })?;
    if alias == to {
        return Ok(account_noop_result(
            markdown,
            "account.rename",
            alias,
            account,
            usage,
            Op::AccountRename {
                alias: alias.to_owned(),
                to: to.to_owned(),
            },
            index,
        ));
    }
    Err(account_migration_error(
        "account_alias_immutable",
        format!("refused: {} rows and {} journal events are filed under {alias} · account aliases cannot be renamed", usage.account_rows(alias), usage.account_journal_events(alias)),
        usage,
        alias,
        index,
        "edit bank.md in YAML or wait for a migration flow that rewrites row and journal identities",
    ))
}

fn refuse_account_default(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    alias: &str,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let account = config.account(alias).ok_or_else(|| {
        OpError::new(
            "unknown_account",
            format!("account {alias} does not exist"),
            None,
        )
        .at(index)
    })?;
    if config
        .default_account()
        .is_some_and(|default| default.alias == alias)
    {
        return Ok(account_noop_result(
            markdown,
            "account.default",
            alias,
            account,
            usage,
            Op::AccountDefault {
                alias: alias.to_owned(),
            },
            index,
        ));
    }
    let default = config
        .default_account()
        .map(|account| account.alias.as_str())
        .unwrap_or("?");
    Err(account_order_error(
        config,
        usage,
        index,
        format!("refused: changing the default from {default} to {alias} can reattribute rows without [acct:: alias]"),
    ))
}

fn refuse_account_move(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    alias: &str,
    place: &Anchor,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let account = config.account(alias).ok_or_else(|| {
        OpError::new(
            "unknown_account",
            format!("account {alias} does not exist"),
            None,
        )
        .at(index)
    })?;
    let target = match place {
        Anchor::Before(target) | Anchor::After(target) => target,
    };
    if config.account(target).is_none() {
        return Err(OpError::new(
            "unknown_account",
            format!("account position anchor {target} does not exist"),
            None,
        )
        .at(index));
    }
    if target == alias {
        return Ok(account_noop_result(
            markdown,
            "account.move",
            alias,
            account,
            usage,
            Op::AccountMove {
                alias: alias.to_owned(),
                place: place.clone(),
            },
            index,
        ));
    }
    Err(account_order_error(
        config,
        usage,
        index,
        format!("refused: account order cannot change around {alias}; the first account determines rows without [acct:: alias]"),
    ))
}

fn account_order_error(
    config: &BankConfig,
    usage: &UsageIndex,
    index: usize,
    message: String,
) -> OpError {
    let default = config
        .default_account()
        .map(|account| account.alias.as_str())
        .unwrap_or("");
    let rows = usage.default_rows();
    let events = usage.account_journal_events(default);
    account_error(
        "account_default",
        format!("{message} · {rows} rows without [acct:: alias] currently inherit {default}; {events} journal events remain keyed to it"),
        rows,
        events,
        index,
        default,
    )
    .with_hint("account order and default changes need a row migration; edit bank.md in YAML or wait for that migration flow")
}

fn account_migration_error(
    code: &'static str,
    message: String,
    usage: &UsageIndex,
    alias: &str,
    index: usize,
    hint: &str,
) -> OpError {
    let rows = usage.account_rows(alias);
    let events = usage.account_journal_events(alias);
    account_error(code, message, rows, events, index, alias).with_hint(hint)
}

fn account_error(
    code: &'static str,
    message: String,
    rows: usize,
    events: usize,
    index: usize,
    alias: &str,
) -> OpError {
    let mut error = OpError::new(code, message, None);
    error.op_index = Some(index);
    error.key_path = Some(format!("bank.accounts.{alias}"));
    error.affected = Some(rows + events);
    error
}

impl OpError {
    fn with_hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.to_owned());
        self
    }
}

fn account_noop_result(
    markdown: &str,
    op_name: &'static str,
    alias: &str,
    account: &Account,
    usage: &UsageIndex,
    inverse: Op,
    index: usize,
) -> OneOutcome {
    let rows = usage.account_rows(alias);
    let events = usage.account_journal_events(alias);
    OneOutcome {
        contents: markdown.to_owned(),
        result: OpResult {
            op_index: index,
            op_name,
            summary: format!("{alias} is already in the requested state"),
            key: Some(account.alias.clone()),
            hotkey: None,
            rows: Some(rows),
            rule_refs: None,
            journal_events: Some(events),
        },
        inverse: InverseOp {
            op: inverse,
            restore: None,
        },
    }
}

fn remove_account(
    markdown: &str,
    config: &BankConfig,
    usage: &UsageIndex,
    alias: &str,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let account = config.account(alias).ok_or_else(|| {
        OpError::new(
            "unknown_account",
            format!("account {alias} does not exist"),
            None,
        )
        .at(index)
    })?;
    let rows = usage.account_rows(alias);
    let events = usage.account_journal_events(alias);
    if config
        .default_account()
        .is_some_and(|default| default.alias == alias)
    {
        return Err(account_error(
            "account_default",
            format!("{alias} is the default account · the first account cannot be removed"),
            rows,
            events,
            index,
            alias,
        )
        .with_hint("keep the first account, or wait for a migration flow that reassigns rows and journal history"));
    }
    if rows > 0 || events > 0 {
        return Err(account_error(
            "account_in_use",
            format!("{rows} rows and {events} journal events are filed under {alias} · account is in use"),
            rows,
            events,
            index,
            alias,
        )
        .with_hint("edit bank.md in YAML or wait until the account has no rows or journal events"));
    }
    let edit = config_spans::remove_account(markdown, alias)
        .map_err(|error| span_error(&error).at(index))?;
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "account.remove",
            summary: format!(
                "removes {} · 0 rows · 0 journal events",
                account.label
            ),
            key: Some(alias.to_owned()),
            hotkey: None,
            rows: Some(0),
            rule_refs: None,
            journal_events: Some(0),
        },
        inverse: InverseOp {
            op: Op::AccountAdd {
                alias: Some(alias.to_owned()),
                label: account.label.clone(),
                kind: account.kind,
                start: account
                    .start
                    .map(|day| day.format("%Y-%m-%d").to_string()),
            },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn add_rule(
    markdown: &str,
    config: &BankConfig,
    match_text: &str,
    payee: Option<&str>,
    cat: Option<&str>,
    link: Option<&str>,
    position: Option<usize>,
    index: usize,
) -> Result<OneOutcome, OpError> {
    validate_rule_category(config, cat, index)?;
    let position = position.unwrap_or(config.rules.len());
    if position > config.rules.len() {
        return Err(rule_position_error(position, config.rules.len(), index));
    }
    let draft = RuleDraft {
        match_text: match_text.to_owned(),
        payee: payee.map(str::to_owned),
        cat: cat.map(str::to_owned),
        link: link.map(str::to_owned),
    };
    let mut text = markdown.to_owned();
    let mut spans = Vec::new();
    if bank_container_missing(&text, "rules") {
        let ensured =
            config_spans::ensure_container(&text, BankContainer::Rules)
                .map_err(|error| span_error(&error).at(index))?;
        text = ensured.contents;
        spans.extend(ensured.spans);
    }
    let place = if position == config.rules.len() {
        SeqPlace::End
    } else {
        SeqPlace::Before(position)
    };
    let edit = config_spans::insert_rule(&text, &draft, place)
        .map_err(|error| span_error(&error).at(index))?;
    spans.extend(edit.spans);
    let inverse = Op::RuleRemove {
        index: position,
        match_was: match_text.to_owned(),
    };
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "rule.add",
            summary: rule_effect_summary(format!(
                "adds rule {} {:?}",
                position + 1,
                match_text
            )),
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
            journal_events: None,
        },
        inverse: InverseOp {
            op: inverse,
            restore: Some(restoration_from(&spans)),
        },
    })
}

fn set_rule(
    markdown: &str,
    config: &BankConfig,
    index_in_rules: usize,
    match_was: &str,
    match_text: Option<&str>,
    payee: Option<&str>,
    cat: Option<&str>,
    link: Option<&str>,
    unset: &[RuleUnsetField],
    op_index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = guarded_rule(config, index_in_rules, match_was, op_index)?;
    validate_rule_category(config, cat, op_index)?;
    let prior = existing.clone();
    let mut next = prior.clone();
    if let Some(value) = match_text {
        next.match_text = value.to_owned();
    }
    if let Some(value) = payee {
        next.payee = Some(value.to_owned());
    }
    if let Some(value) = cat {
        next.cat = Some(value.to_owned());
    }
    if let Some(value) = link {
        next.link = Some(value.to_owned());
    }
    for field in unset {
        match field {
            RuleUnsetField::Payee => next.payee = None,
            RuleUnsetField::Cat => next.cat = None,
            RuleUnsetField::Link => next.link = None,
        }
    }
    if next == prior {
        return Ok(OneOutcome {
            contents: markdown.to_owned(),
            result: OpResult {
                op_index,
                op_name: "rule.set",
                summary: format!(
                    "rule {} {:?} is already current",
                    index_in_rules + 1,
                    prior.match_text
                ),
                key: None,
                hotkey: None,
                rows: None,
                rule_refs: None,
                journal_events: None,
            },
            inverse: InverseOp {
                op: Op::RuleSet {
                    index: index_in_rules,
                    match_was: match_was.to_owned(),
                    match_text: match_text.map(str::to_owned),
                    payee: payee.map(str::to_owned),
                    cat: cat.map(str::to_owned),
                    link: link.map(str::to_owned),
                    unset: unset.to_vec(),
                },
                restore: None,
            },
        });
    }
    let draft = RuleDraft {
        match_text: next.match_text.clone(),
        payee: next.payee.clone(),
        cat: next.cat.clone(),
        link: next.link.clone(),
    };
    let edit = config_spans::replace_rule(markdown, index_in_rules, &draft)
        .map_err(|error| span_error(&error).at(op_index))?;
    let mut inverse_unset = Vec::new();
    if prior.payee.is_none() && next.payee.is_some() {
        inverse_unset.push(RuleUnsetField::Payee);
    }
    if prior.cat.is_none() && next.cat.is_some() {
        inverse_unset.push(RuleUnsetField::Cat);
    }
    if prior.link.is_none() && next.link.is_some() {
        inverse_unset.push(RuleUnsetField::Link);
    }
    let inverse = Op::RuleSet {
        index: index_in_rules,
        match_was: next.match_text.clone(),
        match_text: (prior.match_text != next.match_text)
            .then(|| prior.match_text.clone()),
        payee: (prior.payee != next.payee)
            .then(|| prior.payee.clone())
            .flatten(),
        cat: (prior.cat != next.cat).then(|| prior.cat.clone()).flatten(),
        link: (prior.link != next.link)
            .then(|| prior.link.clone())
            .flatten(),
        unset: inverse_unset,
    };
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index,
            op_name: "rule.set",
            summary: rule_effect_summary(format!(
                "updates rule {} {:?}",
                index_in_rules + 1,
                next.match_text
            )),
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
            journal_events: None,
        },
        inverse: InverseOp {
            op: inverse,
            restore: (!edit.spans.is_empty())
                .then(|| restoration_from(&edit.spans)),
        },
    })
}

fn move_rule(
    markdown: &str,
    config: &BankConfig,
    index_in_rules: usize,
    match_was: &str,
    position: usize,
    op_index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = guarded_rule(config, index_in_rules, match_was, op_index)?;
    if position >= config.rules.len() {
        return Err(rule_position_error(
            position,
            config.rules.len().saturating_sub(1),
            op_index,
        ));
    }
    if position == index_in_rules {
        return Ok(rule_noop_result(
            markdown,
            "rule.move",
            index_in_rules,
            match_was,
            op_index,
        ));
    }
    let place = if position < index_in_rules {
        SeqPlace::Before(position)
    } else {
        SeqPlace::After(position)
    };
    let edit = config_spans::move_rule(markdown, index_in_rules, place)
        .map_err(|error| span_error(&error).at(op_index))?;
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index,
            op_name: "rule.move",
            summary: rule_effect_summary(format!(
                "moves rule {:?} to position {}",
                existing.match_text,
                position + 1
            )),
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
            journal_events: None,
        },
        inverse: InverseOp {
            op: Op::RuleMove {
                index: position,
                match_was: match_was.to_owned(),
                position: index_in_rules,
            },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn remove_rule(
    markdown: &str,
    config: &BankConfig,
    index_in_rules: usize,
    match_was: &str,
    op_index: usize,
) -> Result<OneOutcome, OpError> {
    let existing = guarded_rule(config, index_in_rules, match_was, op_index)?;
    let edit = config_spans::remove_rule(markdown, index_in_rules)
        .map_err(|error| span_error(&error).at(op_index))?;
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: op_index,
            op_name: "rule.remove",
            summary: rule_effect_summary(format!(
                "removes rule {} {:?}",
                index_in_rules + 1,
                existing.match_text
            )),
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
            journal_events: None,
        },
        inverse: InverseOp {
            op: Op::RuleAdd {
                match_text: existing.match_text.clone(),
                payee: existing.payee.clone(),
                cat: existing.cat.clone(),
                link: existing.link.clone(),
                position: Some(index_in_rules),
            },
            restore: Some(restoration_from(&edit.spans)),
        },
    })
}

fn guarded_rule<'a>(
    config: &'a BankConfig,
    index_in_rules: usize,
    match_was: &str,
    op_index: usize,
) -> Result<&'a Rule, OpError> {
    let rule = config.rules.get(index_in_rules).ok_or_else(|| {
        rule_mismatch_error(index_in_rules, match_was, op_index)
    })?;
    if rule.match_text != match_was {
        return Err(rule_mismatch_error(index_in_rules, match_was, op_index));
    }
    Ok(rule)
}

fn rule_mismatch_error(
    index_in_rules: usize,
    match_was: &str,
    index: usize,
) -> OpError {
    let mut error = OpError::new(
        "rule_mismatch",
        format!(
            "rule {} no longer matches {:?}",
            index_in_rules + 1,
            match_was
        ),
        Some(
            "refresh the config report and preview the rule edit again"
                .to_owned(),
        ),
    );
    error.op_index = Some(index);
    error.key_path = Some(format!("bank.rules.{index_in_rules}"));
    error
}

fn validate_rule_category(
    config: &BankConfig,
    cat: Option<&str>,
    index: usize,
) -> Result<(), OpError> {
    if let Some(cat) = cat
        && config.category(cat).is_none()
    {
        let mut error = OpError::new(
            "unknown_category",
            format!("rule category {cat:?} does not exist"),
            Some("choose a category from the effective bank setup".to_owned()),
        );
        error.op_index = Some(index);
        error.key_path = Some(format!("bank.categories.{cat}"));
        return Err(error);
    }
    Ok(())
}

fn rule_position_error(position: usize, max: usize, index: usize) -> OpError {
    let mut error = OpError::new(
        "rule_position",
        format!("rule position {position} is out of range (0..={max})"),
        None,
    );
    error.op_index = Some(index);
    error.key_path = Some("bank.rules".to_owned());
    error
}

fn rule_effect_summary(summary: String) -> String {
    format!("{summary} · future imports and hints; filed rows do not change")
}

fn rule_noop_result(
    markdown: &str,
    op_name: &'static str,
    rule_index: usize,
    match_text: &str,
    op_index: usize,
) -> OneOutcome {
    OneOutcome {
        contents: markdown.to_owned(),
        result: OpResult {
            op_index,
            op_name,
            summary: format!(
                "rule {} {:?} is already in that position",
                rule_index + 1,
                match_text
            ),
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
            journal_events: None,
        },
        inverse: InverseOp {
            op: Op::RuleMove {
                index: rule_index,
                match_was: match_text.to_owned(),
                position: rule_index,
            },
            restore: None,
        },
    }
}

fn set_stale_after(
    markdown: &str,
    config: &BankConfig,
    stale_after: Option<u32>,
    unset: bool,
    index: usize,
) -> Result<OneOutcome, OpError> {
    let days = if unset { None } else { stale_after };
    let edit = config_spans::set_stale_after(markdown, days)
        .map_err(|error| span_error(&error).at(index))?;
    let summary = if unset {
        "restores the 10-day balance freshness default".to_owned()
    } else {
        format!(
            "sets balances stale after {} day{}",
            stale_after.unwrap_or(config.stale_after),
            if stale_after.unwrap_or(config.stale_after) == 1 {
                ""
            } else {
                "s"
            }
        )
    };
    let inverse = Op::BankSet {
        stale_after: config.stale_after_authored.then_some(config.stale_after),
        unset_stale_after: !config.stale_after_authored,
    };
    Ok(OneOutcome {
        contents: edit.contents,
        result: OpResult {
            op_index: index,
            op_name: "bank.set",
            summary,
            key: None,
            hotkey: None,
            rows: None,
            rule_refs: None,
            journal_events: None,
        },
        inverse: InverseOp {
            op: inverse,
            restore: (!edit.spans.is_empty())
                .then(|| restoration_from(&edit.spans)),
        },
    })
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
        Op::AccountAdd {
            alias,
            label,
            kind,
            start,
        } => {
            let mut payload = json!({
                "op": "account.add",
                "label": label,
                "kind": kind.as_str(),
            });
            if let Some(alias) = alias {
                payload["alias"] = json!(alias);
            }
            if let Some(start) = start {
                payload["start"] = json!(start);
            }
            payload
        }
        Op::AccountSet {
            alias,
            label,
            rename_to,
            kind,
            start,
            unset,
        } => {
            let mut payload = json!({"op": "account.set", "alias": alias});
            let mut set = serde_json::Map::new();
            if let Some(label) = label {
                set.insert("label".to_owned(), json!(label));
            }
            if let Some(rename_to) = rename_to {
                set.insert("alias".to_owned(), json!(rename_to));
            }
            if let Some(kind) = kind {
                set.insert("kind".to_owned(), json!(kind.as_str()));
            }
            if let Some(start) = start {
                set.insert("start".to_owned(), json!(start));
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
        Op::AccountRename { alias, to } => {
            json!({"op": "account.rename", "alias": alias, "to": to})
        }
        Op::AccountDefault { alias } => {
            json!({"op": "account.default", "alias": alias})
        }
        Op::AccountMove { alias, place } => {
            let mut payload = json!({"op": "account.move", "alias": alias});
            match place {
                Anchor::Before(target) => payload["before"] = json!(target),
                Anchor::After(target) => payload["after"] = json!(target),
            }
            payload
        }
        Op::AccountRemove { alias } => {
            json!({"op": "account.remove", "alias": alias})
        }
        Op::RuleAdd {
            match_text,
            payee,
            cat,
            link,
            position,
        } => {
            let mut payload = json!({"op": "rule.add", "match": match_text});
            if let Some(payee) = payee {
                payload["payee"] = json!(payee);
            }
            if let Some(cat) = cat {
                payload["cat"] = json!(cat);
            }
            if let Some(link) = link {
                payload["link"] = json!(link);
            }
            if let Some(position) = position {
                payload["position"] = json!(position);
            }
            payload
        }
        Op::RuleSet {
            index,
            match_was,
            match_text,
            payee,
            cat,
            link,
            unset,
        } => {
            let mut payload = json!({"op": "rule.set", "index": index, "match_was": match_was});
            let mut set = serde_json::Map::new();
            if let Some(match_text) = match_text {
                set.insert("match".to_owned(), json!(match_text));
            }
            if let Some(payee) = payee {
                set.insert("payee".to_owned(), json!(payee));
            }
            if let Some(cat) = cat {
                set.insert("cat".to_owned(), json!(cat));
            }
            if let Some(link) = link {
                set.insert("link".to_owned(), json!(link));
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
        Op::RuleMove {
            index,
            match_was,
            position,
        } => {
            json!({"op": "rule.move", "index": index, "match_was": match_was, "position": position})
        }
        Op::RuleRemove { index, match_was } => {
            json!({"op": "rule.remove", "index": index, "match_was": match_was})
        }
        Op::BankSet {
            stale_after,
            unset_stale_after,
        } => {
            if *unset_stale_after {
                json!({"op": "bank.set", "unset": ["stale_after"]})
            } else {
                json!({"op": "bank.set", "set": {"stale_after": stale_after}})
            }
        }
    }
}

pub(crate) fn parse_live(markdown: &str) -> Result<BankConfig, OpError> {
    parse_ok(markdown)
}
