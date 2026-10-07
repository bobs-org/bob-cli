//! Obsidian engine: eval script, CLI process, and protocol parsing.

use super::super::env as bob_env;
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::OsString,
    io,
    process::{Command, Output},
    thread,
    time::Duration,
};

pub(super) const OBSIDIAN_EVAL_SCRIPT: &str = r#"
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

pub(super) fn run_obsidian_eval(
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

pub(super) fn run_obsidian_process(
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

pub(super) fn obsidian_process(
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

pub(super) fn is_text_file_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(26)
}

pub(super) fn obsidian_command() -> OsString {
    bob_env::var_os(ENV_OBSIDIAN_COMMAND)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("obsidian"))
}

pub(super) fn obsidian_failure(output: Output) -> DataviewError {
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

pub(super) fn build_obsidian_javascript(
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

pub(super) fn parse_protocol_stdout(
    stdout: &[u8],
) -> Result<EngineOutput, DataviewError> {
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

pub(super) fn parse_protocol_payload(
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

#[derive(Debug, Serialize)]
pub(super) struct ObsidianEvalRequest {
    pub(super) format: &'static str,
    pub(super) origin: Option<String>,
    pub(super) query: ObsidianEvalQuery,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ObsidianEvalQuery {
    Source { source: String },
    Dql { query: String },
}

#[derive(Debug)]
pub(super) struct EngineOutput {
    pub(super) response: EngineResponse,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug)]
pub(super) enum EngineResponse {
    SourcePaths(Vec<String>),
    DqlJson(Value),
    Markdown(String),
}

#[derive(Debug, Deserialize)]
pub(super) struct ProtocolEnvelope {
    pub(super) status: String,
    #[serde(default)]
    pub(super) kind: Option<String>,
    #[serde(default)]
    pub(super) paths: Option<Vec<String>>,
    #[serde(default)]
    pub(super) result: Option<Value>,
    #[serde(default)]
    pub(super) markdown: Option<String>,
    #[serde(default)]
    pub(super) warnings: Vec<String>,
    #[serde(default)]
    pub(super) code: Option<String>,
    #[serde(default)]
    pub(super) message: Option<String>,
}

impl ProtocolEnvelope {
    pub(super) fn into_engine_output(
        self,
    ) -> Result<EngineOutput, DataviewError> {
        match self.status.as_str() {
            "ok" => self.ok_output(),
            "error" => Err(protocol_error(self.code, self.message)),
            other => Err(DataviewError::MalformedProtocolResponse {
                reason: format!("unexpected protocol status: {other}"),
            }),
        }
    }

    pub(super) fn ok_output(self) -> Result<EngineOutput, DataviewError> {
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

pub(super) fn protocol_error(
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
impl Request {
    pub(super) fn obsidian_eval_request(
        &self,
    ) -> Result<ObsidianEvalRequest, DataviewError> {
        let query = match &self.query {
            QueryInput::Source(source) => ObsidianEvalQuery::Source {
                source: source.clone(),
            },
            QueryInput::Dql(input) => ObsidianEvalQuery::Dql {
                query: input.read_query()?,
            },
            QueryInput::Tasks(_) | QueryInput::TasksNote(_) => {
                return Err(DataviewError::TasksQuery {
                    message:
                        "the Obsidian engine does not support Tasks queries"
                            .to_string(),
                });
            }
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
