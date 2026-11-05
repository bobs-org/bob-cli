//! Shared row types for Google integration setup checks.

pub(crate) struct Check {
    pub(crate) id: &'static str,
    pub(crate) status: CheckStatus,
    pub(crate) summary: String,
    pub(crate) fix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        self.as_str()
    }
}
