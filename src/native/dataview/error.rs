//! Dataview error type, reporting, and output excerpts.

use super::super::env as bob_env;
use super::*;
use std::{ffi::OsString, io, path::PathBuf};

#[derive(Debug)]
pub(crate) enum DataviewError {
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
    TasksQuery {
        message: String,
    },
    TasksSettingsParse {
        path: PathBuf,
        error: serde_json::Error,
    },
    TasksSettingsRead {
        path: PathBuf,
        error: io::Error,
    },
}

impl DataviewError {
    pub(super) fn report(&self) {
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
            Self::TasksQuery { message } => {
                eprintln!("{COMMAND_NAME}: Tasks query failed");
                eprintln!("{message}");
            }
            Self::TasksSettingsParse { path, error } => {
                eprintln!(
                    "{COMMAND_NAME}: failed to parse Tasks settings {}: {error}",
                    path.display()
                );
            }
            Self::TasksSettingsRead { path, error } => {
                eprintln!(
                    "{COMMAND_NAME}: failed to read Tasks settings {}: {error}",
                    path.display()
                );
            }
        }
    }

    pub(super) fn exit_code(&self) -> i32 {
        match self {
            Self::ObsidianFailed { exit_code, .. }
            | Self::ObsidianNotRunning { exit_code, .. } => *exit_code,
            _ => 1,
        }
    }
}
pub(super) fn child_output_excerpt(stdout: &str, stderr: &str) -> String {
    let output = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    output_excerpt(output)
}

pub(super) fn stdout_excerpt(stdout: &str) -> String {
    output_excerpt(stdout.trim())
}

pub(super) fn output_excerpt(output: &str) -> String {
    let redacted = redact_generated_code(output);
    let mut excerpt = redacted.chars().take(600).collect::<String>();
    if redacted.chars().count() > 600 {
        excerpt.push_str("...");
    }
    excerpt
}

pub(super) fn redact_generated_code(output: &str) -> String {
    if let Some(position) = output.find("code=") {
        let mut redacted = output[..position + "code=".len()].to_string();
        redacted.push_str("<generated JavaScript>");
        return redacted;
    }
    output.to_string()
}
