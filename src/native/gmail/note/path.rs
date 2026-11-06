//! Stable mail-note path allocation.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use super::text::subject_without_prefixes;
use super::NoteError;

const MAX_SLUG_CHARS: usize = 60;

/// Compute the birth path for a thread. Once saved, callers keep this path on
/// refresh even if the subject or the thread's timestamps change.
pub(crate) fn allocate(
    folder: &Path,
    subject: &str,
    first_message_at: DateTime<Utc>,
    timezone: Tz,
    occupied: &BTreeSet<PathBuf>,
) -> Result<PathBuf, NoteError> {
    validate_folder(folder)?;
    let local = first_message_at.with_timezone(&timezone);
    let directory = folder.join(local.format("%Y").to_string());
    let date = local.format("%y%m%d");
    let slug = slug(subject);
    for suffix in 1u32.. {
        let stem = if suffix == 1 {
            format!("{date}_{slug}")
        } else {
            format!("{date}_{slug}_{suffix}")
        };
        let candidate = directory.join(format!("{stem}.md"));
        if !occupied.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("the suffix counter cannot be exhausted")
}

pub(crate) fn validate_folder(folder: &Path) -> Result<(), NoteError> {
    if folder.as_os_str().is_empty()
        || folder.is_absolute()
        || folder
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(NoteError::InvalidFolder);
    }
    Ok(())
}

/// ASCII-fold and word-separate a subject into a filename slug.
pub(crate) fn slug(subject: &str) -> String {
    let subject = subject_without_prefixes(subject);
    let mut output = String::new();
    let mut separator = true;
    for character in subject.nfkd() {
        if is_combining_mark(character) {
            continue;
        }
        let folded = match character {
            'ß' | 'ẞ' => "ss",
            'ø' | 'Ø' => "o",
            'æ' | 'Æ' => "ae",
            'œ' | 'Œ' => "oe",
            'ł' | 'Ł' => "l",
            'đ' | 'Đ' => "d",
            'þ' | 'Þ' => "th",
            character if character.is_ascii_alphanumeric() => {
                let character = character.to_ascii_lowercase();
                if separator {
                    separator = false;
                }
                output.push(character);
                continue;
            }
            _ => "",
        };
        if !folded.is_empty() {
            output.push_str(folded);
            separator = false;
        } else if !separator {
            output.push('_');
            separator = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    if output.is_empty() {
        return "no_subject".to_string();
    }
    if output.chars().count() > MAX_SLUG_CHARS {
        let mut cut = output
            .char_indices()
            .nth(MAX_SLUG_CHARS)
            .map_or(output.len(), |(index, _)| index);
        if let Some(boundary) = output[..cut].rfind('_')
            && boundary > 0
        {
            cut = boundary;
        }
        output.truncate(cut);
        while output.ends_with('_') {
            output.pop();
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn strips_prefixes_and_slugifies_to_ascii() {
        assert_eq!(slug("Re: Café lease / renewal"), "cafe_lease_renewal");
        assert_eq!(slug("Fwd: 📬"), "no_subject");
    }

    #[test]
    fn truncates_at_a_word_boundary_and_numbers_collisions() {
        let long = format!("{} another", "a".repeat(58));
        assert_eq!(slug(&long), "a".repeat(58));
        let first_message_at =
            Utc.with_ymd_and_hms(2026, 9, 19, 13, 46, 32).unwrap();
        let occupied = BTreeSet::from([PathBuf::from(
            "mail/2026/260919_lease_renewal.md",
        )]);
        assert_eq!(
            allocate(
                Path::new("mail"),
                "Lease renewal",
                first_message_at,
                chrono_tz::America::New_York,
                &occupied,
            )
            .unwrap(),
            PathBuf::from("mail/2026/260919_lease_renewal_2.md")
        );
    }

    #[test]
    fn rejects_absolute_and_parent_folders() {
        let at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        for folder in [Path::new("/mail"), Path::new("mail/../other")] {
            assert_eq!(
                allocate(
                    folder,
                    "subject",
                    at,
                    chrono_tz::UTC,
                    &BTreeSet::new()
                ),
                Err(NoteError::InvalidFolder)
            );
        }
    }
}
