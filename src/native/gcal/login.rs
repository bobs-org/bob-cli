//! `bob gcal login`: guided client setup and read-only Calendar consent.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::json;

use super::{
    account,
    client::{parse_client_json, peek_project, ClientOrigin, OAuthClient},
    config::GcalConfig,
    gog::{Gog, GogCalendar, API_CONSOLE_URL, CLIENT},
    login_ui::LoginUi,
    signin, ui, GcalError, LoginArgs,
};
use crate::native::{env as bob_env, vault_sync};

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
    client_source: &'static str,
    client_project: Option<String>,
    calendars: usize,
    syncing: usize,
    syncing_ids: Vec<String>,
    sign_in: &'static str,
    reconnected: bool,
}

pub(crate) fn run(args: &LoginArgs) -> i32 {
    let format = args.format.as_str();
    let config = match GcalConfig::resolve() {
        Ok(config) => config,
        Err(error) => return ui::report_error("login", &error, format),
    };
    let ui_login = LoginUi::new(args.format.is_json());
    match login(args, &config, &ui_login) {
        Ok(report) => {
            print_report(&report, &ui_login);
            0
        }
        Err(error) => ui::report_error("login", &error, format),
    }
}

fn login(
    args: &LoginArgs,
    config: &GcalConfig,
    ui: &LoginUi,
) -> Result<LoginReport, GcalError> {
    let gog = Gog::resolve(config.timeout())?;
    let _version = gog.require_version()?;
    ui.title();

    let resolved = ensure_client(&gog, args.credentials.as_deref(), ui)?;
    render_client_step(ui, &resolved);
    let account =
        account::resolve_login(&gog, args.account.as_deref(), config, ui)?;

    let listed = gog.accounts(Some(CLIENT))?;
    let has_readonly_grant = listed.iter().any(|entry| {
        entry.email.eq_ignore_ascii_case(&account)
            && entry.has_calendar_readonly()
    });

    let mut already_connected = false;
    let mut reconnected = false;
    let mut testing_tip = false;
    let mut sign_in = "already_connected";
    let mut first_check_error = None;

    if has_readonly_grant {
        match gog.calendars(&account) {
            Ok(calendars) => {
                already_connected = true;
                let report = build_report(
                    &account,
                    &resolved,
                    sign_in,
                    already_connected,
                    reconnected,
                    calendars,
                    config,
                );
                render_check_step(ui, &report, config);
                return finish_login(
                    report,
                    ui,
                    config,
                    &gog,
                    reconnected,
                    testing_tip,
                );
            }
            Err(error) if is_reconnect_error(&error) => {
                testing_tip = error.kind() == "invalid_grant";
                reconnected = true;
                ui.step_warn(
                    2,
                    "Sign in",
                    "the saved sign-in stopped working; signing in again…",
                );
            }
            Err(error) if error.kind() == "api_disabled" => {
                already_connected = true;
                first_check_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    if !already_connected {
        let (flow, reason) =
            signin::choose_flow(args.manual, signin::FlowEnv::detect());
        let flow = signin::run(
            &gog,
            ui,
            &account,
            flow,
            reason,
            resolved.project.as_deref(),
        )?;
        sign_in = flow.as_str();
    }

    let calendars = check_calendars(
        &gog,
        ui,
        &account,
        resolved.project.as_deref(),
        first_check_error,
    )?;
    let report = build_report(
        &account,
        &resolved,
        sign_in,
        already_connected,
        reconnected,
        calendars,
        config,
    );
    render_check_step(ui, &report, config);
    finish_login(report, ui, config, &gog, reconnected, testing_tip)
}

fn finish_login(
    report: LoginReport,
    ui: &LoginUi,
    config: &GcalConfig,
    gog: &Gog,
    reconnected: bool,
    testing_tip: bool,
) -> Result<LoginReport, GcalError> {
    if !report.already_connected {
        ui.summary(&report.account);
        let host = vault_sync::hostname_slug();
        if let Some(writer) = config.writer().filter(|writer| *writer != host) {
            ui.writer_note(writer);
        }
        if account::would_be_ambiguous(gog, config) {
            ui.account_tip(&report.account);
        }
        if reconnected && testing_tip {
            ui.testing_tip();
        }
    }
    Ok(report)
}

fn build_report(
    account: &str,
    resolved: &ResolvedClient,
    sign_in: &'static str,
    already_connected: bool,
    reconnected: bool,
    calendars: Vec<GogCalendar>,
    config: &GcalConfig,
) -> LoginReport {
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
    LoginReport {
        already_connected,
        account: account.to_string(),
        client: CLIENT.to_string(),
        client_source: resolved.origin.as_str(),
        client_project: resolved.project.clone(),
        calendars: calendars.len(),
        syncing: syncing_ids.len(),
        syncing_ids,
        sign_in,
        reconnected,
    }
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

fn render_check_step(ui: &LoginUi, report: &LoginReport, config: &GcalConfig) {
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
    let missing: Vec<_> = config
        .calendars()
        .iter()
        .filter(|configured| {
            !report.syncing_ids.iter().any(|id| {
                id == &configured.id
                    || (configured.id == "primary" && id == "primary")
            })
        })
        .map(|configured| configured.id.as_str())
        .collect();
    if !missing.is_empty() {
        ui.step_warn(
            3,
            "Check",
            &format!("configured calendars not found: {}", missing.join(", ")),
        );
        ui.note("run `bob gcal calendars` to choose available calendars");
    }
}

fn check_calendars(
    gog: &Gog,
    ui: &LoginUi,
    account: &str,
    project: Option<&str>,
    mut first_error: Option<GcalError>,
) -> Result<Vec<GogCalendar>, GcalError> {
    loop {
        let result = match first_error.take() {
            Some(error) => Err(error),
            None => gog.calendars(account),
        };
        match result {
            Ok(calendars) => return Ok(calendars),
            Err(error) if error.kind() == "api_disabled" => {
                let url = api_disabled_url(&error, project);
                ui.step_fail(
                    3,
                    "Check",
                    "the Calendar API is turned off for this project",
                );
                ui.note_interactive("Turn it on, then come back:");
                ui.link(&url);
                if !ui.is_tty() {
                    return Err(error.with_hint(&format!("enable it at {url}")));
                }
                match ui
                    .prompt("Press Enter to check again, or q to stop › ")?
                {
                    None => return Err(api_retry_cancelled(error)),
                    Some(answer)
                        if answer.trim().eq_ignore_ascii_case("q")
                            || answer.trim().eq_ignore_ascii_case("quit") =>
                    {
                        return Err(api_retry_cancelled(error));
                    }
                    Some(_) => {}
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn api_retry_cancelled(error: GcalError) -> GcalError {
    GcalError::runtime("api_disabled", error.message().to_string())
        .with_hint(error.hint().unwrap_or("enable the Calendar API and retry"))
}

fn api_disabled_url(error: &GcalError, project: Option<&str>) -> String {
    if let Some(hint) = error.hint()
        && let Some(start) = hint.find("https://")
    {
        let candidate = hint[start..]
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(['.', ',', ')']);
        if let Ok(mut url) = url::Url::parse(candidate) {
            if url
                .query_pairs()
                .any(|(key, value)| key == "project" && !value.is_empty())
            {
                return url.into();
            }
            if let Some(project) = project {
                url.query_pairs_mut().append_pair("project", project);
                return url.into();
            }
            return url.into();
        }
    }
    if let Some(project) = project
        && let Ok(mut url) = url::Url::parse(API_CONSOLE_URL)
    {
        url.query_pairs_mut().append_pair("project", project);
        return url.into();
    }
    API_CONSOLE_URL.to_string()
}

fn is_reconnect_error(error: &GcalError) -> bool {
    matches!(
        error.kind(),
        "invalid_grant" | "insufficient_scope" | "not_logged_in"
    )
}

fn ensure_client(
    gog: &Gog,
    credentials: Option<&Path>,
    ui: &LoginUi,
) -> Result<ResolvedClient, GcalError> {
    let clients = gog.credentials_list()?;
    if let Some(path) = credentials {
        return store_file_client(gog, &bob_env::expand_tilde(path));
    }

    if let Some(stored) = clients.iter().find(|client| client.client == CLIENT)
    {
        let project = stored.path.as_deref().and_then(|path| {
            fs::read(path).ok().and_then(|bytes| peek_project(&bytes))
        });
        return Ok(ResolvedClient {
            origin: ClientOrigin::Stored,
            project,
            reused_name: None,
            file_display: None,
        });
    }

    let mut skipped_keyring = Vec::new();
    let mut ordered: Vec<_> = clients
        .iter()
        .filter(|client| client.client != CLIENT)
        .collect();
    ordered.sort_by_key(|client| !client.default);
    for other in ordered {
        if other.client_secret_in_keyring {
            skipped_keyring.push(other.client.clone());
            continue;
        }
        let Some(path) = other.path.as_deref() else {
            continue;
        };
        let Ok(bytes) = fs::read(path) else {
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

    guided_client(gog, ui, &skipped_keyring)
}

fn store_file_client(
    gog: &Gog,
    path: &Path,
) -> Result<ResolvedClient, GcalError> {
    let display = path.display().to_string();
    let bytes = fs::read(path).map_err(|error| {
        GcalError::setup(
            "client_invalid",
            format!("{}: cannot read client JSON: {error}", display),
        )
        .with_hint(
            "download a Desktop app client JSON and pass its path with `bob gcal login -c PATH`",
        )
    })?;
    let client = parse_client_json(&bytes, ClientOrigin::File)
        .map_err(|problem| problem.into_error(&display))?;
    store_client(gog, &client)?;
    Ok(ResolvedClient {
        origin: ClientOrigin::File,
        project: Some(client.project),
        reused_name: None,
        file_display: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    })
}

fn guided_client(
    gog: &Gog,
    ui: &LoginUi,
    skipped_keyring: &[String],
) -> Result<ResolvedClient, GcalError> {
    if !ui.is_tty() {
        return Err(checklist_error(skipped_keyring));
    }

    ui.step_open(
        1,
        "Google client",
        "Bob needs an OAuth client from your own Google Cloud project.",
    );
    let mut checklist_shown = false;
    loop {
        if let Some((path, client)) = newest_download_client() {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "client_secret.json".to_string());
            let question = format!(
                "Use ~/Downloads/{name} (Desktop app · project {})? [Y/n]",
                client.project
            );
            match ui.prompt(&question)? {
                None => return Err(setup_cancelled()),
                Some(answer)
                    if !matches!(
                        answer.trim().to_ascii_lowercase().as_str(),
                        "n" | "no"
                    ) =>
                {
                    let client = parse_client_json(
                        &fs::read(&path).map_err(|error| {
                            GcalError::setup(
                                "client_invalid",
                                format!(
                                    "{}: cannot read client JSON: {error}",
                                    path.display()
                                ),
                            )
                        })?,
                        ClientOrigin::Downloads,
                    )
                    .map_err(|problem| {
                        problem.into_error(&path.display().to_string())
                    })?;
                    store_client(gog, &client)?;
                    return Ok(ResolvedClient {
                        origin: ClientOrigin::Downloads,
                        project: Some(client.project),
                        reused_name: None,
                        file_display: Some(name),
                    });
                }
                Some(_) => {}
            }
        }

        if !checklist_shown {
            print_setup_checklist(ui, None);
            checklist_shown = true;
        }
        let Some(line) = ui.prompt(
            "Press Enter once the JSON is downloaded (Bob checks ~/Downloads), or paste its path › ",
        )? else {
            return Err(setup_cancelled());
        };
        let input = line.trim();
        if input.eq_ignore_ascii_case("q") || input.eq_ignore_ascii_case("quit")
        {
            return Err(setup_cancelled());
        }
        if input.is_empty() {
            continue;
        }
        if input.starts_with('{') {
            match parse_client_json(input.as_bytes(), ClientOrigin::Pasted) {
                Ok(client) => {
                    store_client(gog, &client)?;
                    return Ok(ResolvedClient {
                        origin: ClientOrigin::Pasted,
                        project: Some(client.project),
                        reused_name: None,
                        file_display: Some("pasted JSON".to_string()),
                    });
                }
                Err(problem) => {
                    ui.note_interactive(problem.message());
                    ui.note_interactive(problem.hint());
                }
            }
            continue;
        }
        let path_text = unescape_path(input);
        let path = bob_env::expand_tilde(Path::new(&path_text));
        match store_file_client(gog, &path) {
            Ok(mut client) => {
                client.origin = ClientOrigin::File;
                return Ok(client);
            }
            Err(error) => {
                ui.note_interactive(error.message());
                if let Some(hint) = error.hint() {
                    ui.note_interactive(hint);
                }
            }
        }
    }
}

fn print_setup_checklist(ui: &LoginUi, project: Option<&str>) {
    let api = project_url("https://console.cloud.google.com/apis/library/calendar-json.googleapis.com", project);
    let audience =
        project_url("https://console.cloud.google.com/auth/audience", project);
    let create = project_url(
        "https://console.cloud.google.com/auth/clients/create",
        project,
    );
    ui.note_interactive("One time, about 3 minutes, in your browser:");
    ui.note_interactive("a. Turn on the Calendar API");
    ui.link(&api);
    ui.note_interactive(
        "b. Audience: External, then Publish app (In production;",
    );
    ui.note_interactive("   unverified is fine for personal use)");
    ui.link(&audience);
    ui.note_interactive(
        "c. Clients → Create client → Desktop app → Download JSON",
    );
    ui.link(&create);
}

fn project_url(base: &str, project: Option<&str>) -> String {
    let Ok(mut url) = url::Url::parse(base) else {
        return base.to_string();
    };
    if let Some(project) = project {
        url.query_pairs_mut().append_pair("project", project);
    }
    url.into()
}

fn newest_download_client() -> Option<(PathBuf, OAuthClient)> {
    let downloads = bob_env::home_dir().join("Downloads");
    let mut paths: Vec<(SystemTime, PathBuf)> = fs::read_dir(downloads)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with("client_secret") || !name.ends_with(".json") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some((metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), path))
        })
        .collect();
    paths.sort_by(|a, b| b.0.cmp(&a.0));
    paths.into_iter().find_map(|(_, path)| {
        let bytes = fs::read(&path).ok()?;
        let client = parse_client_json(&bytes, ClientOrigin::Downloads).ok()?;
        Some((path, client))
    })
}

fn unescape_path(input: &str) -> String {
    let input = input.trim();
    let input = if input.len() >= 2
        && ((input.starts_with('"') && input.ends_with('"'))
            || (input.starts_with('\'') && input.ends_with('\'')))
    {
        &input[1..input.len() - 1]
    } else {
        input
    };
    input.replace("\\ ", " ")
}

fn store_client(gog: &Gog, client: &OAuthClient) -> Result<(), GcalError> {
    gog.credentials_set_json(&client.to_gog_json())
}

fn setup_cancelled() -> GcalError {
    GcalError::setup("cancelled", "Google client setup cancelled".to_string())
        .with_hint("a stored client stays stored; rerun `bob gcal login`")
}

fn checklist_error(skipped_keyring: &[String]) -> GcalError {
    let api = project_url("https://console.cloud.google.com/apis/library/calendar-json.googleapis.com", None);
    let audience =
        project_url("https://console.cloud.google.com/auth/audience", None);
    let create = project_url(
        "https://console.cloud.google.com/auth/clients/create",
        None,
    );
    let mut hint = format!(
        "1. Turn on the Calendar API: {api}\n\
         2. Set the audience to External and publish it: {audience}\n\
         3. Create and download a Desktop app client: {create}\n\
         Run `bob gcal login` in a terminal to continue."
    );
    for name in skipped_keyring {
        hint.push_str(&format!(
            "\n  gog's \"{name}\" client keeps its secret in the system keyring, so Bob can't copy it."
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
                "client_source": report.client_source,
                "client_project": report.client_project,
                "reconnected": report.reconnected,
            })
        );
        ui::flush_stdout();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_names_match_json() {
        assert_eq!(signin::Flow::Browser.as_str(), "browser");
        assert_eq!(signin::Flow::Paste.as_str(), "paste");
    }

    #[test]
    fn api_url_prefers_message_then_client_project() {
        let with_google_project = GcalError::runtime("api_disabled", "off".to_string())
            .with_hint("enable it at https://console.developers.google.com/apis/api/calendar-json.googleapis.com/overview?project=123");
        assert!(api_disabled_url(&with_google_project, Some("client-proj"))
            .contains("project=123"));
        let generic = GcalError::runtime("api_disabled", "off".to_string())
            .with_hint(&format!("enable it at {API_CONSOLE_URL}"));
        assert!(api_disabled_url(&generic, Some("client-proj"))
            .contains("project=client-proj"));
    }
}
