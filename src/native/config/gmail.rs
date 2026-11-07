//! Parsed `gmail:` settings before command-level defaults are applied.

use std::path::Path;

use super::ConfigError;

/// Parsed `gmail:` settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GmailSettings {
    pub(crate) account: Option<String>,
    pub(crate) body: Option<GmailBodyMode>,
    pub(crate) folder: Option<String>,
    pub(crate) link_label: Option<String>,
    pub(crate) never_save: GmailNeverSave,
    pub(crate) pull: Option<GmailPullSettings>,
    pub(crate) signature: Option<GmailSignature>,
    pub(crate) sync: Option<GmailSyncSettings>,
    pub(crate) timeout_secs: Option<u64>,
    pub(crate) writer: Option<String>,
}

/// Which message bodies `gmail-save` keeps by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GmailBodyMode {
    #[default]
    Full,
    Headers,
}

/// Which signature `gmail-compose` appends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GmailSignature {
    #[default]
    Gmail,
    None,
}

/// `gmail.never_save` blocklists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GmailNeverSave {
    pub(crate) from: Vec<String>,
    pub(crate) labels: Vec<String>,
}

/// `gmail.pull` block, the older spelling of `gmail.sync`.
///
/// `label` is `None` when the key is unset or null and `Some("")` when it
/// is explicitly empty, so `gmail.sync.label` can fall back to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GmailPullSettings {
    pub(crate) inbox: Option<String>,
    pub(crate) label: Option<String>,
}

/// `gmail.sync` block. Unset keys fall back to `gmail.pull` and then to
/// the defaults when the command-level config resolves.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GmailSyncSettings {
    /// `None` when unset or null; `Some("")` disables sync.
    pub(crate) label: Option<String>,
    /// `None` when unset, null, or empty.
    pub(crate) inbox: Option<String>,
    /// `None` when unset or null; validated entries otherwise.
    pub(crate) route_prefixes: Option<Vec<String>>,
    pub(crate) reply_wait_days: Option<u32>,
}

impl Default for GmailPullSettings {
    fn default() -> Self {
        Self {
            inbox: Some("gmail_inbox.md".to_string()),
            label: Some("bob".to_string()),
        }
    }
}

pub(crate) fn load_gmail_config(
    path: &Path,
) -> Result<GmailSettings, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GmailSettings::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_gmail_config(&text, path)
}

pub(crate) fn parse_gmail_config(
    text: &str,
    path: &Path,
) -> Result<GmailSettings, ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            ConfigError::Invalid(format!("parse {}: {error}", path.display()))
        })?;
    parse_gmail_value(config.gmail, path)
}

fn parse_gmail_value(
    gmail: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<GmailSettings, ConfigError> {
    let path_display = path.display();
    let Some(gmail) = gmail else {
        return Ok(GmailSettings::default());
    };
    if gmail.is_null() {
        return Ok(GmailSettings::default());
    }
    let serde_yaml::Value::Mapping(ref mapping) = gmail else {
        return Err(ConfigError::Invalid(format!(
            "gmail in {path_display} must be a mapping; got {}",
            render_scalar(&gmail)
        )));
    };

    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };

    Ok(GmailSettings {
        account: optional_string("account", get("account"), path)?,
        body: parse_body(get("body"), path)?,
        folder: optional_string("folder", get("folder"), path)?,
        link_label: optional_string("link_label", get("link_label"), path)?,
        never_save: parse_never_save(get("never_save"), path)?,
        pull: parse_pull(get("pull"), path)?,
        signature: parse_signature(get("signature"), path)?,
        sync: parse_sync(get("sync"), path)?,
        timeout_secs: parse_timeout(get("timeout_secs"), path)?,
        writer: optional_string("writer", get("writer"), path)?,
    })
}

fn optional_string(
    name: &str,
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    match value {
        serde_yaml::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Ok(Some(trimmed.to_string()))
        }
        other => Err(ConfigError::Invalid(format!(
            "gmail.{name} in {} must be a string; got {}",
            path.display(),
            render_scalar(&other)
        ))),
    }
}

