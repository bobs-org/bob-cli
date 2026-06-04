use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read},
    iter,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
    thread,
    time::Duration,
};

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use clap::{
    builder::{NonEmptyStringValueParser, OsStringValueParser},
    error::ErrorKind,
    Arg, ArgAction, ArgGroup, ArgMatches, Command as ClapCommand,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

use self::{
    index::DataviewIndex,
    value::{DataviewLink, DataviewValue},
};
use super::env as bob_env;

mod index;
mod value;

const COMMAND_NAME: &str = "bob dataview";
const ENV_OBSIDIAN_COMMAND: &str = "BOB_DATAVIEW_OBSIDIAN_COMMAND";
const ENV_VAULT: &str = "BOB_DATAVIEW_VAULT";
const RESULT_PREFIX: &str = "BOB_DATAVIEW_RESULT\t";
const OBSIDIAN_EVAL_SCRIPT: &str = r#"
(async () => {
  function plain(value, seen = new WeakSet()) {
    if (value == null || typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
      return value;
    }
    if (typeof value === "bigint") {
      return value.toString();
    }
    if (Array.isArray(value)) {
      return value.map((item) => plain(item, seen));
    }
    if (typeof value !== "object") {
      return String(value);
    }
    if (seen.has(value)) {
      return "[Circular]";
    }
    seen.add(value);
    if (typeof value.path === "string" && ("display" in value || "embed" in value || "type" in value)) {
      return {
        type: "link",
        path: value.path,
        display: value.display ?? null,
        embed: Boolean(value.embed),
      };
    }
    if (typeof value.toISO === "function") {
      try {
        return value.toISO();
      } catch (_error) {
      }
    }
    if (typeof value.array === "function") {
      try {
        return plain(value.array(), seen);
      } catch (_error) {
      }
    }
    const output = {};
    for (const [key, item] of Object.entries(value)) {
      if (typeof item !== "function") {
        output[key] = plain(item, seen);
      }
    }
    return output;
  }

  function messageFor(error) {
    if (error == null) {
      return "unknown error";
    }
    if (typeof error === "string") {
      return error;
    }
    if (typeof error.message === "string" && error.message.length > 0) {
      return error.message;
    }
    return JSON.stringify(plain(error));
  }

  function dataviewApi() {
    return globalThis.app?.plugins?.plugins?.dataview?.api
      ?? globalThis.window?.DataviewAPI
      ?? globalThis.DataviewAPI;
  }

  async function sleep(milliseconds) {
    await new Promise((resolve) => setTimeout(resolve, milliseconds));
  }

  async function waitForDataview() {
    for (let attempt = 0; attempt < 50; attempt += 1) {
      const api = dataviewApi();
      if (api) {
        return api;
      }
      await sleep(100);
    }
    const error = new Error("Dataview is disabled, missing, or not loaded in this Obsidian vault");
    error.bobCode = "DATAVIEW_MISSING";
    throw error;
  }

  async function waitForIndexReady() {
    if (globalThis.app?.metadataCache?.on) {
      await Promise.race([
        new Promise((resolve) => {
          const off = globalThis.app.metadataCache.on("dataview:index-ready", () => {
            if (typeof off === "function") {
              off();
            }
            resolve();
          });
        }),
        sleep(1500),
      ]);
    } else {
      await sleep(250);
    }
  }

  function unwrapDataviewResult(result) {
    if (result && typeof result === "object" && result.successful === false) {
      const error = new Error(messageFor(result.error ?? result));
      error.bobCode = "DATAVIEW_QUERY_ERROR";
      error.details = result.error ?? result;
      throw error;
    }
    if (result && typeof result === "object" && result.successful === true && "value" in result) {
      return result.value;
    }
    return result;
  }

  function emit(payload) {
    console.log(resultPrefix + JSON.stringify(payload));
  }

  try {
    const api = await waitForDataview();
    await waitForIndexReady();

    if (request.query.kind === "source") {
      const paths = Array.from(await api.pagePaths(request.query.source) ?? []);
      emit({
        status: "ok",
        kind: "source_paths",
        paths: plain(paths),
        warnings: [],
      });
      return;
    }

    const origin = request.origin ?? undefined;
    if (request.format === "markdown") {
      const markdown = unwrapDataviewResult(await api.tryQueryMarkdown(request.query.query, origin));
      emit({
        status: "ok",
        kind: "markdown",
        markdown: String(markdown ?? ""),
        warnings: [],
      });
      return;
    }

    const result = unwrapDataviewResult(await api.tryQuery(request.query.query, origin, { forceId: true }));
    emit({
      status: "ok",
      kind: "dql_json",
      result: plain(result),
      warnings: [],
    });
  } catch (error) {
    emit({
      status: "error",
      code: error?.bobCode ?? "ENGINE_ERROR",
      message: messageFor(error),
      details: plain(error?.details ?? error),
    });
  }
})();
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    query: QueryInput,
    format: OutputFormat,
    engine: Engine,
    vault: VaultConfig,
    strict_paths: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueryInput {
    Source(String),
    Dql(DqlInput),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DqlInput {
    Inline(String),
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    Markdown,
    Paths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Engine {
    Native,
    Obsidian,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VaultConfig {
    bob_dir: PathBuf,
    origin: Option<PathBuf>,
    obsidian_vault: Option<String>,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let request = match Request::from_matches(&matches, &mut command) {
        Ok(request) => request,
        Err(error) => return print_clap_error(error),
    };

    match run_request(&request) {
        Ok(()) => 0,
        Err(error) => {
            error.report();
            error.exit_code()
        }
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn run_request(request: &Request) -> Result<(), DataviewError> {
    match request.engine {
        Engine::Obsidian => run_obsidian(request),
        Engine::Native => run_native(request),
    }
}

fn run_obsidian(request: &Request) -> Result<(), DataviewError> {
    let eval_request = request.obsidian_eval_request()?;

    let javascript = build_obsidian_javascript(&eval_request)?;
    let output = run_obsidian_eval(&request.vault, &javascript)?;
    let engine_output = parse_protocol_stdout(&output.stdout)?;
    emit_engine_output(request, engine_output)
}

fn run_native(request: &Request) -> Result<(), DataviewError> {
    let vault = NativeVault::read(
        &request.vault.bob_dir,
        request.vault.origin.as_deref(),
    )?;
    match &request.query {
        QueryInput::Source(source) => {
            let source = NativeSourceExpr::parse(source)?;
            let output = vault.evaluate_source(&source);
            emit_engine_output(request, output)
        }
        QueryInput::Dql(input) => {
            let query = NativeQuery::parse(&input.read_query()?)?;
            if request.format == OutputFormat::Markdown {
                let settings =
                    NativeMarkdownSettings::read(&request.vault.bob_dir);
                let output = vault.evaluate_markdown(&query, &settings)?;
                emit_engine_output(request, output)
            } else {
                let output = vault.evaluate(&query);
                emit_native_output(request, output)
            }
        }
    }
}

fn run_obsidian_eval(
    vault: &VaultConfig,
    javascript: &str,
) -> Result<Output, DataviewError> {
    let command = obsidian_command();
    let output =
        run_obsidian_process(&command, vault, javascript).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                DataviewError::MissingObsidianCommand {
                    command: command.clone(),
                }
            } else {
                DataviewError::RunObsidian {
                    command: command.clone(),
                    error,
                }
            }
        })?;

    if output.status.success() {
        Ok(output)
    } else {
        Err(obsidian_failure(output))
    }
}

fn run_obsidian_process(
    command: &OsString,
    vault: &VaultConfig,
    javascript: &str,
) -> io::Result<Output> {
    let first = obsidian_process(command, vault, javascript).output();
    if first.as_ref().is_err_and(is_text_file_busy) {
        thread::sleep(Duration::from_millis(10));
        return obsidian_process(command, vault, javascript).output();
    }

    first
}

fn obsidian_process(
    command: &OsString,
    vault: &VaultConfig,
    javascript: &str,
) -> Command {
    let mut process = Command::new(command);
    if let Some(obsidian_vault) = &vault.obsidian_vault {
        process.arg(format!("vault={obsidian_vault}"));
    }
    process.arg("eval").arg(format!("code={javascript}"));
    process
}

fn is_text_file_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(26)
}

fn obsidian_command() -> OsString {
    env::var_os(ENV_OBSIDIAN_COMMAND)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("obsidian"))
}

fn obsidian_failure(output: Output) -> DataviewError {
    let exit_code = bob_env::exit_code(output.status);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stderr}\n{stdout}");
    let lower = combined.to_lowercase();

    if lower.contains("unable to find obsidian")
        || lower.contains("make sure obsidian is running")
        || lower.contains("could not connect")
    {
        return DataviewError::ObsidianNotRunning {
            exit_code,
            output: child_output_excerpt(&stdout, &stderr),
        };
    }

    DataviewError::ObsidianFailed {
        exit_code,
        output: child_output_excerpt(&stdout, &stderr),
    }
}

fn build_obsidian_javascript(
    request: &ObsidianEvalRequest,
) -> Result<String, DataviewError> {
    let request_json = serde_json::to_string(request)
        .map_err(DataviewError::SerializeRequest)?;
    let prefix_json = serde_json::to_string(RESULT_PREFIX)
        .map_err(DataviewError::SerializeRequest)?;

    Ok(format!(
        "const request = {request_json};\n\
         const resultPrefix = {prefix_json};\n\
         {OBSIDIAN_EVAL_SCRIPT}"
    ))
}

fn parse_protocol_stdout(stdout: &[u8]) -> Result<EngineOutput, DataviewError> {
    let stdout_text = String::from_utf8_lossy(stdout);
    let payloads = stdout_text
        .lines()
        .filter_map(|line| line.strip_prefix(RESULT_PREFIX))
        .collect::<Vec<_>>();

    match payloads.as_slice() {
        [] => Err(DataviewError::MissingProtocolSentinel {
            output: stdout_excerpt(&stdout_text),
        }),
        [payload] => parse_protocol_payload(payload),
        _ => Err(DataviewError::MalformedProtocolResponse {
            reason: "multiple sentinel responses found".to_string(),
        }),
    }
}

fn parse_protocol_payload(
    payload: &str,
) -> Result<EngineOutput, DataviewError> {
    let envelope: ProtocolEnvelope =
        serde_json::from_str(payload).map_err(|error| {
            DataviewError::MalformedProtocolResponse {
                reason: format!("invalid sentinel JSON: {error}"),
            }
        })?;

    envelope.into_engine_output()
}

fn emit_engine_output(
    request: &Request,
    output: EngineOutput,
) -> Result<(), DataviewError> {
    let EngineOutput {
        response,
        mut warnings,
    } = output;

    match (response, request.format) {
        (EngineResponse::SourcePaths(paths), OutputFormat::Paths) => {
            let extraction =
                extract_source_paths(&paths, request.strict_paths)?;
            warnings.extend(extraction.warnings);
            emit_warnings(&warnings);
            if !extraction.paths.is_empty() {
                println!("{}", extraction.paths.join("\n"));
            }
            Ok(())
        }
        (EngineResponse::SourcePaths(paths), OutputFormat::Json) => {
            let extraction = extract_source_paths(&paths, false)?;
            warnings.extend(extraction.warnings);
            emit_warnings(&warnings);
            print_json(serde_json::json!({
                "engine": request.engine.as_str(),
                "query_kind": "source",
                "format": request.format.as_str(),
                "paths": extraction.paths,
                "warnings": warnings,
            }))
        }
        (EngineResponse::DqlJson(result), OutputFormat::Json) => {
            let extraction = extract_dql_paths(&result, false)?;
            warnings.extend(extraction.warnings);
            emit_warnings(&warnings);
            print_json(serde_json::json!({
                "engine": request.engine.as_str(),
                "query_kind": "dql",
                "format": request.format.as_str(),
                "paths": extraction.paths,
                "result": result,
                "warnings": warnings,
            }))
        }
        (EngineResponse::DqlJson(result), OutputFormat::Paths) => {
            let extraction =
                extract_dql_paths(&result, request.strict_paths)?;
            warnings.extend(extraction.warnings);
            emit_warnings(&warnings);
            if !extraction.paths.is_empty() {
                println!("{}", extraction.paths.join("\n"));
            }
            Ok(())
        }
        (EngineResponse::DqlJson(_), OutputFormat::Markdown) => Err(
            DataviewError::MalformedProtocolResponse {
                reason:
                    "DQL JSON protocol response did not match requested format"
                        .to_string(),
            },
        ),
        (EngineResponse::Markdown(markdown), OutputFormat::Markdown) => {
            emit_warnings(&warnings);
            print!("{markdown}");
            Ok(())
        }
        (EngineResponse::Markdown(_), _) => Err(
            DataviewError::MalformedProtocolResponse {
                reason:
                    "markdown protocol response did not match requested format"
                        .to_string(),
            },
        ),
        (EngineResponse::SourcePaths(_), OutputFormat::Markdown) => Err(
            DataviewError::MalformedProtocolResponse {
                reason:
                    "source path protocol response did not match requested format"
                        .to_string(),
            },
        ),
    }
}

fn emit_native_output(
    request: &Request,
    output: NativeOutput,
) -> Result<(), DataviewError> {
    let NativeOutput {
        result,
        mut warnings,
    } = output;

    match request.format {
        OutputFormat::Paths => {
            let extraction = extract_dql_paths(&result, request.strict_paths)?;
            warnings.extend(extraction.warnings);
            emit_warnings(&warnings);
            if !extraction.paths.is_empty() {
                println!("{}", extraction.paths.join("\n"));
            }
            Ok(())
        }
        OutputFormat::Json => {
            let extraction = extract_dql_paths(&result, false)?;
            warnings.extend(extraction.warnings);
            emit_warnings(&warnings);
            print_json(serde_json::json!({
                "engine": request.engine.as_str(),
                "query_kind": "dql",
                "format": request.format.as_str(),
                "paths": extraction.paths,
                "result": result,
                "warnings": warnings,
            }))
        }
        OutputFormat::Markdown => unreachable!(
            "native markdown output is handled before native JSON emission"
        ),
    }
}

fn emit_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("{COMMAND_NAME}: warning: {warning}");
    }
}

