//! Sandboxed JavaScript support for Tasks' `by function` instructions.
//!
//! The Phase 5 spike first tried Boa. Boa 0.21.1 parsed Moment 2.29.4 but
//! failed in Moment's date-construction path, so the design's documented
//! fallback to QuickJS is used here. Moment is vendored at its Tasks v8
//! dependency version and covered by `vendor/MOMENT_LICENSE`.

use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::NaiveDateTime;
use rquickjs::{CatchResultExt, Context, Runtime};
use serde_json::{json, Value};

use super::{
    super::DataviewError,
    parse::{
        GroupInstruction, GroupKey, QueryContext, SortInstruction, SortKey,
    },
    task::Task,
};

const MOMENT_SOURCE: &str = include_str!("vendor/moment.min.js");
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_STACK_BYTES: usize = 1024 * 1024;
const EXPRESSION_TIMEOUT: Duration = Duration::from_secs(2);

type Deadline = Arc<Mutex<Instant>>;

const BOOTSTRAP_SOURCE: &str = r#"
const __bobOriginalMoment = globalThis.moment;
const __bobPinnedNow = globalThis.__bobNow;
function __bobMoment(...args) {
    if (args.length === 0) {
        return __bobOriginalMoment(__bobPinnedNow, "YYYY-MM-DDTHH:mm:ss", true);
    }
    return __bobOriginalMoment(...args);
}
for (const key of Object.keys(__bobOriginalMoment)) {
    __bobMoment[key] = __bobOriginalMoment[key];
}
__bobMoment.fn = __bobOriginalMoment.fn;
globalThis.moment = __bobMoment;

class TasksDate {
    constructor(raw) {
        this._moment = raw === null || raw === undefined
            ? null
            : moment(raw, "YYYY-MM-DD", true);
    }
    get moment() {
        return this._moment === null ? null : this._moment.clone();
    }
    format(format, fallBackText = "") {
        return this._moment === null ? fallBackText : this._moment.format(format);
    }
    formatAsDate(fallBackText = "") {
        return this.format("YYYY-MM-DD", fallBackText);
    }
    formatAsDateAndTime(fallBackText = "") {
        return this.format("YYYY-MM-DD HH:mm", fallBackText);
    }
    toISOString(keepOffset) {
        return this._moment === null ? "" : this._moment.toISOString(keepOffset);
    }
}

const hydrateTasksFile = function(file) {
    if (file === null || file === undefined) return file;
    const properties = file.__properties || {};
    file.frontmatter = properties;
    file.cachedMetadata = { frontmatter: properties };
    file.tags = file.tags || [];
    file.outlinks = file.outlinks || [];
    file.outlinksInProperties = file.outlinksInProperties || [];
    file.outlinksInBody = file.outlinksInBody || [];
    file.hasProperty = function(key) {
        const found = Object.keys(properties).find(
            candidate => candidate.toLowerCase() === String(key).toLowerCase()
        );
        return found !== undefined && properties[found] !== null && properties[found] !== undefined;
    };
    file.property = function(key) {
        const found = Object.keys(properties).find(
            candidate => candidate.toLowerCase() === String(key).toLowerCase()
        );
        if (found === undefined || properties[found] === undefined) return null;
        const value = properties[found];
        return Array.isArray(value) ? value.filter(item => item !== null) : value;
    };
    return file;
};

function __bobHydrateTask(task) {
    task.file = hydrateTasksFile(task.file);
    task.priorityName = task.priority;
    for (const field of ["cancelled", "created", "done", "due", "scheduled", "start"]) {
        const value = task[field];
        task[field] = new TasksDate(value === null ? null : value.raw);
    }
    const happens = [task.start.moment, task.scheduled.moment, task.due.moment]
        .filter(value => value !== null && value.isValid())
        .sort((left, right) => left.valueOf() - right.valueOf());
    task.happens = new TasksDate(happens.length === 0 ? null : happens[0].format("YYYY-MM-DD"));
    task.isBlocked = function(allTasks) {
        if (this.dependsOn.length === 0 || this.isDone) return false;
        return this.dependsOn.some(id => allTasks.some(candidate => candidate.id === id && !candidate.isDone));
    };
    task.isBlocking = function(allTasks) {
        if (this.id === "" || this.isDone) return false;
        return allTasks.some(candidate => !candidate.isDone && candidate.dependsOn.includes(this.id));
    };
    return task;
}

