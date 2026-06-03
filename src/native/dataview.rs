use std::{
    collections::{HashMap, HashSet},
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

use clap::{
    builder::{NonEmptyStringValueParser, OsStringValueParser},
    error::ErrorKind,
    Arg, ArgAction, ArgGroup, ArgMatches, Command as ClapCommand,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::env as bob_env;

const COMMAND_NAME: &str = "bob dataview";
const ENV_DYNOMARK_COMMAND: &str = "BOB_DATAVIEW_DYNOMARK_COMMAND";
const ENV_OBSIDIAN_COMMAND: &str = "BOB_DATAVIEW_OBSIDIAN_COMMAND";
const ENV_VAULT: &str = "BOB_DATAVIEW_VAULT";
const RESULT_PREFIX: &str = "BOB_DATAVIEW_RESULT\t";
const DYNOMARK_COMPAT_WARNING: &str = "dynomark is a partial \
Dataview-compatible headless engine; validate results before relying on them \
for automation";
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
    Dynomark,
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
        Engine::Dynomark => run_dynomark(request),
    }
}

fn run_obsidian(request: &Request) -> Result<(), DataviewError> {
    let eval_request = request.obsidian_eval_request()?;

    let javascript = build_obsidian_javascript(&eval_request)?;
    let output = run_obsidian_eval(&request.vault, &javascript)?;
    let engine_output = parse_protocol_stdout(&output.stdout)?;
    emit_engine_output(request, engine_output)
}

fn run_dynomark(request: &Request) -> Result<(), DataviewError> {
    let query = match &request.query {
        QueryInput::Dql(input) => input.read_query()?,
        QueryInput::Source(_) => unreachable!(
            "dynomark source expressions are rejected during argument parsing"
        ),
    };

    let output = run_dynomark_query(&request.vault.bob_dir, &query)?;
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }

    let dynomark_output =
        parse_dynomark_stdout(&output.stdout, &request.vault.bob_dir);
    emit_dynomark_output(request, dynomark_output)
}

fn run_native(request: &Request) -> Result<(), DataviewError> {
    let query = match &request.query {
        QueryInput::Dql(input) => input.read_query()?,
        QueryInput::Source(_) => unreachable!(
            "native source expressions are rejected during argument parsing"
        ),
    };

    let query = NativeQuery::parse(&query)?;
    let vault = NativeVault::read(&request.vault.bob_dir)?;
    let output = vault.evaluate(&query);
    emit_native_output(request, output)
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

fn run_dynomark_query(
    bob_dir: &Path,
    query: &str,
) -> Result<Output, DataviewError> {
    let command = dynomark_command();
    let output = Command::new(&command)
        .arg("--query")
        .arg(query)
        .arg("--metadata")
        .current_dir(bob_dir)
        .output()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                DataviewError::MissingDynomarkCommand {
                    command: command.clone(),
                }
            } else {
                DataviewError::RunDynomark {
                    command: command.clone(),
                    error,
                }
            }
        })?;

    if output.status.success() {
        Ok(output)
    } else {
        Err(dynomark_failure(output))
    }
}

fn dynomark_command() -> OsString {
    env::var_os(ENV_DYNOMARK_COMMAND)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("dynomark"))
}