fn print_json(value: Value) -> Result<(), DataviewError> {
    let json = serde_json::to_string(&value)
        .map_err(DataviewError::SerializeOutput)?;
    println!("{json}");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathExtraction {
    paths: Vec<String>,
    warnings: Vec<String>,
}

fn extract_source_paths(
    paths: &[String],
    strict: bool,
) -> Result<PathExtraction, DataviewError> {
    let mut collector = PathCollector::default();
    for (index, path) in paths.iter().enumerate() {
        let context = format!("source path {}", index + 1);
        collector.add_raw_path(path, &context);
    }
    collector.finish(strict)
}

fn extract_dql_paths(
    result: &Value,
    strict: bool,
) -> Result<PathExtraction, DataviewError> {
    let mut collector = PathCollector::default();
    match result.get("type").and_then(Value::as_str) {
        Some("list") => collect_list_paths(result, &mut collector),
        Some("table") => collect_table_paths(result, &mut collector),
        Some("task") => collect_task_paths(result, &mut collector),
        Some("calendar") => collect_calendar_paths(result, &mut collector),
        Some(other) => collector.warn(format!(
            "DQL {other} results do not have supported path extraction"
        )),
        None => collect_unknown_result_paths(result, &mut collector),
    }
    collector.finish(strict)
}

#[derive(Debug)]
struct NativeOutput {
    warnings: Vec<String>,
    result: Value,
}

#[derive(Debug, Clone)]
struct NativeMarkdownSettings {
    render_null_as: String,
    table_id_column_name: String,
    table_group_column_name: String,
}

#[derive(Debug)]
struct NativeQuery {
    kind: NativeQueryKind,
    commands: Vec<NativeDataCommand>,
}

#[derive(Debug)]
enum NativeQueryKind {
    List {
        expression: Option<NativeExpression>,
        without_id: bool,
    },
    Table {
        columns: Vec<NativeSelect>,
        without_id: bool,
    },
    Task {
        _without_id: bool,
    },
    Calendar {
        expression: NativeExpression,
        _without_id: bool,
    },
}

#[derive(Debug)]
struct NativeSelect {
    expression: NativeExpression,
    alias: Option<String>,
}

#[derive(Debug)]
struct NativeExpression {
    raw: String,
    expr: NativeExpr,
}

#[derive(Debug)]
enum NativeDataCommand {
    From(NativeSourceExpr),
    Where(NativeExpression),
    Sort {
        expression: NativeExpression,
        direction: Option<SortDirection>,
    },
    GroupBy {
        expression: NativeExpression,
        alias: Option<String>,
    },
    Flatten {
        expression: NativeExpression,
        alias: Option<String>,
    },
    Limit(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug)]
enum NativeSourceExpr {
    All,
    And(Box<NativeSourceExpr>, Box<NativeSourceExpr>),
    IncomingLink(String),
    Not(Box<NativeSourceExpr>),
    Or(Box<NativeSourceExpr>, Box<NativeSourceExpr>),
    OutgoingLink(String),
    Path(String),
    Tag(String),
}

#[derive(Debug)]
enum NativeExpr {
    Array(Vec<NativeExpr>),
    Binary {
        op: NativeBinaryOp,
        left: Box<NativeExpr>,
        right: Box<NativeExpr>,
    },
    Call {
        function: String,
        args: Vec<NativeExpr>,
    },
    GetAttr {
        target: Box<NativeExpr>,
        field: String,
    },
    Identifier(String),
    Lambda {
        parameter: String,
        body: Box<NativeExpr>,
    },
    LinkLiteral(String),
    Literal(DataviewValue),
    Object(Vec<(String, NativeExpr)>),
    Unary {
        op: NativeUnaryOp,
        expr: Box<NativeExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeBinaryOp {
    Add,
    And,
    Divide,
    Equal,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Multiply,
    NotEqual,
    Or,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeUnaryOp {
    Not,
    Negate,
}

#[derive(Debug)]
struct NativeVault {
    index: DataviewIndex,
    origin_index: Option<usize>,
}

#[derive(Debug, Clone)]
struct NativeRow {
    page_index: usize,
    source_page_index: Option<usize>,
    value: DataviewValue,
    variables: BTreeMap<String, DataviewValue>,
}

#[derive(Debug, Clone, PartialEq)]
enum NativeToken {
    And,
    As,
    Asc,
    Bool(bool),
    By,
    Calendar,
    Colon,
    Comma,
    Desc,
    Dot,
    Equal,
    Arrow,
    Eof,
    Flatten,
    From,
    Greater,
    GreaterEqual,
    Group,
    Identifier(String),
    LBrace,
    LBracket,
    Less,
    LessEqual,
    Link(String),
    List,
    LParen,
    Minus,
    Not,
    NotEqual,
    Null,
    Number(String),
    Or,
    Plus,
    RBrace,
    RBracket,
    RParen,
    Slash,
    String(String),
    Sort,
    Star,
    Tag(String),
    Table,
    Task,
    Limit,
    Without,
    Where,
    Id,
}

impl NativeQuery {
    fn parse(query: &str) -> Result<Self, DataviewError> {
        let tokens = NativeLexer::new(query)
            .tokenize()
            .map_err(native_query_error)?;
        NativeParser::new(tokens)
            .parse_query()
            .map_err(native_query_error)
    }
}

impl NativeSourceExpr {
    fn parse(source: &str) -> Result<Self, DataviewError> {
        if source.trim().is_empty() {
            return Ok(Self::All);
        }
        let tokens = NativeLexer::new(source)
            .tokenize()
            .map_err(native_query_error)?;
        NativeParser::new(tokens)
            .parse_source_query()
            .map_err(native_query_error)
    }
}

impl NativeVault {
    fn read(
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

    fn origin_index(
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

    fn evaluate_source(&self, source: &NativeSourceExpr) -> EngineOutput {
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

    fn evaluate(&self, query: &NativeQuery) -> NativeOutput {
        let rows = self.evaluate_rows(query);
        NativeOutput {
            warnings: self.index.warnings.clone(),
            result: self.result_json(query, &rows),
        }
    }

    fn evaluate_markdown(
        &self,
        query: &NativeQuery,
        settings: &NativeMarkdownSettings,
    ) -> Result<EngineOutput, DataviewError> {
        let rows = self.evaluate_rows(query);
        Ok(EngineOutput {
            response: EngineResponse::Markdown(
                self.result_markdown(query, &rows, settings)?,
            ),
            warnings: self.index.warnings.clone(),
        })
    }

    fn evaluate_rows(&self, query: &NativeQuery) -> Vec<NativeRow> {
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
                    rows =
                        self.flatten_rows(rows, expression, alias.as_deref());
                }
            }
        }
        rows
    }

    fn initial_rows(&self, query: &NativeQuery) -> Vec<NativeRow> {
        match &query.kind {
            NativeQueryKind::Task { .. } => self.task_rows(),
            NativeQueryKind::List { .. }
            | NativeQueryKind::Table { .. }
            | NativeQueryKind::Calendar { .. } => self.page_rows(),
        }
    }

    fn page_rows(&self) -> Vec<NativeRow> {
        self.index_order_indices()
            .into_iter()
            .map(|page_index| NativeRow::page(self, page_index))
            .collect()
    }

    fn task_rows(&self) -> Vec<NativeRow> {
        let mut rows = Vec::new();
        for page_index in self.index_order_indices() {
            for task in self.top_level_page_tasks(page_index) {
                rows.push(NativeRow::task(page_index, task));
            }
        }
        rows
    }

    fn top_level_page_tasks(&self, page_index: usize) -> Vec<DataviewValue> {
        let tasks = self
            .page_field_value(page_index, "file")
            .as_object_field("tasks")
            .and_then(|value| match value {
                DataviewValue::Array(values) => Some(values.clone()),
                _ => None,
            })
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

    fn filter_rows_by_source(
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

    fn group_rows(
        &self,
        rows: Vec<NativeRow>,
        expression: &NativeExpression,
        alias: Option<&str>,
    ) -> Vec<NativeRow> {
        let mut groups: Vec<(String, DataviewValue, Vec<NativeRow>)> =
            Vec::new();
        for row in rows {
            let key = expression.evaluate(&row.context(self));
            let group_key = value_group_key(&key);
            if let Some((_, _, rows)) = groups
                .iter_mut()
                .find(|(existing, _, _)| existing == &group_key)
            {
                rows.push(row);
            } else {
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

    fn flatten_rows(
        &self,
        rows: Vec<NativeRow>,
        expression: &NativeExpression,
        alias: Option<&str>,
    ) -> Vec<NativeRow> {
        let field = alias.unwrap_or(&expression.raw);
        let mut flattened = Vec::new();
        for row in rows {
            let value = expression.evaluate(&row.context(self));
            let values = match value {
                DataviewValue::Array(values) => values,
                DataviewValue::Null => vec![DataviewValue::Null],
                value => vec![value],
            };
            for value in values {
                flattened.push(row.clone().with_field(field, value));
            }
        }
        flattened
    }

    fn result_json(&self, query: &NativeQuery, rows: &[NativeRow]) -> Value {
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

    fn list_result_json(
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

    fn table_result_json(
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

    fn task_result_json(&self, rows: &[NativeRow]) -> Value {
        serde_json::json!({
            "type": "task",
            "values": rows
                .iter()
                .map(|row| row.value.to_plain_json())
                .collect::<Vec<_>>(),
        })
    }

    fn calendar_result_json(
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

    fn result_markdown(
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

    fn list_result_markdown(
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

    fn table_result_markdown(
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

    fn task_result_markdown(
        &self,
        rows: &[NativeRow],
        settings: &NativeMarkdownSettings,
    ) -> String {
        let values =
            rows.iter().map(|row| row.value.clone()).collect::<Vec<_>>();
        markdown_task_values(&values, settings, 0)
    }

    fn index_order_indices(&self) -> Vec<usize> {
        (0..self.index.pages.len()).collect()
    }

    fn source_order_indices(&self) -> Vec<usize> {
        let mut indices = self.index_order_indices();
        indices.sort_by(|left, right| {
            source_order_key(&self.index.pages[*left].path)
                .cmp(&source_order_key(&self.index.pages[*right].path))
        });
        indices
    }

    fn evaluate_source_indices(&self, source: &NativeSourceExpr) -> Vec<usize> {
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

    fn tag_source_indices(&self, tag: &str) -> Vec<usize> {
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

    fn path_source_indices(&self, raw: &str) -> Vec<usize> {
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

    fn incoming_link_source_indices(&self, raw: &str) -> Vec<usize> {
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

    fn outgoing_link_source_indices(&self, raw: &str) -> Vec<usize> {
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

    fn resolve_source_link(&self, raw: &str) -> Option<String> {
        let target = native_link_target(raw)?;
        self.index.resolve_target_path(&target)
    }

    fn page_value(&self, page_index: usize) -> DataviewValue {
        let Some(page) = self.index.pages.get(page_index) else {
            return DataviewValue::Null;
        };
        DataviewValue::Object(
            page.fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    fn page_field_value(
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

    fn attr_value(&self, value: &DataviewValue, field: &str) -> DataviewValue {
        match value {
            DataviewValue::Object(object) => {
                object.get(field).cloned().unwrap_or(DataviewValue::Null)
            }
            DataviewValue::Array(values) => DataviewValue::Array(
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

    fn link_intrinsic_value(
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

    fn resolve_link(&self, raw: &str) -> Option<usize> {
        self.index
            .resolve_link_path(raw)
            .and_then(|path| self.resolve_link_path(&path))
    }

    fn resolve_link_path(&self, path: &str) -> Option<usize> {
        let path = path.split_once('#').map_or(path, |(path, _)| path);
        self.index.by_path.get(path).copied()
    }

    fn field_value_matches_link(
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
    fn page(vault: &NativeVault, page_index: usize) -> Self {
        Self {
            page_index,
            source_page_index: Some(page_index),
            value: vault.page_value(page_index),
            variables: BTreeMap::new(),
        }
    }

    fn task(page_index: usize, value: DataviewValue) -> Self {
        Self {
            page_index,
            source_page_index: Some(page_index),
            value,
            variables: BTreeMap::new(),
        }
    }

    fn group(
        page_index: usize,
        key: DataviewValue,
        rows: Vec<Self>,
        expression: &NativeExpression,
        alias: Option<&str>,
    ) -> Self {
        let rows_value = DataviewValue::Array(
            rows.iter().map(|row| row.value.clone()).collect(),
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
            value: DataviewValue::Object(object),
            variables,
        }
    }

    fn context<'a>(&'a self, vault: &'a NativeVault) -> EvalContext<'a> {
        EvalContext {
            vault,
            page_index: self.page_index,
            row_value: &self.value,
            variables: self.variables.clone(),
        }
    }

    fn identity_value(&self, vault: &NativeVault) -> DataviewValue {
        self.source_page_index.map_or_else(
            || self.group_key_value(),
            |page_index| {
                DataviewValue::Link(DataviewLink::page(
                    &vault.index.pages[page_index].path,
                ))
            },
        )
    }

    fn group_key_value(&self) -> DataviewValue {
        self.value
            .as_object_field("key")
            .cloned()
            .unwrap_or(DataviewValue::Null)
    }

    fn display_value(&self, vault: &NativeVault) -> String {
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

    fn with_field(mut self, field: &str, value: DataviewValue) -> Self {
        self.variables.insert(field.to_string(), value.clone());
        if let DataviewValue::Object(object) = &mut self.value {
            object.insert(field.to_string(), value);
        }
        self
    }
}

impl DataviewValue {
    fn as_object_field(&self, field: &str) -> Option<&DataviewValue> {
        let Self::Object(object) = self else {
            return None;
        };
        object.get(field)
    }
}

fn list_pair_json(key: Value, value: Value) -> Value {
    serde_json::json!({
        "$widget": "dataview:list-pair",
        "key": key,
        "value": value,
    })
}

fn identity_meaning_json(grouped: bool) -> Value {
    if grouped {
        serde_json::json!({ "type": "group" })
    } else {
        serde_json::json!({ "type": "path" })
    }
}

fn value_group_key(value: &DataviewValue) -> String {
    serde_json::to_string(&value.to_plain_json())
        .unwrap_or_else(|_| value_text(value))
}

fn calendar_date_text(value: &DataviewValue) -> Option<String> {
    match value {
        DataviewValue::Date(value) => Some(value.clone()),
        DataviewValue::DateTime(value) => Some(value.clone()),
        DataviewValue::String(value) if date_from_text(value).is_some() => {
            Some(value.clone())
        }
        _ => None,
    }
}

impl NativeSelect {
    fn header(&self) -> String {
        self.alias
            .clone()
            .unwrap_or_else(|| self.expression.raw.clone())
    }
}

impl Default for NativeMarkdownSettings {
    fn default() -> Self {
        Self {
            render_null_as: "\\-".to_string(),
            table_id_column_name: "File".to_string(),
            table_group_column_name: "Group".to_string(),
        }
    }
}

impl NativeMarkdownSettings {
    fn read(bob_dir: &Path) -> Self {
        let mut settings = Self::default();
        let path = bob_dir.join(".obsidian/plugins/dataview/data.json");
        let Ok(contents) = fs::read_to_string(path) else {
            return settings;
        };
        let Ok(value) = serde_json::from_str::<Value>(&contents) else {
            return settings;
        };

        settings.apply(&value);
        settings
    }

    fn apply(&mut self, value: &Value) {
        if let Some(render_null_as) =
            value.get("renderNullAs").and_then(Value::as_str)
        {
            self.render_null_as = render_null_as.to_string();
        }
        if let Some(column_name) =
            value.get("tableIdColumnName").and_then(Value::as_str)
        {
            self.table_id_column_name = column_name.to_string();
        }
        if let Some(column_name) =
            value.get("tableGroupColumnName").and_then(Value::as_str)
        {
            self.table_group_column_name = column_name.to_string();
        }
    }
}

fn markdown_table(
    headers: &[String],
    values: &[Vec<DataviewValue>],
    settings: &NativeMarkdownSettings,
) -> String {
    let mut rendered_rows = Vec::new();
    let mut max_lengths = headers
        .iter()
        .map(|header| escape_table(header).len())
        .collect::<Vec<_>>();

    for row in values {
        let rendered = (0..headers.len())
            .map(|index| {
                row.get(index)
                    .map(|value| {
                        escape_table(&markdown_table_literal(value, settings))
                    })
                    .unwrap_or_else(|| escape_table(&settings.render_null_as))
            })
            .collect::<Vec<_>>();
        for (index, cell) in rendered.iter().enumerate() {
            max_lengths[index] = max_lengths[index].max(cell.len());
        }
        rendered_rows.push(rendered);
    }

    let mut table = String::new();
    table.push_str("| ");
    table.push_str(
        &headers
            .iter()
            .enumerate()
            .map(|(index, header)| {
                padright(&escape_table(header), max_lengths[index])
            })
            .collect::<Vec<_>>()
            .join(" | "),
    );
    table.push_str(" |\n| ");
    table.push_str(
        &max_lengths
            .iter()
            .map(|length| "-".repeat(*length))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    table.push_str(" |\n");

    for row in rendered_rows {
        table.push_str("| ");
        table.push_str(
            &row.iter()
                .enumerate()
                .map(|(index, cell)| padright(cell, max_lengths[index]))
                .collect::<Vec<_>>()
                .join(" | "),
        );
        table.push_str(" |\n");
    }

    table
}

fn markdown_table_literal(
    value: &DataviewValue,
    settings: &NativeMarkdownSettings,
) -> String {
    match value {
        DataviewValue::Array(values) => values
            .iter()
            .map(|value| markdown_literal(value, settings))
            .collect::<Vec<_>>()
            .join(", "),
        DataviewValue::Object(values) => values
            .iter()
            .map(|(key, value)| {
                format!("{key}: {}", markdown_literal(value, settings))
            })
            .collect::<Vec<_>>()
            .join(", "),
        value => markdown_literal(value, settings),
    }
}

fn markdown_task_values(
    values: &[DataviewValue],
    settings: &NativeMarkdownSettings,
    depth: usize,
) -> String {
    if !values.is_empty()
        && values.iter().all(|value| task_group_value(value).is_some())
    {
        let mut markdown = String::new();
        for value in values {
            let Some((key, rows)) = task_group_value(value) else {
                continue;
            };
            markdown.push_str(&"#".repeat(depth + 1));
            markdown.push(' ');
            markdown.push_str(&markdown_literal(key, settings));
            markdown.push_str("\n\n");
            markdown.push_str(&markdown_task_values(rows, settings, depth + 1));
        }
        return markdown;
    }

    let mut markdown = String::new();
    for value in values {
        markdown.push_str(&markdown_task_value(value, settings, depth));
    }
    markdown
}

fn task_group_value(
    value: &DataviewValue,
) -> Option<(&DataviewValue, &[DataviewValue])> {
    let key = value.as_object_field("key")?;
    let rows = match value.as_object_field("rows")? {
        DataviewValue::Array(rows) => rows.as_slice(),
        _ => return None,
    };
    Some((key, rows))
}

fn markdown_task_value(
    value: &DataviewValue,
    settings: &NativeMarkdownSettings,
    depth: usize,
) -> String {
    let indent = "  ".repeat(depth);
    let task = value
        .as_object_field("task")
        .and_then(|value| match value {
            DataviewValue::Bool(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(false);
    let status = value
        .as_object_field("status")
        .and_then(DataviewValue::as_str)
        .and_then(|value| value.chars().next())
        .unwrap_or(' ');
    let text = value
        .as_object_field("visual")
        .or_else(|| value.as_object_field("text"))
        .and_then(DataviewValue::as_str)
        .map(|value| value.split('\n').collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| markdown_literal(value, settings));

    let mut markdown = String::new();
    markdown.push_str(&indent);
    markdown.push_str("- ");
    if task {
        markdown.push('[');
        markdown.push(status);
        markdown.push_str("] ");
    }
    markdown.push_str(&text);
    markdown.push('\n');

    if let Some(DataviewValue::Array(children)) =
        value.as_object_field("children")
    {
        markdown.push_str(&markdown_task_values(children, settings, depth + 1));
    }

    markdown
}

fn markdown_literal(
    value: &DataviewValue,
    settings: &NativeMarkdownSettings,
) -> String {
    match value {
        DataviewValue::Null => settings.render_null_as.clone(),
        DataviewValue::Bool(value) => value.to_string(),
        DataviewValue::Number(value) => value.to_string(),
        DataviewValue::String(value)
        | DataviewValue::Date(value)
        | DataviewValue::DateTime(value)
        | DataviewValue::Duration(value) => value.clone(),
        DataviewValue::Link(link) => markdown_link(link),
        DataviewValue::Array(values) => values
            .iter()
            .map(|value| markdown_literal(value, settings))
            .collect::<Vec<_>>()
            .join(", "),
        DataviewValue::Object(values) => {
            let fields = values
                .iter()
                .map(|(key, value)| {
                    format!("{key}: {}", markdown_literal(value, settings))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {fields} }}")
        }
    }
}

fn markdown_link(link: &DataviewLink) -> String {
    let mut markdown = String::new();
    if link.embed {
        markdown.push('!');
    }
    markdown.push_str("[[");
    markdown.push_str(&link.path.replace('|', "\\|"));
    markdown.push('|');
    let display = link
        .display
        .clone()
        .unwrap_or_else(|| default_link_display(&link.path));
    markdown.push_str(&display);
    markdown.push_str("]]");
    markdown
}

fn default_link_display(path: &str) -> String {
    let (base, subpath) = path
        .split_once('#')
        .map_or((path, None), |(base, subpath)| {
            (base, Some(subpath.trim_start_matches('^')))
        });
    let mut display = note_stem(base).unwrap_or_else(|| base.to_string());
    if let Some(subpath) = subpath
        && !subpath.is_empty()
    {
        display.push_str(" > ");
        display.push_str(subpath);
    }
    display
}

fn escape_table(text: &str) -> String {
    let mut output = String::new();
    let mut previous = None;
    for ch in text.chars() {
        if ch == '|' && previous != Some('\\') {
            output.push('\\');
        }
        output.push(ch);
        previous = Some(ch);
    }
    output
}

fn padright(text: &str, length: usize) -> String {
    if text.len() >= length {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(length - text.len()))
    }
}

impl NativeExpression {
    fn new(tokens: Vec<NativeToken>) -> Result<Self, String> {
        if tokens.is_empty() {
            return Err("expected expression".to_string());
        }
        let raw = expression_tokens_to_string(&tokens);
        let expr = parse_native_expression(tokens)?;
        Ok(Self { raw, expr })
    }

    fn where_clause(tokens: Vec<NativeToken>) -> Result<Self, String> {
        Self::new(tokens)
    }

    fn evaluate(&self, context: &EvalContext<'_>) -> DataviewValue {
        self.expr.evaluate(context)
    }
}

impl NativeExpr {
    fn evaluate(&self, context: &EvalContext<'_>) -> DataviewValue {
        match self {
            Self::Array(values) => DataviewValue::Array(
                values.iter().map(|value| value.evaluate(context)).collect(),
            ),
            Self::Binary { op, left, right } => match op {
                NativeBinaryOp::And => {
                    let left = left.evaluate(context);
                    if !left.is_truthy() {
                        DataviewValue::Bool(false)
                    } else {
                        DataviewValue::Bool(right.evaluate(context).is_truthy())
                    }
                }
                NativeBinaryOp::Or => {
                    let left = left.evaluate(context);
                    if left.is_truthy() {
                        DataviewValue::Bool(true)
                    } else {
                        DataviewValue::Bool(right.evaluate(context).is_truthy())
                    }
                }
                NativeBinaryOp::Equal
                | NativeBinaryOp::NotEqual
                | NativeBinaryOp::Less
                | NativeBinaryOp::LessEqual
                | NativeBinaryOp::Greater
                | NativeBinaryOp::GreaterEqual => {
                    let left = left.evaluate(context);
                    let right = right.evaluate(context);
                    compare_operator_value(context.vault, *op, &left, &right)
                }
                NativeBinaryOp::Add
                | NativeBinaryOp::Subtract
                | NativeBinaryOp::Multiply
                | NativeBinaryOp::Divide => {
                    let left = left.evaluate(context);
                    let right = right.evaluate(context);
                    arithmetic_value(*op, left, right)
                }
            },
            Self::Call { function, args } => {
                evaluate_call(function, args, context)
            }
            Self::GetAttr { target, field } => {
                let target = target.evaluate(context);
                context.vault.attr_value(&target, field)
            }
            Self::Identifier(identifier) if identifier == "this" => {
                context.vault.page_value(
                    context.vault.origin_index.unwrap_or(context.page_index),
                )
            }
            Self::Identifier(identifier) => context
                .variables
                .get(identifier)
                .cloned()
                .or_else(|| context.row_field_value(identifier))
                .unwrap_or_else(|| {
                    context
                        .vault
                        .page_field_value(context.page_index, identifier)
                }),
            Self::Lambda { .. } => DataviewValue::Null,
            Self::LinkLiteral(raw) => DataviewValue::Link(
                native_expression_link(raw)
                    .map(|mut link| {
                        if let Some(path) = context
                            .vault
                            .index
                            .resolve_target_path(&link.raw_target)
                        {
                            link.path = path;
                        }
                        link
                    })
                    .unwrap_or_else(|| DataviewLink::page(raw)),
            ),
            Self::Literal(value) => value.clone(),
            Self::Object(fields) => DataviewValue::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.evaluate(context)))
                    .collect(),
            ),
            Self::Unary { op, expr } => {
                let value = expr.evaluate(context);
                match op {
                    NativeUnaryOp::Not => {
                        DataviewValue::Bool(!value.is_truthy())
                    }
                    NativeUnaryOp::Negate => negate_value(value),
                }
            }
        }
    }
}

#[derive(Clone)]
struct EvalContext<'a> {
    vault: &'a NativeVault,
    page_index: usize,
    row_value: &'a DataviewValue,
    variables: BTreeMap<String, DataviewValue>,
}

impl<'a> EvalContext<'a> {
    fn row_field_value(&self, field: &str) -> Option<DataviewValue> {
        self.row_value.as_object_field(field).cloned()
    }

    fn with_variable(
        &self,
        name: &str,
        value: DataviewValue,
    ) -> EvalContext<'a> {
        let mut variables = self.variables.clone();
        variables.insert(name.to_string(), value);
        EvalContext {
            vault: self.vault,
            page_index: self.page_index,
            row_value: self.row_value,
            variables,
        }
    }
}

fn evaluate_call(
    function: &str,
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    match function.to_ascii_lowercase().as_str() {
        "object" => evaluate_object_call(args, context),
        "list" | "array" => DataviewValue::Array(evaluated_args(args, context)),
        "date" => evaluate_unary_vector_call(args, context, date_value),
        "dur" => evaluate_unary_vector_call(args, context, duration_value),
        "number" => evaluate_unary_vector_call(args, context, number_value),
        "string" => evaluate_unary_vector_call(args, context, string_value),
        "link" => evaluate_link_call(args, context, false),
        "elink" => evaluate_external_link_call(args, context),
        "embed" => evaluate_embed_call(args, context),
        "typeof" => evaluate_unary_scalar_call(args, context, typeof_value),
        "round" => evaluate_round_call(args, context),
        "trunc" => evaluate_numeric_round_call(args, context, f64::trunc),
        "floor" => evaluate_numeric_round_call(args, context, f64::floor),
        "ceil" => evaluate_numeric_round_call(args, context, f64::ceil),
        "min" => evaluate_extreme_call(args, context, Ordering::Less),
        "max" => evaluate_extreme_call(args, context, Ordering::Greater),
        "sum" => evaluate_numeric_aggregate_call(
            args,
            context,
            NumericAggregate::Sum,
        ),
        "product" => evaluate_numeric_aggregate_call(
            args,
            context,
            NumericAggregate::Product,
        ),
        "reduce" => evaluate_reduce_call(args, context),
        "average" => evaluate_numeric_aggregate_call(
            args,
            context,
            NumericAggregate::Average,
        ),
        "contains" => {
            evaluate_contains_call(args, context, ContainsMode::Contains)
        }
        "icontains" => {
            evaluate_contains_call(args, context, ContainsMode::Insensitive)
        }
        "econtains" => {
            evaluate_contains_call(args, context, ContainsMode::Exact)
        }
        "containsword" => evaluate_containsword_call(args, context),
        "extract" => evaluate_extract_call(args, context),
        "sort" => evaluate_sort_call(args, context),
        "reverse" => evaluate_reverse_call(args, context),
        "length" => evaluate_unary_scalar_call(args, context, length_value),
        "nonnull" => evaluate_nonnull_call(args, context),
        "firstvalue" => evaluate_firstvalue_call(args, context),
        "filter" => evaluate_filter_call(args, context),
        "map" => evaluate_map_call(args, context),
        "any" => evaluate_quantifier_call(args, context, Quantifier::Any),
        "all" => evaluate_quantifier_call(args, context, Quantifier::All),
        "none" => evaluate_quantifier_call(args, context, Quantifier::None),
        "join" => evaluate_join_call(args, context),
        "unique" => evaluate_unique_call(args, context),
        "flat" => evaluate_flat_call(args, context),
        "slice" => evaluate_slice_call(args, context),
        "regextest" => evaluate_regex_test_call(args, context, false),
        "regexmatch" => evaluate_regex_test_call(args, context, true),
        "regexreplace" => evaluate_regex_replace_call(args, context),
        "replace" => evaluate_replace_call(args, context),
        "lower" => evaluate_unary_vector_call(args, context, |value| {
            string_map(value, str::to_lowercase)
        }),
        "upper" => evaluate_unary_vector_call(args, context, |value| {
            string_map(value, str::to_uppercase)
        }),
        "split" => evaluate_split_call(args, context),
        "startswith" => {
            evaluate_string_predicate_call(args, context, |text, prefix| {
                text.starts_with(prefix)
            })
        }
        "endswith" => {
            evaluate_string_predicate_call(args, context, |text, suffix| {
                text.ends_with(suffix)
            })
        }
        "padleft" => evaluate_pad_call(args, context, PadSide::Left),
        "padright" => evaluate_pad_call(args, context, PadSide::Right),
        "substring" => evaluate_substring_call(args, context),
        "truncate" => evaluate_truncate_call(args, context),
        "default" => evaluate_default_call(args, context, true),
        "ldefault" => evaluate_default_call(args, context, false),
        "display" => evaluate_unary_vector_call(args, context, display_value),
        "choice" => evaluate_choice_call(args, context),
        "hash" => evaluate_hash_call(args, context),
        "striptime" => {
            evaluate_unary_vector_call(args, context, striptime_value)
        }
        "dateformat" => evaluate_dateformat_call(args, context),
        "durationformat" => evaluate_durationformat_call(args, context),
        "currencyformat" => evaluate_currencyformat_call(args, context),
        "localtime" => {
            evaluate_unary_vector_call(args, context, localtime_value)
        }
        "meta" => evaluate_unary_vector_call(args, context, meta_value),
        "minby" => evaluate_extreme_by_call(args, context, Ordering::Less),
        "maxby" => evaluate_extreme_by_call(args, context, Ordering::Greater),
        _ => DataviewValue::Null,
    }
}

fn evaluated_args(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> Vec<DataviewValue> {
    args.iter().map(|arg| arg.evaluate(context)).collect()
}

fn evaluate_unary_scalar_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    function: impl FnOnce(DataviewValue) -> DataviewValue,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    function(arg.evaluate(context))
}

fn evaluate_unary_vector_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    function: impl Fn(DataviewValue) -> DataviewValue,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    vectorize_unary(arg.evaluate(context), function)
}

fn vectorize_unary(
    value: DataviewValue,
    function: impl Fn(DataviewValue) -> DataviewValue,
) -> DataviewValue {
    match value {
        DataviewValue::Array(values) => {
            DataviewValue::Array(values.into_iter().map(function).collect())
        }
        value => function(value),
    }
}

fn vectorize_binary(
    left: DataviewValue,
    right: DataviewValue,
    function: impl Fn(DataviewValue, DataviewValue) -> DataviewValue + Copy,
) -> DataviewValue {
    match (left, right) {
        (DataviewValue::Array(left), DataviewValue::Array(right)) => {
            let fallback = DataviewValue::Null;
            DataviewValue::Array(
                left.into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        function(
                            value,
                            right
                                .get(index)
                                .cloned()
                                .unwrap_or_else(|| fallback.clone()),
                        )
                    })
                    .collect(),
            )
        }
        (DataviewValue::Array(values), right) => DataviewValue::Array(
            values
                .into_iter()
                .map(|value| function(value, right.clone()))
                .collect(),
        ),
        (left, DataviewValue::Array(values)) => DataviewValue::Array(
            values
                .into_iter()
                .map(|value| function(left.clone(), value))
                .collect(),
        ),
        (left, right) => function(left, right),
    }
}

fn evaluate_object_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let mut object = BTreeMap::new();
    for pair in values.chunks_exact(2) {
        object.insert(value_text(&pair[0]), pair[1].clone());
    }
    DataviewValue::Object(object)
}

fn date_value(value: DataviewValue) -> DataviewValue {
    match value {
        DataviewValue::Date(_) | DataviewValue::DateTime(_) => value,
        DataviewValue::Link(link) => date_from_text(&link.path)
            .or_else(|| {
                note_stem(&link.path).and_then(|stem| date_from_text(&stem))
            })
            .unwrap_or(DataviewValue::Null),
        value => {
            date_from_text(&value_text(&value)).unwrap_or(DataviewValue::Null)
        }
    }
}

fn date_from_text(text: &str) -> Option<DataviewValue> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("today") {
        return Some(DataviewValue::Date(Utc::now().date_naive().to_string()));
    }
    if trimmed.eq_ignore_ascii_case("now") {
        return Some(DataviewValue::DateTime(
            Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        ));
    }
    if DateTime::parse_from_rfc3339(trimmed).is_ok()
        || NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S").is_ok()
    {
        return Some(DataviewValue::DateTime(trimmed.to_string()));
    }
    if NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").is_ok() {
        return Some(DataviewValue::Date(trimmed.to_string()));
    }

    let date = Regex::new(r"\d{4}-\d{2}-\d{2}").expect("valid date regex");
    date.find(trimmed)
        .and_then(|match_| date_from_text(match_.as_str()))
}

fn duration_value(value: DataviewValue) -> DataviewValue {
    match value {
        DataviewValue::Duration(_) => value,
        value => duration_text_to_iso(&value_text(&value))
            .map(DataviewValue::Duration)
            .unwrap_or(DataviewValue::Null),
    }
}

fn duration_text_to_iso(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('P') && duration_to_millis(trimmed).is_some() {
        return Some(trimmed.to_string());
    }

    let unit = Regex::new(
        r"(?i)([-+]?\d+)\s*(milliseconds?|msecs?|ms|seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d|weeks?|w|months?|mos?|mo|years?|yrs?|y)",
    )
    .expect("valid duration regex");
    let mut years = 0i64;
    let mut months = 0i64;
    let mut days = 0i64;
    let mut hours = 0i64;
    let mut minutes = 0i64;
    let mut seconds = 0i64;
    let mut milliseconds = 0i64;
    let mut matched = false;

    for captures in unit.captures_iter(trimmed) {
        matched = true;
        let amount = captures[1].parse::<i64>().ok()?;
        match captures[2].to_ascii_lowercase().as_str() {
            "millisecond" | "milliseconds" | "msec" | "msecs" | "ms" => {
                milliseconds += amount;
            }
            "second" | "seconds" | "sec" | "secs" | "s" => seconds += amount,
            "minute" | "minutes" | "min" | "mins" | "m" => minutes += amount,
            "hour" | "hours" | "hr" | "hrs" | "h" => hours += amount,
            "day" | "days" | "d" => days += amount,
            "week" | "weeks" | "w" => days += amount * 7,
            "month" | "months" | "mo" | "mos" => months += amount,
            "year" | "years" | "yr" | "yrs" | "y" => years += amount,
            _ => return None,
        }
    }
    if !matched {
        return None;
    }

    let mut output = String::from("P");
    if years != 0 {
        output.push_str(&format!("{years}Y"));
    }
    if months != 0 {
        output.push_str(&format!("{months}M"));
    }
    if days != 0 {
        output.push_str(&format!("{days}D"));
    }
    if hours != 0 || minutes != 0 || seconds != 0 || milliseconds != 0 {
        output.push('T');
        if hours != 0 {
            output.push_str(&format!("{hours}H"));
        }
        if minutes != 0 {
            output.push_str(&format!("{minutes}M"));
        }
        if milliseconds != 0 {
            let whole_seconds = seconds + milliseconds / 1000;
            let millis = milliseconds.rem_euclid(1000);
            if millis == 0 {
                if whole_seconds != 0 {
                    output.push_str(&format!("{whole_seconds}S"));
                }
            } else {
                output.push_str(&format!("{whole_seconds}.{millis:03}S"));
            }
        } else if seconds != 0 {
            output.push_str(&format!("{seconds}S"));
        }
    }
    if output == "P" {
        output.push_str("T0S");
    }
    Some(output)
}

fn number_value(value: DataviewValue) -> DataviewValue {
    match value {
        DataviewValue::Number(_) => value,
        value => {
            let number = Regex::new(r"[-+]?\d+(?:\.\d+)?")
                .expect("valid number extraction regex");
            number
                .find(&value_text(&value))
                .and_then(|match_| {
                    parse_expression_number(match_.as_str()).ok()
                })
                .map(DataviewValue::Number)
                .unwrap_or(DataviewValue::Null)
        }
    }
}

fn string_value(value: DataviewValue) -> DataviewValue {
    DataviewValue::String(match value {
        DataviewValue::Duration(value) => duration_to_human(&value),
        value => value_text(&value),
    })
}

fn duration_to_human(value: &str) -> String {
    let Some(milliseconds) = duration_to_millis(value) else {
        return value.to_string();
    };
    let mut seconds = milliseconds / 1000;
    let days = seconds / 86_400;
    seconds %= 86_400;
    let hours = seconds / 3_600;
    seconds %= 3_600;
    let minutes = seconds / 60;
    seconds %= 60;
    for (amount, singular) in [
        (days, "day"),
        (hours, "hour"),
        (minutes, "minute"),
        (seconds, "second"),
    ] {
        if amount != 0 {
            let suffix = if amount == 1 { "" } else { "s" };
            return format!("{amount} {singular}{suffix}");
        }
    }
    "0 seconds".to_string()
}

fn typeof_value(value: DataviewValue) -> DataviewValue {
    let name = match value {
        DataviewValue::Null => "null",
        DataviewValue::Bool(_) => "boolean",
        DataviewValue::Number(_) => "number",
        DataviewValue::String(_) => "string",
        DataviewValue::Date(_) => "date",
        DataviewValue::DateTime(_) => "date",
        DataviewValue::Duration(_) => "duration",
        DataviewValue::Link(_) => "link",
        DataviewValue::Array(_) => "array",
        DataviewValue::Object(_) => "object",
    };
    DataviewValue::String(name.to_string())
}

fn evaluate_link_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    embed: bool,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let ([path] | [path, _]) = values.as_slice() else {
        return DataviewValue::Null;
    };
    let display = values
        .get(1)
        .map(value_text)
        .filter(|value| !value.is_empty());
    let path_text = value_text(path);
    DataviewValue::Link(DataviewLink::new(
        normalized_link_literal_path(&path_text),
        display,
        embed,
        path_text,
    ))
}

fn evaluate_external_link_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let ([url] | [url, _]) = values.as_slice() else {
        return DataviewValue::Null;
    };
    let url_text = value_text(url);
    DataviewValue::Link(DataviewLink::new(
        url_text.clone(),
        values
            .get(1)
            .map(value_text)
            .filter(|value| !value.is_empty()),
        false,
        url_text,
    ))
}

fn evaluate_embed_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    match evaluated_args(args, context).as_slice() {
        [DataviewValue::Link(link)] => {
            let mut link = link.clone();
            link.embed = true;
            DataviewValue::Link(link)
        }
        [DataviewValue::Link(link), embed] => {
            let mut link = link.clone();
            link.embed = embed.is_truthy();
            DataviewValue::Link(link)
        }
        [_, ..] => evaluate_link_call(args, context, true),
        _ => DataviewValue::Null,
    }
}

fn evaluate_round_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([value] | [value, _]) = args else {
        return DataviewValue::Null;
    };
    let digits = args
        .get(1)
        .map(|arg| integer_value(&arg.evaluate(context)))
        .unwrap_or(Some(0))
        .unwrap_or(0);
    vectorize_unary(value.evaluate(context), |value| {
        let Some(number) = numeric_f64(&value) else {
            return DataviewValue::Null;
        };
        let factor = 10_f64.powi(digits as i32);
        number_from_f64_smart((number * factor).round() / factor)
    })
}

fn evaluate_numeric_round_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    function: impl Fn(f64) -> f64 + Copy,
) -> DataviewValue {
    evaluate_unary_vector_call(args, context, |value| {
        numeric_f64(&value)
            .map(|value| number_from_f64_smart(function(value)))
            .unwrap_or(DataviewValue::Null)
    })
}

fn evaluate_extreme_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    target_ordering: Ordering,
) -> DataviewValue {
    let values = aggregate_args(args, context);
    let mut best: Option<DataviewValue> = None;
    for value in values {
        match &best {
            None => best = Some(value),
            Some(best_value)
                if compare_values(context.vault, &value, best_value)
                    == target_ordering =>
            {
                best = Some(value);
            }
            _ => {}
        }
    }
    best.unwrap_or(DataviewValue::Null)
}

#[derive(Clone, Copy)]
enum NumericAggregate {
    Average,
    Product,
    Sum,
}

fn evaluate_numeric_aggregate_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    aggregate: NumericAggregate,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let values = collection_value(arg.evaluate(context));
    let numbers = values.iter().filter_map(numeric_f64).collect::<Vec<_>>();
    if numbers.is_empty() {
        return DataviewValue::Null;
    }
    let value = match aggregate {
        NumericAggregate::Average => {
            numbers.iter().sum::<f64>() / numbers.len() as f64
        }
        NumericAggregate::Product => numbers.iter().product(),
        NumericAggregate::Sum => numbers.iter().sum(),
    };
    number_from_f64_smart(value)
}

fn evaluate_reduce_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [collection, operator] = args else {
        return DataviewValue::Null;
    };
    let mut values = collection_value(collection.evaluate(context)).into_iter();
    let Some(mut result) = values.next() else {
        return DataviewValue::Null;
    };
    let operator = value_text(&operator.evaluate(context));
    for value in values {
        result = match operator.as_str() {
            "+" => add_value(result, value),
            "-" => {
                numeric_binary_value(result, value, |left, right| left - right)
            }
            "*" => multiply_value(result, value),
            "/" => {
                numeric_binary_value(result, value, |left, right| left / right)
            }
            "&" => DataviewValue::Bool(result.is_truthy() && value.is_truthy()),
            "|" => DataviewValue::Bool(result.is_truthy() || value.is_truthy()),
            _ => return DataviewValue::Null,
        };
    }
    result
}

#[derive(Clone, Copy)]
enum ContainsMode {
    Contains,
    Exact,
    Insensitive,
}

fn evaluate_contains_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    mode: ContainsMode,
) -> DataviewValue {
    let [container, needle] = args else {
        return DataviewValue::Null;
    };
    let container = container.evaluate(context);
    let needle = needle.evaluate(context);
    DataviewValue::Bool(contains_value(
        context.vault,
        &container,
        &needle,
        mode,
    ))
}

fn contains_value(
    vault: &NativeVault,
    container: &DataviewValue,
    needle: &DataviewValue,
    mode: ContainsMode,
) -> bool {
    match container {
        DataviewValue::Object(object) => {
            let needle = value_text(needle);
            match mode {
                ContainsMode::Insensitive => {
                    object.keys().any(|key| key.eq_ignore_ascii_case(&needle))
                }
                ContainsMode::Contains | ContainsMode::Exact => {
                    object.contains_key(&needle)
                }
            }
        }
        DataviewValue::Array(values) => values.iter().any(|value| match mode {
            ContainsMode::Insensitive => {
                value_text(value).eq_ignore_ascii_case(&value_text(needle))
            }
            ContainsMode::Contains | ContainsMode::Exact => {
                values_equal(vault, value, needle)
            }
        }),
        value => {
            let haystack = value_text(value);
            let needle = value_text(needle);
            match mode {
                ContainsMode::Insensitive => haystack
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase()),
                ContainsMode::Contains | ContainsMode::Exact => {
                    haystack.contains(&needle)
                }
            }
        }
    }
}

fn evaluate_containsword_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [container, needle] = args else {
        return DataviewValue::Null;
    };
    let container = container.evaluate(context);
    let needle = value_text(&needle.evaluate(context)).to_ascii_lowercase();
    match container {
        DataviewValue::Array(values) => DataviewValue::Array(
            values
                .iter()
                .map(|value| {
                    DataviewValue::Bool(text_has_word(
                        &value_text(value),
                        &needle,
                    ))
                })
                .collect(),
        ),
        value => {
            DataviewValue::Bool(text_has_word(&value_text(&value), &needle))
        }
    }
}

fn text_has_word(text: &str, needle: &str) -> bool {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .any(|word| word.eq_ignore_ascii_case(needle))
}

fn evaluate_extract_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [object, keys @ ..] = args else {
        return DataviewValue::Null;
    };
    let DataviewValue::Object(object) = object.evaluate(context) else {
        return DataviewValue::Null;
    };
    let mut extracted = BTreeMap::new();
    for key in keys {
        let key = value_text(&key.evaluate(context));
        if let Some(value) = object.get(&key) {
            extracted.insert(key, value.clone());
        }
    }
    DataviewValue::Object(extracted)
}

fn evaluate_sort_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let mut values = collection_value(arg.evaluate(context));
    values.sort_by(|left, right| compare_values(context.vault, left, right));
    DataviewValue::Array(values)
}

fn evaluate_reverse_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let mut values = collection_value(arg.evaluate(context));
    values.reverse();
    DataviewValue::Array(values)
}

fn length_value(value: DataviewValue) -> DataviewValue {
    let length = match value {
        DataviewValue::Array(values) => values.len(),
        DataviewValue::Object(values) => values.len(),
        value => value_text(&value).chars().count(),
    };
    DataviewValue::Number(Number::from(length as u64))
}

fn evaluate_nonnull_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    DataviewValue::Array(
        collection_value(arg.evaluate(context))
            .into_iter()
            .filter(|value| !matches!(value, DataviewValue::Null))
            .collect(),
    )
}

fn evaluate_firstvalue_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    collection_value(arg.evaluate(context))
        .into_iter()
        .find(|value| !matches!(value, DataviewValue::Null))
        .unwrap_or(DataviewValue::Null)
}

#[derive(Debug, Clone, Copy)]
enum Quantifier {
    All,
    Any,
    None,
}

fn evaluate_filter_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let Some((values, parameter, body)) = collection_lambda_args(args, context)
    else {
        return DataviewValue::Null;
    };
    DataviewValue::Array(
        values
            .into_iter()
            .filter(|value| {
                body.evaluate(&context.with_variable(parameter, value.clone()))
                    .is_truthy()
            })
            .collect(),
    )
}

fn evaluate_map_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let Some((values, parameter, body)) = collection_lambda_args(args, context)
    else {
        return DataviewValue::Null;
    };
    DataviewValue::Array(
        values
            .into_iter()
            .map(|value| {
                body.evaluate(&context.with_variable(parameter, value))
            })
            .collect(),
    )
}

fn evaluate_quantifier_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    quantifier: Quantifier,
) -> DataviewValue {
    let values = match args {
        [collection] => collection_value(collection.evaluate(context)),
        [collection, NativeExpr::Lambda { parameter, body }] => {
            let values = collection_value(collection.evaluate(context));
            let projected = values
                .into_iter()
                .map(|value| {
                    body.evaluate(&context.with_variable(parameter, value))
                })
                .collect::<Vec<_>>();
            return DataviewValue::Bool(match quantifier {
                Quantifier::All => {
                    projected.iter().all(DataviewValue::is_truthy)
                }
                Quantifier::Any => {
                    projected.iter().any(DataviewValue::is_truthy)
                }
                Quantifier::None => {
                    !projected.iter().any(DataviewValue::is_truthy)
                }
            });
        }
        _ => {
            let values = evaluated_args(args, context);
            return DataviewValue::Bool(match quantifier {
                Quantifier::All => values.iter().all(DataviewValue::is_truthy),
                Quantifier::Any => values.iter().any(DataviewValue::is_truthy),
                Quantifier::None => {
                    !values.iter().any(DataviewValue::is_truthy)
                }
            });
        }
    };

    DataviewValue::Bool(match quantifier {
        Quantifier::All => values.iter().all(DataviewValue::is_truthy),
        Quantifier::Any => values.iter().any(DataviewValue::is_truthy),
        Quantifier::None => !values.iter().any(DataviewValue::is_truthy),
    })
}

fn evaluate_join_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([collection] | [collection, _]) = args else {
        return DataviewValue::Null;
    };
    let separator = args
        .get(1)
        .map(|arg| value_text(&arg.evaluate(context)))
        .unwrap_or_else(|| ", ".to_string());
    let values = collection_value(collection.evaluate(context));
    DataviewValue::String(
        values
            .iter()
            .map(value_text)
            .collect::<Vec<_>>()
            .join(&separator),
    )
}

fn evaluate_unique_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let mut unique = Vec::new();
    for value in collection_value(arg.evaluate(context)) {
        if !unique
            .iter()
            .any(|existing| values_equal(context.vault, existing, &value))
        {
            unique.push(value);
        }
    }
    DataviewValue::Array(unique)
}

fn evaluate_flat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([collection] | [collection, _]) = args else {
        return DataviewValue::Null;
    };
    let depth = args
        .get(1)
        .and_then(|arg| integer_value(&arg.evaluate(context)))
        .unwrap_or(1)
        .max(0) as usize;
    let mut output = Vec::new();
    flatten_values(
        collection_value(collection.evaluate(context)),
        depth,
        &mut output,
    );
    DataviewValue::Array(output)
}