function __bobType(value) {
    if (value === null) return "null";
    if (moment.isMoment(value)) return "Moment";
    if (value instanceof TasksDate) return "TasksDate";
    if (typeof value === "object") return value.constructor.name;
    return typeof value;
}

function __bobCompareDates(left, right) {
    if (left === null && right === null) return 0;
    if (left === null) return 1;
    if (right === null) return -1;
    return left.valueOf() - right.valueOf();
}

function __bobCompareSortKeys(left, right) {
    if (left === undefined || Number.isNaN(left) || Array.isArray(left)) {
        const type = left === undefined ? "undefined" : Number.isNaN(left) ? "NaN (Not a Number)" : "array";
        throw new Error(`\"${type}\" is not a valid sort key`);
    }
    if (right === undefined || Number.isNaN(right) || Array.isArray(right)) {
        const type = right === undefined ? "undefined" : Number.isNaN(right) ? "NaN (Not a Number)" : "array";
        throw new Error(`\"${type}\" is not a valid sort key`);
    }
    const leftType = __bobType(left);
    const rightType = __bobType(right);
    const leftIsMoment = leftType === "Moment";
    const rightIsMoment = rightType === "Moment";
    if ((leftIsMoment && rightIsMoment) || (leftIsMoment && right === null) || (rightIsMoment && left === null)) {
        return __bobCompareDates(left, right);
    }
    if (left === null && right === null) return 0;
    if (left === null) return -1;
    if (right === null) return 1;
    if (leftType !== rightType) {
        throw new Error(`Unable to compare two different sort key types '${leftType}' and '${rightType}' order`);
    }
    if (leftType === "string") return left.localeCompare(right, undefined, { numeric: true });
    if (leftType === "TasksDate") return __bobCompareDates(left.moment, right.moment);
    if (leftType === "boolean") return Number(right) - Number(left);
    const result = Number(left) - Number(right);
    if (Number.isNaN(result)) {
        throw new Error(`Unable to determine sort order for sort key types '${leftType}' and '${rightType}'`);
    }
    return result;
}

function __bobGroupKeys(value) {
    if (Array.isArray(value)) return value.map(item => item.toString());
    if (value === null) return [];
    if (typeof value === "number" && !Number.isInteger(value)) return [value.toFixed(5)];
    return [value.toString()];
}
"#;

const HYDRATE_SOURCE: &str = r#"
globalThis.__bobTasks = globalThis.__bobTasks.map(__bobHydrateTask);
if (globalThis.__bobQuery.file !== null) {
    globalThis.__bobQuery.file = hydrateTasksFile(globalThis.__bobQuery.file);
}
globalThis.__bobQuery.allTasks = globalThis.__bobTasks;
globalThis.__bobQuery.searchCache = {};
"#;

pub(super) struct JsSandbox {
    _runtime: Runtime,
    context: Context,
    deadline: Deadline,
    task_indices: HashMap<(String, usize), usize>,
}

