//! `--force[=MODE]` parsing shared by create, its hidden clip alias, and
//! collections.
//!
//! The `force` argument is one clap argument with an optional value:
//! absent stays absent, a bare `-f`/`--force` means `reread`, and an
//! explicit `--force=MODE` (or `-f=MODE`) names the mode. A `reread` or
//! `overwrite` TARGET directly after a bare force flag is adopted as the
//! mode and removed from TARGET; an explicit `=` value never adopts.
//! Clap records the same index pattern for a bare flag and an explicit
//! `=` value (the flag slot plus one value slot), so bareness is read
//! from the raw argv instead: only a `--force=`/`-f=` token counts as
//! explicit.

use std::ffi::OsString;

use clap::{Arg, ArgAction, ArgMatches};

use super::{sources as sources_mod, CommandError, Result};

/// What a repeated capture does with an existing reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ForceMode {
    /// Read the source again as a new `<stem>__vN` reading beside it.
    Reread,
    /// Recapture the existing reference in place.
    Overwrite,
}

impl ForceMode {
    /// Parse a mode word case-insensitively.
    fn from_word(word: &str) -> Option<ForceMode> {
        if word.eq_ignore_ascii_case("reread") {
            Some(ForceMode::Reread)
        } else if word.eq_ignore_ascii_case("overwrite") {
            Some(ForceMode::Overwrite)
        } else {
            None
        }
    }
}

/// The `force` argument definition shared by create, clip, and
/// collections. Absent stays absent; a bare flag means `reread`.
pub(super) fn force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .short('f')
        .value_name("MODE")
        .num_args(0..=1)
        .require_equals(true)
        .default_missing_value("reread")
        .value_parser(["reread", "overwrite"])
        .action(ArgAction::Set)
        .ignore_case(true)
        .help(
            "Recapture a source that is already a reference (bare -f rereads)",
        )
}

/// A decoded force selection: the mode plus TARGET with any adopted
/// mode word removed, in its original order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ForceSelection {
    pub(super) mode: Option<ForceMode>,
    pub(super) targets: Vec<OsString>,
}

/// Why force decoding failed: a stray mode-looking TARGET (exit 2), or
/// an adoption that left no TARGET (the normal missing-TARGET usage
/// error, exit 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ForceDecodeError {
    StrayMode { word: String },
    MissingTarget,
}

/// Decode the force mode and TARGET list for create, clip, and
/// collections. `raw` carries the argv words after the subcommand name
/// (`create` or its `clip` alias); it is only read to tell a bare
/// `-f`/`--force` from an explicit `--force=`/`-f=` token.
pub(super) fn decode_force(
    matches: &ArgMatches,
    raw: &[OsString],
) -> std::result::Result<ForceSelection, ForceDecodeError> {
    let all_targets: Vec<OsString> = matches
        .get_many::<OsString>("target")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    if !matches.contains_id("force") {
        let checked = check_stray_modes(&all_targets)?;
        return Ok(ForceSelection {
            mode: None,
            targets: checked,
        });
    }
    let explicit = is_explicit_force(raw);
    let mut targets = all_targets;
    let mut adopted: Option<ForceMode> = None;
    if !explicit
        && let (Some(force_index), Some(target_indices)) = (
            matches.index_of("force"),
            matches
                .indices_of("target")
                .map(|indices| indices.collect::<Vec<_>>()),
        )
        && target_indices.len() == targets.len()
    {
        let mut adopted_at: Option<(usize, ForceMode)> = None;
        for (position, target) in targets.iter().enumerate() {
            if target_indices[position] != force_index + 1 {
                continue;
            }
            if let Some(word) = target.to_str().and_then(ForceMode::from_word) {
                adopted_at = Some((position, word));
                break;
            }
        }
        if let Some((position, word)) = adopted_at {
            adopted = Some(word);
            targets.remove(position);
        }
    }
    if targets.is_empty() {
        return Err(ForceDecodeError::MissingTarget);
    }
    let checked = check_stray_modes(&targets)?;
    let mode = match adopted {
        Some(mode) => Some(mode),
        // Clap's value parser already validated the spelling
        // (case-insensitively); the fallback is unreachable.
        None => matches
            .get_one::<String>("force")
            .and_then(|value| ForceMode::from_word(value))
            .or(Some(ForceMode::Reread)),
    };
    Ok(ForceSelection {
        mode,
        targets: checked,
    })
}

