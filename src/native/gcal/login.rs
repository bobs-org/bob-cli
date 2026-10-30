//! `bob gcal login`: one-time read-only Calendar consent through gog.

use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::Path,
};

use serde_json::json;

use super::super::style::Styler;
use super::{
    client::{parse_client_json, ClientOrigin},
    config::{assume_tty, GcalConfig},
    gog::{self, Gog, CLIENT},
    ui, GcalError, LoginArgs,
};
use crate::native::env as bob_env;

pub(crate) fn run(args: &LoginArgs) -> i32 {
    let format = args.format.as_str();
    let config = match GcalConfig::resolve() {
        Ok(config) => config,
        Err(error) => return ui::report_error("login", &error, format),
    };
    match login(args, &config) {
        Ok(report) => {
            print_report(&report, args.format.is_json());
            0
        }
        Err(error) => ui::report_error("login", &error, format),
    }
}

struct LoginReport {
    already_connected: bool,
    account: String,
    client: String,
    calendars: usize,
    syncing: usize,
    syncing_ids: Vec<String>,
}

fn login(
    args: &LoginArgs,
    config: &GcalConfig,
) -> Result<LoginReport, GcalError> {
    let gog = Gog::resolve(config.timeout())?;
    let _version = gog.require_version()?;
    let client_name = ensure_client(&gog, args.credentials.as_deref())?;
    let account = resolve_account(&gog, args.account.as_deref(), config)?;
    let already = is_already_connected(&gog, &account)?;
    if !already {
        if !args.manual && bob_env_has("SSH_CONNECTION") {
            eprintln!(
                "hint: you are on SSH; rerun with `bob gcal login -m` if the \
                 browser cannot open"
            );
        }
        gog.auth_add(&account, args.manual)?;
    }
    let calendars = gog.calendars(&account)?;
    let syncing_ids: Vec<String> = calendars
        .iter()
        .filter(|calendar| config.syncs(&calendar.id, calendar.primary))
        .map(|calendar| {
            if calendar.primary {
                "primary".to_string()
            } else {
                calendar.id.clone()
            }
        })
        .collect();
    Ok(LoginReport {
        already_connected: already,
        account,
        client: client_name,
        calendars: calendars.len(),
        syncing: syncing_ids.len(),
        syncing_ids,
    })
}

fn ensure_client(
    gog: &Gog,
    credentials: Option<&Path>,
) -> Result<String, GcalError> {
    let clients = gog.credentials_list()?;
    // `-c FILE` replaces any stored `bob-gcal` client.
    if let Some(path) = credentials {
        let expanded = bob_env::expand_tilde(path);
        let display = expanded.display().to_string();
        let bytes = std::fs::read(&expanded).map_err(|error| {
            GcalError::setup(
                "client_invalid",
                format!("{}: cannot read client JSON: {error}", display),
            )
            .with_hint(
                "download a Desktop app client JSON and pass its path with \
                 `bob gcal login -c PATH`",
            )
        })?;
        let client = parse_client_json(&bytes, ClientOrigin::File)
            .map_err(|problem| problem.into_error(&display))?;
        gog.credentials_set_json(&client.to_gog_json())?;
        return Ok(CLIENT.to_string());
    }
    if clients.iter().any(|client| client.client == CLIENT) {
        return Ok(CLIENT.to_string());
    }
    // Reuse another gog client without a prompt: Google's consent screen
    // is the real confirmation. Keyring-held clients cannot be copied.
    let mut skipped_keyring: Vec<String> = Vec::new();
    for client in &clients {
        if client.client == CLIENT {
            continue;
        }
        if client.client_secret_in_keyring {
            skipped_keyring.push(client.client.clone());
        }
    }
    let mut ordered: Vec<_> = clients
        .iter()
        .filter(|client| {
            client.client != CLIENT && !client.client_secret_in_keyring
        })
        .collect();
    ordered.sort_by_key(|client| !client.default);
    for other in ordered {
        let Some(path) = other.path.as_deref() else {
            continue;
        };
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(client) = parse_client_json(&bytes, ClientOrigin::GogClient)
        else {
            continue;
        };
        gog.credentials_set_json(&client.to_gog_json())?;
        return Ok(CLIENT.to_string());
    }
    Err(checklist_error(&skipped_keyring))
}

