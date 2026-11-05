//! Account selection shared by Google integrations.

use super::{
    gog::{Gog, GogAccount},
    login_ui::LoginUi,
    GoogleError,
};

/// Resolve the account for login, honoring explicit and configured choices
/// before consulting gog's account buckets or prompting.
pub(crate) fn resolve_login(
    gog: &Gog,
    override_email: Option<&str>,
    configured_account: Option<&str>,
    ui: &LoginUi,
) -> Result<String, GoogleError> {
    if let Some(email) = override_email.filter(|email| !email.is_empty()) {
        return require_email(email);
    }
    if let Some(email) = configured_account {
        return require_email(email);
    }

    let service = gog.service();
    let bob_emails = unique_emails(&gog.accounts(Some(service.client_bucket))?);
    if let [email] = bob_emails.as_slice() {
        return Ok(email.clone());
    }

    let all_emails = unique_emails(&gog.accounts(None)?);
    if let [email] = all_emails.as_slice() {
        return Ok(email.clone());
    }

    if !ui.is_tty() {
        return Err(account_choice_needed(all_emails.len() > 1, service));
    }

    // Prefer Bob's existing service grant, then other gog accounts.
    let mut candidates = bob_emails;
    for email in all_emails {
        push_unique(&mut candidates, email);
    }
    choose_account(ui, &candidates)
}

/// Resolve the account for commands that must not prompt.
pub(crate) fn resolve_command(
    gog: &Gog,
    configured_account: Option<&str>,
) -> Result<String, GoogleError> {
    if let Some(email) = configured_account {
        return require_email(email);
    }

    let service = gog.service();
    let bob_emails = unique_emails(&gog.accounts(Some(service.client_bucket))?);
    if let [email] = bob_emails.as_slice() {
        return Ok(email.clone());
    }

    let all_emails = unique_emails(&gog.accounts(None)?);
    match all_emails.as_slice() {
        [email] => Ok(email.clone()),
        [] => Err(GoogleError::setup(
            "not_logged_in",
            "no Google account is configured".to_string(),
        )
        .with_hint(&format!(
            "run `{} login` or set {}.account",
            service.command,
            service
                .command
                .strip_prefix("bob ")
                .unwrap_or(service.command)
        ))),
        _ => Err(GoogleError::setup(
            "account_ambiguous",
            "more than one Google account is available".to_string(),
        )
        .with_hint(&format!(
            "set {}.account to the address Bob should use",
            service
                .command
                .strip_prefix("bob ")
                .unwrap_or(service.command)
        ))),
    }
}

/// Resolve an account for logout without prompting or touching other buckets.
pub(crate) fn resolve_logout(
    gog: &Gog,
    override_email: Option<&str>,
    configured_account: Option<&str>,
) -> Result<String, GoogleError> {
    // Logout passes explicit account identifiers through to gog, as Calendar
    // did before this extraction; it does not apply login's email validation.
    if let Some(email) = override_email.filter(|email| !email.is_empty()) {
        return Ok(email.to_string());
    }
    if let Some(email) = configured_account {
        return Ok(email.to_string());
    }

    let service = gog.service();
    let emails = unique_emails(&gog.accounts(Some(service.client_bucket))?);
    match emails.as_slice() {
        [email] => Ok(email.clone()),
        [] => Err(GoogleError::setup(
            "not_logged_in",
            format!(
                "no {} {} grant is stored",
                service.client_bucket, service.service_name
            ),
        )
        .with_hint("nothing to remove")),
        _ => Err(GoogleError::setup(
            "not_logged_in",
            format!(
                "gog holds more than one {} account; pass -a EMAIL",
                service.client_bucket
            ),
        )),
    }
}

pub(crate) fn would_be_ambiguous(
    gog: &Gog,
    configured_account: Option<&str>,
) -> bool {
    resolve_command(gog, configured_account)
        .err()
        .is_some_and(|error| error.kind() == "account_ambiguous")
}

fn choose_account(
    ui: &LoginUi,
    candidates: &[String],
) -> Result<String, GoogleError> {
    ui.note_interactive(ui.service().account_question);
    for (index, email) in candidates.iter().enumerate() {
        ui.note_interactive(&format!("  {}  {email}", index + 1));
    }
    loop {
        let Some(line) = ui.prompt("Number or email › ")? else {
            return Err(cancelled(ui));
        };
        let answer = line.trim();
        if answer.eq_ignore_ascii_case("q")
            || answer.eq_ignore_ascii_case("quit")
        {
            return Err(cancelled(ui));
        }
        if let Ok(number) = answer.parse::<usize>()
            && let Some(email) = candidates.get(number.saturating_sub(1))
            && number != 0
        {
            return Ok(email.clone());
        }
        if answer.contains('@') {
            return Ok(answer.to_string());
        }
        ui.note_interactive("Choose a listed number or type an email address.");
    }
}

pub(crate) fn unique_emails(accounts: &[GogAccount]) -> Vec<String> {
    let mut emails = Vec::new();
    for account in accounts {
        let email = account.email.trim();
        if !email.is_empty() {
            push_unique(&mut emails, email.to_string());
        }
    }
    emails
}

fn push_unique(emails: &mut Vec<String>, candidate: String) {
    if !emails
        .iter()
        .any(|known| known.eq_ignore_ascii_case(&candidate))
    {
        emails.push(candidate);
    }
}

fn require_email(email: &str) -> Result<String, GoogleError> {
    if email.contains('@') {
        Ok(email.to_string())
    } else {
        Err(GoogleError::setup(
            "config",
            format!("account {email:?} must contain '@'"),
        ))
    }
}

fn account_choice_needed(
    ambiguous: bool,
    service: super::GoogleService,
) -> GoogleError {
    let settings = service
        .command
        .strip_prefix("bob ")
        .unwrap_or(service.command);
    let hint = format!(
        "pass `-a EMAIL`, set {settings}.account, or run login in a terminal"
    );
    if ambiguous {
        GoogleError::setup(
            "needs_tty",
            "gog has more than one account and login needs a choice"
                .to_string(),
        )
        .with_hint(&hint)
    } else {
        GoogleError::setup(
            "needs_tty",
            "login needs a Google account address".to_string(),
        )
        .with_hint(&hint)
    }
}

fn cancelled(ui: &LoginUi) -> GoogleError {
    GoogleError::setup("cancelled", "account choice cancelled".to_string())
        .with_hint(&format!(
            "rerun `{} login` to choose an account",
            ui.service().command
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_emails_ignores_case_and_whitespace_duplicates() {
        let accounts = [
            GogAccount {
                email: "you@example.com".to_string(),
                client: None,
                services: vec![],
                scopes: vec![],
            },
            GogAccount {
                email: " YOU@example.com ".to_string(),
                client: None,
                services: vec![],
                scopes: vec![],
            },
        ];
        assert_eq!(unique_emails(&accounts), ["you@example.com"]);
    }
}