impl JsSandbox {
    pub(super) fn new(
        tasks: &[Task],
        query_context: Option<&QueryContext>,
        now: NaiveDateTime,
    ) -> Result<Self, DataviewError> {
        let tasks_json = serde_json::to_value(tasks).map_err(|error| {
            query_error(format!(
                "could not expose tasks to JavaScript: {error}"
            ))
        })?;
        let query_json = query_value(query_context)?;
        let tasks_json =
            serde_json::to_string(&tasks_json).map_err(|error| {
                query_error(format!(
                    "could not encode JavaScript tasks: {error}"
                ))
            })?;
        let query_json =
            serde_json::to_string(&query_json).map_err(|error| {
                query_error(format!(
                    "could not encode JavaScript query: {error}"
                ))
            })?;
        let tasks_json = serde_json::to_string(&tasks_json)
            .expect("serializing a JSON string cannot fail");
        let query_json = serde_json::to_string(&query_json)
            .expect("serializing a JSON string cannot fail");
        let now =
            serde_json::to_string(&now.format("%Y-%m-%dT%H:%M:%S").to_string())
                .expect("serializing a clock string cannot fail");

        let runtime = Runtime::new().map_err(|error| {
            query_error(format!(
                "could not create the JavaScript runtime: {error}"
            ))
        })?;
        runtime.set_memory_limit(MAX_MEMORY_BYTES);
        runtime.set_max_stack_size(MAX_STACK_BYTES);
        let deadline =
            Arc::new(Mutex::new(Instant::now() + EXPRESSION_TIMEOUT));
        let interrupt_deadline = Arc::clone(&deadline);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            Instant::now()
                >= *interrupt_deadline
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
        })));
        let context = Context::full(&runtime).map_err(|error| {
            query_error(format!(
                "could not create the JavaScript context: {error}"
            ))
        })?;
        let initialization = format!(
            "globalThis.__bobTasks = JSON.parse({tasks_json});\n\
             globalThis.__bobQuery = JSON.parse({query_json});\n\
             globalThis.__bobNow = {now};\n\
             {MOMENT_SOURCE}\n{BOOTSTRAP_SOURCE}\n{HYDRATE_SOURCE}"
        );
        eval_unit(
            &context,
            &deadline,
            &initialization,
            "initializing the Tasks JavaScript sandbox",
        )?;
        eval_unit(
            &context,
            &deadline,
            "if (typeof moment !== 'function' || !moment().isValid()) throw new Error('Moment initialization failed');",
            "verifying the pinned Moment.js clock",
        )?;

        let task_indices = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| ((task.path.clone(), task.line_number), index))
            .collect();
        Ok(Self {
            _runtime: runtime,
            context,
            deadline,
            task_indices,
        })
    }

    pub(super) fn validate_expression(
        &mut self,
        source: &str,
    ) -> Result<(), DataviewError> {
        eval_unit(
            &self.context,
            &self.deadline,
            &format!("void {};", expression_function(source)),
            "parsing by-function expression",
        )
    }

    pub(super) fn matches_filter(
        &mut self,
        source: &str,
        task: &Task,
    ) -> Result<bool, DataviewError> {
        let expression = self.expression_for(source, task)?;
        let result = eval_string(
            &self.context,
            &self.deadline,
            &format!(
                "(() => {{ const value = {expression}; return JSON.stringify({{\
                 isBoolean: typeof value === 'boolean', value: value === true, display: String(value)\
                 }}); }})()"
            ),
            "evaluating filter by function",
        )?;
        let result: Value = serde_json::from_str(&result).map_err(|error| {
            query_error(format!(
                "could not read JavaScript filter result: {error}"
            ))
        })?;
        if result["isBoolean"].as_bool() == Some(true) {
            Ok(result["value"].as_bool().unwrap_or(false))
        } else {
            let returned =
                result["display"].as_str().unwrap_or("<unprintable value>");
            Err(query_error(format!(
                "filtering function must return true or false. This returned \"{returned}\"."
            )))
        }
    }

    pub(super) fn validate_function_sorts(
        &mut self,
        sorting: &[SortInstruction],
    ) -> Result<(), DataviewError> {
        let functions = sorting
            .iter()
            .filter(|instruction| instruction.key == SortKey::Function)
            .collect::<Vec<_>>();
        for instruction in &functions {
            self.validate_expression(
                instruction.function.as_deref().unwrap_or_default(),
            )?;
        }
        Ok(())
    }

    pub(super) fn compare_function_sort(
        &mut self,
        source: &str,
        left: &Task,
        right: &Task,
    ) -> Result<Ordering, DataviewError> {
        self.compare(source, left, right).map_err(|message| {
            query_error(format!(
                "{message}: while evaluating instruction 'sort by function {source}'"
            ))
        })
    }

    pub(super) fn function_groups(
        &mut self,
        grouping: &[GroupInstruction],
        tasks: &[Task],
    ) -> Value {
        let evaluations = grouping
            .iter()
            .filter(|instruction| instruction.key == GroupKey::Function)
            .map(|instruction| {
                let source =
                    instruction.function.as_deref().unwrap_or_default();
                let entries = tasks
                    .iter()
                    .map(|task| {
                        let groups = self.function_group_keys(source, task);
                        json!({
                            "path": task.path,
                            "lineNumber": task.line_number,
                            "groups": groups,
                        })
                    })
                    .collect::<Vec<_>>();
                json!({
                    "function": source,
                    "reverse": instruction.reverse,
                    "tasks": entries,
                })
            })
            .collect::<Vec<_>>();
        Value::Array(evaluations)
    }

    pub(super) fn function_group_keys(
        &mut self,
        source: &str,
        task: &Task,
    ) -> Vec<String> {
        let result = self.expression_for(source, task).and_then(|expression| {
            eval_string(
                &self.context,
                &self.deadline,
                &format!("JSON.stringify(__bobGroupKeys({expression}))"),
                "evaluating group by function",
            )
        });
        match result.and_then(|value| {
            serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
                query_error(format!("could not read JavaScript group keys: {error}"))
            })
        }) {
            Ok(values) => values,
            Err(error) => vec![format!(
                "Error: Failed calculating expression \"{source}\". The error message was: {}",
                task_error_message(error)
            )],
        }
    }

    fn compare(
        &mut self,
        source: &str,
        left: &Task,
        right: &Task,
    ) -> Result<Ordering, String> {
        let left = self
            .expression_for(source, left)
            .map_err(task_error_message)?;
        let right = self
            .expression_for(source, right)
            .map_err(task_error_message)?;
        let number = eval_number(
            &self.context,
            &self.deadline,
            &format!("__bobCompareSortKeys({left}, {right})"),
            "evaluating sort by function",
        )
        .map_err(task_error_message)?;
        Ok(if number < 0.0 {
            Ordering::Less
        } else if number > 0.0 {
            Ordering::Greater
        } else {
            Ordering::Equal
        })
    }

    fn expression_for(
        &self,
        source: &str,
        task: &Task,
    ) -> Result<String, DataviewError> {
        let index = self
            .task_indices
            .get(&(task.path.clone(), task.line_number))
            .copied()
            .ok_or_else(|| {
                query_error("task was not registered in the JavaScript sandbox")
            })?;
        Ok(format!(
            "{}(__bobTasks[{index}], __bobQuery)",
            expression_function(source)
        ))
    }
}