fn resolve_account(
    gog: &Gog,
    override_email: Option<&str>,
    config: &GcalConfig,
) -> Result<String, GcalError> {
    if let Some(email) = override_email.filter(|email| !email.is_empty()) {
        return require_email(email);
    }
    if let Some(email) = config.account() {
        return require_email(email);
    }
    let accounts = gog.accounts(None)?;
    let emails: Vec<String> = unique_emails(&accounts);
    match emails.as_slice() {
        [email] => Ok(email.clone()),
        [] => {
            if is_tty() {
                prompt_email()
            } else {
                Err(GcalError::setup(
                    "not_logged_in",
                    "no Google account is configured".to_string(),
                )
                .with_hint(
                    "pass -a EMAIL, set gcal.account, or run `bob gcal login` \
                     on a TTY",
                ))
            }
        }
        _ => {
            if is_tty() {
                prompt_email()
            } else {
                Err(GcalError::setup(
                    "not_logged_in",
                    "gog holds more than one account; pass -a EMAIL"
                        .to_string(),
                )
                .with_hint(
                    "set gcal.account or pass `bob gcal login -a EMAIL`",
                ))
            }
        }
    }
}

fn unique_emails(accounts: &[gog::GogAccount]) -> Vec<String> {
    let mut emails = Vec::new();
    for account in accounts {
        let email = account.email.trim();
        if email.is_empty() {
            continue;
        }
        if !emails.iter().any(|seen: &String| seen == email) {
            emails.push(email.to_string());
        }
    }
    emails
}

fn require_email(email: &str) -> Result<String, GcalError> {
    if email.contains('@') {
        Ok(email.to_string())
    } else {
        Err(GcalError::setup(
            "config",
            format!("account {email:?} must contain '@'"),
        ))
    }
}

fn prompt_email() -> Result<String, GcalError> {
    eprint!("Google account email: ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).map_err(|error| {
        GcalError::setup(
            "not_logged_in",
            format!("read the account prompt: {error}"),
        )
    })?;
    require_email(line.trim())
}

fn is_already_connected(gog: &Gog, account: &str) -> Result<bool, GcalError> {
    let accounts = gog.accounts(Some(CLIENT))?;
    Ok(accounts
        .iter()
        .any(|entry| entry.email == account && entry.has_calendar_readonly()))
}

fn checklist_error(skipped_keyring: &[String]) -> GcalError {
    let mut hint = "1. Enable the Calendar API in the Cloud project.\n  \
         2. Set the consent screen to External and In production \
         (unverified is fine for personal use).\n  \
         3. Download a Desktop app client.\n  \
         4. Run `bob gcal login -c PATH`."
        .to_string();
    for name in skipped_keyring {
        hint.push_str(&format!(
            "\n  gog's \"{name}\" client keeps its secret in the system \
             keyring, so Bob can't copy it."
        ));
    }
    GcalError::setup(
        "no_client",
        "no bob-gcal OAuth client is stored".to_string(),
    )
    .with_hint(&hint)
}

fn is_tty() -> bool {
    assume_tty() || io::stdin().is_terminal()
}

fn bob_env_has(key: &str) -> bool {
    crate::native::env::var_os(key).is_some_and(|value| !value.is_empty())
}

fn print_report(report: &LoginReport, json: bool) {
    if json {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "already_connected": report.already_connected,
                "account": report.account,
                "client": report.client,
                "calendars": report.calendars,
                "syncing": report.syncing,
                "syncing_ids": report.syncing_ids,
            })
        );
        ui::flush_stdout();
        return;
    }
    let styler = Styler::detect();
    println!(
        "{}",
        styler.cyan("Google Calendar · one-time setup · read-only")
    );
    println!();
    println!(
        "1/3  OAuth client   {} {} (Desktop app)",
        styler.green("✓"),
        report.client
    );
    if report.already_connected {
        println!(
            "2/3  Sign in        {} already connected · {}",
            styler.green("✓"),
            report.account
        );
    } else {
        println!(
            "2/3  Sign in        {} granted: view your calendars (read-only)",
            styler.green("✓")
        );
    }
    let syncing = if report.syncing_ids.is_empty() {
        "none".to_string()
    } else {
        report.syncing_ids.join(", ")
    };
    println!(
        "3/3  Check          {} {} calendars (syncing {}: {syncing})",
        styler.green("✓"),
        report.calendars,
        report.syncing
    );
    println!();
    println!("Token stored by gog. Remove it with: bob gcal logout");
    println!("Next: bob gcal calendars    list calendars for config");
    ui::flush_stdout();
}