fn flatten_values(
    values: Vec<DataviewValue>,
    depth: usize,
    output: &mut Vec<DataviewValue>,
) {
    for value in values {
        match value {
            DataviewValue::Array(values) if depth > 0 => {
                flatten_values(values, depth - 1, output);
            }
            value => output.push(value),
        }
    }
}

fn evaluate_slice_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([collection] | [collection, _] | [collection, _, _]) = args else {
        return DataviewValue::Null;
    };
    let start = args
        .get(1)
        .and_then(|arg| integer_value(&arg.evaluate(context)))
        .unwrap_or(0);
    let end = args
        .get(2)
        .and_then(|arg| integer_value(&arg.evaluate(context)));
    let values = collection_value(collection.evaluate(context));
    let (start, end) = slice_bounds(values.len(), start, end);
    DataviewValue::Array(values[start..end].to_vec())
}

fn slice_bounds(length: usize, start: i64, end: Option<i64>) -> (usize, usize) {
    let length_i64 = length as i64;
    let start = if start < 0 { length_i64 + start } else { start }
        .clamp(0, length_i64) as usize;
    let end = end.unwrap_or(length_i64);
    let end = if end < 0 { length_i64 + end } else { end }
        .clamp(start as i64, length_i64) as usize;
    (start, end)
}

