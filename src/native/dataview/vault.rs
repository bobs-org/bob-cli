//! Native vault evaluation: running queries over the page index.

use super::*;
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

pub(super) const DEFAULT_FLATTEN_ROW_LIMIT: usize = 100_000;

#[derive(Debug)]
pub(super) struct NativeVault {
    pub(super) index: DataviewIndex,
    pub(super) origin_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub(super) struct NativeRow {
    page_index: usize,
    source_page_index: Option<usize>,
    value: DataviewValue,
    variables: BTreeMap<String, DataviewValue>,
}

impl NativeVault {
    pub(super) fn read(
        bob_dir: &Path,
        origin: Option<&Path>,
    ) -> Result<Self, DataviewError> {
        let index = DataviewIndex::read(bob_dir)?;
        let origin_index = Self::origin_index(&index, origin)?;
        Ok(Self {
            index,
            origin_index,
        })
    }

    pub(super) fn origin_index(
        index: &DataviewIndex,
        origin: Option<&Path>,
    ) -> Result<Option<usize>, DataviewError> {
        let Some(origin) = origin else {
            return Ok(None);
        };
        let path = normalize_note_path(&origin.to_string_lossy()).map_err(
            |message| DataviewError::NativeQuery {
                message: format!(
                    "invalid native --origin {}: {message}",
                    origin.display()
                ),
            },
        )?;
        let Some(index) = index.by_path.get(&path).copied() else {
            return Err(DataviewError::NativeQuery {
                message: format!(
                    "native --origin {path} does not name an indexed note"
                ),
            });
        };
        Ok(Some(index))
    }

    pub(super) fn evaluate_source(
        &self,
        source: &NativeSourceExpr,
    ) -> EngineOutput {
        let paths = self
            .evaluate_source_indices(source)
            .into_iter()
            .map(|index| self.index.pages[index].path.clone())
            .collect();

        EngineOutput {
            response: EngineResponse::SourcePaths(paths),
            warnings: self.index.warnings.clone(),
        }
    }

    pub(super) fn evaluate(
        &self,
        query: &NativeQuery,
    ) -> Result<NativeOutput, DataviewError> {
        self.evaluate_with_flatten_limit(query, DEFAULT_FLATTEN_ROW_LIMIT)
    }

    pub(super) fn evaluate_with_flatten_limit(
        &self,
        query: &NativeQuery,
        flatten_row_limit: usize,
    ) -> Result<NativeOutput, DataviewError> {
        let rows = self.evaluate_rows_with_limit(query, flatten_row_limit)?;
        Ok(NativeOutput {
            warnings: self.index.warnings.clone(),
            result: self.result_json(query, &rows),
        })
    }

    pub(super) fn evaluate_markdown(
        &self,
        query: &NativeQuery,
        settings: &NativeMarkdownSettings,
    ) -> Result<EngineOutput, DataviewError> {
        let rows = self.evaluate_rows(query)?;
        Ok(EngineOutput {
            response: EngineResponse::Markdown(
                self.result_markdown(query, &rows, settings)?,
            ),
            warnings: self.index.warnings.clone(),
        })
    }

    pub(super) fn evaluate_rows(
        &self,
        query: &NativeQuery,
    ) -> Result<Vec<NativeRow>, DataviewError> {
        self.evaluate_rows_with_limit(query, DEFAULT_FLATTEN_ROW_LIMIT)
    }

