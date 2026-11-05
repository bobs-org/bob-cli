//! Shared OAuth client discovery and guided Google client setup.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use super::{
    client::{parse_client_json_for, peek_project, ClientOrigin, OAuthClient},
    gog::Gog,
    login_ui::LoginUi,
    GoogleError,
};
use crate::native::env as bob_env;

pub(crate) struct ResolvedClient {
    pub(crate) origin: ClientOrigin,
    pub(crate) project: Option<String>,
    pub(crate) reused_name: Option<String>,
    pub(crate) file_display: Option<String>,
}

pub(crate) fn ensure_client(
    gog: &Gog,
    credentials: Option<&Path>,
    ui: &LoginUi,
) -> Result<ResolvedClient, GoogleError> {
    let clients = gog.credentials_list()?;
    if let Some(path) = credentials {
        return store_file_client(gog, &bob_env::expand_tilde(path));
    }

    let service = gog.service();
    if let Some(stored) = clients
        .iter()
        .find(|client| client.client == service.client_bucket)
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
        .filter(|client| client.client != service.client_bucket)
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
        let Ok(client) =
            parse_client_json_for(&bytes, ClientOrigin::GogClient, service)
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
) -> Result<ResolvedClient, GoogleError> {
    let service = gog.service();
    let display = path.display().to_string();
    let bytes = fs::read(path).map_err(|error| {
        GoogleError::setup(
            "client_invalid",
            format!("{}: cannot read client JSON: {error}", display),
        )
        .with_hint(&format!(
            "download a Desktop app client JSON and pass its path with `{} login -c PATH`",
            service.command
        ))
    })?;
    let client = parse_client_json_for(&bytes, ClientOrigin::File, service)
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
) -> Result<ResolvedClient, GoogleError> {
    if !ui.is_tty() {
        return Err(checklist_error(gog.service(), skipped_keyring));
    }

    ui.step_open(
        1,
        "Google client",
        "Bob needs an OAuth client from your own Google Cloud project.",
    );
    for name in skipped_keyring {
        ui.note_interactive(&format!(
            "gog's \"{name}\" client keeps its secret in the system keyring, so Bob can't copy it."
        ));
    }
    let mut checklist_shown = false;
    loop {
        if let Some((path, client)) = newest_download_client(gog.service()) {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "client_secret.json".to_string());
            let question = format!(
                "Use ~/Downloads/{name} (Desktop app · project {})? [Y/n]",
                client.project
            );
            let mut store_download = false;
            loop {
                match ui.prompt(&question)? {
                    None => return Err(setup_cancelled(gog.service())),
                    Some(answer) => match parse_confirm_default_yes(&answer) {
                        Confirm::Yes => {
                            store_download = true;
                            break;
                        }
                        Confirm::No => break,
                        Confirm::Quit => {
                            return Err(setup_cancelled(gog.service()))
                        }
                        Confirm::Unknown => {
                            ui.note_interactive("Please answer Y, n, or q.");
                        }
                    },
                }
            }
            if store_download {
                let client = parse_client_json_for(
                    &fs::read(&path).map_err(|error| {
                        GoogleError::setup(
                            "client_invalid",
                            format!(
                                "{}: cannot read client JSON: {error}",
                                path.display()
                            ),
                        )
                    })?,
                    ClientOrigin::Downloads,
                    gog.service(),
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
        }

        if !checklist_shown {
            print_setup_checklist(ui, None);
            checklist_shown = true;
        }
        let Some(line) = ui.prompt(
            "Press Enter once the JSON is downloaded (Bob checks ~/Downloads), or paste its path › ",
        )? else {
            return Err(setup_cancelled(gog.service()));
        };
        let input = line.trim();
        if input.eq_ignore_ascii_case("q") || input.eq_ignore_ascii_case("quit")
        {
            return Err(setup_cancelled(gog.service()));
        }
        if input.is_empty() {
            continue;
        }
        if input.starts_with('{') {
            match parse_client_json_for(
                input.as_bytes(),
                ClientOrigin::Pasted,
                gog.service(),
            ) {
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
    let service = ui.service();
    let api = project_url(service.api_console_url, project);
    let audience =
        project_url("https://console.cloud.google.com/auth/audience", project);
    let create = project_url(
        "https://console.cloud.google.com/auth/clients/create",
        project,
    );
    ui.note_interactive("One time, about 3 minutes, in your browser:");
    ui.note_interactive(&format!(
        "a. Turn on the {} API",
        service.service_name
    ));
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

fn newest_download_client(
    service: super::GoogleService,
) -> Option<(PathBuf, OAuthClient)> {
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
        let client =
            parse_client_json_for(&bytes, ClientOrigin::Downloads, service)
                .ok()?;
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

fn store_client(gog: &Gog, client: &OAuthClient) -> Result<(), GoogleError> {
    gog.credentials_set_json(&client.to_gog_json())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirm {
    Yes,
    No,
    Quit,
    Unknown,
}

pub(crate) fn parse_confirm_default_yes(answer: &str) -> Confirm {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return Confirm::Yes;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "y" | "yes" => Confirm::Yes,
        "n" | "no" => Confirm::No,
        "q" | "quit" => Confirm::Quit,
        _ => Confirm::Unknown,
    }
}

fn setup_cancelled(service: super::GoogleService) -> GoogleError {
    GoogleError::setup("cancelled", "Google client setup cancelled".to_string())
        .with_hint(&format!(
            "a stored client stays stored; rerun `{} login`",
            service.command
        ))
}

fn checklist_error(
    service: super::GoogleService,
    skipped_keyring: &[String],
) -> GoogleError {
    let api = project_url(service.api_console_url, None);
    let audience =
        project_url("https://console.cloud.google.com/auth/audience", None);
    let create = project_url(
        "https://console.cloud.google.com/auth/clients/create",
        None,
    );
    let mut hint = format!(
        "1. Turn on the {} API: {api}\n\
         2. Set the audience to External and publish it: {audience}\n\
         3. Create and download a Desktop app client: {create}\n\
         Run `{} login` in a terminal to continue.",
        service.service_name, service.command
    );
    for name in skipped_keyring {
        hint.push_str(&format!(
            "\n  gog's \"{name}\" client keeps its secret in the system keyring, so Bob can't copy it."
        ));
    }
    GoogleError::setup(
        "no_client",
        format!("no {} OAuth client is stored", service.client_bucket),
    )
    .with_hint(&hint)
}