fn evaluate_regex_test_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    full_match: bool,
) -> DataviewValue {
    let [pattern, input] = args else {
        return DataviewValue::Null;
    };
    let pattern = value_text(&pattern.evaluate(context));
    let input = input.evaluate(context);
    vectorize_unary(input, |input| {
        let text = value_text(&input);
        let pattern = if full_match {
            format!("^(?:{pattern})$")
        } else {
            pattern.clone()
        };
        Regex::new(&pattern)
            .map(|regex| DataviewValue::Bool(regex.is_match(&text)))
            .unwrap_or(DataviewValue::Bool(false))
    })
}

fn evaluate_regex_replace_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [input, pattern, replacement] = args else {
        return DataviewValue::Null;
    };
    let pattern = value_text(&pattern.evaluate(context));
    let replacement = value_text(&replacement.evaluate(context));
    let Ok(regex) = Regex::new(&pattern) else {
        return DataviewValue::Null;
    };
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(
            regex
                .replace_all(&value_text(&input), replacement.as_str())
                .into_owned(),
        )
    })
}

fn evaluate_replace_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [input, pattern, replacement] = args else {
        return DataviewValue::Null;
    };
    let pattern = value_text(&pattern.evaluate(context));
    let replacement = value_text(&replacement.evaluate(context));
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(
            value_text(&input).replace(&pattern, &replacement),
        )
    })
}

fn string_map(
    value: DataviewValue,
    function: impl FnOnce(&str) -> String,
) -> DataviewValue {
    DataviewValue::String(function(&value_text(&value)))
}

fn evaluate_split_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([input, delimiter] | [input, delimiter, _]) = args else {
        return DataviewValue::Null;
    };
    let delimiter = value_text(&delimiter.evaluate(context));
    let limit = args
        .get(2)
        .and_then(|arg| integer_value(&arg.evaluate(context)))
        .map(|value| value.max(0) as usize);
    let Ok(regex) = Regex::new(&delimiter) else {
        return DataviewValue::Null;
    };
    vectorize_unary(input.evaluate(context), |input| {
        let pieces = regex
            .split(&value_text(&input))
            .map(|value| DataviewValue::String(value.to_string()))
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        DataviewValue::Array(pieces)
    })
}

fn evaluate_string_predicate_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    predicate: impl Fn(&str, &str) -> bool + Copy,
) -> DataviewValue {
    let [input, needle] = args else {
        return DataviewValue::Null;
    };
    let needle = value_text(&needle.evaluate(context));
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::Bool(predicate(&value_text(&input), &needle))
    })
}

#[derive(Clone, Copy)]
enum PadSide {
    Left,
    Right,
}

fn evaluate_pad_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    side: PadSide,
) -> DataviewValue {
    let ([input, length] | [input, length, _]) = args else {
        return DataviewValue::Null;
    };
    let length =
        integer_value(&length.evaluate(context)).unwrap_or(0).max(0) as usize;
    let padding = args
        .get(2)
        .map(|arg| value_text(&arg.evaluate(context)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| " ".to_string());
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(pad_text(
            &value_text(&input),
            length,
            &padding,
            side,
        ))
    })
}

fn pad_text(
    input: &str,
    length: usize,
    padding: &str,
    side: PadSide,
) -> String {
    let current = input.chars().count();
    if current >= length {
        return input.to_string();
    }
    let mut pad = String::new();
    while pad.chars().count() < length - current {
        pad.push_str(padding);
    }
    let pad = pad.chars().take(length - current).collect::<String>();
    match side {
        PadSide::Left => format!("{pad}{input}"),
        PadSide::Right => format!("{input}{pad}"),
    }
}

fn evaluate_substring_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([input, start] | [input, start, _]) = args else {
        return DataviewValue::Null;
    };
    let start = integer_value(&start.evaluate(context)).unwrap_or(0);
    let end = args
        .get(2)
        .and_then(|arg| integer_value(&arg.evaluate(context)));
    vectorize_unary(input.evaluate(context), |input| {
        let text = value_text(&input);
        let chars = text.chars().collect::<Vec<_>>();
        let (start, end) = slice_bounds(chars.len(), start, end);
        DataviewValue::String(chars[start..end].iter().collect())
    })
}

fn evaluate_truncate_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([input, length] | [input, length, _]) = args else {
        return DataviewValue::Null;
    };
    let length =
        integer_value(&length.evaluate(context)).unwrap_or(0).max(0) as usize;
    let suffix = args
        .get(2)
        .map(|arg| value_text(&arg.evaluate(context)))
        .unwrap_or_else(|| "...".to_string());
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(truncate_text(
            &value_text(&input),
            length,
            &suffix,
        ))
    })
}

fn truncate_text(input: &str, length: usize, suffix: &str) -> String {
    let input_len = input.chars().count();
    if input_len <= length {
        return input.to_string();
    }
    let suffix_len = suffix.chars().count();
    if suffix_len >= length {
        return suffix.chars().take(length).collect();
    }
    let prefix = input.chars().take(length - suffix_len).collect::<String>();
    format!("{prefix}{suffix}")
}

fn evaluate_default_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    vectorized: bool,
) -> DataviewValue {
    let [value, fallback] = args else {
        return DataviewValue::Null;
    };
    let value = value.evaluate(context);
    let fallback = fallback.evaluate(context);
    if vectorized {
        vectorize_binary(value, fallback, default_pair)
    } else {
        default_pair(value, fallback)
    }
}

fn default_pair(
    value: DataviewValue,
    fallback: DataviewValue,
) -> DataviewValue {
    if matches!(value, DataviewValue::Null) {
        fallback
    } else {
        value
    }
}

fn display_value(value: DataviewValue) -> DataviewValue {
    DataviewValue::String(display_text(&value))
}

fn display_text(value: &DataviewValue) -> String {
    match value {
        DataviewValue::Link(link) => {
            link.display.clone().unwrap_or_else(|| {
                note_stem(&link.path).unwrap_or_else(|| link.path.clone())
            })
        }
        DataviewValue::Array(values) => values
            .iter()
            .map(display_text)
            .collect::<Vec<_>>()
            .join(", "),
        DataviewValue::String(value) => markdown_display_text(value),
        value => value_text(value),
    }
}

fn markdown_display_text(value: &str) -> String {
    let wikilink =
        Regex::new(r"!?\[\[([^\]|#]+)(?:#[^\]|]+)?(?:\|([^\]]+))?\]\]")
            .expect("valid wikilink display regex");
    let markdown_link = Regex::new(r"\[([^\]]+)\]\([^)]+\)")
        .expect("valid markdown link regex");
    let emphasis = Regex::new(r"[*_`]").expect("valid emphasis cleanup regex");
    let value =
        wikilink.replace_all(value, |captures: &regex::Captures<'_>| {
            captures
                .get(2)
                .or_else(|| captures.get(1))
                .map(|match_| match_.as_str())
                .unwrap_or_default()
                .to_string()
        });
    let value = markdown_link.replace_all(&value, "$1");
    emphasis.replace_all(&value, "").into_owned()
}

fn evaluate_choice_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [condition, left, right] = args else {
        return DataviewValue::Null;
    };
    if condition.evaluate(context).is_truthy() {
        left.evaluate(context)
    } else {
        right.evaluate(context)
    }
}

fn evaluate_hash_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value_text(&value));
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let value = u64::from_be_bytes(bytes) & 0x7fff_ffff_ffff_ffff;
    DataviewValue::Number(Number::from(value))
}

fn striptime_value(value: DataviewValue) -> DataviewValue {
    match value {
        DataviewValue::Date(_) => value,
        DataviewValue::DateTime(value) | DataviewValue::String(value) => {
            date_from_text(&value)
                .and_then(|value| match value {
                    DataviewValue::Date(date) => {
                        Some(DataviewValue::Date(date))
                    }
                    DataviewValue::DateTime(datetime) => datetime
                        .split_once('T')
                        .map(|(date, _)| DataviewValue::Date(date.to_string())),
                    _ => None,
                })
                .unwrap_or(DataviewValue::Null)
        }
        _ => DataviewValue::Null,
    }
}

fn evaluate_dateformat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [date, format] = args else {
        return DataviewValue::Null;
    };
    let format = value_text(&format.evaluate(context));
    vectorize_unary(date.evaluate(context), |value| {
        date_components(&value)
            .map(|date| DataviewValue::String(format_date(&date, &format)))
            .unwrap_or(DataviewValue::Null)
    })
}

struct DateComponents {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    timestamp_millis: Option<i64>,
}

fn date_components(value: &DataviewValue) -> Option<DateComponents> {
    let value = match value {
        DataviewValue::Date(value)
        | DataviewValue::DateTime(value)
        | DataviewValue::String(value) => value.as_str(),
        _ => return None,
    };
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        let datetime = datetime.with_timezone(&Utc);
        return Some(DateComponents {
            year: datetime
                .naive_utc()
                .date()
                .format("%Y")
                .to_string()
                .parse()
                .ok()?,
            month: datetime
                .naive_utc()
                .date()
                .format("%m")
                .to_string()
                .parse()
                .ok()?,
            day: datetime
                .naive_utc()
                .date()
                .format("%d")
                .to_string()
                .parse()
                .ok()?,
            hour: datetime
                .naive_utc()
                .time()
                .format("%H")
                .to_string()
                .parse()
                .ok()?,
            minute: datetime
                .naive_utc()
                .time()
                .format("%M")
                .to_string()
                .parse()
                .ok()?,
            second: datetime
                .naive_utc()
                .time()
                .format("%S")
                .to_string()
                .parse()
                .ok()?,
            timestamp_millis: Some(datetime.timestamp_millis()),
        });
    }
    if let Ok(datetime) =
        NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
    {
        return Some(DateComponents {
            year: datetime.date().format("%Y").to_string().parse().ok()?,
            month: datetime.date().format("%m").to_string().parse().ok()?,
            day: datetime.date().format("%d").to_string().parse().ok()?,
            hour: datetime.time().format("%H").to_string().parse().ok()?,
            minute: datetime.time().format("%M").to_string().parse().ok()?,
            second: datetime.time().format("%S").to_string().parse().ok()?,
            timestamp_millis: Some(datetime.and_utc().timestamp_millis()),
        });
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(DateComponents {
            year: date.format("%Y").to_string().parse().ok()?,
            month: date.format("%m").to_string().parse().ok()?,
            day: date.format("%d").to_string().parse().ok()?,
            hour: 0,
            minute: 0,
            second: 0,
            timestamp_millis: date
                .and_hms_opt(0, 0, 0)
                .map(|datetime| datetime.and_utc().timestamp_millis()),
        });
    }
    None
}

fn format_date(date: &DateComponents, format: &str) -> String {
    let mut output = String::new();
    let mut index = 0usize;
    while index < format.len() {
        let rest = &format[index..];
        let (replacement, width) =
            if rest.starts_with("yyyy") || rest.starts_with("YYYY") {
                (format!("{:04}", date.year), 4)
            } else if rest.starts_with("yy") || rest.starts_with("YY") {
                (format!("{:02}", date.year.rem_euclid(100)), 2)
            } else if rest.starts_with("MM") {
                (format!("{:02}", date.month), 2)
            } else if rest.starts_with('M') {
                (date.month.to_string(), 1)
            } else if rest.starts_with("dd") || rest.starts_with("DD") {
                (format!("{:02}", date.day), 2)
            } else if rest.starts_with('d') || rest.starts_with('D') {
                (date.day.to_string(), 1)
            } else if rest.starts_with("HH") {
                (format!("{:02}", date.hour), 2)
            } else if rest.starts_with('H') {
                (date.hour.to_string(), 1)
            } else if rest.starts_with("mm") {
                (format!("{:02}", date.minute), 2)
            } else if rest.starts_with('m') {
                (date.minute.to_string(), 1)
            } else if rest.starts_with("ss") {
                (format!("{:02}", date.second), 2)
            } else if rest.starts_with('s') {
                (date.second.to_string(), 1)
            } else if rest.starts_with('x') {
                (date.timestamp_millis.unwrap_or_default().to_string(), 1)
            } else {
                let ch = rest.chars().next().expect("non-empty format rest");
                output.push(ch);
                index += ch.len_utf8();
                continue;
            };
        output.push_str(&replacement);
        index += width;
    }
    output
}

fn evaluate_durationformat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [duration, format] = args else {
        return DataviewValue::Null;
    };
    let format = value_text(&format.evaluate(context));
    vectorize_unary(duration.evaluate(context), |value| {
        let duration = match value {
            DataviewValue::Duration(value) | DataviewValue::String(value) => {
                value
            }
            _ => return DataviewValue::Null,
        };
        duration_to_millis(&duration)
            .map(|millis| {
                DataviewValue::String(format_duration(millis, &format))
            })
            .unwrap_or(DataviewValue::Null)
    })
}

fn duration_to_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    if !value.starts_with('P') {
        return duration_text_to_iso(value)
            .and_then(|iso| duration_to_millis(&iso));
    }
    let token = Regex::new(r"(?i)([-+]?\d+(?:\.\d+)?)(Y|M|W|D|H|S)")
        .expect("valid ISO duration regex");
    let mut in_time = false;
    let mut millis = 0f64;
    for ch in value.chars() {
        if ch == 'T' {
            in_time = true;
        }
    }
    let mut time_position = false;
    for captures in token.captures_iter(value) {
        let amount = captures[1].parse::<f64>().ok()?;
        let unit = &captures[2];
        let before = &value[..captures.get(0)?.start()];
        if before.contains('T') {
            time_position = true;
        }
        millis += match unit {
            "Y" | "y" => amount * 365.0 * 86_400_000.0,
            "M" | "m" if !time_position && !in_time => {
                amount * 30.0 * 86_400_000.0
            }
            "M" | "m" if !time_position => amount * 30.0 * 86_400_000.0,
            "M" | "m" => amount * 60_000.0,
            "W" | "w" => amount * 7.0 * 86_400_000.0,
            "D" | "d" => amount * 86_400_000.0,
            "H" | "h" => amount * 3_600_000.0,
            "S" | "s" => amount * 1_000.0,
            _ => return None,
        };
    }
    Some(millis.round() as i64)
}

