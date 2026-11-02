//! Stats request/metric clock. Distinct from ledger `SessionClock`, which
//! classifies a single row and includes Placeholder.

/// Lens for unpinned Pomodoro metrics and the top-level clock object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum StatsClock {
    #[default]
    Worked,
    OffClock,
    Total,
}

impl StatsClock {
    pub(crate) const ALL: [Self; 3] =
        [Self::Worked, Self::OffClock, Self::Total];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Worked => "worked",
            Self::OffClock => "off-clock",
            Self::Total => "total",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Worked => "Worked",
            Self::OffClock => "Off-clock",
            Self::Total => "Total",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Worked => "Closed Pomodoro minutes on the work clock.",
            Self::OffClock => "Closed Pomodoro minutes off the clock.",
            Self::Total => {
                "Closed session minutes on both clocks. The total sums session \
                 minutes and is not occupied or free time."
            }
        }
    }

    /// Config and JSON accept only canonical values.
    pub(crate) fn parse_canonical(text: &str) -> Option<Self> {
        match text.trim() {
            "worked" => Some(Self::Worked),
            "off-clock" => Some(Self::OffClock),
            "total" => Some(Self::Total),
            _ => None,
        }
    }

    /// CLI-only hidden aliases: `on`, `off`, `untracked`, `all`.
    pub(crate) fn parse_cli(text: &str) -> Option<Self> {
        match text.trim() {
            "on" => Some(Self::Worked),
            "off" | "untracked" => Some(Self::OffClock),
            "all" => Some(Self::Total),
            other => Self::parse_canonical(other),
        }
    }

    pub(crate) fn metric_label(self, id: &str, configured: &str) -> String {
        match (id, self) {
            ("worked", Self::Worked) => "Worked".to_string(),
            ("worked", Self::OffClock) => "Off-clock".to_string(),
            ("worked", Self::Total) => "Total".to_string(),
            ("sessions", Self::Worked) => configured.to_string(),
            ("sessions", Self::OffClock) => "Off-clock sessions".to_string(),
            ("sessions", Self::Total) => "Total sessions".to_string(),
            (_, Self::Worked) => configured.to_string(),
            (_, Self::OffClock) => format!("{configured} · off-clock"),
            (_, Self::Total) => format!("{configured} · total"),
        }
    }

    pub(crate) fn is_neutral(self) -> bool {
        matches!(self, Self::OffClock | Self::Total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_and_cli_aliases_parse() {
        assert_eq!(
            StatsClock::parse_canonical("worked"),
            Some(StatsClock::Worked)
        );
        assert_eq!(
            StatsClock::parse_canonical("off-clock"),
            Some(StatsClock::OffClock)
        );
        assert_eq!(
            StatsClock::parse_canonical("total"),
            Some(StatsClock::Total)
        );
        assert_eq!(StatsClock::parse_canonical("on"), None);
        assert_eq!(StatsClock::parse_canonical("off"), None);
        assert_eq!(StatsClock::parse_cli("on"), Some(StatsClock::Worked));
        assert_eq!(StatsClock::parse_cli("off"), Some(StatsClock::OffClock));
        assert_eq!(
            StatsClock::parse_cli("untracked"),
            Some(StatsClock::OffClock)
        );
        assert_eq!(StatsClock::parse_cli("all"), Some(StatsClock::Total));
        assert_eq!(StatsClock::parse_cli("worked"), Some(StatsClock::Worked));
        assert_eq!(
            StatsClock::ALL.map(StatsClock::as_str),
            ["worked", "off-clock", "total"]
        );
    }

    #[test]
    fn labels_follow_the_id_table() {
        assert_eq!(
            StatsClock::Worked.metric_label("worked", "Worked"),
            "Worked"
        );
        assert_eq!(
            StatsClock::OffClock.metric_label("worked", "Worked"),
            "Off-clock"
        );
        assert_eq!(StatsClock::Total.metric_label("worked", "Worked"), "Total");
        assert_eq!(
            StatsClock::Worked.metric_label("sessions", "Sessions"),
            "Sessions"
        );
        assert_eq!(
            StatsClock::OffClock.metric_label("sessions", "Sessions"),
            "Off-clock sessions"
        );
        assert_eq!(
            StatsClock::Total.metric_label("sessions", "Sessions"),
            "Total sessions"
        );
        assert_eq!(
            StatsClock::Worked.metric_label("sase_time", "SASE time"),
            "SASE time"
        );
        assert_eq!(
            StatsClock::OffClock.metric_label("sase_time", "SASE time"),
            "SASE time · off-clock"
        );
        assert_eq!(
            StatsClock::Total.metric_label("sase_time", "SASE time"),
            "SASE time · total"
        );
        assert!(StatsClock::OffClock.is_neutral());
        assert!(StatsClock::Total.is_neutral());
        assert!(!StatsClock::Worked.is_neutral());
    }
}
