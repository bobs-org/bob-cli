//! Resolved `bob gcal` configuration: the `gcal:` config section.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::native::{config as bob_config, env as bob_env};

use super::{render::TASKS_SIGNIFIER_CHARS, GcalError};

/// Bob glyphs that must never appear in a calendar mark, ignoring U+FE0F.
const BOB_MARK_RESERVED_CHARS: &[char] = &[
    '📹', '📍', '🍅', '✎', '↤', '↦', '↻', '⚠', '⚑', '≡', '🥀', '·', '●', '—',
    '✓', '✗', '✔', '↺',
];

pub(crate) const DEFAULT_ROOT: &str = "meet_inbox";
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Most bullet lines per Event Task, overflow line included.
pub(crate) const DEFAULT_DESCRIPTION_LINES: u64 = 8;
/// One content line plus the overflow line.
pub(crate) const MIN_DESCRIPTION_LINES: u64 = 2;
pub(crate) const ASSUME_TTY_ENV: &str = "BOB_GCAL_ASSUME_TTY";

/// Resolved `bob gcal` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GcalConfig {
    account: Option<String>,
    writer: Option<String>,
    root: String,
    calendars: Vec<bob_config::GcalCalendarSettings>,
    #[allow(dead_code)]
    colors: BTreeMap<String, String>,
    #[allow(dead_code)]
    tentative: bob_config::GcalTentative,
    #[allow(dead_code)]
    ack: bob_config::GcalAckMode,
    description: bob_config::GcalDescriptionMode,
    description_lines: usize,
    gog: Option<PathBuf>,
    timeout_secs: u64,
}

impl Default for GcalConfig {
    fn default() -> Self {
        Self::from_settings(bob_config::GcalSettings::default())
            .expect("default gcal config resolves")
    }
}

impl GcalConfig {
    /// Resolve the config file, apply defaults, and validate types.
    ///
    /// Every failure is a setup error (exit 2).
    pub(crate) fn resolve() -> Result<Self, GcalError> {
        Self::resolve_at(&bob_config::config_path())
    }

    fn resolve_at(path: &Path) -> Result<Self, GcalError> {
        let settings = bob_config::load_gcal_config(path).map_err(|error| {
            let message = match error {
                bob_config::ConfigError::Read(message)
                | bob_config::ConfigError::Invalid(message) => message,
            };
            GcalError::setup(
                "config",
                format!("read the gcal config: {message}"),
            )
            .with_hint(
                "set BOB_CONFIG_FILE or run 'chezmoi apply \
                         ~/.config/bob/config.yml'",
            )
        })?;
        Self::from_settings(settings)
    }

    pub(crate) fn from_settings(
        settings: bob_config::GcalSettings,
    ) -> Result<Self, GcalError> {
        if let Some(account) = settings.account.as_deref()
            && !account.contains('@')
        {
            return Err(GcalError::setup(
                "config",
                format!("gcal.account {account:?} must contain '@'"),
            ));
        }
        let timeout_secs =
            settings.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if timeout_secs == 0 {
            return Err(GcalError::setup(
                "config",
                "gcal.timeout_secs must be greater than 0".to_string(),
            ));
        }
        let calendars = settings.calendars.unwrap_or_else(|| {
            vec![bob_config::GcalCalendarSettings {
                id: "primary".to_string(),
                mark: None,
                root: None,
            }]
        });
        if calendars.is_empty() {
            return Err(GcalError::setup(
                "config",
                "gcal.calendars must not be empty".to_string(),
            ));
        }
        validate_calendars(&calendars, settings.account.as_deref())?;
        let description_lines = settings
            .description_lines
            .unwrap_or(DEFAULT_DESCRIPTION_LINES);
        if description_lines < MIN_DESCRIPTION_LINES {
            return Err(GcalError::setup(
                "config",
                format!(
                    "gcal.description_lines must be an integer >= {MIN_DESCRIPTION_LINES}; got {description_lines}"
                ),
            ));
        }
        let description_lines_usize = usize::try_from(description_lines)
            .map_err(|_| {
                GcalError::setup(
                    "config",
                    format!(
                        "gcal.description_lines must be an integer >= {MIN_DESCRIPTION_LINES}; got {description_lines}"
                    ),
                )
            })?;
        Ok(Self {
            account: settings.account,
            writer: settings.writer,
            root: settings.root.unwrap_or_else(|| DEFAULT_ROOT.to_string()),
            calendars,
            colors: settings.colors,
            tentative: settings
                .tentative
                .unwrap_or(bob_config::GcalTentative::Context),
            ack: settings.ack.unwrap_or_default(),
            description: settings.description.unwrap_or_default(),
            description_lines: description_lines_usize,
            gog: settings.gog,
            timeout_secs,
        })
    }