fn format_duration(milliseconds: i64, format: &str) -> String {
    let total_seconds = milliseconds / 1000;
    let total_minutes = total_seconds / 60;
    let total_hours = total_minutes / 60;
    let total_days = total_hours / 24;
    let total_weeks = total_days / 7;
    let total_months = total_days / 30;
    let total_years = total_days / 365;
    let components = [
        ("yyyy", format!("{total_years:04}")),
        ("yyy", format!("{total_years:03}")),
        ("yy", format!("{total_years:02}")),
        ("y", total_years.to_string()),
        ("MMMM", format!("{total_months:04}")),
        ("MMM", format!("{total_months:03}")),
        ("MM", format!("{total_months:02}")),
        ("M", total_months.to_string()),
        ("www", format!("{total_weeks:03}")),
        ("ww", format!("{total_weeks:02}")),
        ("w", total_weeks.to_string()),
        ("ddd", format!("{total_days:03}")),
        ("dd", format!("{total_days:02}")),
        ("d", total_days.to_string()),
        ("hh", format!("{:02}", total_hours % 24)),
        ("h", (total_hours % 24).to_string()),
        ("mm", format!("{:02}", total_minutes % 60)),
        ("m", (total_minutes % 60).to_string()),
        ("ss", format!("{:02}", total_seconds % 60)),
        ("s", (total_seconds % 60).to_string()),
        ("SSS", format!("{:03}", milliseconds % 1000)),
        ("S", (milliseconds % 1000).to_string()),
    ];

    let mut output = String::new();
    let mut index = 0usize;
    let mut literal = false;
    while index < format.len() {
        let rest = &format[index..];
        if rest.starts_with('\'') {
            literal = !literal;
            index += 1;
            continue;
        }
        if !literal
            && let Some((token, replacement)) =
                components.iter().find(|(token, _)| rest.starts_with(token))
        {
            output.push_str(replacement);
            index += token.len();
            continue;
        }
        let ch = rest.chars().next().expect("non-empty duration format rest");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn evaluate_currencyformat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([number] | [number, _]) = args else {
        return DataviewValue::Null;
    };
    let currency = args
        .get(1)
        .map(|arg| value_text(&arg.evaluate(context)))
        .unwrap_or_else(|| "USD".to_string());
    vectorize_unary(number.evaluate(context), |value| {
        numeric_f64(&value)
            .map(|number| {
                DataviewValue::String(format_currency(number, &currency))
            })
            .unwrap_or(DataviewValue::Null)
    })
}

fn format_currency(number: f64, currency: &str) -> String {
    let symbol = match currency.to_ascii_uppercase().as_str() {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        _ => currency,
    };
    format!("{symbol}{}", format_number_with_commas(number))
}

fn format_number_with_commas(number: f64) -> String {
    let sign = if number < 0.0 { "-" } else { "" };
    let abs = number.abs();
    let formatted = format!("{abs:.2}");
    let (whole, fraction) = formatted
        .split_once('.')
        .expect("fixed precision includes decimal");
    let mut grouped = String::new();
    for (index, ch) in whole.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let whole = grouped.chars().rev().collect::<String>();
    format!("{sign}{whole}.{fraction}")
}

fn localtime_value(value: DataviewValue) -> DataviewValue {
    match value {
        DataviewValue::Date(_) | DataviewValue::DateTime(_) => value,
        value => date_value(value),
    }
}

fn meta_value(value: DataviewValue) -> DataviewValue {
    let DataviewValue::Link(link) = value else {
        return DataviewValue::Null;
    };
    let (path, subpath) = link
        .path
        .split_once('#')
        .map_or((link.path.as_str(), None), |(path, subpath)| {
            (path, Some(subpath))
        });
    let subpath =
        subpath.map(|subpath| subpath.trim_start_matches('^').to_string());
    let link_type = match subpath.as_deref() {
        None => "file",
        Some(raw) if link.path.contains("#^") || raw.starts_with('^') => {
            "block"
        }
        Some(_) => "header",
    };
    let mut object = BTreeMap::new();
    object.insert(
        "display".to_string(),
        link.display
            .clone()
            .map(DataviewValue::String)
            .unwrap_or(DataviewValue::Null),
    );
    object.insert("embed".to_string(), DataviewValue::Bool(link.embed));
    object.insert("path".to_string(), DataviewValue::String(path.to_string()));
    object.insert(
        "subpath".to_string(),
        subpath
            .map(DataviewValue::String)
            .unwrap_or(DataviewValue::Null),
    );
    object.insert(
        "type".to_string(),
        DataviewValue::String(link_type.to_string()),
    );
    DataviewValue::Object(object)
}

fn evaluate_extreme_by_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    target_ordering: Ordering,
) -> DataviewValue {
    let Some((values, parameter, body)) = collection_lambda_args(args, context)
    else {
        return DataviewValue::Null;
    };
    let mut best: Option<(DataviewValue, DataviewValue)> = None;
    for value in values {
        let key =
            body.evaluate(&context.with_variable(parameter, value.clone()));
        match &best {
            None => best = Some((value, key)),
            Some((_, best_key))
                if compare_values(context.vault, &key, best_key)
                    == target_ordering =>
            {
                best = Some((value, key));
            }
            _ => {}
        }
    }
    best.map(|(value, _)| value).unwrap_or(DataviewValue::Null)
}

fn collection_lambda_args<'a>(
    args: &'a [NativeExpr],
    context: &EvalContext<'_>,
) -> Option<(Vec<DataviewValue>, &'a str, &'a NativeExpr)> {
    let [collection, NativeExpr::Lambda { parameter, body }] = args else {
        return None;
    };
    Some((
        collection_value(collection.evaluate(context)),
        parameter.as_str(),
        body,
    ))
}

fn collection_value(value: DataviewValue) -> Vec<DataviewValue> {
    match value {
        DataviewValue::Array(values) => values,
        DataviewValue::Null => Vec::new(),
        value => vec![value],
    }
}

fn aggregate_args(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> Vec<DataviewValue> {
    match args {
        [arg] => collection_value(arg.evaluate(context)),
        args => evaluated_args(args, context),
    }
}

fn numeric_f64(value: &DataviewValue) -> Option<f64> {
    match value {
        DataviewValue::Number(number) => number.as_f64(),
        _ => None,
    }
}

fn integer_value(value: &DataviewValue) -> Option<i64> {
    match value {
        DataviewValue::Number(number) => number
            .as_i64()
            .or_else(|| {
                number.as_u64().and_then(|value| i64::try_from(value).ok())
            })
            .or_else(|| number.as_f64().map(|value| value as i64)),
        _ => None,
    }
}

fn compare_operator_value(
    vault: &NativeVault,
    op: NativeBinaryOp,
    left: &DataviewValue,
    right: &DataviewValue,
) -> DataviewValue {
    let equal = values_equal(vault, left, right);
    let value = match op {
        NativeBinaryOp::Equal => equal,
        NativeBinaryOp::NotEqual => !equal,
        NativeBinaryOp::Less if values_include_null(left, right) => false,
        NativeBinaryOp::Less => {
            compare_values(vault, left, right) == Ordering::Less
        }
        NativeBinaryOp::LessEqual if values_include_null(left, right) => false,
        NativeBinaryOp::LessEqual => {
            matches!(
                compare_values(vault, left, right),
                Ordering::Less | Ordering::Equal
            )
        }
        NativeBinaryOp::Greater if values_include_null(left, right) => false,
        NativeBinaryOp::Greater => {
            compare_values(vault, left, right) == Ordering::Greater
        }
        NativeBinaryOp::GreaterEqual if values_include_null(left, right) => {
            false
        }
        NativeBinaryOp::GreaterEqual => {
            matches!(
                compare_values(vault, left, right),
                Ordering::Greater | Ordering::Equal
            )
        }
        NativeBinaryOp::Add
        | NativeBinaryOp::And
        | NativeBinaryOp::Divide
        | NativeBinaryOp::Multiply
        | NativeBinaryOp::Or
        | NativeBinaryOp::Subtract => unreachable!("not a comparison operator"),
    };
    DataviewValue::Bool(value)
}

fn values_include_null(left: &DataviewValue, right: &DataviewValue) -> bool {
    matches!(left, DataviewValue::Null) || matches!(right, DataviewValue::Null)
}

fn values_equal(
    vault: &NativeVault,
    left: &DataviewValue,
    right: &DataviewValue,
) -> bool {
    match (left, right) {
        (DataviewValue::Number(left), DataviewValue::Number(right)) => {
            left.as_f64() == right.as_f64()
        }
        (DataviewValue::Link(_), _) => {
            vault.field_value_matches_link(left, &value_text(right))
        }
        (_, DataviewValue::Link(_)) => {
            vault.field_value_matches_link(right, &value_text(left))
        }
        _ => left == right,
    }
}

fn compare_values(
    vault: &NativeVault,
    left: &DataviewValue,
    right: &DataviewValue,
) -> Ordering {
    if values_equal(vault, left, right) {
        return Ordering::Equal;
    }
    match (left, right) {
        (DataviewValue::Null, _) => Ordering::Greater,
        (_, DataviewValue::Null) => Ordering::Less,
        (DataviewValue::Number(left), DataviewValue::Number(right)) => left
            .as_f64()
            .partial_cmp(&right.as_f64())
            .unwrap_or(Ordering::Equal),
        (DataviewValue::Bool(left), DataviewValue::Bool(right)) => {
            left.cmp(right)
        }
        (DataviewValue::Array(left), DataviewValue::Array(right)) => {
            left.len().cmp(&right.len())
        }
        _ => value_text(left).cmp(&value_text(right)),
    }
}

fn arithmetic_value(
    op: NativeBinaryOp,
    left: DataviewValue,
    right: DataviewValue,
) -> DataviewValue {
    match op {
        NativeBinaryOp::Add => add_value(left, right),
        NativeBinaryOp::Subtract => {
            numeric_binary_value(left, right, |left, right| left - right)
        }
        NativeBinaryOp::Multiply => multiply_value(left, right),
        NativeBinaryOp::Divide => {
            numeric_binary_value(left, right, |left, right| left / right)
        }
        NativeBinaryOp::And
        | NativeBinaryOp::Equal
        | NativeBinaryOp::Greater
        | NativeBinaryOp::GreaterEqual
        | NativeBinaryOp::Less
        | NativeBinaryOp::LessEqual
        | NativeBinaryOp::NotEqual
        | NativeBinaryOp::Or => unreachable!("not an arithmetic operator"),
    }
}

fn add_value(left: DataviewValue, right: DataviewValue) -> DataviewValue {
    match (left, right) {
        (DataviewValue::Number(left), DataviewValue::Number(right)) => {
            add_numbers(&left, &right)
        }
        (DataviewValue::Array(mut left), DataviewValue::Array(right)) => {
            left.extend(right);
            DataviewValue::Array(left)
        }
        (DataviewValue::Array(mut left), right) => {
            left.push(right);
            DataviewValue::Array(left)
        }
        (left, DataviewValue::Array(mut right)) => {
            right.insert(0, left);
            DataviewValue::Array(right)
        }
        (DataviewValue::Null, _) | (_, DataviewValue::Null) => {
            DataviewValue::Null
        }
        (left, right) => DataviewValue::String(format!(
            "{}{}",
            value_text(&left),
            value_text(&right)
        )),
    }
}

fn add_numbers(left: &Number, right: &Number) -> DataviewValue {
    if let (Some(left), Some(right)) = (left.as_i64(), right.as_i64())
        && let Some(value) = left.checked_add(right)
    {
        return DataviewValue::Number(Number::from(value));
    }
    number_from_f64(
        left.as_f64().unwrap_or(0.0) + right.as_f64().unwrap_or(0.0),
    )
}

fn numeric_binary_value(
    left: DataviewValue,
    right: DataviewValue,
    op: impl FnOnce(f64, f64) -> f64,
) -> DataviewValue {
    let (DataviewValue::Number(left), DataviewValue::Number(right)) =
        (left, right)
    else {
        return DataviewValue::Null;
    };
    number_from_f64_smart(op(
        left.as_f64().unwrap_or(0.0),
        right.as_f64().unwrap_or(0.0),
    ))
}

fn multiply_value(left: DataviewValue, right: DataviewValue) -> DataviewValue {
    match (&left, &right) {
        (DataviewValue::String(text), DataviewValue::Number(number))
        | (DataviewValue::Number(number), DataviewValue::String(text)) => {
            let count = number
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(0);
            DataviewValue::String(text.repeat(count))
        }
        _ => numeric_binary_value(left, right, |left, right| left * right),
    }
}

fn negate_value(value: DataviewValue) -> DataviewValue {
    let DataviewValue::Number(number) = value else {
        return DataviewValue::Null;
    };
    if let Some(value) = number.as_i64()
        && let Some(value) = value.checked_neg()
    {
        return DataviewValue::Number(Number::from(value));
    }
    number_from_f64(-number.as_f64().unwrap_or(0.0))
}

fn number_from_f64(value: f64) -> DataviewValue {
    Number::from_f64(value)
        .map(DataviewValue::Number)
        .unwrap_or(DataviewValue::Null)
}

fn number_from_f64_smart(value: f64) -> DataviewValue {
    if !value.is_finite() {
        return DataviewValue::Null;
    }
    if value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        return DataviewValue::Number(Number::from(value as i64));
    }
    number_from_f64(value)
}

fn value_text(value: &DataviewValue) -> String {
    match value {
        DataviewValue::Null => String::new(),
        DataviewValue::Bool(value) => value.to_string(),
        DataviewValue::Number(value) => value.to_string(),
        DataviewValue::String(value)
        | DataviewValue::Date(value)
        | DataviewValue::DateTime(value)
        | DataviewValue::Duration(value) => value.clone(),
        DataviewValue::Link(link) => {
            link.display.clone().unwrap_or_else(|| link.path.clone())
        }
        DataviewValue::Array(values) => {
            values.iter().map(value_text).collect::<Vec<_>>().join(", ")
        }
        DataviewValue::Object(_) => {
            serde_json::to_string(&value.to_plain_json()).unwrap_or_default()
        }
    }
}

