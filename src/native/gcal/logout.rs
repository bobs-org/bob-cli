//! `bob gcal logout`: remove Bob's Calendar grant only.

use std::io::{self, BufRead, IsTerminal, Write};

use serde_json::json;

use super::super::style::Styler;
use super::{
    account,
    config::{assume_tty, GcalConfig},
    gog::{Gog, CLIENT, REVOKE_URL},
    ui, GcalError, LogoutArgs,
};

pub(crate) fn run(args: &LogoutArgs) -> i32 {
    let format = args.format.as_str();
    let config = match GcalConfig::resolve() {
        Ok(config) => config,
        Err(error) => return ui::report_error("logout", &error, format),
    };
    match logout(args, &config) {
        Ok(account) => {
            print_report(&account, args.format.is_json());
            0
        }
        Err(error) => ui::report_error("logout", &error, format),
    }
}

fn logout(args: &LogoutArgs, config: &GcalConfig) -> Result<String, GcalError> {
    let gog = Gog::resolve(config.timeout())?;
    let _ = gog.require_version()?;
    let account = resolve_account(&gog, args.account.as_deref(), config)?;
    if !args.force {
        confirm_logout(&account)?;
    }
    gog.auth_remove(&account)?;
    Ok(account)
}

fn resolve_account(
    gog: &Gog,
    override_email: Option<&str>,
    config: &GcalConfig,
) -> Result<String, GcalError> {
    if let Some(email) = override_email.filter(|email| !email.is_empty()) {
        return Ok(email.to_string());
    }
    if let Some(email) = config.account() {
        return Ok(email.to_string());
    }
    let emails = account::unique_emails(&gog.accounts(Some(CLIENT))?);
    match emails.as_slice() {
        [email] => Ok(email.clone()),
        [] => Err(GcalError::setup(
            "not_logged_in",
            "no bob-gcal Calendar grant is stored".to_string(),
        )
        .with_hint("nothing to remove")),
        _ => Err(GcalError::setup(
            "not_logged_in",
            "gog holds more than one bob-gcal account; pass -a EMAIL"
                .to_string(),
        )),
    }
}

fn confirm_logout(account: &str) -> Result<(), GcalError> {
    if !(assume_tty() || io::stdin().is_terminal()) {
        return Err(GcalError::setup(
            "not_logged_in",
            "logout needs --force when stdin is not a TTY".to_string(),
        )
        .with_hint("rerun `bob gcal logout -y`"));
    }
    eprint!("Remove bob-gcal Calendar grant for {account}? [y/N] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).map_err(|error| {
        GcalError::setup(
            "not_logged_in",
            format!("read the logout prompt: {error}"),
        )
    })?;
    let answer = line.trim().to_ascii_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        Err(GcalError::setup(
            "not_logged_in",
            "logout cancelled".to_string(),
        ))
    }
}

fn print_report(account: &str, json: bool) {
    if json {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "account": account,
                "client": CLIENT,
                "revoke_url": REVOKE_URL,
            })
        );
        ui::flush_stdout();
        return;
    }
    let styler = Styler::detect();
    println!(
        "{} removed bob-gcal Calendar grant for {account}",
        styler.green("✓")
    );
    println!("Revoke Bob's Calendar access at {REVOKE_URL}");
    ui::flush_stdout();
}
