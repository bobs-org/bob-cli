use sha2::{Digest, Sha256};

use super::fields::{FieldProvenance, Fields};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewState {
    Reflected,
    Drafted,
    Edited,
    Empty,
}

impl ReviewState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reflected => "reflected",
            Self::Drafted => "drafted",
            Self::Edited => "edited",
            Self::Empty => "empty",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SummaryState {
    Drafted,
    Edited,
    Empty,
}

impl SummaryState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Drafted => "drafted",
            Self::Edited => "edited",
            Self::Empty => "empty",
        }
    }
}

/// The shared historical digest recipe: normalize line endings, strip trailing
/// whitespace per line, trim the joined result, and retain 16 lowercase hex.
pub(crate) fn digest(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<String> = normalized
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect();
    let canonical = lines.join("\n").trim().to_owned();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hex = hex::encode(hasher.finalize());
    format!("sha256:{}", &hex[..16])
}

pub(crate) fn summary_state(fields: &Fields) -> SummaryState {
    let Some(summary) = fields
        .summary
        .value
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return SummaryState::Empty;
    };
    if fields.summary.provenance == FieldProvenance::LegacyReview {
        if fields.review_digest.value.as_deref()
            == Some(&digest(&fields.review_body))
        {
            SummaryState::Drafted
        } else {
            SummaryState::Edited
        }
    } else if fields.summary_digest.value.as_deref() == Some(&digest(summary)) {
        SummaryState::Drafted
    } else {
        SummaryState::Edited
    }
}

pub(crate) fn evaluate(fields: &Fields, has_review: bool) -> ReviewState {
    if fields
        .reflection
        .value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return ReviewState::Reflected;
    }
    let summary_present = fields
        .summary
        .value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let review_present = has_review && !fields.review_body.trim().is_empty();
    if !summary_present && !review_present {
        return ReviewState::Empty;
    }
    let summary_matches =
        !summary_present || summary_state(fields) == SummaryState::Drafted;
    let review_matches = !review_present
        || fields.review_digest.value.as_deref()
            == Some(&digest(&fields.review_body));
    if summary_matches && review_matches {
        ReviewState::Drafted
    } else {
        ReviewState::Edited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_shared_machine_readable_vectors() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/intentions/digest-vectors.json"
        ))
        .unwrap();
        for vector in fixture["vectors"].as_array().unwrap() {
            let input = vector["input"].as_str().unwrap();
            assert_eq!(
                digest(input),
                vector["digest"].as_str().unwrap(),
                "{}",
                vector["name"]
            );
        }
    }
}