struct NativeLexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> NativeLexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().peekable(),
        }
    }

    fn tokenize(mut self) -> Result<Vec<NativeToken>, String> {
        let mut tokens = Vec::new();
        while let Some(ch) = self.chars.next() {
            match ch {
                ch if ch.is_whitespace() => {}
                '(' => tokens.push(NativeToken::LParen),
                ')' => tokens.push(NativeToken::RParen),
                '[' if self.chars.peek() == Some(&'[') => {
                    self.chars.next();
                    tokens.push(NativeToken::Link(self.read_wikilink()?));
                }
                '[' => tokens.push(NativeToken::LBracket),
                ']' => tokens.push(NativeToken::RBracket),
                '{' => tokens.push(NativeToken::LBrace),
                '}' => tokens.push(NativeToken::RBrace),
                ',' => tokens.push(NativeToken::Comma),
                ':' => tokens.push(NativeToken::Colon),
                '.' => tokens.push(NativeToken::Dot),
                '+' => tokens.push(NativeToken::Plus),
                '-' => tokens.push(NativeToken::Minus),
                '*' => tokens.push(NativeToken::Star),
                '/' => tokens.push(NativeToken::Slash),
                '!' if self.chars.peek() == Some(&'=') => {
                    self.chars.next();
                    tokens.push(NativeToken::NotEqual);
                }
                '!' => tokens.push(NativeToken::Not),
                '=' if self.chars.peek() == Some(&'>') => {
                    self.chars.next();
                    tokens.push(NativeToken::Arrow);
                }
                '=' => tokens.push(NativeToken::Equal),
                '<' if self.chars.peek() == Some(&'=') => {
                    self.chars.next();
                    tokens.push(NativeToken::LessEqual);
                }
                '<' => tokens.push(NativeToken::Less),
                '>' if self.chars.peek() == Some(&'=') => {
                    self.chars.next();
                    tokens.push(NativeToken::GreaterEqual);
                }
                '>' => tokens.push(NativeToken::Greater),
                '#' => tokens.push(NativeToken::Tag(self.read_tag())),
                '"' => tokens
                    .push(NativeToken::String(self.read_quoted_string('"')?)),
                '\'' => tokens
                    .push(NativeToken::String(self.read_quoted_string('\'')?)),
                ch if ch.is_ascii_digit() => {
                    tokens.push(NativeToken::Number(self.read_number(ch)));
                }
                ch if is_native_identifier_start(ch) => {
                    let identifier = self.read_identifier(ch);
                    tokens.push(native_identifier_token(identifier));
                }
                other => {
                    return Err(format!(
                        "unsupported token {other:?}; native engine supports \
                         LIST, TABLE, FROM, WHERE, AND, OR, parentheses, \
                         comma-separated table fields, field names, strings, \
                         booleans, and wikilinks"
                    ));
                }
            }
        }
        tokens.push(NativeToken::Eof);
        Ok(tokens)
    }

    fn read_quoted_string(&mut self, quote: char) -> Result<String, String> {
        let mut output = String::new();
        while let Some(ch) = self.chars.next() {
            if ch == quote {
                return Ok(output);
            }
            if ch == '\\' && quote == '"' {
                let Some(escaped) = self.chars.next() else {
                    return Err("unterminated escape in string literal".into());
                };
                output.push(match escaped {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            } else {
                output.push(ch);
            }
        }

        Err("unterminated string literal".into())
    }

    fn read_wikilink(&mut self) -> Result<String, String> {
        let mut output = String::new();
        while let Some(ch) = self.chars.next() {
            if ch == ']' && self.chars.peek() == Some(&']') {
                self.chars.next();
                return Ok(output);
            }
            output.push(ch);
        }

        Err("unterminated wikilink literal".into())
    }

    fn read_tag(&mut self) -> String {
        let mut output = String::from("#");
        while self
            .chars
            .peek()
            .is_some_and(|ch| is_native_tag_continue(*ch))
        {
            output
                .push(self.chars.next().expect("peek confirmed tag character"));
        }
        output
    }

    fn read_number(&mut self, first: char) -> String {
        let mut output = String::from(first);
        while self
            .chars
            .peek()
            .is_some_and(|ch| ch.is_ascii_digit() || *ch == '.')
        {
            output.push(
                self.chars.next().expect("peek confirmed number character"),
            );
        }
        output
    }

    fn read_identifier(&mut self, first: char) -> String {
        let mut output = String::from(first);
        while self
            .chars
            .peek()
            .is_some_and(|ch| is_native_identifier_continue(*ch))
        {
            output.push(
                self.chars
                    .next()
                    .expect("peek confirmed identifier character"),
            );
        }
        output
    }
}

struct NativeParser {
    tokens: Vec<NativeToken>,
    position: usize,
}

impl NativeParser {
    fn new(tokens: Vec<NativeToken>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse_query(&mut self) -> Result<NativeQuery, String> {
        let kind = self.parse_query_kind()?;
        let mut commands = Vec::new();
        while !self.at_eof() {
            commands.push(self.parse_data_command()?);
        }
        self.expect_eof()?;
        Ok(NativeQuery { kind, commands })
    }

    fn parse_source_query(&mut self) -> Result<NativeSourceExpr, String> {
        let source = self.parse_source_expr()?;
        self.expect_eof()?;
        Ok(source)
    }

    fn parse_data_command(&mut self) -> Result<NativeDataCommand, String> {
        match self.peek() {
            NativeToken::From => {
                self.position += 1;
                Ok(NativeDataCommand::From(self.parse_source_expr()?))
            }
            NativeToken::Where => {
                self.position += 1;
                let tokens = self.collect_expression(ExpressionStop::Data)?;
                Ok(NativeDataCommand::Where(NativeExpression::where_clause(
                    tokens,
                )?))
            }
            NativeToken::Sort => {
                self.position += 1;
                let mut tokens =
                    self.collect_expression(ExpressionStop::Data)?;
                let direction = match tokens.last() {
                    Some(NativeToken::Asc) => {
                        tokens.pop();
                        Some(SortDirection::Ascending)
                    }
                    Some(NativeToken::Desc) => {
                        tokens.pop();
                        Some(SortDirection::Descending)
                    }
                    _ => None,
                };
                Ok(NativeDataCommand::Sort {
                    expression: NativeExpression::new(tokens)?,
                    direction,
                })
            }
            NativeToken::Group => {
                self.position += 1;
                self.expect_by()?;
                let expression = self.parse_aliased_command_expression()?;
                Ok(NativeDataCommand::GroupBy {
                    expression: expression.0,
                    alias: expression.1,
                })
            }
            NativeToken::Flatten => {
                self.position += 1;
                let expression = self.parse_aliased_command_expression()?;
                Ok(NativeDataCommand::Flatten {
                    expression: expression.0,
                    alias: expression.1,
                })
            }
            NativeToken::Limit => {
                self.position += 1;
                Ok(NativeDataCommand::Limit(self.expect_limit()?))
            }
            token => Err(format!(
                "expected DQL data command, found {}; native parser supports \
                 FROM, WHERE, SORT, GROUP BY, FLATTEN, and LIMIT",
                native_token_name(token)
            )),
        }
    }

    fn parse_aliased_command_expression(
        &mut self,
    ) -> Result<(NativeExpression, Option<String>), String> {
        let tokens = self.collect_expression(ExpressionStop::DataOrAs)?;
        let expression = NativeExpression::new(tokens)?;
        let alias = if self.take_as() {
            Some(self.expect_alias()?)
        } else {
            None
        };
        Ok((expression, alias))
    }

    fn parse_source_expr(&mut self) -> Result<NativeSourceExpr, String> {
        self.parse_source_or()
    }

    fn parse_source_or(&mut self) -> Result<NativeSourceExpr, String> {
        let mut source = self.parse_source_and()?;
        while self.take_or() {
            let right = self.parse_source_and()?;
            source = NativeSourceExpr::Or(Box::new(source), Box::new(right));
        }
        Ok(source)
    }

    fn parse_source_and(&mut self) -> Result<NativeSourceExpr, String> {
        let mut source = self.parse_source_unary()?;
        while self.take_and() {
            let right = self.parse_source_unary()?;
            source = NativeSourceExpr::And(Box::new(source), Box::new(right));
        }
        Ok(source)
    }

    fn parse_source_unary(&mut self) -> Result<NativeSourceExpr, String> {
        if self.take_minus() {
            return Ok(NativeSourceExpr::Not(Box::new(
                self.parse_source_unary()?,
            )));
        }
        self.parse_source_primary()
    }

    fn parse_source_primary(&mut self) -> Result<NativeSourceExpr, String> {
        match self.peek() {
            NativeToken::Tag(tag) => {
                let tag = tag.clone();
                self.position += 1;
                Ok(NativeSourceExpr::Tag(tag))
            }
            NativeToken::String(path) => {
                let path = path.clone();
                self.position += 1;
                Ok(NativeSourceExpr::Path(path))
            }
            NativeToken::Link(link) => {
                let link = link.clone();
                self.position += 1;
                Ok(NativeSourceExpr::IncomingLink(link))
            }
            NativeToken::Identifier(identifier)
                if identifier.eq_ignore_ascii_case("outgoing") =>
            {
                self.position += 1;
                self.expect_lparen()?;
                let link = self.expect_link()?;
                self.expect_rparen()?;
                Ok(NativeSourceExpr::OutgoingLink(link))
            }
            NativeToken::LParen => {
                self.position += 1;
                let source = self.parse_source_expr()?;
                self.expect_rparen()?;
                Ok(source)
            }
            token => Err(format!(
                "expected Dataview source expression, found {}; native source \
                 expressions support tags, quoted folders/files, wikilinks, \
                 outgoing([[note]]), AND, OR, unary -, and parentheses",
                native_token_name(token)
            )),
        }
    }

    fn parse_expr(&mut self) -> Result<NativeExpr, String> {
        self.parse_lambda()
    }

    fn parse_lambda(&mut self) -> Result<NativeExpr, String> {
        let start = self.position;
        if let NativeToken::Identifier(parameter) = self.peek().clone() {
            self.position += 1;
            if self.take_arrow() {
                let body = self.parse_lambda()?;
                return Ok(NativeExpr::Lambda {
                    parameter,
                    body: Box::new(body),
                });
            }
        }
        self.position = start;

        if matches!(self.peek(), NativeToken::LParen) {
            self.position += 1;
            if let NativeToken::Identifier(parameter) = self.peek().clone() {
                self.position += 1;
                if matches!(self.peek(), NativeToken::RParen) {
                    self.position += 1;
                    if self.take_arrow() {
                        let body = self.parse_lambda()?;
                        return Ok(NativeExpr::Lambda {
                            parameter,
                            body: Box::new(body),
                        });
                    }
                }
            }
        }
        self.position = start;

        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_and()?;
        while self.take_or() {
            let right = self.parse_and()?;
            expr = NativeExpr::Binary {
                op: NativeBinaryOp::Or,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_comparison()?;
        while self.take_and() {
            let right = self.parse_comparison()?;
            expr = NativeExpr::Binary {
                op: NativeBinaryOp::And,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_term()?;
        while let Some(op) = self.take_comparison_op() {
            let right = self.parse_term()?;
            expr = NativeExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_term(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_factor()?;
        while let Some(op) = self.take_additive_op() {
            let right = self.parse_factor()?;
            expr = NativeExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_factor(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_unary()?;
        while let Some(op) = self.take_multiplicative_op() {
            let right = self.parse_unary()?;
            expr = NativeExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<NativeExpr, String> {
        if self.take_not() {
            return Ok(NativeExpr::Unary {
                op: NativeUnaryOp::Not,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.take_minus() {
            return Ok(NativeExpr::Unary {
                op: NativeUnaryOp::Negate,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_primary()?;
        while self.take_dot() {
            let field = self.expect_identifier()?;
            expr = NativeExpr::GetAttr {
                target: Box::new(expr),
                field,
            };
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<NativeExpr, String> {
        match self.peek() {
            NativeToken::Bool(value) => {
                let value = *value;
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::Bool(value)))
            }
            NativeToken::Null => {
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::Null))
            }
            NativeToken::Number(value) => {
                let value = parse_expression_number(value)?;
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::Number(value)))
            }
            NativeToken::String(value) => {
                let value = value.clone();
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::String(value)))
            }
            NativeToken::Link(value) => {
                let value = value.clone();
                self.position += 1;
                Ok(NativeExpr::LinkLiteral(value))
            }
            NativeToken::Identifier(identifier) => {
                let identifier = identifier.clone();
                self.position += 1;
                if self.take_lparen() {
                    Ok(NativeExpr::Call {
                        function: identifier,
                        args: self.parse_call_args()?,
                    })
                } else {
                    Ok(NativeExpr::Identifier(identifier))
                }
            }
            NativeToken::List => {
                self.position += 1;
                if self.take_lparen() {
                    Ok(NativeExpr::Call {
                        function: "list".to_string(),
                        args: self.parse_call_args()?,
                    })
                } else {
                    Err("LIST is only valid as a query type or list(...) constructor".to_string())
                }
            }
            NativeToken::Sort => {
                self.position += 1;
                if self.take_lparen() {
                    Ok(NativeExpr::Call {
                        function: "sort".to_string(),
                        args: self.parse_call_args()?,
                    })
                } else {
                    Err("SORT is only valid as a data command or sort(...) function".to_string())
                }
            }
            NativeToken::LParen => {
                self.position += 1;
                let expr = self.parse_expr()?;
                self.expect_rparen()?;
                Ok(expr)
            }
            NativeToken::LBracket => self.parse_array(),
            NativeToken::LBrace => self.parse_object(),
            token => Err(format!(
                "expected expression, found {}; native expression parser \
                 supports literals, field access, calls, arrays, objects, \
                 lambdas, operators, and parentheses",
                native_token_name(token)
            )),
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<NativeExpr>, String> {
        let mut args = Vec::new();
        if self.take_rparen() {
            return Ok(args);
        }
        loop {
            args.push(self.parse_expr()?);
            if self.take_comma() {
                continue;
            }
            self.expect_rparen()?;
            return Ok(args);
        }
    }

    fn parse_array(&mut self) -> Result<NativeExpr, String> {
        self.position += 1;
        let mut values = Vec::new();
        if self.take_rbracket() {
            return Ok(NativeExpr::Array(values));
        }
        loop {
            values.push(self.parse_expr()?);
            if self.take_comma() {
                continue;
            }
            self.expect_rbracket()?;
            return Ok(NativeExpr::Array(values));
        }
    }

    fn parse_object(&mut self) -> Result<NativeExpr, String> {
        self.position += 1;
        let mut values = Vec::new();
        if self.take_rbrace() {
            return Ok(NativeExpr::Object(values));
        }
        loop {
            let key = self.expect_object_key()?;
            self.expect_colon()?;
            let value = self.parse_expr()?;
            values.push((key, value));
            if self.take_comma() {
                continue;
            }
            self.expect_rbrace()?;
            return Ok(NativeExpr::Object(values));
        }
    }

    fn parse_query_kind(&mut self) -> Result<NativeQueryKind, String> {
        match self.peek() {
            NativeToken::List => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                let expression = if self.at_data_command() || self.at_eof() {
                    None
                } else {
                    Some(NativeExpression::new(
                        self.collect_expression(ExpressionStop::Data)?,
                    )?)
                };
                Ok(NativeQueryKind::List {
                    expression,
                    without_id,
                })
            }
            NativeToken::Table => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                let mut columns = vec![self.parse_table_select()?];
                while self.take_comma() {
                    columns.push(self.parse_table_select()?);
                }
                Ok(NativeQueryKind::Table {
                    columns,
                    without_id,
                })
            }
            NativeToken::Task => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                Ok(NativeQueryKind::Task {
                    _without_id: without_id,
                })
            }
            NativeToken::Calendar => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                let expression = NativeExpression::new(
                    self.collect_expression(ExpressionStop::Data)?,
                )?;
                Ok(NativeQueryKind::Calendar {
                    expression,
                    _without_id: without_id,
                })
            }
            token => Err(format!(
                "native parser supports LIST, TABLE, TASK, and CALENDAR \
                 queries; found {}",
                native_token_name(token)
            )),
        }
    }

    fn parse_table_select(&mut self) -> Result<NativeSelect, String> {
        let expression = NativeExpression::new(
            self.collect_expression(ExpressionStop::TableSelect)?,
        )?;
        let alias = if self.take_as() {
            Some(self.expect_alias()?)
        } else {
            None
        };
        Ok(NativeSelect { expression, alias })
    }

    fn collect_expression(
        &mut self,
        stop: ExpressionStop,
    ) -> Result<Vec<NativeToken>, String> {
        let mut tokens = Vec::new();
        let mut depth = 0usize;
        loop {
            let token = self.peek();
            if matches!(token, NativeToken::Eof) {
                break;
            }
            if depth == 0
                && stop.stops_at(token)
                && !self.current_token_starts_function_call()
            {
                break;
            }

            match token {
                NativeToken::LParen
                | NativeToken::LBracket
                | NativeToken::LBrace => {
                    depth += 1;
                }
                NativeToken::RParen
                | NativeToken::RBracket
                | NativeToken::RBrace => {
                    if depth == 0 {
                        return Err(format!(
                            "unexpected {} in DQL expression",
                            native_token_name(token)
                        ));
                    }
                    depth -= 1;
                }
                _ => {}
            }
            tokens.push(token.clone());
            self.position += 1;
        }

        if depth != 0 {
            return Err("unterminated grouping in DQL expression".to_string());
        }
        if tokens.is_empty() {
            return Err("expected DQL expression".to_string());
        }
        Ok(tokens)
    }

    fn take_and(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::And))
    }

    fn take_or(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Or))
    }

    fn take_dot(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Dot))
    }

    fn take_comma(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Comma))
    }

    fn take_minus(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Minus))
    }

    fn take_not(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Not))
    }

    fn take_arrow(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Arrow))
    }

    fn take_lparen(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::LParen))
    }

    fn take_rparen(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::RParen))
    }

    fn take_rbracket(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::RBracket))
    }

    fn take_rbrace(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::RBrace))
    }

    fn take_comparison_op(&mut self) -> Option<NativeBinaryOp> {
        let op = match self.peek() {
            NativeToken::Equal => NativeBinaryOp::Equal,
            NativeToken::NotEqual => NativeBinaryOp::NotEqual,
            NativeToken::Less => NativeBinaryOp::Less,
            NativeToken::LessEqual => NativeBinaryOp::LessEqual,
            NativeToken::Greater => NativeBinaryOp::Greater,
            NativeToken::GreaterEqual => NativeBinaryOp::GreaterEqual,
            _ => return None,
        };
        self.position += 1;
        Some(op)
    }

    fn take_additive_op(&mut self) -> Option<NativeBinaryOp> {
        let op = match self.peek() {
            NativeToken::Plus => NativeBinaryOp::Add,
            NativeToken::Minus => NativeBinaryOp::Subtract,
            _ => return None,
        };
        self.position += 1;
        Some(op)
    }

    fn take_multiplicative_op(&mut self) -> Option<NativeBinaryOp> {
        let op = match self.peek() {
            NativeToken::Star => NativeBinaryOp::Multiply,
            NativeToken::Slash => NativeBinaryOp::Divide,
            _ => return None,
        };
        self.position += 1;
        Some(op)
    }

    fn take_as(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::As))
    }

    fn take_without_id(&mut self) -> Result<bool, String> {
        if !self.take(|token| matches!(token, NativeToken::Without)) {
            return Ok(false);
        }
        if self.take(|token| matches!(token, NativeToken::Id)) {
            Ok(true)
        } else {
            Err(format!(
                "expected ID after WITHOUT, found {}",
                native_token_name(self.peek())
            ))
        }
    }

    fn take(&mut self, predicate: impl FnOnce(&NativeToken) -> bool) -> bool {
        if predicate(self.peek()) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect_alias(&mut self) -> Result<String, String> {
        match self.peek() {
            NativeToken::Identifier(alias) => {
                let alias = alias.clone();
                self.position += 1;
                Ok(alias)
            }
            NativeToken::String(alias) => {
                let alias = alias.clone();
                self.position += 1;
                Ok(alias)
            }
            token => Err(format!(
                "expected alias after AS, found {}",
                native_token_name(token)
            )),
        }
    }

    fn expect_identifier(&mut self) -> Result<String, String> {
        let NativeToken::Identifier(identifier) = self.peek() else {
            return Err(format!(
                "expected field name, found {}",
                native_token_name(self.peek())
            ));
        };
        let identifier = identifier.clone();
        self.position += 1;
        Ok(identifier)
    }

    fn expect_link(&mut self) -> Result<String, String> {
        let NativeToken::Link(link) = self.peek() else {
            return Err(format!(
                "expected wikilink, found {}",
                native_token_name(self.peek())
            ));
        };
        let link = link.clone();
        self.position += 1;
        Ok(link)
    }

    fn expect_limit(&mut self) -> Result<usize, String> {
        let NativeToken::Number(limit) = self.peek() else {
            return Err(format!(
                "expected LIMIT count, found {}",
                native_token_name(self.peek())
            ));
        };
        let limit = limit.parse::<usize>().map_err(|_| {
            format!("LIMIT count must be a non-negative integer: {limit}")
        })?;
        self.position += 1;
        Ok(limit)
    }

    fn expect_by(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::By) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected BY after GROUP, found {}",
            native_token_name(self.peek())
        ))
    }

    fn expect_lparen(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::LParen) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected '(', found {}",
            native_token_name(self.peek())
        ))
    }

    fn expect_rparen(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::RParen) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected ')', found {}",
            native_token_name(self.peek())
        ))
    }

    fn expect_rbracket(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::RBracket) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected ']', found {}",
            native_token_name(self.peek())
        ))
    }

    fn expect_rbrace(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::RBrace) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected '}}', found {}",
            native_token_name(self.peek())
        ))
    }

    fn expect_colon(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::Colon) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected ':', found {}",
            native_token_name(self.peek())
        ))
    }

    fn expect_object_key(&mut self) -> Result<String, String> {
        match self.peek() {
            NativeToken::Identifier(key) | NativeToken::String(key) => {
                let key = key.clone();
                self.position += 1;
                Ok(key)
            }
            token => Err(format!(
                "expected object key, found {}",
                native_token_name(token)
            )),
        }
    }

    fn expect_eof(&self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::Eof) {
            return Ok(());
        }
        Err(format!(
            "unexpected {} after native query; native engine supports LIST \
             or TABLE <fields> FROM \"folder\" WHERE <expression>",
            native_token_name(self.peek())
        ))
    }

    fn peek(&self) -> &NativeToken {
        self.tokens
            .get(self.position)
            .unwrap_or_else(|| self.tokens.last().expect("lexer adds EOF"))
    }

    fn current_token_starts_function_call(&self) -> bool {
        matches!(self.peek(), NativeToken::Sort)
            && matches!(
                self.tokens.get(self.position + 1),
                Some(NativeToken::LParen)
            )
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), NativeToken::Eof)
    }

    fn at_data_command(&self) -> bool {
        is_data_command(self.peek())
    }
}

#[derive(Debug, Clone, Copy)]
enum ExpressionStop {
    Data,
    DataOrAs,
    TableSelect,
}

impl ExpressionStop {
    fn stops_at(self, token: &NativeToken) -> bool {
        match self {
            Self::Data => is_data_command(token),
            Self::DataOrAs => {
                is_data_command(token) || matches!(token, NativeToken::As)
            }
            Self::TableSelect => {
                is_data_command(token)
                    || matches!(token, NativeToken::Comma | NativeToken::As)
            }
        }
    }
}

fn native_query_error(message: String) -> DataviewError {
    DataviewError::NativeQuery { message }
}

fn parse_native_expression(
    tokens: Vec<NativeToken>,
) -> Result<NativeExpr, String> {
    let mut tokens = tokens;
    tokens.push(NativeToken::Eof);
    let mut parser = NativeParser::new(tokens);
    let expr = parser.parse_expr()?;
    parser.expect_eof()?;
    Ok(expr)
}

fn parse_expression_number(value: &str) -> Result<Number, String> {
    if let Ok(value) = value.parse::<i64>() {
        return Ok(Number::from(value));
    }
    if let Ok(value) = value.parse::<u64>() {
        return Ok(Number::from(value));
    }
    value
        .parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .ok_or_else(|| format!("invalid number literal: {value}"))
}

fn field_chain_from_tokens(tokens: &[NativeToken]) -> Option<Vec<String>> {
    let mut tokens = tokens.iter();
    let NativeToken::Identifier(first) = tokens.next()? else {
        return None;
    };
    let mut chain = vec![first.clone()];
    loop {
        match tokens.next() {
            None => return Some(chain),
            Some(NativeToken::Dot) => {
                let Some(NativeToken::Identifier(field)) = tokens.next() else {
                    return None;
                };
                chain.push(field.clone());
            }
            Some(_) => return None,
        }
    }
}

fn expression_tokens_to_string(tokens: &[NativeToken]) -> String {
    if let Some(chain) = field_chain_from_tokens(tokens) {
        return chain.join(".");
    }

    let mut output = String::new();
    let mut previous_word = false;
    for token in tokens {
        let piece = token_expression_piece(token);
        let current_word = token_is_wordlike(token);
        if !output.is_empty()
            && should_space_expression_piece(
                &output,
                previous_word,
                current_word,
                token,
            )
        {
            output.push(' ');
        }
        output.push_str(&piece);
        previous_word = current_word;
    }
    output
}

fn token_expression_piece(token: &NativeToken) -> String {
    match token {
        NativeToken::And => "AND".to_string(),
        NativeToken::As => "AS".to_string(),
        NativeToken::Asc => "ASC".to_string(),
        NativeToken::Bool(value) => value.to_string(),
        NativeToken::By => "BY".to_string(),
        NativeToken::Calendar => "CALENDAR".to_string(),
        NativeToken::Colon => ":".to_string(),
        NativeToken::Comma => ",".to_string(),
        NativeToken::Desc => "DESC".to_string(),
        NativeToken::Dot => ".".to_string(),
        NativeToken::Equal => "=".to_string(),
        NativeToken::Arrow => "=>".to_string(),
        NativeToken::Eof => String::new(),
        NativeToken::Flatten => "FLATTEN".to_string(),
        NativeToken::From => "FROM".to_string(),
        NativeToken::Greater => ">".to_string(),
        NativeToken::GreaterEqual => ">=".to_string(),
        NativeToken::Group => "GROUP".to_string(),
        NativeToken::Identifier(value) => value.clone(),
        NativeToken::LBrace => "{".to_string(),
        NativeToken::LBracket => "[".to_string(),
        NativeToken::Less => "<".to_string(),
        NativeToken::LessEqual => "<=".to_string(),
        NativeToken::Link(value) => format!("[[{value}]]"),
        NativeToken::List => "LIST".to_string(),
        NativeToken::LParen => "(".to_string(),
        NativeToken::Minus => "-".to_string(),
        NativeToken::Not => "!".to_string(),
        NativeToken::NotEqual => "!=".to_string(),
        NativeToken::Null => "null".to_string(),
        NativeToken::Number(value) => value.clone(),
        NativeToken::Or => "OR".to_string(),
        NativeToken::Plus => "+".to_string(),
        NativeToken::RBrace => "}".to_string(),
        NativeToken::RBracket => "]".to_string(),
        NativeToken::RParen => ")".to_string(),
        NativeToken::Slash => "/".to_string(),
        NativeToken::String(value) => format!("{value:?}"),
        NativeToken::Sort => "SORT".to_string(),
        NativeToken::Star => "*".to_string(),
        NativeToken::Tag(value) => value.clone(),
        NativeToken::Table => "TABLE".to_string(),
        NativeToken::Task => "TASK".to_string(),
        NativeToken::Limit => "LIMIT".to_string(),
        NativeToken::Without => "WITHOUT".to_string(),
        NativeToken::Where => "WHERE".to_string(),
        NativeToken::Id => "ID".to_string(),
    }
}