    pub(crate) fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub(crate) fn writer(&self) -> Option<&str> {
        self.writer.as_deref()
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn calendars(&self) -> &[bob_config::GcalCalendarSettings] {
        &self.calendars
    }

    #[allow(dead_code)]
    pub(crate) fn colors(&self) -> &BTreeMap<String, String> {
        &self.colors
    }

    #[allow(dead_code)]
    pub(crate) fn tentative(&self) -> bob_config::GcalTentative {
        self.tentative
    }

    pub(crate) fn ack_mode(&self) -> bob_config::GcalAckMode {
        self.ack
    }

    pub(crate) fn description_lines(&self) -> usize {
        self.description_lines
    }

    pub(crate) fn description_bullets(&self) -> bool {
        self.description == bob_config::GcalDescriptionMode::Bullets
    }

    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    pub(crate) fn gog(&self) -> Option<&Path> {
        self.gog.as_deref()
    }

    /// Calendar root: per-calendar override, else the default root.
    pub(crate) fn root_for(&self, calendar_id: &str) -> &str {
        self.calendars
            .iter()
            .find(|calendar| calendar.id == calendar_id)
            .and_then(|calendar| calendar.root.as_deref())
            .unwrap_or(&self.root)
    }

    /// Configured per-calendar override, without falling through to the default.
    pub(crate) fn calendar_root(&self, calendar_id: &str) -> Option<&str> {
        self.calendars
            .iter()
            .find(|calendar| calendar.id == calendar_id)
            .and_then(|calendar| calendar.root.as_deref())
    }

    /// Root mapped to a Google event colour id, if configured.
    pub(crate) fn color_root(&self, color_id: &str) -> Option<&str> {
        self.colors.iter().find_map(|(key, root)| {
            (bob_config::color_id_for_key(key) == Some(color_id))
                .then_some(root.as_str())
        })
    }

    /// Whether `calendar_id` is in the configured sync set.
    ///
    /// `primary` matches the configured id `primary`.
    pub(crate) fn syncs(&self, calendar_id: &str, primary: bool) -> bool {
        self.calendars.iter().any(|calendar| {
            calendar.id == calendar_id
                || (calendar.id == "primary"
                    && (calendar_id == "primary" || primary))
        })
    }

    /// Configured mark for `calendar_id`, if that entry has one.
    pub(crate) fn mark_for(&self, calendar_id: &str) -> Option<&str> {
        self.calendars
            .iter()
            .find(|calendar| calendar.id == calendar_id)
            .and_then(|calendar| calendar.mark.as_deref())
    }
}

fn is_unmarked_primary_id(id: &str, account: Option<&str>) -> bool {
    id == "primary"
        || account.is_some_and(|account| id.eq_ignore_ascii_case(account))
}

fn validate_calendars(
    calendars: &[bob_config::GcalCalendarSettings],
    account: Option<&str>,
) -> Result<(), GcalError> {
    let mut seen_ids = BTreeMap::new();
    let mut seen_marks = BTreeMap::new();
    for (index, calendar) in calendars.iter().enumerate() {
        if let Some(first) = seen_ids.insert(calendar.id.clone(), index) {
            return Err(GcalError::setup(
                "config",
                format!(
                    "gcal.calendars[{index}].id {:?} is already used by gcal.calendars[{first}]",
                    calendar.id
                ),
            ));
        }
        match calendar.mark.as_deref() {
            None if is_unmarked_primary_id(&calendar.id, account) => {}
            None => return Err(needs_mark_error(index, &calendar.id)),
            Some(mark) => {
                validate_mark(index, mark)?;
                if let Some(first) = seen_marks.insert(mark.to_string(), index)
                {
                    return Err(GcalError::setup(
                        "config",
                        format!(
                            "gcal.calendars[{index}].mark {mark:?} is already used by gcal.calendars[{first}]"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn needs_mark_error(index: usize, id: &str) -> GcalError {
    GcalError::setup(
        "config",
        format!("gcal.calendars[{index}] {id} needs a mark"),
    )
    .with_hint(
        "add `mark: 🎂` (any short emoji) under that entry so its rows say which calendar they came from",
    )
}

pub(crate) fn needs_mark_setup_error(index: usize, id: &str) -> GcalError {
    needs_mark_error(index, id)
}

fn validate_mark(index: usize, mark: &str) -> Result<(), GcalError> {
    let error = |message: String| {
        GcalError::setup(
            "config",
            format!("gcal.calendars[{index}].mark {message}"),
        )
    };
    let scalars = mark.chars().count();
    let invalid_shape = scalars == 0
        || scalars > 8
        || mark.chars().any(|ch| {
            ch.is_ascii()
                || ch.is_whitespace()
                || ch.is_control()
                || ch == '\u{200B}'
        });
    if invalid_shape {
        return Err(error(format!(
            "{mark:?} must be 1–8 Unicode characters with no ASCII, whitespace, or control"
        )));
    }
    for ch in mark.chars().filter(|&ch| ch != '\u{FE0F}') {
        if TASKS_SIGNIFIER_CHARS.contains(&ch) {
            let reason = if matches!(ch, '📅' | '📆' | '🗓') {
                "Tasks due-date signifier"
            } else {
                "Tasks signifier"
            };
            return Err(error(format!(
                "{mark:?} is reserved ({reason}); choose another emoji"
            )));
        }
        if BOB_MARK_RESERVED_CHARS.contains(&ch) {
            return Err(error(format!(
                "{mark:?} is reserved (Bob glyph); choose another emoji"
            )));
        }
    }
    Ok(())
}

/// Whether stdin should be treated as a TTY for login prompts.
pub(crate) fn assume_tty() -> bool {
    matches!(
        bob_env::var(ASSUME_TTY_ENV).ok().as_deref(),
        Some("1") | Some("true")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> bob_config::GcalSettings {
        bob_config::GcalSettings {
            account: None,
            writer: None,
            root: None,
            calendars: None,
            colors: BTreeMap::new(),
            tentative: None,
            ack: None,
            description: None,
            description_lines: None,
            gog: None,
            timeout_secs: None,
        }
    }

    #[test]
    fn defaults_when_block_is_missing() {
        let config = GcalConfig::from_settings(settings()).expect("defaults");
        assert_eq!(config.root(), "meet_inbox");
        assert_eq!(config.calendars().len(), 1);
        assert_eq!(config.calendars()[0].id, "primary");
        assert_eq!(config.timeout(), Duration::from_secs(60));
        assert!(config.description_bullets());
        assert_eq!(config.description_lines(), 8);
        assert_eq!(config.ack_mode(), bob_config::GcalAckMode::External);
        assert!(config.syncs("primary", true));
        assert!(config.syncs("you@example.com", true));
        assert!(!config.syncs("work", false));
    }

    #[test]
    fn rejects_account_without_at() {
        let error = GcalConfig::from_settings(bob_config::GcalSettings {
            account: Some("not-an-email".to_string()),
            ..settings()
        })
        .expect_err("bad email");
        assert_eq!(error.exit_code(), 2);
        assert!(error.message().contains("gcal.account"));
    }

    #[test]
    fn description_keys_resolve_and_reject() {
        let config = GcalConfig::from_settings(bob_config::GcalSettings {
            description: Some(bob_config::GcalDescriptionMode::Off),
            description_lines: Some(3),
            ..settings()
        })
        .expect("explicit description keys");
        assert!(!config.description_bullets());
        assert_eq!(config.description_lines(), 3);
        let error = GcalConfig::from_settings(bob_config::GcalSettings {
            description_lines: Some(1),
            ..settings()
        })
        .expect_err("one line is not enough for content plus overflow");
        assert_eq!(error.exit_code(), 2);
        assert!(error.message().contains("gcal.description_lines"));
    }

    #[test]
    fn ack_key_resolves_to_external_by_default_or_off() {
        let config = GcalConfig::from_settings(bob_config::GcalSettings {
            ack: Some(bob_config::GcalAckMode::Off),
            ..settings()
        })
        .expect("ack config");
        assert_eq!(config.ack_mode(), bob_config::GcalAckMode::Off);
    }

    fn calendar(
        id: &str,
        mark: Option<&str>,
    ) -> bob_config::GcalCalendarSettings {
        bob_config::GcalCalendarSettings {
            id: id.to_string(),
            mark: mark.map(str::to_string),
            root: None,
        }
    }

    fn calendars_config(
        account: Option<&str>,
        calendars: Vec<bob_config::GcalCalendarSettings>,
    ) -> Result<GcalConfig, GcalError> {
        GcalConfig::from_settings(bob_config::GcalSettings {
            account: account.map(str::to_string),
            calendars: Some(calendars),
            ..settings()
        })
    }

    fn calendars_error(
        account: Option<&str>,
        calendars: Vec<bob_config::GcalCalendarSettings>,
    ) -> GcalError {
        calendars_config(account, calendars).expect_err("calendars invalid")
    }

    #[test]
    fn default_primary_may_be_unmarked() {
        let config = GcalConfig::from_settings(settings()).expect("defaults");
        assert_eq!(config.calendars()[0].id, "primary");
        assert_eq!(config.calendars()[0].mark, None);
        assert_eq!(config.mark_for("primary"), None);
    }

    #[test]
    fn account_email_may_be_unmarked() {
        let config = calendars_config(
            Some("you@example.com"),
            vec![calendar("YOU@example.com", None)],
        )
        .expect("account email is primary");
        assert_eq!(config.mark_for("YOU@example.com"), None);
    }

    #[test]
    fn zwj_mark_is_accepted() {
        let mark = "👨\u{200d}👩\u{200d}👧";
        let config = calendars_config(
            None,
            vec![
                calendar("primary", None),
                calendar("work@group.calendar.google.com", Some(mark)),
            ],
        )
        .expect("ZWJ mark");
        assert_eq!(
            config.mark_for("work@group.calendar.google.com"),
            Some(mark)
        );
    }

    #[test]
    fn missing_mark_on_non_primary_is_a_setup_error() {
        let error = calendars_error(
            None,
            vec![
                calendar("primary", None),
                calendar("work@group.calendar.google.com", None),
            ],
        );
        assert_eq!(error.exit_code(), 2);
        assert!(
            error.message().contains(
                "gcal.calendars[1] work@group.calendar.google.com needs a mark"
            ),
            "{}",
            error.message()
        );
        assert!(
            error.hint().is_some_and(|hint| hint.contains("mark: 🎂")),
            "{:?}",
            error.hint()
        );
    }

    #[test]
    fn bare_non_primary_string_needs_a_mark() {
        let error = calendars_error(None, vec![calendar("work", None)]);
        assert!(
            error
                .message()
                .contains("gcal.calendars[0] work needs a mark"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn ascii_whitespace_and_overlong_marks_are_rejected() {
        for mark in ["A", "cake", "🎂 🎉", "\u{200B}🎂", "🎂🎁🥳🎉🎊🎈🎀🎯🏆"]
        {
            let error = calendars_error(
                None,
                vec![calendar("work@group.calendar.google.com", Some(mark))],
            );
            assert!(
                error.message().contains("gcal.calendars[0].mark"),
                "{mark:?}: {}",
                error.message()
            );
            assert!(
                error.message().contains("1–8 Unicode characters"),
                "{mark:?}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn reserved_glyphs_are_rejected() {
        let calendar_date = calendars_error(
            None,
            vec![calendar("work@group.calendar.google.com", Some("📅"))],
        );
        assert!(
            calendar_date
                .message()
                .contains("reserved (Tasks due-date signifier)"),
            "{}",
            calendar_date.message()
        );
        let tomato = calendars_error(
            None,
            vec![calendar("work@group.calendar.google.com", Some("🍅"))],
        );
        assert!(
            tomato.message().contains("reserved (Bob glyph)"),
            "{}",
            tomato.message()
        );
        let with_vs = calendars_error(
            None,
            vec![calendar(
                "work@group.calendar.google.com",
                Some("📅\u{FE0F}"),
            )],
        );
        assert!(
            with_vs.message().contains("reserved"),
            "{}",
            with_vs.message()
        );
    }

    #[test]
    fn duplicate_mark_and_duplicate_id_are_rejected() {
        let mark = calendars_error(
            None,
            vec![
                calendar("primary", None),
                calendar("work@group.calendar.google.com", Some("🎂")),
                calendar("play@group.calendar.google.com", Some("🎂")),
            ],
        );
        assert!(
            mark.message().contains(
                "gcal.calendars[2].mark \"🎂\" is already used by gcal.calendars[1]"
            ),
            "{}",
            mark.message()
        );
        let id = calendars_error(
            None,
            vec![calendar("primary", None), calendar("primary", None)],
        );
        assert!(
            id.message().contains(
                "gcal.calendars[1].id \"primary\" is already used by gcal.calendars[0]"
            ),
            "{}",
            id.message()
        );
    }
}
