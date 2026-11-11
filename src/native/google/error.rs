/// A typed Google integration failure with its process exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoogleError {
    kind: String,
    message: String,
    hint: Option<String>,
    warnings: Vec<String>,
    exit_code: i32,
}

impl GoogleError {
    /// A runtime failure (exit 1).
    pub(crate) fn runtime(kind: &str, message: String) -> Self {
        Self {
            kind: kind.to_string(),
            message,
            hint: None,
            warnings: Vec::new(),
            exit_code: 1,
        }
    }

    /// A usage or setup error (exit 2).
    pub(crate) fn setup(kind: &str, message: String) -> Self {
        Self {
            kind: kind.to_string(),
            message,
            hint: None,
            warnings: Vec::new(),
            exit_code: 2,
        }
    }

    /// Attach a `hint: …` line to the report.
    pub(crate) fn with_hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.to_string());
        self
    }

    /// Carry warnings that must print with the error, so a failure
    /// never silently drops them.
    pub(crate) fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings = warnings;
        self
    }

    pub(crate) fn kind(&self) -> &str {
        &self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub(crate) fn exit_code(&self) -> i32 {
        self.exit_code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_exit_codes() {
        let error = GoogleError::runtime("network", "down".to_string());
        assert_eq!(error.exit_code(), 1);
        assert_eq!(error.kind(), "network");
        assert_eq!(error.message(), "down");
        assert_eq!(error.hint(), None);

        let error = GoogleError::setup("config", "bad".to_string())
            .with_hint("run `bob gcal login`");
        assert_eq!(error.exit_code(), 2);
        assert_eq!(error.hint(), Some("run `bob gcal login`"));
    }
}