fn token_is_wordlike(token: &NativeToken) -> bool {
    matches!(
        token,
        NativeToken::And
            | NativeToken::As
            | NativeToken::Asc
            | NativeToken::Bool(_)
            | NativeToken::By
            | NativeToken::Calendar
            | NativeToken::Desc
            | NativeToken::Flatten
            | NativeToken::From
            | NativeToken::Group
            | NativeToken::Identifier(_)
            | NativeToken::Link(_)
            | NativeToken::List
            | NativeToken::Null
            | NativeToken::Number(_)
            | NativeToken::Or
            | NativeToken::Sort
            | NativeToken::String(_)
            | NativeToken::Tag(_)
            | NativeToken::Table
            | NativeToken::Task
            | NativeToken::Limit
            | NativeToken::Without
            | NativeToken::Where
            | NativeToken::Id
    )
}

fn should_space_expression_piece(
    output: &str,
    previous_word: bool,
    current_word: bool,
    token: &NativeToken,
) -> bool {
    if matches!(
        token,
        NativeToken::Comma
            | NativeToken::Dot
            | NativeToken::RParen
            | NativeToken::RBracket
            | NativeToken::RBrace
    ) {
        return false;
    }
    if output.ends_with(['(', '[', '{', '.', '-', '!', '/']) {
        return false;
    }
    previous_word || current_word
}

fn is_data_command(token: &NativeToken) -> bool {
    matches!(
        token,
        NativeToken::From
            | NativeToken::Where
            | NativeToken::Sort
            | NativeToken::Group
            | NativeToken::Flatten
            | NativeToken::Limit
    )
}

fn source_order_key(path: &str) -> (String, String) {
    let name = note_stem(path).unwrap_or_else(|| path.to_string());
    (name.to_ascii_lowercase(), path.to_ascii_lowercase())
}

fn normalize_source_tag(tag: &str) -> String {
    let tag = tag.trim();
    if tag.starts_with('#') {
        tag.to_string()
    } else {
        format!("#{tag}")
    }
}

fn tag_matches_source(page_tag: &str, source_tag: &str) -> bool {
    page_tag == source_tag
        || page_tag
            .strip_prefix(source_tag)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn page_tags(page: &index::DataviewPage) -> Vec<String> {
    page.source_tags.clone()
}

fn page_outlink_paths(page: &index::DataviewPage) -> Vec<String> {
    page_file_array(page, "outlinks")
        .into_iter()
        .filter_map(|value| match value {
            DataviewValue::Link(link) => Some(link.path),
            _ => None,
        })
        .collect()
}

fn page_file_array(
    page: &index::DataviewPage,
    field: &str,
) -> Vec<DataviewValue> {
    let Some(DataviewValue::Object(file)) = page.fields.get("file") else {
        return Vec::new();
    };
    let Some(DataviewValue::Array(values)) = file.get(field) else {
        return Vec::new();
    };
    values.clone()
}

fn source_link_base(path: &str) -> &str {
    path.split_once('#').map_or(path, |(base, _)| base)
}

fn collect_native_markdown_paths(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), DataviewError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        }
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.clone(),
                error,
            }
        })?;
        if file_type.is_dir() {
            if !is_hidden_path_component(&entry.file_name()) {
                collect_native_markdown_paths(&path, paths)?;
            }
        } else if file_type.is_file() && has_markdown_extension(&path) {
            paths.push(path);
        }
    }

    Ok(())
}

fn is_hidden_path_component(component: &OsStr) -> bool {
    component.to_string_lossy().starts_with('.')
}

fn has_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn native_frontmatter_block(contents: &str) -> Option<&str> {
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
        let line_content = line.trim_end_matches(['\r', '\n']);
        if line_content == "---" {
            return Some(&rest[..offset]);
        }
        offset += line.len();
    }

    None
}

fn unquote_native_scalar(value: &str) -> String {
    if value.len() < 2 {
        return value.to_string();
    }

    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let Some(last) = value.chars().last() else {
        return String::new();
    };
    if !matches!(first, '"' | '\'') || first != last {
        return value.to_string();
    }

    let inner = &value[first.len_utf8()..value.len() - last.len_utf8()];
    if first == '\'' {
        return inner.replace("''", "'");
    }

    let mut output = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let Some(escaped) = chars.next() else {
                output.push(ch);
                break;
            };
            output.push(match escaped {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
        } else {
            output.push(ch);
        }
    }
    output
}

fn normalize_native_source_folder(source: &str) -> Result<String, String> {
    let mut folder = source.trim().replace('\\', "/");
    while let Some(stripped) = folder.strip_prefix("./") {
        folder = stripped.to_string();
    }
    folder = folder.trim_matches('/').to_string();

    if folder.is_empty() {
        return Err("native folder source must not be empty".to_string());
    }
    if folder.contains('\0') {
        return Err("native folder source contains a NUL byte".to_string());
    }
    for segment in folder.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!(
                "native folder source {source:?} is not a clean \
                 vault-relative folder"
            ));
        }
    }

    Ok(folder)
}

fn native_link_target(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let link = if let Some(rest) = trimmed.strip_prefix("[[") {
        let end = rest.find("]]")?;
        &rest[..end]
    } else {
        trimmed
    };
    let before_alias = link.split_once('|').map_or(link, |(target, _)| target);
    let before_subpath = before_alias
        .split_once('#')
        .map_or(before_alias, |(target, _)| target);
    let target = before_subpath.trim().replace('\\', "/");
    (!target.is_empty()).then_some(target)
}

fn native_expression_link(raw: &str) -> Option<DataviewLink> {
    let (target, display) = raw
        .split_once('|')
        .map_or((raw, None), |(target, display)| {
            (target, Some(display.trim().to_string()))
        });
    let target = target.trim();
    if target.is_empty() {
        return None;
    }

    Some(DataviewLink::new(
        normalized_link_literal_path(target),
        display.filter(|display| !display.is_empty()),
        false,
        target.to_string(),
    ))
}

fn normalized_link_literal_path(target: &str) -> String {
    let (base, subpath) = target
        .split_once('#')
        .map_or((target, None), |(base, subpath)| (base, Some(subpath)));
    let mut path = normalize_note_path(base.trim())
        .unwrap_or_else(|_| target.trim().replace('\\', "/"));
    if let Some(subpath) = subpath.filter(|subpath| !subpath.is_empty()) {
        path.push('#');
        path.push_str(subpath);
    }
    path
}

fn comparable_link_path(raw: &str) -> Option<String> {
    native_link_target(raw).and_then(|target| normalize_note_path(&target).ok())
}

fn note_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

fn is_native_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_native_identifier_continue(ch: char) -> bool {
    ch == '_' || ch == '-' || ch.is_ascii_alphanumeric()
}

fn is_native_tag_continue(ch: char) -> bool {
    !ch.is_whitespace()
        && !matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | ',' | '"' | '\'')
}

fn native_identifier_token(identifier: String) -> NativeToken {
    match identifier.to_ascii_lowercase().as_str() {
        "and" => NativeToken::And,
        "as" => NativeToken::As,
        "asc" | "ascending" => NativeToken::Asc,
        "by" => NativeToken::By,
        "calendar" => NativeToken::Calendar,
        "desc" | "descending" => NativeToken::Desc,
        "flatten" => NativeToken::Flatten,
        "false" => NativeToken::Bool(false),
        "from" => NativeToken::From,
        "group" => NativeToken::Group,
        "id" => NativeToken::Id,
        "limit" => NativeToken::Limit,
        "list" => NativeToken::List,
        "null" => NativeToken::Null,
        "or" => NativeToken::Or,
        "sort" => NativeToken::Sort,
        "table" => NativeToken::Table,
        "task" => NativeToken::Task,
        "true" => NativeToken::Bool(true),
        "where" => NativeToken::Where,
        "without" => NativeToken::Without,
        _ => NativeToken::Identifier(identifier),
    }
}

fn native_token_name(token: &NativeToken) -> &'static str {
    match token {
        NativeToken::And => "AND",
        NativeToken::As => "AS",
        NativeToken::Asc => "ASC",
        NativeToken::Bool(_) => "boolean",
        NativeToken::By => "BY",
        NativeToken::Calendar => "CALENDAR",
        NativeToken::Colon => "':'",
        NativeToken::Comma => "','",
        NativeToken::Desc => "DESC",
        NativeToken::Dot => "'.'",
        NativeToken::Equal => "'='",
        NativeToken::Arrow => "'=>'",
        NativeToken::Eof => "end of query",
        NativeToken::Flatten => "FLATTEN",
        NativeToken::From => "FROM",
        NativeToken::Greater => "'>'",
        NativeToken::GreaterEqual => "'>='",
        NativeToken::Group => "GROUP",
        NativeToken::Identifier(_) => "field name",
        NativeToken::LBrace => "'{'",
        NativeToken::LBracket => "'['",
        NativeToken::Less => "'<'",
        NativeToken::LessEqual => "'<='",
        NativeToken::Link(_) => "wikilink",
        NativeToken::List => "LIST",
        NativeToken::LParen => "'('",
        NativeToken::Minus => "'-'",
        NativeToken::Not => "'!'",
        NativeToken::NotEqual => "'!='",
        NativeToken::Null => "null",
        NativeToken::Number(_) => "number",
        NativeToken::Or => "OR",
        NativeToken::Plus => "'+'",
        NativeToken::RBrace => "'}'",
        NativeToken::RBracket => "']'",
        NativeToken::RParen => "')'",
        NativeToken::Slash => "'/'",
        NativeToken::String(_) => "string",
        NativeToken::Sort => "SORT",
        NativeToken::Star => "'*'",
        NativeToken::Tag(_) => "tag",
        NativeToken::Table => "TABLE",
        NativeToken::Task => "TASK",
        NativeToken::Limit => "LIMIT",
        NativeToken::Without => "WITHOUT",
        NativeToken::Where => "WHERE",
        NativeToken::Id => "ID",
    }
}

fn collect_list_paths(result: &Value, collector: &mut PathCollector) {
    let Some(values) = result_array(result) else {
        collector.warn("DQL list result missing values array".to_string());
        return;
    };

    if identifier_is_grouped(result.get("primaryMeaning")) {
        warn_grouped_rows("DQL list row", values.len(), collector);
        return;
    }

    for (index, value) in values.iter().enumerate() {
        let context = format!("DQL list row {}", index + 1);
        if !collector.add_identity(value, &context) {
            collector.warn(format!("{context} has no source note identity"));
        }
    }
}

fn collect_table_paths(result: &Value, collector: &mut PathCollector) {
    let Some(rows) = result_array(result) else {
        collector.warn("DQL table result missing values array".to_string());
        return;
    };

    if identifier_is_grouped(result.get("idMeaning")) {
        warn_grouped_rows("DQL table row", rows.len(), collector);
        return;
    }

    for (index, row) in rows.iter().enumerate() {
        let context = format!("DQL table row {}", index + 1);
        match table_row_identity(row) {
            Some(identity) if collector.add_identity(identity, &context) => {}
            _ => {
                collector.warn(format!("{context} has no source note identity"))
            }
        }
    }
}

fn collect_task_paths(result: &Value, collector: &mut PathCollector) {
    let Some(values) = result_array(result) else {
        collector.warn("DQL task result missing values array".to_string());
        return;
    };

    let mut row_number = 0;
    for value in values {
        collect_task_grouping(value, collector, &mut row_number);
    }
}

fn collect_calendar_paths(result: &Value, collector: &mut PathCollector) {
    let Some(values) = result_array(result) else {
        collector.warn("DQL calendar result missing values array".to_string());
        return;
    };

    for (index, row) in values.iter().enumerate() {
        let context = format!("DQL calendar row {}", index + 1);
        let identity =
            row.get("link").or_else(|| row.get("path")).unwrap_or(row);
        if !collector.add_identity(identity, &context) {
            collector.warn(format!("{context} has no source note identity"));
        }
    }
}

fn collect_unknown_result_paths(result: &Value, collector: &mut PathCollector) {
    if let Some(rows) = result_array(result) {
        for (index, row) in rows.iter().enumerate() {
            let context = format!("DQL row {}", index + 1);
            if !collector.add_identity(row, &context) {
                collector
                    .warn(format!("{context} has no source note identity"));
            }
        }
    } else {
        collector.warn(
            "DQL result missing a recognized type and values array".to_string(),
        );
    }
}

fn collect_task_grouping(
    value: &Value,
    collector: &mut PathCollector,
    row_number: &mut usize,
) {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                collect_task_grouping(entry, collector, row_number);
            }
        }
        Value::Object(map)
            if map.contains_key("key") && map.contains_key("rows") =>
        {
            collect_task_grouping(&map["rows"], collector, row_number);
        }
        Value::Object(_) => {
            *row_number += 1;
            let context = format!("DQL task row {}", *row_number);
            let identity = value
                .get("path")
                .or_else(|| value.get("link"))
                .or_else(|| value.get("section"))
                .or_else(|| value.get("file"))
                .unwrap_or(value);
            if !collector.add_identity(identity, &context) {
                collector
                    .warn(format!("{context} has no source note identity"));
            }
        }
        _ => {
            *row_number += 1;
            collector.warn(format!(
                "DQL task row {} has no source note identity",
                *row_number
            ));
        }
    }
}

fn result_array(result: &Value) -> Option<&Vec<Value>> {
    result
        .get("values")
        .or_else(|| result.get("rows"))
        .and_then(Value::as_array)
}

fn table_row_identity(row: &Value) -> Option<&Value> {
    match row {
        Value::Array(cells) => cells.first(),
        Value::Object(map) => map
            .get("id")
            .or_else(|| map.get("key"))
            .or_else(|| map.get("path"))
            .or_else(|| map.get("file"))
            .or(Some(row)),
        _ => Some(row),
    }
}

fn identifier_is_grouped(value: Option<&Value>) -> bool {
    value
        .and_then(|meaning| meaning.get("type"))
        .and_then(Value::as_str)
        == Some("group")
}

fn warn_grouped_rows(
    context_prefix: &str,
    row_count: usize,
    collector: &mut PathCollector,
) {
    if row_count == 0 {
        collector.warn(format!(
            "{context_prefix} set uses grouped identity; cannot derive \
             source note paths"
        ));
        return;
    }

    for index in 0..row_count {
        collector.warn(format!(
            "{context_prefix} {} uses grouped identity; cannot derive a \
             source note path",
            index + 1
        ));
    }
}

#[derive(Debug, Default)]
struct PathCollector {
    paths: Vec<String>,
    seen: HashSet<String>,
    warnings: Vec<String>,
}

impl PathCollector {
    fn add_identity(&mut self, value: &Value, context: &str) -> bool {
        if let Some(identity) = list_pair_identity(value) {
            return self.add_identity(identity, context);
        }

        if let Some(raw_path) = direct_path(value) {
            self.add_raw_path(raw_path, context)
        } else {
            false
        }
    }

    fn add_raw_path(&mut self, raw_path: &str, context: &str) -> bool {
        match normalize_note_path(raw_path) {
            Ok(path) => {
                if self.seen.insert(path.clone()) {
                    self.paths.push(path);
                }
                true
            }
            Err(reason) => {
                self.warn(format!("{context}: {reason}"));
                false
            }
        }
    }

    fn warn(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    fn finish(self, strict: bool) -> Result<PathExtraction, DataviewError> {
        if strict && !self.warnings.is_empty() {
            return Err(DataviewError::StrictPaths {
                warnings: self.warnings,
            });
        }

        Ok(PathExtraction {
            paths: self.paths,
            warnings: self.warnings,
        })
    }
}

fn list_pair_identity(value: &Value) -> Option<&Value> {
    let map = value.as_object()?;
    let widget = map.get("$widget").and_then(Value::as_str);
    if widget == Some("dataview:list-pair") {
        return map.get("key").or_else(|| map.get("id"));
    }

    None
}

fn direct_path(value: &Value) -> Option<&str> {
    match value {
        Value::String(path) => Some(path),
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("link")
                && let Some(path) = map.get("path").and_then(Value::as_str)
            {
                return Some(path);
            }

            map.get("path")
                .and_then(Value::as_str)
                .or_else(|| nested_path(map.get("file")))
                .or_else(|| nested_path(map.get("link")))
                .or_else(|| nested_path(map.get("section")))
        }
        _ => None,
    }
}

fn nested_path(value: Option<&Value>) -> Option<&str> {
    value?.as_object()?.get("path").and_then(Value::as_str)
}

fn normalize_note_path(raw_path: &str) -> Result<String, String> {
    if raw_path.is_empty() {
        return Err("empty path".to_string());
    }

    let without_subpath =
        raw_path.split_once('#').map_or(raw_path, |(path, _)| path);
    let mut path = without_subpath.replace('\\', "/");
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_string();
    }

    if path.is_empty() {
        return Err(format!("path {raw_path:?} does not name a note"));
    }
    if path.starts_with('/') {
        return Err(format!("path {raw_path:?} is not vault-relative"));
    }
    if path.contains('\0') {
        return Err(format!("path {raw_path:?} contains a NUL byte"));
    }

    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!(
                "path {raw_path:?} is not a clean vault-relative path"
            ));
        }
    }

    if !path.ends_with(".md") {
        path.push_str(".md");
    }

    Ok(path)
}

