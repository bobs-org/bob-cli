//! Engine output emission: JSON, markdown, paths, and warnings.

use super::*;
use serde_json::Value;

pub(super) fn emit_engine_output(
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

pub(super) fn emit_native_output(
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

pub(super) fn emit_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("{COMMAND_NAME}: warning: {warning}");
    }
}

pub(super) fn print_json(value: Value) -> Result<(), DataviewError> {
    let json = serde_json::to_string(&value)
        .map_err(DataviewError::SerializeOutput)?;
    println!("{json}");
    Ok(())
}