    pub(super) fn evaluate_rows_with_limit(
        &self,
        query: &NativeQuery,
        flatten_row_limit: usize,
    ) -> Result<Vec<NativeRow>, DataviewError> {
        let mut rows = self.initial_rows(query);
        for command in &query.commands {
            match command {
                NativeDataCommand::From(source) => {
                    rows = self.filter_rows_by_source(rows, source);
                }
                NativeDataCommand::Where(expression) => {
                    rows.retain(|row| {
                        expression.evaluate(&row.context(self)).is_truthy()
                    });
                }
                NativeDataCommand::Limit(limit) => {
                    rows.truncate(*limit);
                }
                NativeDataCommand::Sort {
                    expression,
                    direction,
                } => {
                    rows.sort_by(|left, right| {
                        let left_value =
                            expression.evaluate(&left.context(self));
                        let right_value =
                            expression.evaluate(&right.context(self));
                        let ordering =
                            compare_values(self, &left_value, &right_value);
                        match direction.unwrap_or(SortDirection::Ascending) {
                            SortDirection::Ascending => ordering,
                            SortDirection::Descending => ordering.reverse(),
                        }
                    });
                }
                NativeDataCommand::GroupBy { expression, alias } => {
                    rows = self.group_rows(rows, expression, alias.as_deref());
                }
                NativeDataCommand::Flatten { expression, alias } => {
                    rows = self.flatten_rows(
                        rows,
                        expression,
                        alias.as_deref(),
                        flatten_row_limit,
                    )?;
                }
            }
        }
        Ok(rows)
    }

    pub(super) fn initial_rows(&self, query: &NativeQuery) -> Vec<NativeRow> {
        match &query.kind {
            NativeQueryKind::Task { .. } => self.task_rows(),
            NativeQueryKind::List { .. }
            | NativeQueryKind::Table { .. }
            | NativeQueryKind::Calendar { .. } => self.page_rows(),
        }
    }

    pub(super) fn page_rows(&self) -> Vec<NativeRow> {
        self.index_order_indices()
            .into_iter()
            .map(|page_index| NativeRow::page(self, page_index))
            .collect()
    }

    pub(super) fn task_rows(&self) -> Vec<NativeRow> {
        let mut rows = Vec::new();
        for page_index in self.index_order_indices() {
            for task in self.top_level_page_tasks(page_index) {
                rows.push(NativeRow::task(page_index, task));
            }
        }
        rows
    }

    pub(super) fn top_level_page_tasks(
        &self,
        page_index: usize,
    ) -> Vec<DataviewValue> {
        let tasks = self
            .page_field_value(page_index, "file")
            .as_object_field("tasks")
            .and_then(|value| value.as_array().map(|values| values.to_vec()))
            .unwrap_or_default();

        tasks
            .into_iter()
            .filter(|task| {
                matches!(
                    task.as_object_field("parent"),
                    None | Some(DataviewValue::Null)
                )
            })
            .collect()
    }

    pub(super) fn filter_rows_by_source(
        &self,
        rows: Vec<NativeRow>,
        source: &NativeSourceExpr,
    ) -> Vec<NativeRow> {
        let mut rows_by_page: BTreeMap<usize, Vec<NativeRow>> = BTreeMap::new();
        for row in rows {
            if let Some(page_index) = row.source_page_index {
                rows_by_page.entry(page_index).or_default().push(row);
            }
        }

        let mut filtered = Vec::new();
        for page_index in self.evaluate_source_indices(source) {
            if let Some(mut page_rows) = rows_by_page.remove(&page_index) {
                filtered.append(&mut page_rows);
            }
        }
        filtered
    }

    pub(super) fn group_rows(
        &self,
        rows: Vec<NativeRow>,
        expression: &NativeExpression,
        alias: Option<&str>,
    ) -> Vec<NativeRow> {
        let mut groups: Vec<(String, DataviewValue, Vec<NativeRow>)> =
            Vec::new();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        for row in rows {
            let key = expression.evaluate(&row.context(self));
            let group_key = value_group_key(&key);
            if let Some(&index) = by_key.get(&group_key) {
                groups[index].2.push(row);
            } else {
                by_key.insert(group_key.clone(), groups.len());
                groups.push((group_key, key, vec![row]));
            }
        }

        groups
            .into_iter()
            .map(|(_, key, rows)| {
                let page_index = rows.first().map_or(0, |row| row.page_index);
                NativeRow::group(page_index, key, rows, expression, alias)
            })
            .collect()
    }