#[derive(Debug, Serialize)]
struct ObsidianEvalRequest {
    format: &'static str,
    origin: Option<String>,
    query: ObsidianEvalQuery,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ObsidianEvalQuery {
    Source { source: String },
    Dql { query: String },
}

#[derive(Debug)]
struct EngineOutput {
    response: EngineResponse,
    warnings: Vec<String>,
}

#[derive(Debug)]
enum EngineResponse {
    SourcePaths(Vec<String>),
    DqlJson(Value),
    Markdown(String),
}

#[derive(Debug, Deserialize)]
struct ProtocolEnvelope {
    status: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl ProtocolEnvelope {
    fn into_engine_output(self) -> Result<EngineOutput, DataviewError> {
        match self.status.as_str() {
            "ok" => self.ok_output(),
            "error" => Err(protocol_error(self.code, self.message)),
            other => Err(DataviewError::MalformedProtocolResponse {
                reason: format!("unexpected protocol status: {other}"),
            }),
        }
    }

    fn ok_output(self) -> Result<EngineOutput, DataviewError> {
        let response = match self.kind.as_deref() {
            Some("source_paths") => {
                EngineResponse::SourcePaths(self.paths.ok_or_else(|| {
                    DataviewError::MalformedProtocolResponse {
                        reason: "source_paths response missing paths"
                            .to_string(),
                    }
                })?)
            }
            Some("dql_json") => {
                EngineResponse::DqlJson(self.result.ok_or_else(|| {
                    DataviewError::MalformedProtocolResponse {
                        reason: "dql_json response missing result".to_string(),
                    }
                })?)
            }
            Some("markdown") => {
                EngineResponse::Markdown(self.markdown.ok_or_else(|| {
                    DataviewError::MalformedProtocolResponse {
                        reason: "markdown response missing markdown"
                            .to_string(),
                    }
                })?)
            }
            Some(other) => {
                return Err(DataviewError::MalformedProtocolResponse {
                    reason: format!(
                        "unexpected protocol response kind: {other}"
                    ),
                });
            }
            None => {
                return Err(DataviewError::MalformedProtocolResponse {
                    reason: "protocol response missing kind".to_string(),
                });
            }
        };

        Ok(EngineOutput {
            response,
            warnings: self.warnings,
        })
    }
}

fn protocol_error(
    code: Option<String>,
    message: Option<String>,
) -> DataviewError {
    let code = code.unwrap_or_else(|| "ENGINE_ERROR".to_string());
    let message = message
        .unwrap_or_else(|| "Obsidian Dataview engine failed".to_string());

    match code.as_str() {
        "DATAVIEW_MISSING" => DataviewError::DataviewMissing { message },
        "DATAVIEW_QUERY_ERROR" => DataviewError::DataviewQuery { message },
        _ => DataviewError::ProtocolEngine { code, message },
    }
}

#[derive(Debug)]
enum DataviewError {
    DataviewMissing {
        message: String,
    },
    DataviewQuery {
        message: String,
    },
    MalformedProtocolResponse {
        reason: String,
    },
    MissingObsidianCommand {
        command: OsString,
    },
    MissingProtocolSentinel {
        output: String,
    },
    NativeQuery {
        message: String,
    },
    NativeVaultRead {
        path: PathBuf,
        error: io::Error,
    },
    ObsidianFailed {
        exit_code: i32,
        output: String,
    },
    ObsidianNotRunning {
        exit_code: i32,
        output: String,
    },
    ProtocolEngine {
        code: String,
        message: String,
    },
    QueryRead {
        path: Option<PathBuf>,
        error: io::Error,
    },
    RunObsidian {
        command: OsString,
        error: io::Error,
    },
    SerializeOutput(serde_json::Error),
    SerializeRequest(serde_json::Error),
    StrictPaths {
        warnings: Vec<String>,
    },
}

impl DataviewError {
    fn report(&self) {
        match self {
            Self::DataviewMissing { message } => {
                eprintln!(
                    "{COMMAND_NAME}: Dataview is disabled, missing, or not \
                     ready in Obsidian"
                );
                eprintln!("Dataview reported: {message}");
            }
            Self::DataviewQuery { message } => {
                eprintln!("{COMMAND_NAME}: Dataview query failed");
                eprintln!("Dataview reported: {message}");
            }
            Self::MalformedProtocolResponse { reason } => {
                eprintln!(
                    "{COMMAND_NAME}: malformed Obsidian protocol response"
                );
                eprintln!("{reason}");
            }
            Self::MissingObsidianCommand { command } => {
                eprintln!(
                    "{COMMAND_NAME}: Obsidian command not found: {}",
                    bob_env::os_to_string(command)
                );
                eprintln!(
                    "Install the Obsidian CLI, start Obsidian, or set \
                     {ENV_OBSIDIAN_COMMAND} to an executable path."
                );
            }
            Self::MissingProtocolSentinel { output } => {
                eprintln!("{COMMAND_NAME}: missing Obsidian protocol response");
                eprintln!(
                    "Expected a {RESULT_PREFIX:?}-prefixed JSON line from \
                     `obsidian eval`."
                );
                if !output.is_empty() {
                    eprintln!("obsidian stdout excerpt: {output}");
                }
            }
            Self::NativeQuery { message } => {
                eprintln!("{COMMAND_NAME}: native query failed");
                eprintln!("{message}");
            }
            Self::NativeVaultRead { path, error } => {
                eprintln!(
                    "{COMMAND_NAME}: failed to read vault path {}: {error}",
                    path.display()
                );
            }
            Self::ObsidianFailed { exit_code, output } => {
                eprintln!(
                    "{COMMAND_NAME}: Obsidian CLI eval failed with exit code \
                     {exit_code}"
                );
                if !output.is_empty() {
                    eprintln!("obsidian output excerpt: {output}");
                }
            }
            Self::ObsidianNotRunning { exit_code, output } => {
                eprintln!(
                    "{COMMAND_NAME}: Obsidian is not running or the CLI could \
                     not connect to it (exit code {exit_code})"
                );
                if !output.is_empty() {
                    eprintln!("obsidian output excerpt: {output}");
                }
            }
            Self::ProtocolEngine { code, message } => {
                eprintln!("{COMMAND_NAME}: Obsidian Dataview engine failed");
                eprintln!("{code}: {message}");
            }
            Self::QueryRead {
                path: Some(path),
                error,
            } => {
                eprintln!(
                    "{COMMAND_NAME}: failed to read query file {}: {error}",
                    path.display()
                );
            }
            Self::QueryRead { path: None, error } => {
                eprintln!(
                    "{COMMAND_NAME}: failed to read query from stdin: {error}"
                );
            }
            Self::RunObsidian { command, error } => {
                eprintln!(
                    "{COMMAND_NAME}: failed to run Obsidian command {}: {error}",
                    bob_env::os_to_string(command)
                );
            }
            Self::SerializeOutput(error) => {
                eprintln!(
                    "{COMMAND_NAME}: failed to serialize output JSON: {error}"
                );
            }
            Self::SerializeRequest(error) => {
                eprintln!(
                    "{COMMAND_NAME}: failed to serialize Obsidian eval request: \
                     {error}"
                );
            }
            Self::StrictPaths { warnings } => {
                eprintln!(
                    "{COMMAND_NAME}: paths output could not derive clean note \
                     paths"
                );
                for warning in warnings {
                    eprintln!("{COMMAND_NAME}: warning: {warning}");
                }
                eprintln!(
                    "Use --format json to inspect the raw Dataview result or \
                     omit --strict-paths for best-effort path output."
                );
            }
        }
    }

    fn exit_code(&self) -> i32 {
        match self {
            Self::ObsidianFailed { exit_code, .. }
            | Self::ObsidianNotRunning { exit_code, .. } => *exit_code,
            _ => 1,
        }
    }
}

fn child_output_excerpt(stdout: &str, stderr: &str) -> String {
    let output = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    output_excerpt(output)
}

fn stdout_excerpt(stdout: &str) -> String {
    output_excerpt(stdout.trim())
}

fn output_excerpt(output: &str) -> String {
    let redacted = redact_generated_code(output);
    let mut excerpt = redacted.chars().take(600).collect::<String>();
    if redacted.chars().count() > 600 {
        excerpt.push_str("...");
    }
    excerpt
}

fn redact_generated_code(output: &str) -> String {
    if let Some(position) = output.find("code=") {
        let mut redacted = output[..position + "code=".len()].to_string();
        redacted.push_str("<generated JavaScript>");
        return redacted;
    }
    output.to_string()
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Run Dataview queries against the Bob vault")
        .long_about(
            "Run Dataview source expressions or DQL queries against the Bob \
vault.\n\n\
Source expressions return matching page paths. DQL queries support path, JSON, \
and markdown output modes. The default native engine is a headless local \
implementation of the supported Bob source-expression and DQL surface. The \
explicit Obsidian engine runs against the live Dataview plugin when exact \
installed-plugin behavior is needed.",
        )
        .after_help(
            "Examples:\n  bob dataview --source '#project and -\"archive\"'\n  bob dataview --query 'LIST FROM #waiting'\n  bob dataview --format json --query-file ~/queries/projects.dql",
        )
        .arg_required_else_help(true)
        .group(
            ArgGroup::new("query-input")
                .required(true)
                .multiple(false)
                .args(["source", "query", "query-file"]),
        )
        .arg(bob_dir_arg())
        .arg(engine_arg())
        .arg(format_arg())
        .arg(origin_arg())
        .arg(query_arg())
        .arg(query_file_arg())
        .arg(source_arg())
        .arg(strict_paths_arg())
        .arg(vault_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn engine_arg() -> Arg {
    Arg::new("engine")
        .long("engine")
        .short('e')
        .value_name("ENGINE")
        .default_value("native")
        .value_parser(["native", "obsidian"])
        .help("Query engine: native for local headless Dataview, obsidian for exact live Dataview")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .default_value("paths")
        .value_parser(["json", "markdown", "paths"])
        .help("Output format; markdown is available only for DQL")
}

fn origin_arg() -> Arg {
    Arg::new("origin")
        .long("origin")
        .short('o')
        .value_name("VAULT_RELATIVE_PATH")
        .value_parser(OsStringValueParser::new())
        .help("Origin note for relative links and this")
}

fn query_arg() -> Arg {
    Arg::new("query")
        .long("query")
        .short('q')
        .value_name("DQL")
        .value_parser(NonEmptyStringValueParser::new())
        .help("Full Dataview DQL query")
}

fn query_file_arg() -> Arg {
    Arg::new("query-file")
        .long("query-file")
        .short('Q')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Read a Dataview DQL query from a file; use - for stdin")
}

fn source_arg() -> Arg {
    Arg::new("source")
        .long("source")
        .short('s')
        .value_name("SOURCE")
        .value_parser(NonEmptyStringValueParser::new())
        .help("Dataview source expression for page path lookup")
}

fn strict_paths_arg() -> Arg {
    Arg::new("strict-paths")
        .long("strict-paths")
        .short('S')
        .action(ArgAction::SetTrue)
        .help("Fail when paths output cannot derive clean note paths")
}

fn vault_arg() -> Arg {
    Arg::new("vault")
        .long("vault")
        .short('v')
        .value_name("NAME_OR_ID")
        .value_parser(NonEmptyStringValueParser::new())
        .help(
            "Obsidian engine vault name or ID; defaults to BOB_DATAVIEW_VAULT",
        )
}

impl Request {
    fn from_matches(
        matches: &ArgMatches,
        command: &mut ClapCommand,
    ) -> Result<Self, clap::Error> {
        let query = QueryInput::from_matches(matches);
        let format = OutputFormat::from_matches(matches);
        let engine = Engine::from_matches(matches);
        let strict_paths = matches.get_flag("strict-paths");

        if query.is_source() && format == OutputFormat::Markdown {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--format markdown requires a DQL query",
            ));
        }

        if strict_paths && format != OutputFormat::Paths {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--strict-paths can only be used with --format paths",
            ));
        }

        if engine == Engine::Native
            && matches.get_one::<String>("vault").is_some()
        {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--vault can only be used with --engine obsidian",
            ));
        }

        Ok(Self {
            query,
            format,
            engine,
            vault: VaultConfig::from_matches(
                matches,
                command,
                engine == Engine::Native,
                engine == Engine::Obsidian,
            )?,
            strict_paths,
        })
    }

    fn obsidian_eval_request(
        &self,
    ) -> Result<ObsidianEvalRequest, DataviewError> {
        let query = match &self.query {
            QueryInput::Source(source) => ObsidianEvalQuery::Source {
                source: source.clone(),
            },
            QueryInput::Dql(input) => ObsidianEvalQuery::Dql {
                query: input.read_query()?,
            },
        };

        Ok(ObsidianEvalRequest {
            format: self.format.as_str(),
            origin: self
                .vault
                .origin
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            query,
        })
    }
}

impl QueryInput {
    fn from_matches(matches: &ArgMatches) -> Self {
        if let Some(source) = matches.get_one::<String>("source") {
            return Self::Source(source.clone());
        }

        if let Some(query) = matches.get_one::<String>("query") {
            return Self::Dql(DqlInput::Inline(query.clone()));
        }

        let query_file = matches
            .get_one::<OsString>("query-file")
            .expect("clap query-input group requires query-file")
            .into();
        Self::Dql(DqlInput::File(query_file))
    }

    fn is_source(&self) -> bool {
        matches!(self, Self::Source(_))
    }
}

impl DqlInput {
    fn read_query(&self) -> Result<String, DataviewError> {
        match self {
            Self::Inline(query) => Ok(query.clone()),
            Self::File(path) if path.as_os_str() == OsStr::new("-") => {
                let mut query = String::new();
                io::stdin().read_to_string(&mut query).map_err(|error| {
                    DataviewError::QueryRead { path: None, error }
                })?;
                Ok(query)
            }
            Self::File(path) => fs::read_to_string(path).map_err(|error| {
                DataviewError::QueryRead {
                    path: Some(path.clone()),
                    error,
                }
            }),
        }
    }
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .expect("clap provides a default format")
            .as_str()
        {
            "json" => Self::Json,
            "markdown" => Self::Markdown,
            "paths" => Self::Paths,
            value => unreachable!("unexpected format value from clap: {value}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Paths => "paths",
        }
    }
}

impl Engine {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("engine")
            .expect("clap provides a default engine")
            .as_str()
        {
            "native" => Self::Native,
            "obsidian" => Self::Obsidian,
            value => unreachable!("unexpected engine value from clap: {value}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Obsidian => "obsidian",
        }
    }
}

impl VaultConfig {
    fn from_matches(
        matches: &ArgMatches,
        command: &mut ClapCommand,
        validate_default_bob_dir: bool,
        use_obsidian_vault: bool,
    ) -> Result<Self, clap::Error> {
        let bob_dir_arg = matches.get_one::<OsString>("bob-dir");
        let bob_dir = bob_dir_arg
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        if bob_dir_arg.is_some() || validate_default_bob_dir {
            validate_bob_dir(&bob_dir, command)?;
        }

        let origin = matches
            .get_one::<OsString>("origin")
            .map(PathBuf::from)
            .map(|path| {
                validate_origin_path(&path, command)?;
                Ok::<PathBuf, clap::Error>(path)
            })
            .transpose()?;
        let obsidian_vault = use_obsidian_vault
            .then(|| {
                matches
                    .get_one::<String>("vault")
                    .cloned()
                    .or_else(default_vault_from_env)
            })
            .flatten();

        Ok(Self {
            bob_dir,
            origin,
            obsidian_vault,
        })
    }
}

fn validate_bob_dir(
    bob_dir: &Path,
    command: &mut ClapCommand,
) -> Result<(), clap::Error> {
    if bob_dir.is_dir() {
        return Ok(());
    }

    Err(command.error(
        ErrorKind::ValueValidation,
        format!(
            "--bob-dir must name an existing Bob vault directory: {}",
            bob_dir.display()
        ),
    ))
}

fn validate_origin_path(
    origin: &Path,
    command: &mut ClapCommand,
) -> Result<(), clap::Error> {
    validate_vault_relative_path(origin).map_err(|reason| {
        command.error(
            ErrorKind::ValueValidation,
            format!("invalid --origin {}: {reason}", origin.display()),
        )
    })
}

fn validate_vault_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("path must not be empty".to_string());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    if path.to_string_lossy().contains('\0') {
        return Err("NUL bytes are not allowed".to_string());
    }

    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(".. traversal is not allowed".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }

    Ok(())
}

fn default_vault_from_env() -> Option<String> {
    env::var(ENV_VAULT).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn native_source_parser_accepts_phase3_source_surface() {
        assert!(matches!(
            NativeSourceExpr::parse("").expect("empty source parses"),
            NativeSourceExpr::All
        ));

        let source =
            NativeSourceExpr::parse(r#"(#project or "Daily") and -"Archive""#)
                .expect("source algebra parses");
        assert!(matches!(source, NativeSourceExpr::And(_, _)));

        let outgoing = NativeSourceExpr::parse("outgoing([[Links/Hub]])")
            .expect("outgoing source parses");
        assert!(matches!(outgoing, NativeSourceExpr::OutgoingLink(_)));
    }

    #[test]
    fn native_dql_parser_accepts_phase3_command_surface() {
        let query = NativeQuery::parse(
            r#"
TABLE WITHOUT ID owner AS Owner, choice(ready, "yes", "no") AS Readiness
FROM (#project OR "Daily") AND -"Archive"
WHERE ready AND owner = [[People/Ada Lovelace]]
SORT due DESC
GROUP BY status AS Status
FLATTEN aliases AS alias
LIMIT 5
"#,
        )
        .expect("phase 3 DQL surface parses");

        match query.kind {
            NativeQueryKind::Table {
                columns,
                without_id,
            } => {
                assert!(without_id);
                assert_eq!(columns.len(), 2);
                assert_eq!(columns[0].header(), "Owner");
                assert_eq!(columns[1].header(), "Readiness");
            }
            other => panic!("expected table query, got {other:?}"),
        }
        assert_eq!(query.commands.len(), 6);
        assert!(matches!(query.commands[0], NativeDataCommand::From(_)));
        assert!(matches!(query.commands[1], NativeDataCommand::Where(_)));
        assert!(matches!(query.commands[2], NativeDataCommand::Sort { .. }));
        assert!(matches!(
            query.commands[3],
            NativeDataCommand::GroupBy { .. }
        ));
        assert!(matches!(
            query.commands[4],
            NativeDataCommand::Flatten { .. }
        ));
        assert!(matches!(query.commands[5], NativeDataCommand::Limit(5)));
    }

    #[test]
    fn native_dql_parser_reports_representative_invalid_queries() {
        let table = native_error_message(
            NativeQuery::parse("TABLE FROM #project")
                .expect_err("missing table expression should fail"),
        );
        assert!(table.contains("expected DQL expression"), "{table}");

        let source = native_error_message(
            NativeQuery::parse("LIST FROM (#project or")
                .expect_err("unfinished source should fail"),
        );
        assert!(
            source.contains("expected Dataview source expression"),
            "{source}"
        );

        let outgoing = native_error_message(
            NativeSourceExpr::parse(r#"outgoing("Projects")"#)
                .expect_err("outgoing source requires wikilink"),
        );
        assert!(outgoing.contains("expected wikilink"), "{outgoing}");
    }

    #[test]
    fn source_paths_are_normalized_and_deduplicated() {
        let paths = vec![
            "Projects\\Alpha".to_string(),
            "Projects/Alpha.md".to_string(),
            "./Inbox/Waiting.md#Task".to_string(),
        ];

        let extraction = extract_source_paths(&paths, false)
            .expect("source paths should extract");

        assert_eq!(
            extraction.paths,
            vec!["Projects/Alpha.md", "Inbox/Waiting.md"]
        );
        assert!(extraction.warnings.is_empty(), "{extraction:?}");
    }

    #[test]
    fn dql_list_paths_use_list_pair_identity() {
        let result = json!({
            "type": "list",
            "primaryMeaning": { "type": "path" },
            "values": [
                {
                    "$widget": "dataview:list-pair",
                    "key": { "type": "link", "path": "Projects/Alpha.md", "display": null, "embed": false },
                    "value": "active"
                },
                {
                    "$widget": "dataview:list-pair",
                    "key": { "type": "link", "path": "Projects/Alpha.md", "display": null, "embed": false },
                    "value": "duplicate"
                },
                { "type": "link", "path": "Inbox/Waiting", "display": null, "embed": false }
            ]
        });

        let extraction = extract_dql_paths(&result, false)
            .expect("list paths should extract");

        assert_eq!(
            extraction.paths,
            vec!["Projects/Alpha.md", "Inbox/Waiting.md"]
        );
        assert!(extraction.warnings.is_empty(), "{extraction:?}");
    }

    #[test]
    fn dql_table_paths_use_first_identity_column() {
        let result = json!({
            "type": "table",
            "idMeaning": { "type": "path" },
            "headers": ["File", "Status"],
            "values": [
                [
                    { "type": "link", "path": "Areas\\Odd Name.md", "display": null, "embed": false },
                    "active"
                ],
                [
                    { "path": "Root Note" },
                    "waiting"
                ]
            ]
        });

        let extraction = extract_dql_paths(&result, false)
            .expect("table paths should extract");

        assert_eq!(extraction.paths, vec!["Areas/Odd Name.md", "Root Note.md"]);
        assert!(extraction.warnings.is_empty(), "{extraction:?}");
    }

    #[test]
    fn dql_task_paths_resolve_grouped_task_source_notes() {
        let result = json!({
            "type": "task",
            "values": [
                {
                    "key": "open",
                    "rows": [
                        { "path": "Tasks/Source.md", "text": "first" },
                        { "path": "Tasks/Source.md", "text": "duplicate" },
                        { "link": { "path": "Tasks/Other.md" }, "text": "fallback" }
                    ]
                }
            ]
        });

        let extraction = extract_dql_paths(&result, false)
            .expect("task paths should extract");

        assert_eq!(extraction.paths, vec!["Tasks/Source.md", "Tasks/Other.md"]);
        assert!(extraction.warnings.is_empty(), "{extraction:?}");
    }

    #[test]
    fn dql_grouped_table_rows_warn_and_fail_when_strict() {
        let result = json!({
            "type": "table",
            "idMeaning": {
                "type": "group",
                "name": "status",
                "on": { "type": "path" }
            },
            "values": [["active", 3], ["waiting", 1]]
        });

        let non_strict = extract_dql_paths(&result, false)
            .expect("grouped paths should be best effort");
        assert!(non_strict.paths.is_empty(), "{non_strict:?}");
        assert_eq!(non_strict.warnings.len(), 2, "{non_strict:?}");

        let strict = extract_dql_paths(&result, true)
            .expect_err("grouped identity should fail in strict mode");
        assert!(
            matches!(strict, DataviewError::StrictPaths { .. }),
            "{strict:?}"
        );
    }

    #[test]
    fn dql_missing_table_identities_warn_per_row() {
        let result = json!({
            "type": "table",
            "idMeaning": { "type": "path" },
            "values": [
                [],
                [{ "path": "Projects/Alpha.md" }, "active"],
                [{}]
            ]
        });

        let extraction = extract_dql_paths(&result, false)
            .expect("non-strict missing identities should warn");

        assert_eq!(extraction.paths, vec!["Projects/Alpha.md"]);
        assert_eq!(extraction.warnings.len(), 2, "{extraction:?}");
        assert!(
            extraction.warnings[0].contains("DQL table row 1")
                && extraction.warnings[1].contains("DQL table row 3"),
            "{extraction:?}"
        );
    }

    fn native_error_message(error: DataviewError) -> String {
        match error {
            DataviewError::NativeQuery { message } => message,
            other => panic!("expected native query error, got {other:?}"),
        }
    }
}
