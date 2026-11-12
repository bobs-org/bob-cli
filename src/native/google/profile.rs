/// Service-specific values used by the shared Google OAuth and gog code.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GoogleService {
    pub(crate) command: &'static str,
    pub(crate) client_bucket: &'static str,
    pub(crate) product_name: &'static str,
    pub(crate) api_name: &'static str,
    pub(crate) service_name: &'static str,
    pub(crate) gog_service: &'static str,
    pub(crate) api_service_id: &'static str,
    pub(crate) api_console_url: &'static str,
    pub(crate) scope_description: &'static str,
    pub(crate) scope_prompt: &'static str,
    pub(crate) gog_override_env: &'static str,
    pub(crate) browser_override_env: &'static str,
    pub(crate) assume_tty_env: &'static str,
    pub(crate) consent_secs_env: &'static str,
    pub(crate) gog_config_key: &'static str,
    pub(crate) timeout_config_key: &'static str,
    pub(crate) account_question: &'static str,
    service_matches: fn(&str) -> bool,
    scope_matches: fn(&str) -> bool,
}

impl GoogleService {
    pub(crate) fn account_has_scope(
        &self,
        services: &[String],
        scopes: &[String],
    ) -> bool {
        services.iter().any(|value| (self.service_matches)(value))
            || scopes.iter().any(|value| (self.scope_matches)(value))
    }
}

fn calendar_service(value: &str) -> bool {
    value == "calendar"
}

fn calendar_scope(value: &str) -> bool {
    value == "https://www.googleapis.com/auth/calendar.readonly"
        || value.contains("calendar.readonly")
        || value.ends_with("/calendar")
}

fn gmail_service(value: &str) -> bool {
    value == "gmail"
}

fn gmail_scope(value: &str) -> bool {
    value == "https://www.googleapis.com/auth/gmail.readonly"
        || value.contains("gmail.readonly")
        || value == "https://www.googleapis.com/auth/gmail.modify"
        || value.contains("gmail.modify")
}

pub(crate) const GCAL: GoogleService = GoogleService {
    command: "bob gcal",
    client_bucket: "bob-gcal",
    product_name: "Google Calendar",
    api_name: "Google Calendar",
    service_name: "Calendar",
    gog_service: "calendar",
    api_service_id: "calendar-json.googleapis.com",
    api_console_url: "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com",
    scope_description: "calendar.readonly",
    scope_prompt: "See and download any calendar you can access",
    gog_override_env: "BOB_GCAL_GOG",
    browser_override_env: "BOB_GCAL_BROWSER",
    assume_tty_env: "BOB_GCAL_ASSUME_TTY",
    consent_secs_env: "BOB_GCAL_CONSENT_SECS",
    gog_config_key: "gcal.gog",
    timeout_config_key: "gcal.timeout_secs",
    account_question: "Which Google account should Bob read?",
    service_matches: calendar_service,
    scope_matches: calendar_scope,
};

pub(crate) const GMAIL: GoogleService = GoogleService {
    command: "bob gmail",
    client_bucket: "bob-gmail",
    product_name: "Gmail",
    api_name: "Google Gmail",
    service_name: "Gmail",
    gog_service: "gmail",
    api_service_id: "gmail.googleapis.com",
    api_console_url:
        "https://console.cloud.google.com/apis/library/gmail.googleapis.com",
    scope_description: "gmail.readonly",
    scope_prompt: "Read your email messages",
    gog_override_env: "BOB_GMAIL_GOG",
    browser_override_env: "BOB_GMAIL_BROWSER",
    assume_tty_env: "BOB_GMAIL_ASSUME_TTY",
    consent_secs_env: "BOB_GMAIL_CONSENT_SECS",
    gog_config_key: "gmail.gog",
    timeout_config_key: "gmail.timeout_secs",
    account_question: "Which Google account should Bob use for Gmail?",
    service_matches: gmail_service,
    scope_matches: gmail_scope,
};