/// True when the raw argv carries an explicit `--force=MODE`,
/// `-f=MODE`, or clustered-short `f=` (for example `-df=reread`) token
/// before any `--` separator. At most one force occurrence survives
/// successful clap parsing (a second is a usage error), so one explicit
/// token decides for the whole command.
fn is_explicit_force(raw: &[OsString]) -> bool {
    for token in raw {
        let bytes = token.as_encoded_bytes();
        if bytes == b"--" {
            break;
        }
        if bytes.starts_with(b"--force=") || bytes.starts_with(b"-f=") {
            return true;
        }
        // A clustered short ending in `f=` (for example `-df=reread`):
        // a single-dash token whose pre-`=` cluster ends in `f`.
        if bytes.first() == Some(&b'-')
            && !bytes.starts_with(b"--")
            && let Some(eq) = bytes.iter().position(|byte| *byte == b'=')
            && eq > 1
            && bytes[eq - 1] == b'f'
        {
            return true;
        }
    }
    false
}

/// Reject TARGET words that look like a `--force` mode: equal to
/// `reread` or `overwrite` (any case), or within edit distance 2 of
/// either. Escapes keep their target meanings: words containing `/`,
/// `.`, or `[[`, existing paths, and URLs pass through untouched.
fn check_stray_modes(
    targets: &[OsString],
) -> std::result::Result<Vec<OsString>, ForceDecodeError> {
    for target in targets {
        let Some(word) = target.to_str() else {
            continue;
        };
        if word.contains('/') || word.contains('.') || word.contains("[[") {
            continue;
        }
        if std::path::Path::new(word).exists() {
            continue;
        }
        if super::target::looks_like_url(word) {
            continue;
        }
        let lowered = word.to_lowercase();
        let is_mode_word = ForceMode::from_word(&lowered).is_some()
            || edit_distance(&lowered, "reread") <= 2
            || edit_distance(&lowered, "overwrite") <= 2;
        if is_mode_word {
            return Err(ForceDecodeError::StrayMode {
                word: word.to_string(),
            });
        }
    }
    Ok(targets.to_vec())
}

