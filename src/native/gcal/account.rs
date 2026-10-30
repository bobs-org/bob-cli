//! One account resolver shared by login, Calendar commands, and doctor.

use super::{
    config::GcalConfig,
    gog::{Gog, GogAccount, CLIENT},
    login_ui::LoginUi,
    GcalError,
};

/// Resolve the account for login, honoring explicit and configured choices
/// before consulting gog's account buckets or prompting.
pub(crate) fn resolve_login(
    gog: &Gog,
    override_email: Option<&str>,
    config: &GcalConfig,
    ui: &LoginUi,
) -> Result<String, GcalError> {
    if let Some(email) = override_email.filter(|email| !email.is_empty()) {
        return require_email(email);
    }
    if let Some(email) = config.account() {
        return require_email(email);
    }

    let bob_emails = unique_emails(&gog.accounts(Some(CLIENT))?);
    if let [email] = bob_emails.as_slice() {
        return Ok(email.clone());
    }

    let all_emails = unique_emails(&gog.accounts(None)?);
    if let [email] = all_emails.as_slice() {
        return Ok(email.clone());
    }

    if !ui.is_tty() {
        return Err(account_choice_needed(all_emails.len() > 1));
    }

    // Put Bob's existing Calendar grants first, then other gog accounts.
    let mut candidates = bob_emails;
    for email in all_emails {
        push_unique(&mut candidates, email);
    }
    choose_account(ui, &candidates)
}

/// Resolve the account for commands that must not prompt.
pub(crate) fn resolve_command(
    gog: &Gog,
    config: &GcalConfig,
) -> Result<String, GcalError> {
    if let Some(email) = config.account() {
        return require_email(email);
    }

    let bob_emails = unique_emails(&gog.accounts(Some(CLIENT))?);
    if let [email] = bob_emails.as_slice() {
        return Ok(email.clone());
    }

    let all_emails = unique_emails(&gog.accounts(None)?);
    match all_emails.as_slice() {
        [email] => Ok(email.clone()),
        [] => Err(GcalError::setup(
            "not_logged_in",
            "no Google account is configured".to_string(),
        )
        .with_hint("run `bob gcal login` or set gcal.account")),
        _ => Err(GcalError::setup(
            "account_ambiguous",
            "more than one Google account is available".to_string(),
        )
        .with_hint("set gcal.account to the address Bob should use")),
    }
}

pub(crate) fn would_be_ambiguous(gog: &Gog, config: &GcalConfig) -> bool {
    resolve_command(gog, config)
        .err()
        .is_some_and(|error| error.kind() == "account_ambiguous")
}

fn choose_account(
    ui: &LoginUi,
    candidates: &[String],
) -> Result<String, GcalError> {
    ui.note_interactive("Which Google account should Bob read?");
    for (index, email) in candidates.iter().enumerate() {
        ui.note_interactive(&format!("  {}  {email}", index + 1));
    }
    loop {
        let Some(line) = ui.prompt("Number or email › ")? else {
            return Err(cancelled());
        };
        let answer = line.trim();
        if answer.eq_ignore_ascii_case("q")
            || answer.eq_ignore_ascii_case("quit")
        {
            return Err(cancelled());
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

fn account_choice_needed(ambiguous: bool) -> GcalError {
    if ambiguous {
        GcalError::setup(
            "needs_tty",
            "gog has more than one account and login needs a choice"
                .to_string(),
        )
        .with_hint(
            "pass `-a EMAIL`, set gcal.account, or run login in a terminal",
        )
    } else {
        GcalError::setup(
            "needs_tty",
            "login needs a Google account address".to_string(),
        )
        .with_hint(
            "pass `-a EMAIL`, set gcal.account, or run login in a terminal",
        )
    }
}

fn cancelled() -> GcalError {
    GcalError::setup("cancelled", "account choice cancelled".to_string())
        .with_hint("rerun `bob gcal login` to choose an account")
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