fn expression_function(source: &str) -> String {
    if source.contains("return") {
        format!("(function(task, query) {{ {source} }})")
    } else {
        format!("(function(task, query) {{ return ({source}); }})")
    }
}

fn query_value(context: Option<&QueryContext>) -> Result<Value, DataviewError> {
    let file = context
        .map(|context| {
            let mut file = serde_json::to_value(&context.file).map_err(|error| {
                query_error(format!("could not expose query file to JavaScript: {error}"))
            })?;
            let properties = serde_json::to_value(&context.properties).map_err(|error| {
                query_error(format!("could not expose query properties to JavaScript: {error}"))
            })?;
            file.as_object_mut()
                .expect("TaskFile serializes as an object")
                .insert("__properties".to_string(), properties);
            Ok(file)
        })
        .transpose()?;
    Ok(json!({ "file": file }))
}

fn eval_unit(
    context: &Context,
    deadline: &Deadline,
    source: &str,
    action: &str,
) -> Result<(), DataviewError> {
    arm_deadline(deadline);
    context.with(|context| {
        context
            .eval::<(), _>(source)
            .catch(&context)
            .map_err(|error| {
                query_error(format!("JavaScript error while {action}: {error}"))
            })
    })
}

fn eval_string(
    context: &Context,
    deadline: &Deadline,
    source: &str,
    action: &str,
) -> Result<String, DataviewError> {
    arm_deadline(deadline);
    context.with(|context| {
        context
            .eval::<String, _>(source)
            .catch(&context)
            .map_err(|error| {
                query_error(format!("JavaScript error while {action}: {error}"))
            })
    })
}

fn eval_number(
    context: &Context,
    deadline: &Deadline,
    source: &str,
    action: &str,
) -> Result<f64, DataviewError> {
    arm_deadline(deadline);
    context.with(|context| {
        context
            .eval::<f64, _>(source)
            .catch(&context)
            .map_err(|error| {
                query_error(format!("JavaScript error while {action}: {error}"))
            })
    })
}

fn arm_deadline(deadline: &Deadline) {
    *deadline
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Instant::now() + EXPRESSION_TIMEOUT;
}

fn query_error(message: impl Into<String>) -> DataviewError {
    DataviewError::TasksQuery {
        message: message.into(),
    }
}

fn task_error_message(error: DataviewError) -> String {
    match error {
        DataviewError::TasksQuery { message } => message,
        other => format!("{other:?}"),
    }
}