fn parse_body(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<GmailBodyMode>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let serde_yaml::Value::String(text) = value else {
        return Err(ConfigError::Invalid(format!(
            "gmail.body in {} must be full or headers; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    match text.trim() {
        "full" => Ok(Some(GmailBodyMode::Full)),
        "headers" => Ok(Some(GmailBodyMode::Headers)),
        other => Err(ConfigError::Invalid(format!(
            "gmail.body in {} must be full or headers; got {other:?}",
            path.display()
        ))),
    }
}

fn parse_signature(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<GmailSignature>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let serde_yaml::Value::String(text) = value else {
        return Err(ConfigError::Invalid(format!(
            "gmail.signature in {} must be gmail or none; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    match text.trim() {
        "gmail" => Ok(Some(GmailSignature::Gmail)),
        "none" => Ok(Some(GmailSignature::None)),
        other => Err(ConfigError::Invalid(format!(
            "gmail.signature in {} must be gmail or none; got {other:?}",
            path.display()
        ))),
    }
}

fn parse_timeout(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<u64>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = match &value {
        serde_yaml::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                int
            } else if let Some(uint) = number.as_u64() {
                i64::try_from(uint).map_err(|_| {
                    ConfigError::Invalid(format!(
                        "gmail.timeout_secs in {} must be an integer > 0; got {value:?}",
                        path.display()
                    ))
                })?
            } else {
                return Err(ConfigError::Invalid(format!(
                    "gmail.timeout_secs in {} must be an integer > 0; got {value:?}",
                    path.display()
                )));
            }
        }
        other => {
            return Err(ConfigError::Invalid(format!(
                "gmail.timeout_secs in {} must be an integer > 0; got {}",
                path.display(),
                render_scalar(other)
            )));
        }
    };
    if number <= 0 {
        return Err(ConfigError::Invalid(format!(
            "gmail.timeout_secs in {} must be an integer > 0; got {number}",
            path.display()
        )));
    }
    u64::try_from(number).map(Some).map_err(|_| {
        ConfigError::Invalid(format!(
            "gmail.timeout_secs in {} must be an integer > 0; got {number}",
            path.display()
        ))
    })
}

fn parse_never_save(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<GmailNeverSave, ConfigError> {
    let Some(value) = value else {
        return Ok(GmailNeverSave::default());
    };
    if value.is_null() {
        return Ok(GmailNeverSave::default());
    }
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "gmail.never_save in {} must be a mapping; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };
    Ok(GmailNeverSave {
        from: parse_string_list("never_save.from", get("from"), path)?,
        labels: parse_string_list("never_save.labels", get("labels"), path)?,
    })
}

fn parse_pull(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<GmailPullSettings>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "gmail.pull in {} must be a mapping; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };
    let inbox = match get("inbox") {
        None | Some(serde_yaml::Value::Null) => {
            Some("gmail_inbox.md".to_string())
        }
        Some(serde_yaml::Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "gmail.pull.inbox in {} must be a string; got {}",
                path.display(),
                render_scalar(&other)
            )));
        }
    };
    let label = label_setting("pull.label", get("label"), path)?;
    Ok(Some(GmailPullSettings { inbox, label }))
}

/// A label key: `None` when unset or null, `Some("")` when explicitly
/// empty (which disables the command), else the trimmed name.
fn label_setting(
    name: &str,
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<String>, ConfigError> {
    match value {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(serde_yaml::Value::String(text)) => {
            Ok(Some(text.trim().to_string()))
        }
        Some(other) => Err(ConfigError::Invalid(format!(
            "gmail.{name} in {} must be a string; got {}",
            path.display(),
            render_scalar(&other)
        ))),
    }
}

fn parse_sync(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<GmailSyncSettings>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "gmail.sync in {} must be a mapping; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };
    Ok(Some(GmailSyncSettings {
        label: label_setting("sync.label", get("label"), path)?,
        inbox: optional_string("sync.inbox", get("inbox"), path)?,
        route_prefixes: parse_route_prefixes(get("route_prefixes"), path)?,
        reply_wait_days: parse_reply_wait_days(get("reply_wait_days"), path)?,
    }))
}

/// `gmail.sync.route_prefixes`: distinct non-empty strings without `/` or
/// whitespace. Unset or null is `None`; `[]` is no-prefix mode.
fn parse_route_prefixes(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<Vec<String>>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let invalid = |detail: String| {
        ConfigError::Invalid(format!(
            "gmail.sync.route_prefixes in {} {detail}",
            path.display()
        ))
    };
    let serde_yaml::Value::Sequence(items) = value else {
        return Err(invalid(format!(
            "must be a sequence of quoted strings such as [\"#\", \"+\"]; got {}",
            render_scalar(&value)
        )));
    };
    let mut prefixes: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let serde_yaml::Value::String(prefix) = item else {
            return Err(invalid(format!(
                "must list strings; got {}",
                render_scalar(&item)
            )));
        };
        if prefix.is_empty() {
            return Err(invalid("must not list an empty prefix".to_string()));
        }
        if prefix.contains('/') || prefix.chars().any(char::is_whitespace) {
            return Err(invalid(format!(
                "entries must not contain '/' or whitespace; got {prefix:?}"
            )));
        }
        if prefixes.contains(&prefix) {
            return Err(invalid(format!(
                "entries must be distinct; {prefix:?} repeats"
            )));
        }
        prefixes.push(prefix);
    }
    Ok(Some(prefixes))
}

fn parse_reply_wait_days(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<u32>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let invalid = || {
        ConfigError::Invalid(format!(
            "gmail.sync.reply_wait_days in {} must be an integer >= 1; got {}",
            path.display(),
            render_scalar(&value)
        ))
    };
    let serde_yaml::Value::Number(number) = &value else {
        return Err(invalid());
    };
    match number.as_u64().map(u32::try_from) {
        Some(Ok(days)) if days >= 1 => Ok(Some(days)),
        _ => Err(invalid()),
    }
}

fn parse_string_list(
    name: &str,
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Vec<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let serde_yaml::Value::Sequence(items) = value else {
        return Err(ConfigError::Invalid(format!(
            "gmail.{name} in {} must be a sequence; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            serde_yaml::Value::String(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            other => {
                return Err(ConfigError::Invalid(format!(
                    "gmail.{name} in {} must list strings; got {}",
                    path.display(),
                    render_scalar(&other)
                )));
            }
        }
    }
    Ok(out)
}

fn render_scalar(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(text) => format!("{text:?}"),
        serde_yaml::Value::Number(number) => number.to_string(),
        serde_yaml::Value::Bool(flag) => flag.to_string(),
        serde_yaml::Value::Null => "null".to_string(),
        serde_yaml::Value::Sequence(_) => "sequence".to_string(),
        serde_yaml::Value::Mapping(_) => "mapping".to_string(),
        serde_yaml::Value::Tagged(tagged) => {
            format!("tagged {}", tagged.tag)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<GmailSettings, ConfigError> {
        parse_gmail_config(text, Path::new("/config.yml"))
    }

    #[test]
    fn missing_and_null_use_defaults() {
        assert_eq!(parse("").unwrap(), GmailSettings::default());
        assert_eq!(parse("gmail:\n").unwrap(), GmailSettings::default());
        assert_eq!(parse("gmail: null\n").unwrap(), GmailSettings::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let settings =
            parse("gmail:\n  extra: 1\n  account: you@example.com\n")
                .expect("unknown keys ignored");
        assert_eq!(settings.account.as_deref(), Some("you@example.com"));
    }

    #[test]
    fn parses_full_block() {
        let settings = parse(
            "gmail:\n  account: you@example.com\n  body: headers\n  folder: mail\n  link_label: \"x {subject}\"\n  never_save:\n    from: [a@example.com, \"@spam.com\"]\n    labels: [promo]\n  pull:\n    inbox: gmail_inbox.md\n    label: bob\n  signature: none\n  timeout_secs: 90\n  writer: host-a\n",
        )
        .expect("full block");
        assert_eq!(settings.account.as_deref(), Some("you@example.com"));
        assert_eq!(settings.body, Some(GmailBodyMode::Headers));
        assert_eq!(settings.folder.as_deref(), Some("mail"));
        assert_eq!(settings.link_label.as_deref(), Some("x {subject}"));
        assert_eq!(settings.never_save.from.len(), 2);
        assert_eq!(settings.never_save.labels, vec!["promo".to_string()]);
        let pull = settings.pull.expect("pull");
        assert_eq!(pull.inbox.as_deref(), Some("gmail_inbox.md"));
        assert_eq!(pull.label.as_deref(), Some("bob"));
        assert_eq!(settings.signature, Some(GmailSignature::None));
        assert_eq!(settings.timeout_secs, Some(90));
        assert_eq!(settings.writer.as_deref(), Some("host-a"));
    }

    #[test]
    fn rejects_bad_values_with_key_names() {
        for text in [
            "gmail:\n  body: everything\n",
            "gmail:\n  body: 1\n",
            "gmail:\n  signature: outlook\n",
            "gmail:\n  timeout_secs: soon\n",
            "gmail:\n  timeout_secs: 0\n",
            "gmail:\n  timeout_secs: -3\n",
            "gmail:\n  never_save: [a]\n",
            "gmail:\n  never_save:\n    from: not-a-list\n",
            "gmail:\n  pull: bob\n",
            "gmail:\n  pull:\n    inbox: 1\n",
        ] {
            let error = parse(text).expect_err(text);
            assert!(
                error.message().contains("gmail."),
                "{text} names the key: {error}"
            );
        }
    }

    #[test]
    fn empty_pull_label_is_kept_apart_from_an_unset_one() {
        let settings =
            parse("gmail:\n  pull:\n    label: \"\"\n").expect("empty label");
        let pull = settings.pull.expect("pull block");
        assert_eq!(pull.label.as_deref(), Some(""));
        let settings =
            parse("gmail:\n  pull:\n    inbox: x.md\n").expect("no label");
        assert_eq!(settings.pull.expect("pull block").label, None);
    }

    #[test]
    fn missing_and_null_sync_blocks_are_unset() {
        assert_eq!(parse("gmail:\n  account: a@b.c\n").unwrap().sync, None);
        assert_eq!(parse("gmail:\n  sync:\n").unwrap().sync, None);
        assert_eq!(
            parse("gmail:\n  sync:\n    label:\n    inbox:\n    route_prefixes:\n    reply_wait_days:\n")
                .unwrap()
                .sync,
            Some(GmailSyncSettings::default())
        );
    }

    #[test]
    fn parses_the_sync_block() {
        let settings = parse(
            "gmail:\n  sync:\n    label: bob\n    inbox: gmail_inbox.md\n    route_prefixes: [\"#\", \"+\"]\n    reply_wait_days: 5\n",
        )
        .expect("sync block");
        assert_eq!(
            settings.sync,
            Some(GmailSyncSettings {
                label: Some("bob".to_string()),
                inbox: Some("gmail_inbox.md".to_string()),
                route_prefixes: Some(vec!["#".to_string(), "+".to_string()]),
                reply_wait_days: Some(5),
            })
        );
    }

    #[test]
    fn sync_label_keeps_an_explicit_empty_value() {
        let sync = parse("gmail:\n  sync:\n    label: \"  \"\n")
            .unwrap()
            .sync
            .expect("sync block");
        assert_eq!(sync.label.as_deref(), Some(""));
    }

    #[test]
    fn empty_route_prefixes_is_no_prefix_mode() {
        let sync = parse("gmail:\n  sync:\n    route_prefixes: []\n")
            .unwrap()
            .sync
            .expect("sync block");
        assert_eq!(sync.route_prefixes, Some(Vec::new()));
    }

    #[test]
    fn rejects_bad_sync_values_with_key_names() {
        for (text, key) in [
            ("gmail:\n  sync: bob\n", "gmail.sync "),
            ("gmail:\n  sync: [bob]\n", "gmail.sync "),
            ("gmail:\n  sync:\n    label: 1\n", "gmail.sync.label"),
            ("gmail:\n  sync:\n    label: [bob]\n", "gmail.sync.label"),
            ("gmail:\n  sync:\n    inbox: 3\n", "gmail.sync.inbox"),
            (
                "gmail:\n  sync:\n    route_prefixes: \"#\"\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    route_prefixes: [1]\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    route_prefixes: [\"\"]\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    route_prefixes: [\"#/\"]\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    route_prefixes: [\"a b\"]\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    route_prefixes: [\" #\"]\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    route_prefixes: [\"#\", \"#\"]\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    reply_wait_days: 0\n",
                "gmail.sync.reply_wait_days",
            ),
            (
                "gmail:\n  sync:\n    reply_wait_days: -2\n",
                "gmail.sync.reply_wait_days",
            ),
            (
                "gmail:\n  sync:\n    reply_wait_days: 1.5\n",
                "gmail.sync.reply_wait_days",
            ),
            (
                "gmail:\n  sync:\n    reply_wait_days: \"3\"\n",
                "gmail.sync.reply_wait_days",
            ),
            (
                "gmail:\n  sync:\n    reply_wait_days: true\n",
                "gmail.sync.reply_wait_days",
            ),
            (
                "gmail:\n  sync:\n    reply_wait_days: 99999999999\n",
                "gmail.sync.reply_wait_days",
            ),
        ] {
            let error = parse(text).expect_err(text);
            assert!(
                error.message().contains(key),
                "{text} names {key}: {error}"
            );
        }
    }

    #[test]
    fn an_unquoted_hash_prefix_is_a_yaml_error() {
        // A bare `#` starts a YAML comment, which leaves the flow sequence
        // unterminated; the docs tell users to quote prefixes.
        assert!(parse("gmail:\n  sync:\n    route_prefixes: [#, +]\n").is_err());
    }
}
