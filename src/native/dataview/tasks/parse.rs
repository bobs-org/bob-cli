use std::{collections::BTreeMap, fs, path::Path};

use regex::Regex;
use serde::Serialize;

use super::{super::DataviewError, settings::TasksSettings, task::TaskFile};

const MAX_EXPANSION_DEPTH: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueryAst {
    pub(super) statements: Vec<Statement>,
    pub(super) filters: Vec<FilterExpr>,
    pub(super) sorting: Vec<SortInstruction>,
    pub(super) grouping: Vec<GroupInstruction>,
    pub(super) limit: Option<usize>,
    pub(super) limit_groups: Option<usize>,
    pub(super) layout: LayoutOptions,
    pub(super) explain: bool,
    pub(super) ignore_global_query: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Statement {
    pub(super) source: StatementSource,
    pub(super) instruction: String,
    pub(super) parsed: Instruction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum StatementSource {
    GlobalQuery,
    QueryFileDefaults,
    Query,
    Preset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum Instruction {
    Comment,
    Filter { expression: FilterExpr },
    Sort { instruction: SortInstruction },
    Group { instruction: GroupInstruction },
    Limit { count: usize },
    LimitGroups { count: usize },
    Layout { option: String, shown: bool },
    Mode { short: bool },
    Explain,
    IgnoreGlobalQuery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum FilterExpr {
    And {
        left: Box<Self>,
        right: Box<Self>,
    },
    Or {
        left: Box<Self>,
        right: Box<Self>,
    },
    Xor {
        left: Box<Self>,
        right: Box<Self>,
    },
    Not {
        expression: Box<Self>,
    },
    Done {
        done: bool,
    },
    StatusType {
        negated: bool,
        value: String,
    },
    Text {
        field: TextField,
        operator: TextOperator,
        value: String,
        regex_flags: Option<String>,
    },
    Date {
        field: DateField,
        relation: DateRelation,
        value: String,
    },
    DatePresence {
        field: DateField,
        presence: Presence,
    },
    Priority {
        relation: PriorityRelation,
        value: String,
    },
    Recurring {
        recurring: bool,
    },
    Presence {
        field: PresenceField,
        present: bool,
    },
    Blocked {
        blocked: bool,
    },
    Blocking {
        blocking: bool,
    },
    Function {
        source: String,
    },
    ExcludeSubItems,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum TextField {
    Description,
    Heading,
    Path,
    Folder,
    Filename,
    Root,
    Backlink,
    Tag,
    Recurrence,
    Id,
    StatusName,
    StatusSymbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum TextOperator {
    Is,
    IsNot,
    Includes,
    DoesNotInclude,
    RegexMatches,
    RegexDoesNotMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum DateField {
    Due,
    Scheduled,
    Start,
    Created,
    Done,
    Cancelled,
    Happens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum DateRelation {
    In,
    On,
    Before,
    After,
    OnOrBefore,
    OnOrAfter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Presence {
    Has,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum PresenceField {
    Id,
    DependsOn,
    Tag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum PriorityRelation {
    Is,
    IsNot,
    Above,
    Below,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SortInstruction {
    pub(super) key: SortKey,
    pub(super) reverse: bool,
    pub(super) function: Option<String>,
    pub(super) tag_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum SortKey {
    Cancelled,
    Created,
    Description,
    Done,
    Due,
    Filename,
    Function,
    Happens,
    Heading,
    Id,
    Path,
    Priority,
    Random,
    Recurring,
    Scheduled,
    Start,
    Status,
    StatusName,
    StatusType,
    Tag,
    Urgency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupInstruction {
    pub(super) key: GroupKey,
    pub(super) reverse: bool,
    pub(super) function: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum GroupKey {
    Backlink,
    Cancelled,
    Created,
    Done,
    Due,
    Filename,
    Folder,
    Function,
    Happens,
    Heading,
    Id,
    Path,
    Priority,
    Recurrence,
    Recurring,
    Root,
    Scheduled,
    Start,
    Status,
    StatusName,
    StatusType,
    Tags,
    Urgency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LayoutOptions {
    pub(super) short_mode: bool,
    pub(super) show_toolbar: bool,
    pub(super) show_postpone_button: bool,
    pub(super) show_task_count: bool,
    pub(super) show_backlink: bool,
    pub(super) show_edit_button: bool,
    pub(super) show_urgency: bool,
    pub(super) show_tree: bool,
    pub(super) show_tags: bool,
    pub(super) show_id: bool,
    pub(super) show_depends_on: bool,
    pub(super) show_priority: bool,
    pub(super) show_recurrence_rule: bool,
    pub(super) show_on_completion: bool,
    pub(super) show_created_date: bool,
    pub(super) show_start_date: bool,
    pub(super) show_scheduled_date: bool,
    pub(super) show_due_date: bool,
    pub(super) show_cancelled_date: bool,
    pub(super) show_done_date: bool,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            short_mode: false,
            show_toolbar: true,
            show_postpone_button: true,
            show_task_count: true,
            show_backlink: true,
            show_edit_button: true,
            show_urgency: false,
            show_tree: false,
            show_tags: true,
            show_id: true,
            show_depends_on: true,
            show_priority: true,
            show_recurrence_rule: true,
            show_on_completion: true,
            show_created_date: true,
            show_start_date: true,
            show_scheduled_date: true,
            show_due_date: true,
            show_cancelled_date: true,
            show_done_date: true,
        }
    }
}

#[derive(Debug, Clone)]
struct QueryContext {
    file: TaskFile,
    properties: BTreeMap<String, serde_yaml::Value>,
}

pub(super) fn parse(
    vault: &Path,
    origin: Option<&Path>,
    source: &str,
    settings: &TasksSettings,
) -> Result<QueryAst, DataviewError> {
    let context = QueryContext::read(vault, origin)?;
    let defaults = query_file_defaults(context.as_ref());
    let parser = Parser {
        settings,
        context: context.as_ref(),
    };

    let mut defaults_and_query = QueryAst::default();
    parser.parse_source(
        &defaults,
        StatementSource::QueryFileDefaults,
        &mut defaults_and_query,
    )?;
    parser.parse_source(
        source,
        StatementSource::Query,
        &mut defaults_and_query,
    )?;

    let mut result = QueryAst::default();
    if !defaults_and_query.ignore_global_query {
        parser.parse_source(
            &settings.global_query,
            StatementSource::GlobalQuery,
            &mut result,
        )?;
        result.ignore_global_query = false;
    }
    result.append(defaults_and_query);
    Ok(result)
}

impl QueryAst {
    fn append(&mut self, other: Self) {
        for statement in other.statements {
            self.apply_statement(statement);
        }
    }

    fn apply_statement(&mut self, statement: Statement) {
        match &statement.parsed {
            Instruction::Comment => {}
            Instruction::Filter { expression } => {
                self.filters.push(expression.clone());
            }
            Instruction::Sort { instruction } => {
                self.sorting.push(instruction.clone());
            }
            Instruction::Group { instruction } => {
                self.grouping.push(instruction.clone());
            }
            Instruction::Limit { count } => self.limit = Some(*count),
            Instruction::LimitGroups { count } => {
                self.limit_groups = Some(*count);
            }
            Instruction::Layout { option, shown } => {
                self.layout.set(option, *shown);
            }
            Instruction::Mode { short } => self.layout.short_mode = *short,
            Instruction::Explain => self.explain = true,
            Instruction::IgnoreGlobalQuery => {
                self.ignore_global_query = true;
            }
        }
        self.statements.push(statement);
    }
}

impl LayoutOptions {
    fn set(&mut self, option: &str, shown: bool) {
        match option {
            "toolbar" => self.show_toolbar = shown,
            "postpone button" => self.show_postpone_button = shown,
            "task count" => self.show_task_count = shown,
            "backlink" => self.show_backlink = shown,
            "edit button" => self.show_edit_button = shown,
            "urgency" => self.show_urgency = shown,
            "tree" => self.show_tree = shown,
            "tags" => self.show_tags = shown,
            "id" => self.show_id = shown,
            "depends on" => self.show_depends_on = shown,
            "priority" => self.show_priority = shown,
            "recurrence rule" => self.show_recurrence_rule = shown,
            "on completion" => self.show_on_completion = shown,
            "created date" => self.show_created_date = shown,
            "start date" => self.show_start_date = shown,
            "scheduled date" => self.show_scheduled_date = shown,
            "due date" => self.show_due_date = shown,
            "cancelled date" => self.show_cancelled_date = shown,
            "done date" => self.show_done_date = shown,
            _ => unreachable!("validated layout option: {option}"),
        }
    }
}

impl QueryContext {
    fn read(
        vault: &Path,
        origin: Option<&Path>,
    ) -> Result<Option<Self>, DataviewError> {
        let Some(origin) = origin else {
            return Ok(None);
        };
        let path = vault.join(origin);
        let contents = fs::read_to_string(&path).map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.clone(),
                error,
            }
        })?;
        let properties = parse_frontmatter(&contents, &path)?;
        Ok(Some(Self {
            file: TaskFile::new(origin.to_string_lossy().replace('\\', "/")),
            properties,
        }))
    }

    fn property(&self, name: &str) -> Option<&serde_yaml::Value> {
        self.properties
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
            .filter(|value| !value.is_null())
    }

    fn has_property(&self, name: &str) -> bool {
        self.property(name).is_some()
    }
}

fn parse_frontmatter(
    contents: &str,
    path: &Path,
) -> Result<BTreeMap<String, serde_yaml::Value>, DataviewError> {
    let frontmatter = frontmatter_block(contents).unwrap_or("");
    if frontmatter.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(frontmatter).map_err(|error| {
            DataviewError::TasksQuery {
                message: format!(
                    "failed to parse query-file defaults in {}: {error}",
                    path.display()
                ),
            }
        })?;
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Ok(BTreeMap::new());
    };
    Ok(mapping
        .into_iter()
        .filter_map(|(key, value)| {
            key.as_str().map(|key| (key.to_string(), value))
        })
        .collect())
}

fn frontmatter_block(contents: &str) -> Option<&str> {
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
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Some(&rest[..offset]);
        }
        offset += line.len();
    }
    None
}

fn query_file_defaults(context: Option<&QueryContext>) -> String {
    let Some(context) = context else {
        return String::new();
    };
    let mut instructions = Vec::<String>::new();
    if let Some(value) = context.property("TQ_show_toolbar") {
        instructions.push(if yaml_truthy(value) {
            "show toolbar".to_string()
        } else {
            "hide toolbar".to_string()
        });
    }
    for (property, on, off) in [
        ("TQ_explain", "explain", ""),
        ("TQ_short_mode", "short mode", "full mode"),
    ] {
        if let Some(value) = context.property(property) {
            let instruction = if yaml_truthy(value) { on } else { off };
            if !instruction.is_empty() {
                instructions.push(instruction.to_string());
            }
        }
    }
    for (property, option) in [
        ("TQ_show_tree", "tree"),
        ("TQ_show_tags", "tags"),
        ("TQ_show_id", "id"),
        ("TQ_show_depends_on", "depends on"),
        ("TQ_show_priority", "priority"),
        ("TQ_show_recurrence_rule", "recurrence rule"),
        ("TQ_show_on_completion", "on completion"),
        ("TQ_show_created_date", "created date"),
        ("TQ_show_start_date", "start date"),
        ("TQ_show_scheduled_date", "scheduled date"),
        ("TQ_show_due_date", "due date"),
        ("TQ_show_cancelled_date", "cancelled date"),
        ("TQ_show_done_date", "done date"),
        ("TQ_show_urgency", "urgency"),
        ("TQ_show_backlink", "backlink"),
        ("TQ_show_edit_button", "edit button"),
        ("TQ_show_postpone_button", "postpone button"),
        ("TQ_show_task_count", "task count"),
    ] {
        if let Some(value) = context.property(property) {
            instructions.push(if yaml_truthy(value) {
                format!("show {option}")
            } else {
                format!("hide {option}")
            });
        }
    }
    let mut result = instructions;
    if let Some(extra) = context
        .property("TQ_extra_instructions")
        .and_then(serde_yaml::Value::as_str)
    {
        result.push(extra.to_string());
    }
    result.join("\n")
}

fn yaml_truthy(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Null => false,
        serde_yaml::Value::Bool(value) => *value,
        serde_yaml::Value::Number(value) => value.as_f64() != Some(0.0),
        serde_yaml::Value::String(value) => !value.is_empty(),
        serde_yaml::Value::Sequence(value) => !value.is_empty(),
        serde_yaml::Value::Mapping(value) => !value.is_empty(),
        serde_yaml::Value::Tagged(value) => yaml_truthy(&value.value),
    }
}

struct Parser<'a> {
    settings: &'a TasksSettings,
    context: Option<&'a QueryContext>,
}

impl Parser<'_> {
    fn parse_source(
        &self,
        source: &str,
        statement_source: StatementSource,
        query: &mut QueryAst,
    ) -> Result<(), DataviewError> {
        let mut preset_stack = Vec::new();
        self.parse_source_inner(
            source,
            statement_source,
            query,
            &mut preset_stack,
            0,
        )
    }

    fn parse_source_inner(
        &self,
        source: &str,
        statement_source: StatementSource,
        query: &mut QueryAst,
        preset_stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<(), DataviewError> {
        if depth > MAX_EXPANSION_DEPTH {
            return Err(query_error(
                "preset or placeholder expansion is nested too deeply",
            ));
        }
        for raw in continue_lines(source) {
            if raw.trim_start().starts_with('#') {
                query.apply_statement(Statement {
                    source: statement_source,
                    instruction: raw,
                    parsed: Instruction::Comment,
                });
                continue;
            }

            for line in self.expand_statement(&raw)? {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.starts_with('#') {
                    query.apply_statement(Statement {
                        source: statement_source,
                        instruction: trimmed.to_string(),
                        parsed: Instruction::Comment,
                    });
                    continue;
                }

                if let Some(name) = strip_prefix_ci(trimmed, "preset ") {
                    let name = name.trim();
                    let Some(preset) = self.settings.presets.get(name) else {
                        return Err(query_error(&unknown_preset_message(
                            name,
                            &self.settings.presets,
                        )));
                    };
                    if preset_stack.iter().any(|active| active == name) {
                        let mut cycle = preset_stack.clone();
                        cycle.push(name.to_string());
                        return Err(query_error(&format!(
                            "preset expansion cycle: {}",
                            cycle.join(" -> ")
                        )));
                    }
                    preset_stack.push(name.to_string());
                    self.parse_source_inner(
                        preset,
                        StatementSource::Preset,
                        query,
                        preset_stack,
                        depth + 1,
                    )?;
                    preset_stack.pop();
                    continue;
                }

                let parsed = parse_instruction(trimmed).map_err(|message| {
                    query_error(&format!(
                        "{message}\nProblem line: \"{trimmed}\""
                    ))
                })?;
                query.apply_statement(Statement {
                    source: statement_source,
                    instruction: trimmed.to_string(),
                    parsed,
                });
            }
        }
        Ok(())
    }

    fn expand_statement(
        &self,
        source: &str,
    ) -> Result<Vec<String>, DataviewError> {
        if !source.contains("{{") || !source.contains("}}") {
            return Ok(vec![source.trim().to_string()]);
        }
        let Some(context) = self.context else {
            return Err(query_error(&format!(
                "The query looks like it contains a placeholder, with \"{{{{\" and \"}}}}\"\n\
                 but no file path has been supplied, so cannot expand placeholder values.\n\
                 The query is:\n{source}"
            )));
        };

        let placeholder = Regex::new(r"\{\{(.*?)\}\}")
            .expect("valid placeholder regular expression");
        let mut expanded = source.to_string();
        for _ in 0..10 {
            let previous = expanded.clone();
            let mut error = None;
            expanded = placeholder
                .replace_all(&previous, |captures: &regex::Captures<'_>| {
                    let expression = captures.get(1).unwrap().as_str().trim();
                    match self.resolve_placeholder(expression, context) {
                        Ok(value) => value,
                        Err(message) => {
                            error = Some(message);
                            captures.get(0).unwrap().as_str().to_string()
                        }
                    }
                })
                .into_owned();
            if let Some(message) = error {
                return Err(query_error(&format!(
                    "There was an error expanding one or more placeholders.\n\n\
                     The error message was:\n    {message}\n\n\
                     The problem is in:\n    {source}"
                )));
            }
            if expanded == previous {
                break;
            }
        }
        Ok(continue_lines(&expanded))
    }

    fn resolve_placeholder(
        &self,
        expression: &str,
        context: &QueryContext,
    ) -> Result<String, String> {
        let value = match expression {
            "query.file.path" => Some(context.file.path.clone()),
            "query.file.pathWithoutExtension" => {
                Some(context.file.path_without_extension.clone())
            }
            "query.file.root" => Some(context.file.root.clone()),
            "query.file.folder" => Some(context.file.folder.clone()),
            "query.file.filename" => Some(context.file.filename.clone()),
            "query.file.filenameWithoutExtension" => {
                Some(context.file.filename_without_extension.clone())
            }
            _ => None,
        };
        if let Some(value) = value {
            return Ok(value);
        }

        if let Some(name) =
            single_string_argument(expression, "query.file.property")
        {
            let Some(value) = context.property(name) else {
                return Err(format!(
                    "Invalid placeholder result 'null'. Missing file property: {name}"
                ));
            };
            return yaml_placeholder_value(value).ok_or_else(|| {
                format!(
                    "Invalid placeholder result 'null'. Missing file property: {name}"
                )
            });
        }
        if let Some(name) =
            single_string_argument(expression, "query.file.hasProperty")
        {
            return Ok(context.has_property(name).to_string());
        }
        if let Some(name) = expression.strip_prefix("preset.") {
            return self
                .settings
                .presets
                .get(name)
                .cloned()
                .ok_or_else(|| format!("Unknown property: preset.{name}"));
        }
        Err(format!("Unknown property: {expression}"))
    }
}

fn single_string_argument<'a>(
    expression: &'a str,
    function: &str,
) -> Option<&'a str> {
    let arguments = expression.strip_prefix(function)?.trim();
    let arguments = arguments.strip_prefix('(')?.strip_suffix(')')?.trim();
    if arguments.len() < 2 {
        return None;
    }
    let quote = arguments.chars().next()?;
    if !matches!(quote, '\'' | '"') || arguments.chars().next_back()? != quote {
        return None;
    }
    Some(&arguments[1..arguments.len() - 1])
}

fn yaml_placeholder_value(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Bool(value) => Some(value.to_string()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        serde_yaml::Value::String(value) => Some(value.clone()),
        serde_yaml::Value::Sequence(values) => Some(
            values
                .iter()
                .filter_map(yaml_placeholder_value)
                .collect::<Vec<_>>()
                .join(","),
        ),
        serde_yaml::Value::Mapping(_) => Some("[object Object]".to_string()),
        serde_yaml::Value::Tagged(value) => {
            yaml_placeholder_value(&value.value)
        }
    }
}

fn continue_lines(source: &str) -> Vec<String> {
    let normalized = source.replace("\r\n", "\n");
    let mut result = Vec::new();
    let mut current = String::new();
    let mut continuing = false;

    for input in normalized.split('\n').chain(std::iter::once("")) {
        let escaped = input.ends_with("\\\\");
        let continues = !escaped && input.ends_with('\\');
        let mut adjusted = if continuing {
            input.trim_start_matches([' ', '\t']).to_string()
        } else {
            input.to_string()
        };
        if escaped {
            adjusted.pop();
        } else if continues {
            adjusted.pop();
            adjusted = adjusted.trim_end_matches([' ', '\t']).to_string();
        }

        if continuing {
            current.push(' ');
            current.push_str(&adjusted);
        } else {
            current = adjusted;
        }
        continuing = continues;
        if !continuing {
            let instruction = current.trim();
            if !instruction.is_empty() {
                result.push(instruction.to_string());
            }
            current.clear();
        }
    }
    result
}

fn unknown_preset_message(
    name: &str,
    presets: &BTreeMap<String, String>,
) -> String {
    let mut message =
        format!("Cannot find preset \"{name}\" in the Tasks settings");
    if presets.is_empty() {
        message.push_str(&format!(
            "\nYou can define the instruction(s) for \"{name}\" in the Tasks settings."
        ));
    } else {
        message.push_str("\nAvailable presets: ");
        message.push_str(
            &presets
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    message
}

fn query_error(message: &str) -> DataviewError {
    DataviewError::TasksQuery {
        message: message.to_string(),
    }
}

fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|start| start.eq_ignore_ascii_case(prefix))?;
    value.get(prefix.len()..)
}

fn parse_instruction(line: &str) -> Result<Instruction, String> {
    if line.starts_with('#') {
        return Ok(Instruction::Comment);
    }
    if line.eq_ignore_ascii_case("explain") {
        return Ok(Instruction::Explain);
    }
    if line.eq_ignore_ascii_case("ignore global query") {
        return Ok(Instruction::IgnoreGlobalQuery);
    }
    if line.eq_ignore_ascii_case("short")
        || line.eq_ignore_ascii_case("short mode")
    {
        return Ok(Instruction::Mode { short: true });
    }
    if line.eq_ignore_ascii_case("full")
        || line.eq_ignore_ascii_case("full mode")
    {
        return Ok(Instruction::Mode { short: false });
    }
    if let Some(instruction) = parse_limit(line) {
        return Ok(instruction);
    }
    if starts_with_ci(line, "sort by ") {
        return parse_sort(line)
            .map(|instruction| Instruction::Sort { instruction });
    }
    if starts_with_ci(line, "group by ") {
        return parse_group(line)
            .map(|instruction| Instruction::Group { instruction });
    }
    if starts_with_ci(line, "hide ") || starts_with_ci(line, "show ") {
        return parse_layout(line);
    }
    parse_filter_expr(line)
        .map(|expression| Instruction::Filter { expression })
        .map_err(|message| {
            if message.is_empty() {
                "do not understand query".to_string()
            } else {
                message
            }
        })
}

fn parse_limit(line: &str) -> Option<Instruction> {
    let words = line.split_whitespace().collect::<Vec<_>>();
    if words
        .first()
        .is_none_or(|word| !word.eq_ignore_ascii_case("limit"))
    {
        return None;
    }
    let mut index = 1;
    let groups = words
        .get(index)
        .is_some_and(|word| word.eq_ignore_ascii_case("groups"));
    if groups {
        index += 1;
    }
    if words
        .get(index)
        .is_some_and(|word| word.eq_ignore_ascii_case("to"))
    {
        index += 1;
    }
    let count = words.get(index)?.parse::<usize>().ok()?;
    index += 1;
    if words.get(index).is_some_and(|word| {
        word.eq_ignore_ascii_case("task") || word.eq_ignore_ascii_case("tasks")
    }) {
        index += 1;
    }
    if index != words.len() {
        return None;
    }
    Some(if groups {
        Instruction::LimitGroups { count }
    } else {
        Instruction::Limit { count }
    })
}

fn parse_layout(line: &str) -> Result<Instruction, String> {
    let (shown, rest) = if let Some(rest) = strip_prefix_ci(line, "show ") {
        (true, rest)
    } else if let Some(rest) = strip_prefix_ci(line, "hide ") {
        (false, rest)
    } else {
        return Err("do not understand hide/show option".to_string());
    };
    let normalized = rest.split_whitespace().collect::<Vec<_>>().join(" ");
    let option = match normalized.to_ascii_lowercase().as_str() {
        "backlink" | "backlinks" => "backlink",
        "cancelled date" => "cancelled date",
        "created date" => "created date",
        "depends on" => "depends on",
        "done date" => "done date",
        "due date" => "due date",
        "edit button" => "edit button",
        "id" => "id",
        "on completion" => "on completion",
        "postpone button" => "postpone button",
        "priority" => "priority",
        "recurrence rule" => "recurrence rule",
        "scheduled date" => "scheduled date",
        "start date" => "start date",
        "tags" => "tags",
        "task count" => "task count",
        "toolbar" => "toolbar",
        "tree" => "tree",
        "urgency" => "urgency",
        _ => return Err("do not understand hide/show option".to_string()),
    };
    Ok(Instruction::Layout {
        option: option.to_string(),
        shown,
    })
}

fn parse_sort(line: &str) -> Result<SortInstruction, String> {
    let rest = strip_prefix_ci(line, "sort by ")
        .expect("caller checked sort prefix")
        .trim();
    if let Some(rest) = strip_prefix_ci(rest, "function ") {
        let (reverse, source) = take_reverse_prefix(rest);
        if source.trim().is_empty() {
            return Err("do not understand query".to_string());
        }
        return Ok(SortInstruction {
            key: SortKey::Function,
            reverse,
            function: Some(source.trim().to_string()),
            tag_index: None,
        });
    }

    let words = rest.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        return Err("do not understand query".to_string());
    }
    if words[0].eq_ignore_ascii_case("tag") {
        let mut reverse = false;
        let mut tag_index = None;
        for word in &words[1..] {
            if word.eq_ignore_ascii_case("reverse") && !reverse {
                reverse = true;
            } else if tag_index.is_none() {
                tag_index =
                    word.parse::<usize>().ok().filter(|index| *index > 0);
                if tag_index.is_none() {
                    return Err("do not understand query".to_string());
                }
            } else {
                return Err("do not understand query".to_string());
            }
        }
        return Ok(SortInstruction {
            key: SortKey::Tag,
            reverse,
            function: None,
            tag_index,
        });
    }

    let (key_text, reverse) = take_reverse_suffix(rest);
    let key = match key_text.to_ascii_lowercase().as_str() {
        "cancelled" => SortKey::Cancelled,
        "created" => SortKey::Created,
        "description" => SortKey::Description,
        "done" => SortKey::Done,
        "due" => SortKey::Due,
        "filename" => SortKey::Filename,
        "happens" => SortKey::Happens,
        "heading" => SortKey::Heading,
        "id" => SortKey::Id,
        "path" => SortKey::Path,
        "priority" => SortKey::Priority,
        "random" => SortKey::Random,
        "recurring" => SortKey::Recurring,
        "scheduled" => SortKey::Scheduled,
        "start" => SortKey::Start,
        "status" => SortKey::Status,
        "status.name" => SortKey::StatusName,
        "status.type" => SortKey::StatusType,
        "urgency" => SortKey::Urgency,
        _ => return Err("do not understand query".to_string()),
    };
    Ok(SortInstruction {
        key,
        reverse,
        function: None,
        tag_index: None,
    })
}

fn parse_group(line: &str) -> Result<GroupInstruction, String> {
    let rest = strip_prefix_ci(line, "group by ")
        .expect("caller checked group prefix")
        .trim();
    if let Some(rest) = strip_prefix_ci(rest, "function ") {
        let (reverse, source) = take_reverse_prefix(rest);
        if source.trim().is_empty() {
            return Err("do not understand query".to_string());
        }
        return Ok(GroupInstruction {
            key: GroupKey::Function,
            reverse,
            function: Some(source.trim().to_string()),
        });
    }
    let (key_text, reverse) = take_reverse_suffix(rest);
    let key = match key_text.to_ascii_lowercase().as_str() {
        "backlink" => GroupKey::Backlink,
        "cancelled" => GroupKey::Cancelled,
        "created" => GroupKey::Created,
        "done" => GroupKey::Done,
        "due" => GroupKey::Due,
        "filename" => GroupKey::Filename,
        "folder" => GroupKey::Folder,
        "happens" => GroupKey::Happens,
        "heading" => GroupKey::Heading,
        "id" => GroupKey::Id,
        "path" => GroupKey::Path,
        "priority" => GroupKey::Priority,
        "recurrence" => GroupKey::Recurrence,
        "recurring" => GroupKey::Recurring,
        "root" => GroupKey::Root,
        "scheduled" => GroupKey::Scheduled,
        "start" => GroupKey::Start,
        "status" => GroupKey::Status,
        "status.name" => GroupKey::StatusName,
        "status.type" => GroupKey::StatusType,
        "tags" => GroupKey::Tags,
        "urgency" => GroupKey::Urgency,
        _ => return Err("do not understand query".to_string()),
    };
    Ok(GroupInstruction {
        key,
        reverse,
        function: None,
    })
}

fn take_reverse_prefix(value: &str) -> (bool, &str) {
    strip_prefix_ci(value.trim(), "reverse ")
        .map_or((false, value), |rest| (true, rest))
}

fn take_reverse_suffix(value: &str) -> (&str, bool) {
    value
        .get(value.len().saturating_sub(" reverse".len())..)
        .filter(|suffix| suffix.eq_ignore_ascii_case(" reverse"))
        .map_or((value, false), |_| {
            (&value[..value.len() - " reverse".len()], true)
        })
}

fn starts_with_ci(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

fn parse_filter_expr(line: &str) -> Result<FilterExpr, String> {
    let line = line.trim();
    if looks_boolean(line) {
        return parse_boolean(line);
    }
    parse_leaf_filter(line)
}

fn looks_boolean(line: &str) -> bool {
    has_top_level_operator(line, " OR ")
        || has_top_level_operator(line, " XOR ")
        || has_top_level_operator(line, " AND ")
        || (starts_with_ci(line, "NOT ")
            && !line.eq_ignore_ascii_case("not done"))
        || is_wrapped_filter(line)
}

fn parse_boolean(line: &str) -> Result<FilterExpr, String> {
    let trimmed = line.trim();
    let unwrapped = strip_wrapping_delimiters(trimmed).unwrap_or(trimmed);
    if unwrapped != trimmed && !has_any_top_level_boolean(unwrapped) {
        return parse_filter_expr(unwrapped);
    }

    for operator in [" OR ", " XOR ", " AND "] {
        if let Some(index) = find_top_level_operator(unwrapped, operator) {
            let left_source = unwrapped[..index].trim();
            let right_source = unwrapped[index + operator.len()..].trim();
            if !boolean_operand_is_delimited(left_source)
                || !boolean_operand_is_delimited(right_source)
            {
                return Err(format!(
                    "Could not interpret the following instruction as a Boolean combination:\n    {line}\n\nAll filters in a Boolean instruction must be inside parentheses or matching quote/bracket delimiters"
                ));
            }
            let left = parse_boolean(left_source)?;
            let right = parse_boolean(right_source)?;
            let left = Box::new(left);
            let right = Box::new(right);
            return Ok(match operator {
                " OR " => FilterExpr::Or { left, right },
                " XOR " => FilterExpr::Xor { left, right },
                " AND " => FilterExpr::And { left, right },
                _ => unreachable!("known boolean operator"),
            });
        }
    }

    if let Some(rest) = strip_prefix_ci(unwrapped, "NOT ") {
        if !is_wrapped_filter(rest.trim()) {
            return Err(format!(
                "Could not interpret the following instruction as a Boolean combination:\n    {line}\n\nThe filter after NOT must be inside parentheses or matching quote/bracket delimiters"
            ));
        }
        return Ok(FilterExpr::Not {
            expression: Box::new(parse_boolean(rest)?),
        });
    }
    parse_leaf_filter(unwrapped).map_err(|message| {
        if message.is_empty() {
            format!(
                "Could not interpret the following instruction as a Boolean combination:\n    {line}"
            )
        } else {
            message
        }
    })
}

fn boolean_operand_is_delimited(value: &str) -> bool {
    if let Some(rest) = strip_prefix_ci(value, "NOT ") {
        return is_wrapped_filter(rest.trim());
    }
    is_wrapped_filter(value)
}

fn has_any_top_level_boolean(line: &str) -> bool {
    [" OR ", " XOR ", " AND "]
        .iter()
        .any(|operator| has_top_level_operator(line, operator))
        || starts_with_ci(line, "NOT ")
}

fn has_top_level_operator(line: &str, operator: &str) -> bool {
    find_top_level_operator(line, operator).is_some()
}

fn find_top_level_operator(line: &str, operator: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let operator_bytes = operator.as_bytes();
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index + operator_bytes.len() <= bytes.len() {
        let character = bytes[index] as char;
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' | '[' | '{' => stack.push(character),
            ')' | ']' | '}' => {
                stack.pop();
            }
            _ => {}
        }
        if stack.is_empty() && bytes[index..].starts_with(operator_bytes) {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn is_wrapped_filter(line: &str) -> bool {
    strip_wrapping_delimiters(line).is_some()
}

fn strip_wrapping_delimiters(line: &str) -> Option<&str> {
    let line = line.trim();
    let (open, close) = match (line.chars().next()?, line.chars().next_back()?)
    {
        ('(', ')') => ('(', ')'),
        ('[', ']') => ('[', ']'),
        ('{', '}') => ('{', '}'),
        ('"', '"') if entire_quoted_expression(line, '"') => ('"', '"'),
        ('\'', '\'') if entire_quoted_expression(line, '\'') => ('\'', '\''),
        _ => return None,
    };
    if matches!(open, '\'' | '"') {
        return Some(
            line[open.len_utf8()..line.len() - close.len_utf8()].trim(),
        );
    }

    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            value if value == open => depth += 1,
            value if value == close => {
                depth = depth.saturating_sub(1);
                if depth == 0 && index + character.len_utf8() != line.len() {
                    return None;
                }
            }
            _ => {}
        }
    }
    (depth == 0)
        .then(|| line[open.len_utf8()..line.len() - close.len_utf8()].trim())
}

fn entire_quoted_expression(line: &str, quote: char) -> bool {
    let mut escaped = false;
    for (index, character) in line.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            return index + character.len_utf8() == line.len();
        }
    }
    false
}

fn parse_leaf_filter(line: &str) -> Result<FilterExpr, String> {
    if line.eq_ignore_ascii_case("done") {
        return Ok(FilterExpr::Done { done: true });
    }
    if line.eq_ignore_ascii_case("not done") {
        return Ok(FilterExpr::Done { done: false });
    }
    if line.eq_ignore_ascii_case("is recurring") {
        return Ok(FilterExpr::Recurring { recurring: true });
    }
    if line.eq_ignore_ascii_case("is not recurring") {
        return Ok(FilterExpr::Recurring { recurring: false });
    }
    for (instruction, expression) in [
        ("has id", presence(PresenceField::Id, true)),
        ("no id", presence(PresenceField::Id, false)),
        ("has depends on", presence(PresenceField::DependsOn, true)),
        ("no depends on", presence(PresenceField::DependsOn, false)),
        ("has tag", presence(PresenceField::Tag, true)),
        ("has tags", presence(PresenceField::Tag, true)),
        ("no tag", presence(PresenceField::Tag, false)),
        ("no tags", presence(PresenceField::Tag, false)),
        ("is blocked", FilterExpr::Blocked { blocked: true }),
        ("is not blocked", FilterExpr::Blocked { blocked: false }),
        ("is blocking", FilterExpr::Blocking { blocking: true }),
        ("is not blocking", FilterExpr::Blocking { blocking: false }),
        ("exclude sub-items", FilterExpr::ExcludeSubItems),
    ] {
        if line.eq_ignore_ascii_case(instruction) {
            return Ok(expression);
        }
    }

    if let Some(source) = strip_prefix_ci(line, "filter by function ") {
        if source.trim().is_empty() {
            return Err("do not understand query".to_string());
        }
        return Ok(FilterExpr::Function {
            source: source.trim().to_string(),
        });
    }
    if starts_with_ci(line, "status.type") {
        return parse_status_type(line);
    }
    if starts_with_ci(line, "priority") {
        return parse_priority(line);
    }
    if let Some(filter) = parse_date_presence(line) {
        return Ok(filter);
    }
    if let Some(filter) = parse_date_filter(line)? {
        return Ok(filter);
    }
    if let Some(filter) = parse_text_filter(line)? {
        return Ok(filter);
    }
    Err(String::new())
}

fn presence(field: PresenceField, present: bool) -> FilterExpr {
    FilterExpr::Presence { field, present }
}

fn parse_status_type(line: &str) -> Result<FilterExpr, String> {
    let rest = strip_prefix_ci(line, "status.type")
        .expect("caller checked status.type")
        .trim();
    let (negated, value) = if let Some(value) = strip_prefix_ci(rest, "is not ")
    {
        (true, value)
    } else if let Some(value) = strip_prefix_ci(rest, "is ") {
        (false, value)
    } else {
        return Err(invalid_status_type(line));
    };
    let value = value.trim().to_ascii_uppercase();
    if !matches!(
        value.as_str(),
        "TODO" | "DONE" | "IN_PROGRESS" | "ON_HOLD" | "CANCELLED" | "NON_TASK"
    ) {
        return Err(invalid_status_type(line));
    }
    Ok(FilterExpr::StatusType { negated, value })
}

fn invalid_status_type(line: &str) -> String {
    format!(
        "Invalid status.type instruction: '{line}'. Allowed values: TODO DONE IN_PROGRESS ON_HOLD CANCELLED NON_TASK"
    )
}

fn parse_priority(line: &str) -> Result<FilterExpr, String> {
    let words = line.split_whitespace().collect::<Vec<_>>();
    let (relation, value) = match words.as_slice() {
        [field, value] if field.eq_ignore_ascii_case("priority") => {
            (PriorityRelation::Is, *value)
        }
        [field, is, value]
            if field.eq_ignore_ascii_case("priority")
                && is.eq_ignore_ascii_case("is") =>
        {
            (PriorityRelation::Is, *value)
        }
        [field, is, relation, value]
            if field.eq_ignore_ascii_case("priority")
                && is.eq_ignore_ascii_case("is") =>
        {
            let relation = if relation.eq_ignore_ascii_case("above") {
                PriorityRelation::Above
            } else if relation.eq_ignore_ascii_case("below") {
                PriorityRelation::Below
            } else if relation.eq_ignore_ascii_case("not") {
                PriorityRelation::IsNot
            } else {
                return Err(
                    "do not understand query filter (priority)".to_string()
                );
            };
            (relation, *value)
        }
        _ => {
            return Err("do not understand query filter (priority)".to_string())
        }
    };
    let value = value.to_ascii_lowercase();
    if !matches!(
        value.as_str(),
        "lowest" | "low" | "none" | "medium" | "high" | "highest"
    ) {
        return Err("do not understand priority".to_string());
    }
    Ok(FilterExpr::Priority { relation, value })
}

fn parse_date_presence(line: &str) -> Option<FilterExpr> {
    for (name, field) in date_presence_fields() {
        if line.eq_ignore_ascii_case(&format!("has {name} date")) {
            return Some(FilterExpr::DatePresence {
                field,
                presence: Presence::Has,
            });
        }
        if line.eq_ignore_ascii_case(&format!("no {name} date")) {
            return Some(FilterExpr::DatePresence {
                field,
                presence: Presence::Missing,
            });
        }
        if field != DateField::Happens
            && line.eq_ignore_ascii_case(&format!("{name} date is invalid"))
        {
            return Some(FilterExpr::DatePresence {
                field,
                presence: Presence::Invalid,
            });
        }
    }
    None
}

fn date_presence_fields() -> [(&'static str, DateField); 7] {
    [
        ("due", DateField::Due),
        ("scheduled", DateField::Scheduled),
        ("start", DateField::Start),
        ("created", DateField::Created),
        ("done", DateField::Done),
        ("cancelled", DateField::Cancelled),
        ("happens", DateField::Happens),
    ]
}

fn parse_date_filter(line: &str) -> Result<Option<FilterExpr>, String> {
    for (name, field) in [
        ("due", DateField::Due),
        ("scheduled", DateField::Scheduled),
        ("starts", DateField::Start),
        ("created", DateField::Created),
        ("done", DateField::Done),
        ("cancelled", DateField::Cancelled),
        ("happens", DateField::Happens),
    ] {
        let Some(rest) = strip_prefix_ci(line, &format!("{name} ")) else {
            continue;
        };
        let rest = rest.trim();
        let (relation, value) = if let Some(value) =
            strip_prefix_ci(rest, "on or before ")
                .or_else(|| strip_prefix_ci(rest, "in or before "))
        {
            (DateRelation::OnOrBefore, value)
        } else if let Some(value) = strip_prefix_ci(rest, "on or after ")
            .or_else(|| strip_prefix_ci(rest, "in or after "))
        {
            (DateRelation::OnOrAfter, value)
        } else if let Some(value) = strip_prefix_ci(rest, "before ") {
            (DateRelation::Before, value)
        } else if let Some(value) = strip_prefix_ci(rest, "after ") {
            (DateRelation::After, value)
        } else if let Some(value) = strip_prefix_ci(rest, "on ") {
            (DateRelation::On, value)
        } else if let Some(value) = strip_prefix_ci(rest, "in ") {
            (DateRelation::In, value)
        } else {
            (DateRelation::In, rest)
        };
        if value.trim().is_empty() {
            return Err(format!("do not understand {name} date"));
        }
        if !looks_like_date_expression(value.trim()) {
            return Err(format!("do not understand {name} date"));
        }
        return Ok(Some(FilterExpr::Date {
            field,
            relation,
            value: value.trim().to_string(),
        }));
    }
    Ok(None)
}

fn looks_like_date_expression(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let words = lower.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        return false;
    }
    let iso_date = |word: &str| {
        let parts = word.split('-').collect::<Vec<_>>();
        parts.len() == 3
            && parts[0].len() == 4
            && parts[1].len() == 2
            && parts[2].len() == 2
            && parts.iter().all(|part| {
                part.chars().all(|character| character.is_ascii_digit())
            })
    };
    if matches!(words.as_slice(), [one] if iso_date(one))
        || matches!(words.as_slice(), [one, two] if iso_date(one) && iso_date(two))
    {
        return true;
    }

    words.iter().all(|word| {
        word.parse::<u32>().is_ok()
            || matches!(
                *word,
                "today"
                    | "tomorrow"
                    | "yesterday"
                    | "monday"
                    | "tuesday"
                    | "wednesday"
                    | "thursday"
                    | "friday"
                    | "saturday"
                    | "sunday"
                    | "this"
                    | "next"
                    | "last"
                    | "week"
                    | "weeks"
                    | "month"
                    | "months"
                    | "quarter"
                    | "quarters"
                    | "year"
                    | "years"
                    | "day"
                    | "days"
                    | "ago"
            )
    })
}

fn parse_text_filter(line: &str) -> Result<Option<FilterExpr>, String> {
    for (name, field) in [
        ("status.name", TextField::StatusName),
        ("status.symbol", TextField::StatusSymbol),
        ("description", TextField::Description),
        ("heading", TextField::Heading),
        ("filename", TextField::Filename),
        ("folder", TextField::Folder),
        ("path", TextField::Path),
        ("root", TextField::Root),
        ("backlink", TextField::Backlink),
        ("recurrence", TextField::Recurrence),
        ("id", TextField::Id),
        ("tags", TextField::Tag),
        ("tag", TextField::Tag),
    ] {
        let Some(rest) = strip_prefix_ci(line, &format!("{name} ")) else {
            continue;
        };
        let parsed = if field == TextField::StatusSymbol {
            strip_prefix_ci(rest, "is not ")
                .map(|value| (TextOperator::IsNot, value.trim()))
                .or_else(|| {
                    strip_prefix_ci(rest, "is ")
                        .map(|value| (TextOperator::Is, value.trim()))
                })
        } else {
            None
        };
        let (operator, value) = parsed
            .or_else(|| parse_text_operator(rest, field == TextField::Tag))
            .ok_or_else(|| {
                format!("do not understand query filter ({name})")
            })?;
        if value.is_empty() {
            return Err(format!("do not understand query filter ({name})"));
        }
        let (value, regex_flags) = if matches!(
            operator,
            TextOperator::RegexMatches | TextOperator::RegexDoesNotMatch
        ) {
            parse_regex(value)?
        } else {
            (value.to_string(), None)
        };
        return Ok(Some(FilterExpr::Text {
            field,
            operator,
            value,
            regex_flags,
        }));
    }
    Ok(None)
}

fn parse_text_operator(rest: &str, tags: bool) -> Option<(TextOperator, &str)> {
    for (operator, parsed) in [
        ("regex does not match ", TextOperator::RegexDoesNotMatch),
        ("does not include ", TextOperator::DoesNotInclude),
        ("regex matches ", TextOperator::RegexMatches),
        ("includes ", TextOperator::Includes),
    ] {
        if let Some(value) = strip_prefix_ci(rest, operator) {
            return Some((parsed, value.trim()));
        }
    }
    if tags {
        for (operator, parsed) in [
            ("do not include ", TextOperator::DoesNotInclude),
            ("include ", TextOperator::Includes),
        ] {
            if let Some(value) = strip_prefix_ci(rest, operator) {
                return Some((parsed, value.trim()));
            }
        }
    }
    None
}

fn parse_regex(value: &str) -> Result<(String, Option<String>), String> {
    let Some(source_and_flags) = value.strip_prefix('/') else {
        return Err(invalid_regex(value));
    };
    let Some(end) = source_and_flags.rfind('/') else {
        return Err(invalid_regex(value));
    };
    let source = &source_and_flags[..end];
    let flags = &source_and_flags[end + 1..];
    if source.is_empty()
        || flags.chars().any(|flag| !"dgimsuvy".contains(flag))
        || flags.chars().any(|flag| flags.matches(flag).count() > 1)
        || (flags.contains('u') && flags.contains('v'))
    {
        return Err(invalid_regex(value));
    }
    Ok((
        source.to_string(),
        (!flags.is_empty()).then(|| flags.to_string()),
    ))
}

fn invalid_regex(value: &str) -> String {
    format!(
        "Invalid regular expression '{value}'. Regular expressions must look like /pattern/ or /pattern/flags"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_presets() -> TasksSettings {
        let mut settings = TasksSettings::default();
        settings.presets.insert(
            "nested".to_string(),
            "preset this_file\nhide due date".to_string(),
        );
        settings
    }

    #[test]
    fn scanner_matches_tasks_line_continuation_rules() {
        assert_eq!(
            continue_lines(concat!(
                "description includes one \\\n",
                "                two \\\n",
                "                three"
            )),
            ["description includes one two three"]
        );
        assert_eq!(
            continue_lines("description includes slash\\\\\nnot done"),
            ["description includes slash\\", "not done"]
        );
        assert_eq!(continue_lines("due today \\"), ["due today"]);
    }

    #[test]
    fn parses_every_filter_family_and_boolean_combinations() {
        for line in [
            "done",
            "status.type is IN_PROGRESS",
            "status.name includes Next",
            "status.symbol is not x",
            "due on or before this week",
            "has scheduled date",
            "scheduled date is invalid",
            "description regex does not match /foo\\/bar/im",
            "tags do not include #hide",
            "priority is above medium",
            "recurrence includes Wednesday",
            "has depends on",
            "is not blocked",
            "filter by function task.file.path !== query.file.path",
            "(status.type is TODO) AND NOT ((is blocked) OR (tag includes #hide))",
            "\"due this week\" AND \"description includes Hello World\"",
            "[due this week] XOR {description includes Hello World}",
        ] {
            parse_filter_expr(line).unwrap_or_else(|error| {
                panic!("failed to parse {line:?}: {error}")
            });
        }
    }

    #[test]
    fn parses_every_v8_sort_group_and_layout_key() {
        for key in [
            "cancelled",
            "created",
            "description",
            "done",
            "due",
            "filename",
            "happens",
            "heading",
            "id",
            "path",
            "priority",
            "random",
            "recurring",
            "scheduled",
            "start",
            "status",
            "status.name",
            "status.type",
            "tag reverse 3",
            "urgency",
        ] {
            parse_sort(&format!("sort by {key}")).unwrap();
        }
        for key in [
            "backlink",
            "cancelled",
            "created",
            "done",
            "due",
            "filename",
            "folder",
            "happens",
            "heading",
            "id",
            "path",
            "priority",
            "recurrence",
            "recurring",
            "root",
            "scheduled",
            "start",
            "status",
            "status.name",
            "status.type",
            "tags",
            "urgency",
        ] {
            parse_group(&format!("group by {key} reverse")).unwrap();
        }
        for option in [
            "toolbar",
            "postpone button",
            "task count",
            "backlinks",
            "edit button",
            "urgency",
            "tree",
            "tags",
            "id",
            "depends on",
            "priority",
            "recurrence rule",
            "on completion",
            "created date",
            "start date",
            "scheduled date",
            "due date",
            "cancelled date",
            "done date",
        ] {
            parse_layout(&format!("hide {option}")).unwrap();
        }
    }

    #[test]
    fn rejects_malformed_filters_with_actionable_errors() {
        assert!(parse_filter_expr("due spaghetti").is_err());
        let boolean = parse_filter_expr(
            "status.type is TODO OR status.type is IN_PROGRESS",
        )
        .unwrap_err();
        assert!(boolean.contains("inside parentheses"), "{boolean}");
        assert!(parse_filter_expr("status.type maybe TODO").is_err());
        assert!(
            parse_filter_expr("description regex matches /pattern/ii").is_err()
        );
    }

    #[test]
    fn composes_global_defaults_presets_and_placeholders_in_order() {
        let vault = std::env::temp_dir()
            .join(format!("bob-cli-task-query-parser-{}", std::process::id()));
        let _ = fs::remove_dir_all(&vault);
        fs::create_dir_all(vault.join("Folder")).unwrap();
        fs::write(
            vault.join("Folder/Query.md"),
            "---\nTQ_extra_instructions: |\n  preset nested\n  folder includes {{query.file.folder}}\n---\n",
        )
        .unwrap();
        let mut settings = settings_with_presets();
        settings.global_query = "not done".to_string();
        let ast = parse(
            &vault,
            Some(Path::new("Folder/Query.md")),
            "status.type is TODO",
            &settings,
        )
        .unwrap();
        assert_eq!(ast.filters.len(), 4);
        assert_eq!(ast.statements[0].source, StatementSource::GlobalQuery);
        assert!(ast.statements.iter().any(|statement| {
            statement.instruction == "path includes Folder/Query.md"
        }));
        assert!(!ast.layout.show_due_date);
        fs::remove_dir_all(vault).unwrap();
    }

    #[test]
    fn ignore_global_query_can_come_from_query_file_defaults() {
        let vault = std::env::temp_dir().join(format!(
            "bob-cli-task-query-ignore-global-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&vault);
        fs::create_dir_all(&vault).unwrap();
        fs::write(
            vault.join("Query.md"),
            "---\ntq_EXTRA_instructions: ignore global query\nTQ_show_due_date: null\n---\n",
        )
        .unwrap();
        let settings = TasksSettings {
            global_query: "not done".to_string(),
            ..TasksSettings::default()
        };
        let ast = parse(
            &vault,
            Some(Path::new("Query.md")),
            "status.type is TODO",
            &settings,
        )
        .unwrap();
        assert_eq!(ast.filters.len(), 1);
        assert!(ast.ignore_global_query);
        assert!(ast.layout.show_due_date);
        fs::remove_dir_all(vault).unwrap();
    }
}