/// Levenshtein edit distance over Unicode scalar values.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (i, &a) in left.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, &b) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(a != b);
            current.push(
                substitution.min(previous[j + 1] + 1).min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

/// Render the stray-mode guard failure (exit 2).
pub(super) fn stray_mode_message(word: &str) -> String {
    format!(
        "`{word}` looks like a --force mode; use -f overwrite (or --force=overwrite)"
    )
}

/// `-f overwrite` on a filed reference: parsed but not implemented
/// (the later overwrite phase owns it), so refuse with exit 2.
pub(super) fn overwrite_filed_unavailable() -> CommandError {
    CommandError::new("-f overwrite is not available yet for filed references")
        .with_exit_code(2)
}

/// `-f overwrite` on a filed reference that already has a reread plan:
/// still refused (overwrite is not implemented yet), naming the reread
/// `-f` would take instead.
pub(super) fn overwrite_filed_unavailable_for(next_stem: &str) -> CommandError {
    CommandError::new(format!(
        "-f overwrite is not available yet; -f reads it again as {next_stem}"
    ))
    .with_exit_code(2)
}

/// `-f overwrite` with no existing reference to recapture in place:
/// refuse with exit 1 and a hint to drop the mode.
pub(super) fn nothing_to_overwrite(source: &str) -> CommandError {
    CommandError::new(format!(
        "nothing to overwrite: {source} is not a reference yet\nhint: drop `overwrite` to capture it as a new reference"
    ))
}

/// Is this an overwrite-mode selection (filed overwrite still pending)?
pub(super) fn is_overwrite(force: Option<ForceMode>) -> bool {
    force == Some(ForceMode::Overwrite)
}

/// Under `--force=overwrite`, triage a URL dedupe outcome before any
/// fetch: filed references are unavailable (exit 2), a queued-only
/// match keeps the old replacement permission, an existing exact-output
/// file keeps the old replacement permission, and a fresh source
/// refuses (exit 1) instead of capturing.
pub(super) fn triage_overwrite_url(
    recorded: &[sources_mod::RecordedSource],
    dedupe_key: &str,
    source: &str,
    output: Option<&std::path::Path>,
) -> Result<()> {
    if output
        .map(super::stamp::exact_output_exists)
        .transpose()?
        .unwrap_or(false)
    {
        return Ok(());
    }
    let hits = sources_mod::refusing_hits(recorded, dedupe_key, None, false);
    if hits.iter().any(|hit| hit.is_ref_note) {
        return Err(overwrite_filed_unavailable());
    }
    if hits.is_empty() {
        return Err(nothing_to_overwrite(source));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::highlights_ref::create;

    /// Parse `argv` after `create` through the real command, then decode.
    fn decode(
        argv: &[&str],
    ) -> std::result::Result<ForceSelection, ForceDecodeError> {
        let raw: Vec<OsString> = argv.iter().map(OsString::from).collect();
        let matches = create::command()
            .try_get_matches_from(
                std::iter::once(OsString::from("create")).chain(raw.clone()),
            )
            .expect("test argv parses");
        decode_force(&matches, &raw)
    }

    /// Parse `argv` after `create`, expecting a clap usage failure.
    fn parse_error(argv: &[&str]) -> clap::error::Error {
        let raw: Vec<OsString> = argv.iter().map(OsString::from).collect();
        create::command()
            .try_get_matches_from(
                std::iter::once(OsString::from("create")).chain(raw),
            )
            .expect_err("test argv must fail")
    }

    fn selection(mode: Option<ForceMode>, targets: &[&str]) -> ForceSelection {
        ForceSelection {
            mode,
            targets: targets.iter().map(OsString::from).collect(),
        }
    }

    #[test]
    fn plain_url_has_no_force() {
        assert_eq!(
            decode(&["https://example.com/essay"]).expect("decodes"),
            selection(None, &["https://example.com/essay"]),
        );
    }

    #[test]
    fn bare_force_means_reread_wherever_it_stands() {
        for argv in [
            vec!["https://example.com/essay", "-f"],
            vec!["-f", "https://example.com/essay"],
            vec!["--force", "https://example.com/essay"],
        ] {
            assert_eq!(
                decode(&argv).expect("decodes"),
                selection(
                    Some(ForceMode::Reread),
                    &["https://example.com/essay"]
                ),
                "{argv:?}",
            );
        }
    }

    #[test]
    fn adopted_mode_word_beats_the_missing_default() {
        for argv in [
            vec!["-f", "overwrite", "https://example.com/essay"],
            vec!["https://example.com/essay", "-f", "overwrite"],
            vec!["https://example.com/essay", "--force", "overwrite"],
            vec!["-f", "Overwrite", "https://example.com/essay"],
        ] {
            assert_eq!(
                decode(&argv).expect("decodes"),
                selection(
                    Some(ForceMode::Overwrite),
                    &["https://example.com/essay"]
                ),
                "{argv:?}",
            );
        }
    }

    #[test]
    fn explicit_equals_value_never_adopts() {
        for argv in [
            vec!["--force=overwrite", "https://example.com/essay"],
            vec!["-f=overwrite", "https://example.com/essay"],
            vec!["--force=reread", "https://example.com/essay"],
            vec!["-f=REREAD", "https://example.com/essay"],
        ] {
            let expected = if argv[0].to_lowercase().ends_with("overwrite") {
                Some(ForceMode::Overwrite)
            } else {
                Some(ForceMode::Reread)
            };
            assert_eq!(
                decode(&argv).expect("decodes"),
                selection(expected, &["https://example.com/essay"]),
                "{argv:?}",
            );
        }
    }

    #[test]
    fn clustered_short_ending_in_f_equals_is_explicit() {
        // `-df=reread` is `-d` plus explicit `--force=reread`: it never
        // adopts a following mode word, so `overwrite` stays a stray
        // TARGET (exit 2) instead of becoming the mode.
        let argv = vec!["-df=reread", "https://example.com/essay"];
        let raw: Vec<OsString> = argv.iter().map(OsString::from).collect();
        assert!(is_explicit_force(&raw), "{argv:?} must count as explicit");
        let decoded = decode(&argv).expect("decodes");
        assert_eq!(decoded.mode, Some(ForceMode::Reread), "{argv:?}");
        // Explicit clustered force never adopts: `overwrite` after
        // `-df=reread` is a stray mode word.
        assert!(matches!(
            decode(&["-df=reread", "overwrite", "https://example.com/essay"]),
            Err(ForceDecodeError::StrayMode { .. }),
        ));
    }

    #[test]
    fn bare_force_combines_with_dry_run() {
        for argv in [
            vec!["-fd", "https://example.com/essay"],
            vec!["-df", "https://example.com/essay"],
            vec!["-f", "-d", "https://example.com/essay"],
            vec!["https://example.com/essay", "-f", "overwrite", "-d"],
        ] {
            let decoded = decode(&argv).expect("decodes");
            assert_eq!(
                decoded.targets,
                vec![OsString::from("https://example.com/essay")],
                "{argv:?}"
            );
            if argv.contains(&"-f") && argv.contains(&"overwrite") {
                assert_eq!(decoded.mode, Some(ForceMode::Overwrite));
            } else {
                assert_eq!(decoded.mode, Some(ForceMode::Reread));
            }
        }
    }

    #[test]
    fn stray_mode_words_refuse() {
        for argv in [
            vec!["-f", "overwite", "https://example.com/essay"],
            vec!["-fd", "overwrite", "https://example.com/essay"],
            vec!["--force=reread", "overwrite", "https://example.com/essay"],
            vec!["overwrite"],
            vec!["REREAD"],
        ] {
            match decode(&argv) {
                Err(ForceDecodeError::StrayMode { word }) => {
                    assert!(
                        word.eq_ignore_ascii_case("overwrite")
                            || word.eq_ignore_ascii_case("overwite")
                            || word.eq_ignore_ascii_case("reread"),
                        "{argv:?} guarded on {word:?}",
                    );
                }
                other => panic!("{argv:?} must guard, got {other:?}"),
            }
        }
    }

    #[test]
    fn guard_escapes_keep_their_meanings() {
        for argv in [
            vec!["./overwrite"],
            vec!["[[overwrite]]"],
            vec!["https://example.com/overwrite"],
        ] {
            let decoded = decode(&argv).expect("escapes decode");
            assert_eq!(decoded.mode, None, "{argv:?}");
            assert_eq!(decoded.targets.len(), 1, "{argv:?}");
        }
    }

    #[test]
    fn adoption_emptying_target_is_missing_target() {
        assert_eq!(
            decode(&["-f", "overwrite"]),
            Err(ForceDecodeError::MissingTarget),
        );
    }

    #[test]
    fn repeated_force_is_a_clap_usage_error() {
        for argv in [
            vec!["-f", "overwrite", "-f", "https://example.com/essay"],
            vec![
                "--force=reread",
                "--force=overwrite",
                "https://example.com/essay",
            ],
        ] {
            let error = parse_error(&argv);
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{argv:?}: {error}",
            );
        }
    }

    #[test]
    fn glued_short_value_parses_as_output() {
        // `-o/--output` owns the short `o`, so `-foverwrite` is `-f`
        // plus `-o verwrite` (not a mode, not a clap error): the
        // downstream output-path validation refuses `verwrite`.
        // Documented in the `Recapture:` help section.
        let raw: Vec<OsString> = ["-foverwrite", "https://example.com/essay"]
            .iter()
            .map(OsString::from)
            .collect();
        let matches = create::command()
            .try_get_matches_from(
                std::iter::once(OsString::from("create")).chain(raw.clone()),
            )
            .expect("glued short parses as -f plus -o");
        assert_eq!(
            decode_force(&matches, &raw).expect("decodes"),
            selection(Some(ForceMode::Reread), &["https://example.com/essay"]),
        );
        assert_eq!(
            matches.get_one::<OsString>("output"),
            Some(&OsString::from("verwrite")),
        );
    }

    #[test]
    fn invalid_equals_mode_is_a_clap_error() {
        let error =
            parse_error(&["--force=bogus", "https://example.com/essay"]);
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::InvalidValue,
            "{error}",
        );
    }

    #[test]
    fn adopted_collection_keeps_exactly_two_targets() {
        assert_eq!(
            decode(&[
                "https://example.com/a",
                "https://example.com/b",
                "-f",
                "overwrite"
            ])
            .expect("decodes"),
            selection(
                Some(ForceMode::Overwrite),
                &["https://example.com/a", "https://example.com/b"]
            ),
        );
    }

    #[test]
    fn full_tree_indices_match_standalone() {
        // The binary parses through the nested `bob ref` tree; decode
        // must behave identically there.
        let raw: Vec<OsString> =
            ["-f", "overwrite"].iter().map(OsString::from).collect();
        let matches = crate::native::highlights_ref::cli::build_cli()
            .try_get_matches_from(
                ["bob ref", "create", "-f", "overwrite"]
                    .iter()
                    .map(OsString::from),
            )
            .expect("full tree parses");
        let (_, sub) = matches.subcommand().expect("create subcommand");
        assert_eq!(
            decode_force(sub, &raw),
            Err(ForceDecodeError::MissingTarget),
        );
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("overwrite", "overwrite"), 0);
        assert_eq!(edit_distance("overwite", "overwrite"), 1);
        assert_eq!(edit_distance("reread", "reread"), 0);
        assert_eq!(edit_distance("read", "reread"), 2);
        assert!(edit_distance("report", "overwrite") > 2);
        assert_eq!(edit_distance("", "reread"), 6);
    }
}
