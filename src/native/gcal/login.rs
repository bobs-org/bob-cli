//! `bob gcal login`: one-time read-only Calendar consent through gog.

use std::path::Path;

use serde_json::json;

use super::{
    client::{parse_client_json, peek_project, ClientOrigin},
    config::GcalConfig,
    gog::{self, Gog, CLIENT},
    login_ui::LoginUi,
    signin, ui, GcalError, LoginArgs,
};
use crate::native::env as bob_env;

pub(crate) fn run(args: &LoginArgs) -> i32 {
    let format = args.format.as_str();
    let config = match GcalConfig::resolve() {
        Ok(config) => config,
        Err(error) => return ui::report_error("login", &error, format),
    };
    let json = args.format.is_json();
    let ui_login = LoginUi::new(json);
    match login(args, &config, &ui_login) {
        Ok(report) => {
            print_report(&report, &ui_login);
            0
        }
        Err(error) => ui::report_error("login", &error, format),
    }
}

struct ResolvedClient {
    origin: ClientOrigin,
    project: Option<String>,
    reused_name: Option<String>,
    file_display: Option<String>,
}

struct LoginReport {
    already_connected: bool,
    account: String,
    client: String,
    calendars: usize,
    syncing: usize,
    syncing_ids: Vec<String>,
    sign_in: &'static str,
}

fn login(
    args: &LoginArgs,
    config: &GcalConfig,
    ui: &LoginUi,
) -> Result<LoginReport, GcalError> {
    let gog = Gog::resolve(config.timeout())?;
    let _version = gog.require_version()?;
    ui.title();
    let resolved = ensure_client(&gog, args.credentials.as_deref())?;
    render_client_step(ui, &resolved);
    let account = resolve_account(&gog, args.account.as_deref(), config)?;
    let already = is_already_connected(&gog, &account)?;
    let sign_in = if already {
        ui.step_ok(2, "Sign in", &format!("already connected · {account}"));
        "already_connected"
    } else {
        let (flow, reason) =
            signin::choose_flow(args.manual, signin::FlowEnv::detect());
        signin::run(
            &gog,
            ui,
            &account,
            flow,
            reason,
            resolved.project.as_deref(),
        )?;
        flow.as_str()
    };
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
    let report = LoginReport {
        already_connected: already,
        account,
        client: CLIENT.to_string(),
        calendars: calendars.len(),
        syncing: syncing_ids.len(),
        syncing_ids,
        sign_in,
    };
    render_check_step(ui, &report);
    if !already {
        ui.summary(&report.account);
    }
    Ok(report)
}

fn render_client_step(ui: &LoginUi, resolved: &ResolvedClient) {
    let project = resolved
        .project
        .as_deref()
        .map(|project| format!(" · project {project}"))
        .unwrap_or_default();
    let detail = match resolved.origin {
        ClientOrigin::GogClient => {
            let name = resolved.reused_name.as_deref().unwrap_or("default");
            format!("reusing gog's \"{name}\" client{project}")
        }
        ClientOrigin::File | ClientOrigin::Downloads | ClientOrigin::Pasted => {
            let file =
                resolved.file_display.as_deref().unwrap_or("client.json");
            format!("{file} · Desktop app{project}")
        }
        ClientOrigin::Stored => format!("bob-gcal{project}"),
    };
    ui.step_ok(1, "Google client", &detail);
    if resolved.origin == ClientOrigin::GogClient {
        ui.note("Bob keeps its own read-only sign-in; Gmail stays untouched.");
    }
}

fn render_check_step(ui: &LoginUi, report: &LoginReport) {
    let syncing = if report.syncing_ids.is_empty() {
        "none".to_string()
    } else {
        report.syncing_ids.join(", ")
    };
    ui.step_ok(
        3,
        "Check",
        &format!(
            "{} calendars · syncing {}: {syncing}",
            report.calendars, report.syncing
        ),
    );
}

fn ensure_client(
    gog: &Gog,
    credentials: Option<&Path>,
) -> Result<ResolvedClient, GcalError> {
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
        let file_display = expanded
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        return Ok(ResolvedClient {
            origin: ClientOrigin::File,
            project: Some(client.project),
            reused_name: None,
            file_display,
        });
    }
    if let Some(stored) = clients.iter().find(|client| client.client == CLIENT)
    {
        let project = stored.path.as_deref().and_then(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| peek_project(&bytes))
        });
        return Ok(ResolvedClient {
            origin: ClientOrigin::Stored,
            project,
            reused_name: None,
            file_display: None,
        });
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
        return Ok(ResolvedClient {
            origin: ClientOrigin::GogClient,
            project: Some(client.project),
            reused_name: Some(other.client.clone()),
            file_display: None,
        });
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
            if super::config::stdin_is_tty() {
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
            if super::config::stdin_is_tty() {
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
    use std::io::{self, BufRead, Write};
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

fn print_report(report: &LoginReport, ui: &LoginUi) {
    if ui.json() {
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
                "sign_in": report.sign_in,
            })
        );
        ui::flush_stdout();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn flow_names_match_json() {
        assert_eq!(super::signin::Flow::Browser.as_str(), "browser");
        assert_eq!(super::signin::Flow::Paste.as_str(), "paste");
    }
}