fn dynomark_failure(output: Output) -> DataviewError {
    let exit_code = bob_env::exit_code(output.status);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    DataviewError::DynomarkFailed {
        exit_code,
        output: child_output_excerpt(&stdout, &stderr),
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

fn emit_dynomark_output(
    request: &Request,
    output: DynomarkOutput,
) -> Result<(), DataviewError> {
    let mut warnings = dynomark_warnings(request);
    let mut path_warnings = output.warnings;
    let extraction =
        extract_dynomark_paths(&output.metadata, &request.vault.bob_dir)?;
    path_warnings.extend(extraction.warnings);
    if request.strict_paths && !path_warnings.is_empty() {
        return Err(DataviewError::StrictPaths {
            warnings: path_warnings,
        });
    }
    warnings.extend(path_warnings);

    match request.format {
        OutputFormat::Paths => {
            emit_warnings(&warnings);
            if !extraction.paths.is_empty() {
                println!("{}", extraction.paths.join("\n"));
            }
            Ok(())
        }
        OutputFormat::Json => {
            emit_warnings(&warnings);
            print_json(serde_json::json!({
                "engine": request.engine.as_str(),
                "query_kind": "dql",
                "format": request.format.as_str(),
                "paths": extraction.paths,
                "result": {
                    "metadata": output.metadata,
                    "markdown": output.rendered,
                },
                "warnings": warnings,
            }))
        }
        OutputFormat::Markdown => unreachable!(
            "dynomark markdown output is rejected during argument parsing"
        ),
    }
}

fn emit_native_output(
    request: &Request,
    output: NativeOutput,
) -> Result<(), DataviewError> {
    match request.format {
        OutputFormat::Paths => {
            if !output.paths.is_empty() {
                println!("{}", output.paths.join("\n"));
            }
            Ok(())
        }
        OutputFormat::Json => {
            let values = output
                .paths
                .iter()
                .map(|path| {
                    serde_json::json!({
                        "type": "link",
                        "path": path,
                        "display": null,
                        "embed": false,
                    })
                })
                .collect::<Vec<_>>();
            print_json(serde_json::json!({
                "engine": request.engine.as_str(),
                "query_kind": "dql",
                "format": request.format.as_str(),
                "paths": output.paths,
                "result": {
                    "type": "list",
                    "values": values,
                },
                "warnings": [],
            }))
        }
        OutputFormat::Markdown => unreachable!(
            "native markdown output is rejected during argument parsing"
        ),
    }
}

fn dynomark_warnings(request: &Request) -> Vec<String> {
    let mut warnings = vec![DYNOMARK_COMPAT_WARNING.to_string()];
    if request.vault.origin.is_some() {
        warnings.push(
            "dynomark does not support Obsidian origin context; ignoring \
             --origin"
                .to_string(),
        );
    }
    warnings
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
struct DynomarkOutput {
    metadata: Vec<Value>,
    rendered: String,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct NativeOutput {
    paths: Vec<String>,
}

#[derive(Debug)]
struct NativeQuery {
    source: Option<NativeSource>,
    filter: Option<NativeExpr>,
}

#[derive(Debug)]
struct NativeSource {
    folder: String,
}

#[derive(Debug)]
enum NativeExpr {
    And(Box<NativeExpr>, Box<NativeExpr>),
    Bool(bool),
    Eq(Vec<String>, NativeValue),
    Field(Vec<String>),
    Or(Box<NativeExpr>, Box<NativeExpr>),
}

#[derive(Debug)]
enum NativeValue {
    Bool(bool),
    Link(String),
    String(String),
}

#[derive(Debug)]
struct NativeVault {
    pages: Vec<NativePage>,
    by_path: HashMap<String, usize>,
    by_stem: HashMap<String, Vec<usize>>,
}

#[derive(Debug)]
struct NativePage {
    path: String,
    fields: HashMap<String, NativeFieldValue>,
}

#[derive(Debug)]
enum NativeFieldValue {
    Bool(bool),
    Null,
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeToken {
    And,
    Bool(bool),
    Dot,
    Equal,
    Eof,
    From,
    Identifier(String),
    Link(String),
    List,
    LParen,
    Or,
    RParen,
    String(String),
    Where,
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

impl NativeVault {
    fn read(bob_dir: &Path) -> Result<Self, DataviewError> {
        let mut paths = Vec::new();
        collect_native_markdown_paths(bob_dir, &mut paths)?;
        paths.sort();

        let mut pages = Vec::new();
        for path in paths {
            let contents = fs::read_to_string(&path).map_err(|error| {
                DataviewError::NativeVaultRead {
                    path: path.clone(),
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
            pages.push(NativePage {
                path: relative_path,
                fields: parse_native_frontmatter(&contents),
            });
        }

        let mut by_path = HashMap::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, page) in pages.iter().enumerate() {
            by_path.insert(page.path.clone(), index);
            if let Some(stem) = note_stem(&page.path) {
                by_stem.entry(stem).or_default().push(index);
            }
        }

        Ok(Self {
            pages,
            by_path,
            by_stem,
        })
    }

    fn evaluate(&self, query: &NativeQuery) -> NativeOutput {
        let paths = self
            .pages
            .iter()
            .enumerate()
            .filter(|(_, page)| query.matches_source(page))
            .filter(|(index, _)| query.matches_filter(self, *index))
            .map(|(_, page)| page.path.clone())
            .collect();

        NativeOutput { paths }
    }

    fn field_chain_value(
        &self,
        page_index: usize,
        chain: &[String],
    ) -> Option<&NativeFieldValue> {
        let mut page_index = page_index;
        for (position, field) in chain.iter().enumerate() {
            let value = self.pages.get(page_index)?.fields.get(field)?;
            if position + 1 == chain.len() {
                return Some(value);
            }
            page_index = self.resolve_field_link(value)?;
        }

        None
    }

    fn resolve_field_link(&self, value: &NativeFieldValue) -> Option<usize> {
        match value {
            NativeFieldValue::String(value) => self.resolve_link(value),
            NativeFieldValue::Bool(_) | NativeFieldValue::Null => None,
        }
    }

    fn resolve_link(&self, raw: &str) -> Option<usize> {
        let target = native_link_target(raw)?;
        let path = normalize_note_path(&target).ok()?;
        if let Some(index) = self.by_path.get(&path) {
            return Some(*index);
        }

        if target.contains('/') || target.contains('\\') {
            return None;
        }

        let stem = path.strip_suffix(".md").unwrap_or(&path);
        match self.by_stem.get(stem).map(Vec::as_slice) {
            Some([index]) => Some(*index),
            _ => None,
        }
    }

    fn field_value_matches_link(
        &self,
        value: &NativeFieldValue,
        expected: &str,
    ) -> bool {
        let NativeFieldValue::String(actual) = value else {
            return false;
        };

        match (self.resolve_link(actual), self.resolve_link(expected)) {
            (Some(actual), Some(expected)) => actual == expected,
            _ => comparable_link_path(actual) == comparable_link_path(expected),
        }
    }
}

impl NativeQuery {
    fn matches_source(&self, page: &NativePage) -> bool {
        let Some(source) = &self.source else {
            return true;
        };
        let prefix = format!("{}/", source.folder);
        page.path.starts_with(&prefix)
    }

    fn matches_filter(&self, vault: &NativeVault, page_index: usize) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|expr| expr.evaluate(vault, page_index))
    }
}

impl NativeExpr {
    fn evaluate(&self, vault: &NativeVault, page_index: usize) -> bool {
        match self {
            Self::And(left, right) => {
                left.evaluate(vault, page_index)
                    && right.evaluate(vault, page_index)
            }
            Self::Bool(value) => *value,
            Self::Eq(chain, value) => {
                let Some(actual) = vault.field_chain_value(page_index, chain)
                else {
                    return false;
                };
                value.matches(vault, actual)
            }
            Self::Field(chain) => vault
                .field_chain_value(page_index, chain)
                .is_some_and(NativeFieldValue::is_truthy),
            Self::Or(left, right) => {
                left.evaluate(vault, page_index)
                    || right.evaluate(vault, page_index)
            }
        }
    }
}

impl NativeValue {
    fn matches(&self, vault: &NativeVault, actual: &NativeFieldValue) -> bool {
        match self {
            Self::Bool(expected) => actual.as_bool() == Some(*expected),
            Self::Link(expected) => {
                vault.field_value_matches_link(actual, expected)
            }
            Self::String(expected) => {
                actual.as_str() == Some(expected.as_str())
            }
        }
    }
}

impl NativeFieldValue {
    fn is_truthy(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Null => false,
            Self::String(value) => !value.is_empty(),
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Null | Self::String(_) => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Bool(_) | Self::Null => None,
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
                '.' => tokens.push(NativeToken::Dot),
                '=' => tokens.push(NativeToken::Equal),
                '"' => tokens
                    .push(NativeToken::String(self.read_quoted_string('"')?)),
                '\'' => tokens
                    .push(NativeToken::String(self.read_quoted_string('\'')?)),
                '[' if self.chars.peek() == Some(&'[') => {
                    self.chars.next();
                    tokens.push(NativeToken::Link(self.read_wikilink()?));
                }
                ch if is_native_identifier_start(ch) => {
                    let identifier = self.read_identifier(ch);
                    tokens.push(native_identifier_token(identifier));
                }
                other => {
                    return Err(format!(
                        "unsupported token {other:?}; native engine supports \
                         LIST, FROM, WHERE, AND, OR, parentheses, field \
                         names, strings, booleans, and wikilinks"
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
        self.expect_list()?;
        let source = if self.take_from() {
            Some(NativeSource {
                folder: normalize_native_source_folder(
                    &self.expect_string_source()?,
                )?,
            })
        } else {
            None
        };
        let filter = if self.take_where() {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect_eof()?;
        Ok(NativeQuery { source, filter })
    }

    fn parse_expr(&mut self) -> Result<NativeExpr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_and()?;
        while self.take_or() {
            let right = self.parse_and()?;
            expr = NativeExpr::Or(Box::new(expr), Box::new(right));
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_primary()?;
        while self.take_and() {
            let right = self.parse_primary()?;
            expr = NativeExpr::And(Box::new(expr), Box::new(right));
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<NativeExpr, String> {
        match self.peek() {
            NativeToken::Bool(value) => {
                let value = *value;
                self.position += 1;
                Ok(NativeExpr::Bool(value))
            }
            NativeToken::Identifier(_) => {
                let chain = self.parse_field_chain()?;
                if self.take_equal() {
                    Ok(NativeExpr::Eq(chain, self.parse_value()?))
                } else {
                    Ok(NativeExpr::Field(chain))
                }
            }
            NativeToken::LParen => {
                self.position += 1;
                let expr = self.parse_expr()?;
                self.expect_rparen()?;
                Ok(expr)
            }
            token => Err(format!(
                "expected expression, found {}; native engine supports field \
                 truthiness, field = [[link]], AND, OR, booleans, and \
                 parentheses",
                native_token_name(token)
            )),
        }
    }

    fn parse_field_chain(&mut self) -> Result<Vec<String>, String> {
        let mut chain = vec![self.expect_identifier()?];
        while self.take_dot() {
            chain.push(self.expect_identifier()?);
        }
        Ok(chain)
    }

    fn parse_value(&mut self) -> Result<NativeValue, String> {
        match self.peek() {
            NativeToken::Bool(value) => {
                let value = *value;
                self.position += 1;
                Ok(NativeValue::Bool(value))
            }
            NativeToken::Link(value) => {
                let value = value.clone();
                self.position += 1;
                Ok(NativeValue::Link(value))
            }
            NativeToken::String(value) => {
                let value = value.clone();
                self.position += 1;
                Ok(NativeValue::String(value))
            }
            token => Err(format!(
                "expected comparison value, found {}; native engine supports \
                 booleans, strings, and wikilinks as comparison values",
                native_token_name(token)
            )),
        }
    }

    fn expect_list(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::List) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "native engine supports LIST queries only; found {}",
            native_token_name(self.peek())
        ))
    }

    fn take_from(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::From))
    }

    fn take_where(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Where))
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

    fn take_equal(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Equal))
    }

    fn take(&mut self, predicate: impl FnOnce(&NativeToken) -> bool) -> bool {
        if predicate(self.peek()) {
            self.position += 1;
            true
        } else {
            false
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

    fn expect_string_source(&mut self) -> Result<String, String> {
        let NativeToken::String(source) = self.peek() else {
            return Err(format!(
                "native engine supports quoted folder sources only, such as \
                 FROM \"ref\"; found {}",
                native_token_name(self.peek())
            ));
        };
        let source = source.clone();
        self.position += 1;
        Ok(source)
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

    fn expect_eof(&self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::Eof) {
            return Ok(());
        }
        Err(format!(
            "unexpected {} after native query; native engine supports LIST \
             FROM \"folder\" WHERE <expression>",
            native_token_name(self.peek())
        ))
    }

    fn peek(&self) -> &NativeToken {
        self.tokens
            .get(self.position)
            .unwrap_or_else(|| self.tokens.last().expect("lexer adds EOF"))
    }
}

fn native_query_error(message: String) -> DataviewError {
    DataviewError::NativeQuery { message }
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

fn parse_native_frontmatter(
    contents: &str,
) -> HashMap<String, NativeFieldValue> {
    let Some(frontmatter) = native_frontmatter_block(contents) else {
        return HashMap::new();
    };

    let mut fields = HashMap::new();
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with('-')
        {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        fields.insert(
            key.to_string(),
            parse_native_frontmatter_scalar(value.trim()),
        );
    }
    fields
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

fn parse_native_frontmatter_scalar(raw: &str) -> NativeFieldValue {
    let value = raw.trim();
    if value.eq_ignore_ascii_case("null") || value == "~" {
        return NativeFieldValue::Null;
    }
    if value.eq_ignore_ascii_case("true") {
        return NativeFieldValue::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return NativeFieldValue::Bool(false);
    }

    NativeFieldValue::String(unquote_native_scalar(value))
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

fn native_identifier_token(identifier: String) -> NativeToken {
    match identifier.to_ascii_lowercase().as_str() {
        "and" => NativeToken::And,
        "false" => NativeToken::Bool(false),
        "from" => NativeToken::From,
        "list" => NativeToken::List,
        "or" => NativeToken::Or,
        "true" => NativeToken::Bool(true),
        "where" => NativeToken::Where,
        _ => NativeToken::Identifier(identifier),
    }
}

fn native_token_name(token: &NativeToken) -> &'static str {
    match token {
        NativeToken::And => "AND",
        NativeToken::Bool(_) => "boolean",
        NativeToken::Dot => "'.'",
        NativeToken::Equal => "'='",
        NativeToken::Eof => "end of query",
        NativeToken::From => "FROM",
        NativeToken::Identifier(_) => "field name",
        NativeToken::Link(_) => "wikilink",
        NativeToken::List => "LIST",
        NativeToken::LParen => "'('",
        NativeToken::Or => "OR",
        NativeToken::RParen => "')'",
        NativeToken::String(_) => "string",
        NativeToken::Where => "WHERE",
    }
}

fn parse_dynomark_stdout(stdout: &[u8], bob_dir: &Path) -> DynomarkOutput {
    let stdout = String::from_utf8_lossy(stdout);
    let (metadata, rendered) = split_dynomark_metadata(&stdout);
    let mut warnings = Vec::new();
    if metadata.is_empty() {
        warnings.push(
            "dynomark did not emit file metadata; paths output may be empty"
                .to_string(),
        );
    }
    if !bob_dir.is_absolute() {
        warnings.push(format!(
            "dynomark was run from non-absolute vault path {}; absolute \
             metadata may not be relativized",
            bob_dir.display()
        ));
    }

    DynomarkOutput {
        metadata,
        rendered,
        warnings,
    }
}

fn split_dynomark_metadata(stdout: &str) -> (Vec<Value>, String) {
    let mut stream = serde_json::Deserializer::from_str(stdout).into_iter();
    let mut metadata = Vec::new();
    let mut offset = 0;

    loop {
        let next: Option<Result<Value, _>> = stream.next();
        let Some(value) = next else {
            offset = stream.byte_offset();
            break;
        };

        match value {
            Ok(value) if dynomark_metadata_has_path(&value) => {
                metadata.push(value);
                offset = stream.byte_offset();
            }
            Ok(_) | Err(_) => break,
        }
    }

    let rendered = stdout[offset..].trim_start().to_string();
    (metadata, rendered)
}

fn dynomark_metadata_has_path(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("file.path"))
        .and_then(Value::as_str)
        .is_some()
}

fn extract_dynomark_paths(
    metadata: &[Value],
    bob_dir: &Path,
) -> Result<PathExtraction, DataviewError> {
    let mut collector = PathCollector::default();
    for (index, item) in metadata.iter().enumerate() {
        let context = format!("dynomark metadata row {}", index + 1);
        let Some(raw_path) = item.get("file.path").and_then(Value::as_str)
        else {
            collector.warn(format!("{context} has no file.path"));
            continue;
        };

        match dynomark_vault_relative_path(raw_path, bob_dir) {
            Ok(path) => {
                collector.add_raw_path(&path, &context);
            }
            Err(reason) => collector.warn(format!("{context}: {reason}")),
        }
    }

    collector.finish(false)
}

fn dynomark_vault_relative_path(
    raw_path: &str,
    bob_dir: &Path,
) -> Result<String, String> {
    let path = Path::new(raw_path);
    if !path.is_absolute() {
        return Ok(raw_path.to_string());
    }

    let relative = path.strip_prefix(bob_dir).map_err(|_| {
        format!(
            "absolute path {raw_path:?} is outside Bob vault {}",
            bob_dir.display()
        )
    })?;

    if relative.as_os_str().is_empty() {
        return Err(format!("absolute path {raw_path:?} names the vault root"));
    }

    Ok(relative.to_string_lossy().into_owned())
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
    DynomarkFailed {
        exit_code: i32,
        output: String,
    },
    MalformedProtocolResponse {
        reason: String,
    },
    MissingDynomarkCommand {
        command: OsString,
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
    RunDynomark {
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
            Self::DynomarkFailed { exit_code, output } => {
                eprintln!(
                    "{COMMAND_NAME}: dynomark query failed with exit code \
                     {exit_code}"
                );
                if !output.is_empty() {
                    eprintln!("dynomark output excerpt: {output}");
                }
            }
            Self::MalformedProtocolResponse { reason } => {
                eprintln!(
                    "{COMMAND_NAME}: malformed Obsidian protocol response"
                );
                eprintln!("{reason}");
            }
            Self::MissingDynomarkCommand { command } => {
                eprintln!(
                    "{COMMAND_NAME}: dynomark command not found: {}",
                    bob_env::os_to_string(command)
                );
                eprintln!(
                    "Install dynomark or set {ENV_DYNOMARK_COMMAND} to an \
                     executable path."
                );
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
            Self::RunDynomark { command, error } => {
                eprintln!(
                    "{COMMAND_NAME}: failed to run dynomark command {}: \
                     {error}",
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
            Self::DynomarkFailed { exit_code, .. }
            | Self::ObsidianFailed { exit_code, .. }
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
        .about("Run Dataview queries against the Bob Obsidian vault")
        .long_about(
            "Run Dataview source expressions or DQL queries against the Bob \
Obsidian vault.\n\n\
Source expressions return matching page paths. DQL queries support path, JSON, \
and markdown output modes. The default Obsidian engine is the exact Dataview \
runtime. The explicit dynomark engine is a partial headless fallback for DQL \
paths and JSON output. The native engine is a headless local frontmatter \
subset for LIST queries.",
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
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn engine_arg() -> Arg {
    Arg::new("engine")
        .long("engine")
        .value_name("ENGINE")
        .default_value("obsidian")
        .value_parser(["dynomark", "native", "obsidian"])
        .help("Query engine: obsidian for exact Dataview, dynomark for partial headless DQL, native for local frontmatter DQL")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .value_name("FORMAT")
        .default_value("paths")
        .value_parser(["json", "markdown", "paths"])
        .help("Output format; markdown is available only for DQL")
}

fn origin_arg() -> Arg {
    Arg::new("origin")
        .long("origin")
        .value_name("VAULT_RELATIVE_PATH")
        .value_parser(OsStringValueParser::new())
        .help("Origin note for relative links and this")
}

fn query_arg() -> Arg {
    Arg::new("query")
        .long("query")
        .value_name("DQL")
        .value_parser(NonEmptyStringValueParser::new())
        .help("Full Dataview DQL query")
}

fn query_file_arg() -> Arg {
    Arg::new("query-file")
        .long("query-file")
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Read a Dataview DQL query from a file; use - for stdin")
}

fn source_arg() -> Arg {
    Arg::new("source")
        .long("source")
        .value_name("SOURCE")
        .value_parser(NonEmptyStringValueParser::new())
        .help("Dataview source expression for page path lookup")
}

fn strict_paths_arg() -> Arg {
    Arg::new("strict-paths")
        .long("strict-paths")
        .action(ArgAction::SetTrue)
        .help("Fail when paths output cannot derive clean note paths")
}

fn vault_arg() -> Arg {
    Arg::new("vault")
        .long("vault")
        .value_name("NAME_OR_ID")
        .value_parser(NonEmptyStringValueParser::new())
        .help("Obsidian vault name or ID; defaults to BOB_DATAVIEW_VAULT")
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

        if engine == Engine::Dynomark && query.is_source() {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--engine dynomark supports DQL queries only; use --query or \
                 --query-file",
            ));
        }

        if engine == Engine::Native && query.is_source() {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--engine native supports DQL queries only; use --query or \
                 --query-file",
            ));
        }

        if matches!(engine, Engine::Dynomark | Engine::Native)
            && format == OutputFormat::Markdown
        {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "--format markdown requires the Obsidian engine for \
                 Dataview-rendered Markdown",
            ));
        }

        Ok(Self {
            query,
            format,
            engine,
            vault: VaultConfig::from_matches(
                matches,
                command,
                matches!(engine, Engine::Dynomark | Engine::Native),
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
            "dynomark" => Self::Dynomark,
            "native" => Self::Native,
            "obsidian" => Self::Obsidian,
            value => unreachable!("unexpected engine value from clap: {value}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Dynomark => "dynomark",
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
        let obsidian_vault = matches
            .get_one::<String>("vault")
            .cloned()
            .or_else(default_vault_from_env);

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
}
