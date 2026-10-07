//! Pre-scan hook configuration and execution.
use super::*;
use crate::native::env as bob_env;

pub(super) fn configured_pre_scan_hook(
    no_hooks: bool,
) -> Result<Option<PreScanHook>> {
    if no_hooks {
        return Ok(None);
    }
    if bob_env::var_os(ENV_LEGACY_PRE_SCAN_COMMAND).is_some() {
        return Err(CommandError::new(format!(
            "{ENV_LEGACY_PRE_SCAN_COMMAND} was renamed; use {ENV_PRE_SCAN_HOOK}"
        )));
    }
    if let Some(command) = bob_env::var_os(ENV_PRE_SCAN_HOOK) {
        return Ok(pre_scan_hook_from_os(command));
    }

    let config = bob_config::load_highlights_config(&bob_config::config_path())
        .map_err(config_error)?;
    Ok(config.pre_scan_hook().map(|command| PreScanHook {
        command: OsString::from(command),
    }))
}

pub(super) fn pre_scan_hook_from_os(command: OsString) -> Option<PreScanHook> {
    (!command.to_string_lossy().trim().is_empty())
        .then_some(PreScanHook { command })
}

pub(super) fn config_error(error: bob_config::ConfigError) -> CommandError {
    match error {
        bob_config::ConfigError::Read(message)
        | bob_config::ConfigError::Invalid(message) => {
            CommandError::new(message)
        }
    }
}

pub(super) fn run_pre_scan_hook(
    config: &Config,
    command: Option<&PreScanHook>,
    dry_run: bool,
) -> Result<()> {
    let Some(command) = command else {
        return Ok(());
    };

    if dry_run {
        println!("pre_scan_hook: would-run {}", command.display());
        return Ok(());
    }

    println!("pre_scan_hook: run {}", command.display());
    let status = process::Command::new("sh")
        .arg("-c")
        .arg(&command.command)
        .current_dir(&config.bob_dir)
        .env("BOB_HIGHLIGHTS_IN_PRE_SCAN_HOOK", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            CommandError::new(format!(
                "run pre-scan hook {}: {error}",
                command.display()
            ))
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "pre-scan hook failed with {}: {}",
            exit_status_label(&status),
            command.display()
        )))
    }
}

pub(super) fn exit_status_label(status: &process::ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit {code}"))
        .unwrap_or_else(|| status.to_string())
}

pub(super) fn check_pre_scan_hook(
    command: Option<&PreScanHook>,
    failures: &mut Vec<String>,
) {
    let Some(command) = command else {
        println!("pre_scan_hook: none");
        return;
    };

    let Some(program) = pre_scan_program(command) else {
        println!(
            "pre_scan_hook: fail ({}; executable not found)",
            command.display()
        );
        failures.push(format!(
            "pre-scan hook has no executable word: {}",
            command.display()
        ));
        return;
    };

    match shell_command_available(&program) {
        Ok(true) => {
            println!(
                "pre_scan_hook: ok ({}; executable: {})",
                command.display(),
                program.to_string_lossy()
            );
        }
        Ok(false) => {
            println!(
                "pre_scan_hook: fail ({}; executable not found: {})",
                command.display(),
                program.to_string_lossy()
            );
            failures.push(format!(
                "pre-scan hook executable not found: {}",
                program.to_string_lossy()
            ));
        }
        Err(error) => {
            println!(
                "pre_scan_hook: fail ({}; executable check failed: {error})",
                command.display()
            );
            failures.push(format!(
                "pre-scan hook executable check failed: {error}"
            ));
        }
    }
}

pub(super) fn pre_scan_program(command: &PreScanHook) -> Option<OsString> {
    command
        .display()
        .split_whitespace()
        .filter(|word| !looks_like_env_assignment(word))
        .map(|word| word.trim_matches(['\'', '"']))
        .find(|word| !word.is_empty())
        .map(OsString::from)
}

pub(super) fn looks_like_env_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_'
        })
}

pub(super) fn shell_command_available(
    program: &OsStr,
) -> std::io::Result<bool> {
    process::Command::new("sh")
        .arg("-c")
        .arg("command -v \"$1\" >/dev/null 2>&1")
        .arg("sh")
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
}
