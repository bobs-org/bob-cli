//! Shared Task Card Cancel / cancel-label vectors.
//!
//! The pure engine lands in phase `cancel-engine`. This module hosts the
//! fixture smoke test so `tests/fixtures/task_cancel/vectors.json` cannot
//! rot before then.

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Vectors {
        cancel: Vec<serde_json::Value>,
        prune: Vec<serde_json::Value>,
        rule: Vec<serde_json::Value>,
        key: Vec<serde_json::Value>,
        config: Vec<serde_json::Value>,
    }

    fn load_vectors() -> Vectors {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/task_cancel/vectors.json"
        ))
        .expect("task_cancel vectors parse")
    }

    #[test]
    fn vectors_file_has_required_sections() {
        let vectors = load_vectors();
        assert!(
            !vectors.cancel.is_empty(),
            "cancel section must not be empty"
        );
        assert!(!vectors.prune.is_empty(), "prune section must not be empty");
        assert!(!vectors.rule.is_empty(), "rule section must not be empty");
        assert!(!vectors.key.is_empty(), "key section must not be empty");
        assert!(
            !vectors.config.is_empty(),
            "config section must not be empty"
        );
    }
}