    pub(super) fn flatten_rows(
        &self,
        rows: Vec<NativeRow>,
        expression: &NativeExpression,
        alias: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NativeRow>, DataviewError> {
        let field = alias.unwrap_or(&expression.raw);
        let mut flattened = Vec::new();
        for row in rows {
            let value = expression.evaluate(&row.context(self));
            let expansion = flatten_expansion_len(&value);
            let next = checked_flatten_len(flattened.len(), expansion, limit)
                .map_err(|kind| {
                flatten_overflow_error(
                    expression,
                    alias,
                    kind,
                    flattened.len(),
                    expansion,
                    limit,
                )
            })?;
            let additional = next - flattened.len();
            flattened.try_reserve(additional).map_err(|_| {
                DataviewError::NativeQuery {
                    message: format!(
                        "native {} failed to reserve {additional} flattened \
                         rows",
                        flatten_command_text(expression, alias)
                    ),
                }
            })?;
            let values = match value.into_vec() {
                Ok(values) => values,
                Err(DataviewValue::Null) => vec![DataviewValue::Null],
                Err(value) => vec![value],
            };
            for value in values {
                flattened.push(row.clone().with_field(field, value));
            }
        }
        Ok(flattened)
    }

    pub(super) fn result_json(
        &self,
        query: &NativeQuery,
        rows: &[NativeRow],
    ) -> Value {
        match &query.kind {
            NativeQueryKind::List { expression, .. } => {
                self.list_result_json(rows, expression.as_ref())
            }
            NativeQueryKind::Table { columns, .. } => {
                self.table_result_json(rows, columns)
            }
            NativeQueryKind::Task { .. } => self.task_result_json(rows),
            NativeQueryKind::Calendar { expression, .. } => {
                self.calendar_result_json(rows, expression)
            }
        }
    }

    pub(super) fn list_result_json(
        &self,
        rows: &[NativeRow],
        expression: Option<&NativeExpression>,
    ) -> Value {
        let grouped = rows.iter().any(|row| row.source_page_index.is_none());
        let values = rows
            .iter()
            .map(|row| match expression {
                Some(expression) => list_pair_json(
                    row.identity_value(self).to_plain_json(),
                    expression.evaluate(&row.context(self)).to_plain_json(),
                ),
                None => {
                    if row.source_page_index.is_some() {
                        row.identity_value(self).to_plain_json()
                    } else {
                        row.group_key_value().to_plain_json()
                    }
                }
            })
            .collect::<Vec<_>>();

        let mut result = serde_json::json!({
            "type": "list",
            "values": values,
        });
        if expression.is_some() || grouped {
            result["primaryMeaning"] = identity_meaning_json(grouped);
        }
        result
    }

    pub(super) fn table_result_json(
        &self,
        rows: &[NativeRow],
        columns: &[NativeSelect],
    ) -> Value {
        let grouped = rows.iter().any(|row| row.source_page_index.is_none());
        let include_identity = !grouped;
        let values = rows
            .iter()
            .map(|row| {
                let mut cells = Vec::new();
                if include_identity {
                    cells.push(row.identity_value(self).to_plain_json());
                }
                cells.extend(columns.iter().map(|column| {
                    column
                        .expression
                        .evaluate(&row.context(self))
                        .to_plain_json()
                }));
                Value::Array(cells)
            })
            .collect::<Vec<_>>();

        serde_json::json!({
            "type": "table",
            "idMeaning": identity_meaning_json(grouped),
            "headers": columns.iter().map(NativeSelect::header).collect::<Vec<_>>(),
            "values": values,
        })
    }

    pub(super) fn task_result_json(&self, rows: &[NativeRow]) -> Value {
        serde_json::json!({
            "type": "task",
            "values": rows
                .iter()
                .map(|row| row.value.to_plain_json())
                .collect::<Vec<_>>(),
        })
    }

    pub(super) fn calendar_result_json(
        &self,
        rows: &[NativeRow],
        expression: &NativeExpression,
    ) -> Value {
        let values = rows
            .iter()
            .filter_map(|row| {
                let date = calendar_date_text(
                    &expression.evaluate(&row.context(self)),
                )?;
                let link = row.identity_value(self).to_plain_json();
                Some(serde_json::json!({
                    "date": date,
                    "link": link,
                    "value": row.display_value(self),
                }))
            })
            .collect::<Vec<_>>();

        serde_json::json!({
            "type": "calendar",
            "values": values,
        })
    }

    pub(super) fn result_markdown(
        &self,
        query: &NativeQuery,
        rows: &[NativeRow],
        settings: &NativeMarkdownSettings,
    ) -> Result<String, DataviewError> {
        match &query.kind {
            NativeQueryKind::List {
                expression,
                without_id,
            } => Ok(self.list_result_markdown(
                rows,
                expression.as_ref(),
                *without_id,
                settings,
            )),
            NativeQueryKind::Table {
                columns,
                without_id,
            } => Ok(self.table_result_markdown(
                rows,
                columns,
                *without_id,
                settings,
            )),
            NativeQueryKind::Task { .. } => {
                Ok(self.task_result_markdown(rows, settings))
            }
            NativeQueryKind::Calendar { .. } => {
                Err(DataviewError::DataviewQuery {
                    message: "Cannot render calendar queries to markdown."
                        .to_string(),
                })
            }
        }
    }

    pub(super) fn list_result_markdown(
        &self,
        rows: &[NativeRow],
        expression: Option<&NativeExpression>,
        without_id: bool,
        settings: &NativeMarkdownSettings,
    ) -> String {
        let mut markdown = String::new();
        for row in rows {
            markdown.push_str("- ");
            match expression {
                Some(expression) if !without_id => {
                    let key = row.identity_value(self);
                    let value = expression.evaluate(&row.context(self));
                    markdown.push_str(&markdown_literal(&key, settings));
                    markdown.push_str(": ");
                    markdown.push_str(&markdown_literal(&value, settings));
                }
                Some(expression) => {
                    let value = expression.evaluate(&row.context(self));
                    markdown.push_str(&markdown_literal(&value, settings));
                }
                None => {
                    markdown.push_str(&markdown_literal(
                        &row.identity_value(self),
                        settings,
                    ));
                }
            }
            markdown.push('\n');
        }
        markdown
    }

    pub(super) fn table_result_markdown(
        &self,
        rows: &[NativeRow],
        columns: &[NativeSelect],
        without_id: bool,
        settings: &NativeMarkdownSettings,
    ) -> String {
        let grouped = rows.iter().any(|row| row.source_page_index.is_none());
        let mut headers = Vec::new();
        if !without_id {
            headers.push(if grouped {
                settings.table_group_column_name.clone()
            } else {
                settings.table_id_column_name.clone()
            });
        }
        headers.extend(columns.iter().map(NativeSelect::header));

        let values = rows
            .iter()
            .map(|row| {
                let mut cells = Vec::new();
                if !without_id {
                    cells.push(row.identity_value(self));
                }
                cells.extend(columns.iter().map(|column| {
                    column.expression.evaluate(&row.context(self))
                }));
                cells
            })
            .collect::<Vec<_>>();

        markdown_table(&headers, &values, settings)
    }

    pub(super) fn task_result_markdown(
        &self,
        rows: &[NativeRow],
        settings: &NativeMarkdownSettings,
    ) -> String {
        let values =
            rows.iter().map(|row| row.value.clone()).collect::<Vec<_>>();
        markdown_task_values(&values, settings, 0)
    }

    pub(super) fn index_order_indices(&self) -> Vec<usize> {
        (0..self.index.pages.len()).collect()
    }

    pub(super) fn source_order_indices(&self) -> Vec<usize> {
        let mut indices = self.index_order_indices();
        indices.sort_by(|left, right| {
            source_order_key(&self.index.pages[*left].path)
                .cmp(&source_order_key(&self.index.pages[*right].path))
        });
        indices
    }

    pub(super) fn evaluate_source_indices(
        &self,
        source: &NativeSourceExpr,
    ) -> Vec<usize> {
        match source {
            NativeSourceExpr::All => self.source_order_indices(),
            NativeSourceExpr::And(left, right) => {
                let right = self
                    .evaluate_source_indices(right)
                    .into_iter()
                    .collect::<HashSet<_>>();
                self.evaluate_source_indices(left)
                    .into_iter()
                    .filter(|index| right.contains(index))
                    .collect()
            }
            NativeSourceExpr::IncomingLink(raw) => {
                self.incoming_link_source_indices(raw)
            }
            NativeSourceExpr::Not(expr) => {
                let excluded = self
                    .evaluate_source_indices(expr)
                    .into_iter()
                    .collect::<HashSet<_>>();
                self.source_order_indices()
                    .into_iter()
                    .filter(|index| !excluded.contains(index))
                    .collect()
            }
            NativeSourceExpr::Or(left, right) => {
                let mut indices = self.evaluate_source_indices(left);
                let mut seen = indices.iter().copied().collect::<HashSet<_>>();
                for index in self.evaluate_source_indices(right) {
                    if seen.insert(index) {
                        indices.push(index);
                    }
                }
                indices
            }
            NativeSourceExpr::OutgoingLink(raw) => {
                self.outgoing_link_source_indices(raw)
            }
            NativeSourceExpr::Path(raw) => self.path_source_indices(raw),
            NativeSourceExpr::Tag(tag) => self.tag_source_indices(tag),
        }
    }

    pub(super) fn tag_source_indices(&self, tag: &str) -> Vec<usize> {
        let tag = normalize_source_tag(tag);
        self.source_order_indices()
            .into_iter()
            .filter(|index| {
                page_tags(&self.index.pages[*index])
                    .iter()
                    .any(|page_tag| tag_matches_source(page_tag, &tag))
            })
            .collect()
    }

    pub(super) fn path_source_indices(&self, raw: &str) -> Vec<usize> {
        if let Ok(folder) = normalize_native_source_folder(raw) {
            let prefix = format!("{folder}/");
            let mut indices = self
                .index_order_indices()
                .into_iter()
                .filter(|index| {
                    self.index.pages[*index].path.starts_with(&prefix)
                })
                .collect::<Vec<_>>();
            if !indices.is_empty() {
                indices.sort_by(|left, right| {
                    source_order_key(&self.index.pages[*left].path)
                        .cmp(&source_order_key(&self.index.pages[*right].path))
                });
                return indices;
            }
        }

        if let Ok(path) = normalize_note_path(raw)
            && let Some(index) = self.index.by_path.get(&path)
        {
            return vec![*index];
        }

        Vec::new()
    }

    pub(super) fn incoming_link_source_indices(&self, raw: &str) -> Vec<usize> {
        let Some(target) = self
            .resolve_source_link(raw)
            .map(|path| source_link_base(&path).to_string())
        else {
            return Vec::new();
        };

        self.source_order_indices()
            .into_iter()
            .filter(|index| {
                page_outlink_paths(&self.index.pages[*index])
                    .iter()
                    .any(|path| source_link_base(path) == target)
            })
            .collect()
    }

    pub(super) fn outgoing_link_source_indices(&self, raw: &str) -> Vec<usize> {
        let Some(source) = self.resolve_source_link(raw) else {
            return Vec::new();
        };
        let source = source_link_base(&source);
        let Some(page_index) = self.index.by_path.get(source).copied() else {
            return Vec::new();
        };

        let mut indices = Vec::new();
        let mut seen = HashSet::new();
        for path in page_outlink_paths(&self.index.pages[page_index]) {
            let path = source_link_base(&path);
            if let Some(index) = self.index.by_path.get(path).copied()
                && seen.insert(index)
            {
                indices.push(index);
            }
        }
        indices
    }

    pub(super) fn resolve_source_link(&self, raw: &str) -> Option<String> {
        let target = native_link_target(raw)?;
        self.index.resolve_target_path(&target)
    }

    pub(super) fn page_value(&self, page_index: usize) -> DataviewValue {
        let Some(page) = self.index.pages.get(page_index) else {
            return DataviewValue::Null;
        };
        DataviewValue::object(
            page.fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    pub(super) fn page_field_value(
        &self,
        page_index: usize,
        field: &str,
    ) -> DataviewValue {
        self.index
            .pages
            .get(page_index)
            .and_then(|page| page.fields.get(field))
            .cloned()
            .unwrap_or(DataviewValue::Null)
    }

    pub(super) fn attr_value(
        &self,
        value: &DataviewValue,
        field: &str,
    ) -> DataviewValue {
        match value {
            DataviewValue::Object(_) => value
                .as_object()
                .and_then(|object| object.get(field).cloned())
                .unwrap_or(DataviewValue::Null),
            DataviewValue::Array(values) => DataviewValue::array(
                values
                    .iter()
                    .map(|value| self.attr_value(value, field))
                    .collect(),
            ),
            DataviewValue::Link(link) => self
                .link_intrinsic_value(link, field)
                .or_else(|| {
                    self.resolve_link_path(&link.path)
                        .map(|index| self.page_field_value(index, field))
                })
                .unwrap_or(DataviewValue::Null),
            DataviewValue::String(value) => self
                .resolve_link(value)
                .map(|index| self.page_field_value(index, field))
                .unwrap_or(DataviewValue::Null),
            DataviewValue::Null
            | DataviewValue::Bool(_)
            | DataviewValue::Number(_)
            | DataviewValue::Date(_)
            | DataviewValue::DateTime(_)
            | DataviewValue::Duration(_) => DataviewValue::Null,
        }
    }

    pub(super) fn link_intrinsic_value(
        &self,
        link: &DataviewLink,
        field: &str,
    ) -> Option<DataviewValue> {
        match field {
            "path" => Some(DataviewValue::String(link.path.clone())),
            "display" => Some(
                link.display
                    .clone()
                    .map(DataviewValue::String)
                    .unwrap_or(DataviewValue::Null),
            ),
            "embed" => Some(DataviewValue::Bool(link.embed)),
            _ => None,
        }
    }

    pub(super) fn resolve_link(&self, raw: &str) -> Option<usize> {
        self.index
            .resolve_link_path(raw)
            .and_then(|path| self.resolve_link_path(&path))
    }

    pub(super) fn resolve_link_path(&self, path: &str) -> Option<usize> {
        let path = path.split_once('#').map_or(path, |(path, _)| path);
        self.index.by_path.get(path).copied()
    }

    pub(super) fn field_value_matches_link(
        &self,
        value: &DataviewValue,
        expected: &str,
    ) -> bool {
        let actual = match value {
            DataviewValue::Link(link) => Some(link.path.clone()),
            DataviewValue::String(value) => comparable_link_path(value),
            _ => None,
        };
        let Some(actual) = actual else { return false };

        match (self.resolve_link_path(&actual), self.resolve_link(expected)) {
            (Some(actual), Some(expected)) => actual == expected,
            _ => Some(actual) == comparable_link_path(expected),
        }
    }
}

impl NativeRow {
    pub(super) fn page(vault: &NativeVault, page_index: usize) -> Self {
        Self {
            page_index,
            source_page_index: Some(page_index),
            value: vault.page_value(page_index),
            variables: BTreeMap::new(),
        }
    }

    pub(super) fn task(page_index: usize, value: DataviewValue) -> Self {
        Self {
            page_index,
            source_page_index: Some(page_index),
            value,
            variables: BTreeMap::new(),
        }
    }

    pub(super) fn group(
        page_index: usize,
        key: DataviewValue,
        rows: Vec<Self>,
        expression: &NativeExpression,
        alias: Option<&str>,
    ) -> Self {
        let rows_value = DataviewValue::array(
            rows.into_iter().map(|row| row.value).collect(),
        );
        let field = alias.unwrap_or(&expression.raw).to_string();
        let mut object = BTreeMap::new();
        object.insert("key".to_string(), key.clone());
        object.insert("rows".to_string(), rows_value.clone());
        object.insert(field.clone(), key.clone());

        let mut variables = BTreeMap::new();
        variables.insert("key".to_string(), key.clone());
        variables.insert("rows".to_string(), rows_value);
        variables.insert(field, key);

        Self {
            page_index,
            source_page_index: None,
            value: DataviewValue::object(object),
            variables,
        }
    }

    pub(super) fn context<'a>(
        &'a self,
        vault: &'a NativeVault,
    ) -> EvalContext<'a> {
        EvalContext {
            vault,
            page_index: self.page_index,
            row_value: &self.value,
            variables: Cow::Borrowed(&self.variables),
        }
    }

    pub(super) fn identity_value(&self, vault: &NativeVault) -> DataviewValue {
        self.source_page_index.map_or_else(
            || self.group_key_value(),
            |page_index| {
                DataviewValue::Link(DataviewLink::page(
                    &vault.index.pages[page_index].path,
                ))
            },
        )
    }

    pub(super) fn group_key_value(&self) -> DataviewValue {
        self.value
            .as_object_field("key")
            .cloned()
            .unwrap_or(DataviewValue::Null)
    }

    pub(super) fn display_value(&self, vault: &NativeVault) -> String {
        let Some(page_index) = self.source_page_index else {
            return display_text(&self.value);
        };
        let file = vault.page_field_value(page_index, "file");
        let name = vault.attr_value(&file, "name");
        let name = value_text(&name);
        if name.is_empty() {
            display_text(&self.identity_value(vault))
        } else {
            name
        }
    }

    pub(super) fn with_field(
        mut self,
        field: &str,
        value: DataviewValue,
    ) -> Self {
        self.variables.insert(field.to_string(), value.clone());
        if let Some(object) = self.value.object_mut() {
            object.insert(field.to_string(), value);
        }
        self
    }
}

impl DataviewValue {
    pub(super) fn as_object_field(
        &self,
        field: &str,
    ) -> Option<&DataviewValue> {
        let Self::Object(object) = self else {
            return None;
        };
        object.get(field)
    }
}

pub(super) fn list_pair_json(key: Value, value: Value) -> Value {
    serde_json::json!({
        "$widget": "dataview:list-pair",
        "key": key,
        "value": value,
    })
}

pub(super) fn identity_meaning_json(grouped: bool) -> Value {
    if grouped {
        serde_json::json!({ "type": "group" })
    } else {
        serde_json::json!({ "type": "path" })
    }
}

pub(super) fn value_group_key(value: &DataviewValue) -> String {
    serde_json::to_string(&value.to_plain_json())
        .unwrap_or_else(|_| value_text(value))
}

pub(super) fn calendar_date_text(value: &DataviewValue) -> Option<String> {
    match value {
        DataviewValue::Date(value) => Some(value.clone()),
        DataviewValue::DateTime(value) => Some(value.clone()),
        DataviewValue::String(value) if date_from_text(value).is_some() => {
            Some(value.clone())
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FlattenOverflow {
    CheckedAdd,
    Limit { attempted: usize },
}

pub(super) fn flatten_expansion_len(value: &DataviewValue) -> usize {
    match value {
        DataviewValue::Array(values) => values.len(),
        _ => 1,
    }
}

pub(super) fn checked_flatten_len(
    current: usize,
    expansion: usize,
    limit: usize,
) -> Result<usize, FlattenOverflow> {
    let next = current
        .checked_add(expansion)
        .ok_or(FlattenOverflow::CheckedAdd)?;
    if next > limit {
        Err(FlattenOverflow::Limit { attempted: next })
    } else {
        Ok(next)
    }
}

fn flatten_command_text(
    expression: &NativeExpression,
    alias: Option<&str>,
) -> String {
    match alias {
        Some(alias) => format!("FLATTEN {} AS {alias}", expression.raw),
        None => format!("FLATTEN {}", expression.raw),
    }
}

fn flatten_overflow_error(
    expression: &NativeExpression,
    alias: Option<&str>,
    kind: FlattenOverflow,
    current: usize,
    expansion: usize,
    limit: usize,
) -> DataviewError {
    let command = flatten_command_text(expression, alias);
    let attempted = match kind {
        FlattenOverflow::CheckedAdd => current.saturating_add(expansion),
        FlattenOverflow::Limit { attempted } => attempted,
    };
    DataviewError::NativeQuery {
        message: format!(
            "native {command} would exceed the {limit}-row limit \
             (at least {attempted} rows). Narrow FROM or filter page rows \
             with WHERE before FLATTEN; for task results, consider a DQL \
             TASK query."
        ),
    }
}
