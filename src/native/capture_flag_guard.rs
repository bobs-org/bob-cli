//! Pre-clap guard against unknown alphabetic long options where
//! capture-style TEXT would begin.
//!
//! Every capture-style TEXT positional is declared
//! `trailing_var_arg(true)` plus `allow_hyphen_values(true)`, so clap can
//! never report an unknown `--flag` for these commands: the flag becomes the
//! start of TEXT and the caller files it as an ordinary task. Scan `argv`
//! before clap runs and reject `--` plus an ASCII letter that no declared
//! long flag owns, unless a `--` end-of-options marker or an earlier TEXT
//! token already claimed the rest of the line.
//!
//! Kept working unchanged: every declared flag, Pomodoro shift tokens such
//! as `--2` and `--1` (`--` plus a non-letter starts TEXT), a bare `--`, and
//! everything after `--` (literal text, so `-- --anything` still captures
//! `--anything`).

use std::ffi::OsString;

/// One declared `--long` flag: `takes_separate_value` is true when the flag
/// consumes the next argv token as its value (`--route NAME`); false for
/// switch flags and for `require_equals` options such as `--clip`.
pub(crate) struct LongFlag {
    pub name: &'static str,
    pub takes_separate_value: bool,
}

/// The declared option surface of one capture-style command.
pub(crate) struct GuardSpec {
    pub longs: &'static [LongFlag],
    /// Short flags that consume the next argv token (`-f json`, `-r notes`).
    pub short_values: &'static str,
    /// Switch shorts that never consume the next token (`-d`, `-n`).
    pub short_flags: &'static str,
    /// `require_equals` shorts that never consume the next token but may
    /// carry an attached value (`-c`, `-c=HDR`); kept separate from
    /// `short_flags` so attached text is not mistaken for TEXT.
    pub equals_only_shorts: &'static str,
}

/// An unknown long option found where TEXT would begin.
pub(crate) struct GuardHit {
    /// The offending token as typed (including any `=value` suffix).
    pub flag: String,
    /// Whether `--format json` (or `-f json`) preceded the hit in option
    /// position, so the rejection must use the JSON error envelope.
    pub format_json: bool,
}

/// Clap's unexpected-argument wording for a rejected long option.
pub(crate) fn unexpected_argument_message(flag: &str) -> String {
    format!("unexpected argument '{flag}' found")
}

const fn long(name: &'static str, takes_separate_value: bool) -> LongFlag {
    LongFlag {
        name,
        takes_separate_value,
    }
}

const FORMAT: LongFlag = long("format", true);
const HELP: LongFlag = long("help", false);
const BOB_DIR: LongFlag = long("bob-dir", true);

/// Declared options of `bob capture`.
pub(crate) fn capture_spec() -> GuardSpec {
    static LONGS: &[LongFlag] = &[
        BOB_DIR,
        long("clip", false),
        long("dry-run", false),
        long("expand", false),
        FORMAT,
        HELP,
        long("no-clip", false),
        long("no-ref", false),
        long("pull-in", true),
        long("refuse-override", false),
        long("require-pull-in-choice", false),
        long("route", true),
        long("section", true),
        long("task", true),
        long("task-ref", true),
        long("task-section", true),
        long("template-origin", true),
    ];
    GuardSpec {
        longs: LONGS,
        short_values: "bfprstS",
        short_flags: "dEhnR",
        equals_only_shorts: "c",
    }
}

/// Declared options of `bob capture-complete`.
pub(crate) fn capture_complete_spec() -> GuardSpec {
    static LONGS: &[LongFlag] = &[
        long("all-tasks", false),
        BOB_DIR,
        long("cursor", true),
        FORMAT,
        HELP,
    ];
    GuardSpec {
        longs: LONGS,
        short_values: "bcf",
        short_flags: "ah",
        equals_only_shorts: "",
    }
}

/// Declared options of `bob capture-parse`.
pub(crate) fn capture_parse_spec() -> GuardSpec {
    static LONGS: &[LongFlag] = &[FORMAT, HELP, long("no-ref", false)];
    GuardSpec {
        longs: LONGS,
        short_values: "f",
        short_flags: "hR",
        equals_only_shorts: "",
    }
}

/// Declared options of `bob capture-rewrite`.
pub(crate) fn capture_rewrite_spec() -> GuardSpec {
    static LONGS: &[LongFlag] = &[long("cursor", true), FORMAT, HELP];
    GuardSpec {
        longs: LONGS,
        short_values: "cf",
        short_flags: "h",
        equals_only_shorts: "",
    }
}

/// Declared options of `bob capture-templates save`, the only
/// capture-templates command with a capture-style TEXT positional. `list`,
/// `show`, `delete`, and `clear` take no TEXT (their template positionals
/// reject hyphen values), so clap already reports unknown flags there.
pub(crate) fn capture_templates_save_spec() -> GuardSpec {
    static LONGS: &[LongFlag] = &[FORMAT, HELP, long("template-origin", true)];
    GuardSpec {
        longs: LONGS,
        short_values: "f",
        short_flags: "h",
        equals_only_shorts: "",
    }
}

/// Scan `args` (argv after the subcommand name) for an unknown alphabetic
/// long option where TEXT would begin. Returns the first hit, if any.
///
/// The scan mirrors clap's option region: declared flags (and their
/// separate values) are skipped, a bare `--` ends the scan, and the first
/// TEXT token — including `--` plus a non-letter such as `--2` — ends the
/// scan because `trailing_var_arg` makes the rest of the line TEXT.
pub(crate) fn find_unknown_long_flag(
    args: &[OsString],
    spec: &GuardSpec,
) -> Option<GuardHit> {
    let mut format_json = false;
    let mut index = 0;
    while index < args.len() {
        let token = args[index].to_string_lossy();
        if token == "--" {
            return None;
        }
        if let Some(body) = token.strip_prefix("--") {
            let Some(first) = body.chars().next() else {
                // `---` starts with `--` plus `-`: not a long option.
                return None;
            };
            if !first.is_ascii_alphabetic() {
                // `--2` and friends are TEXT, not options.
                return None;
            }
            let name = body.split('=').next().unwrap_or(body);
            let Some(known) = spec.longs.iter().find(|flag| flag.name == name)
            else {
                return Some(GuardHit {
                    flag: format!("--{body}"),
                    format_json,
                });
            };
            if name == "format" {
                if let Some((_, value)) = token.split_once('=') {
                    if value == "json" {
                        format_json = true;
                    }
                } else if known.takes_separate_value
                    && let Some(value) = args.get(index + 1)
                {
                    if value.to_string_lossy() == "json" {
                        format_json = true;
                    }
                    index += 1;
                }
            } else if !token.contains('=')
                && known.takes_separate_value
                && index + 1 < args.len()
            {
                index += 1;
            }
            index += 1;
            continue;
        }
        if let Some(body) = token.strip_prefix('-') {
            if body.is_empty() {
                // A bare `-` is TEXT.
                return None;
            }
            if body.chars().all(|flag| spec.short_flags.contains(flag)) {
                index += 1;
                continue;
            }
            let first = body.chars().next().expect("nonempty short body");
            if spec.short_values.contains(first) {
                if body.len() == 1 {
                    // `-f json`: the value may itself select JSON output.
                    if first == 'f'
                        && let Some(value) = args.get(index + 1)
                    {
                        if value.to_string_lossy() == "json" {
                            format_json = true;
                        }
                        index += 1;
                    } else if index + 1 < args.len() {
                        index += 1;
                    }
                } else if first == 'f' {
                    let attached = &body[1..];
                    let value = attached.strip_prefix('=').unwrap_or(attached);
                    if value == "json" {
                        format_json = true;
                    }
                }
                index += 1;
                continue;
            }
            if spec.equals_only_shorts.contains(first) {
                index += 1;
                continue;
            }
            // Unknown shorts and markdown bullets (`-2`, `- foo`) are TEXT:
            // clap keeps parsing them exactly as before.
            return None;
        }
        // A non-option token starts TEXT; the rest of the line is literal.
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn unknown_long_flag_before_text_is_rejected() {
        let spec = capture_spec();
        let hit = find_unknown_long_flag(
            &args(&[
                "--format",
                "json",
                "--require-pull-in-choice-x",
                "--",
                "hi",
            ]),
            &spec,
        )
        .expect("unknown flag");
        assert_eq!(hit.flag, "--require-pull-in-choice-x");
        assert!(hit.format_json);
    }

    #[test]
    fn declared_flags_values_and_text_pass_through() {
        let spec = capture_spec();
        for argv in [
            vec!["--format", "json", "--dry-run", "hello"],
            vec!["-f", "json", "--", "--anything"],
            vec!["-b", "/tmp/vault", "--route", "notes", "hello"],
            vec!["--clip=HDR", "hello"],
            vec!["--", "--require-pull-in-choice-x"],
            vec!["--2"],
            vec!["--", "--2"],
            vec!["-2"],
            vec!["hello", "--world"],
            vec!["--dry-run", "--1"],
        ] {
            assert!(
                find_unknown_long_flag(&args(&argv), &spec).is_none(),
                "should pass: {argv:?}"
            );
        }
    }

    #[test]
    fn text_starting_with_a_flag_word_needs_the_delimiter() {
        let spec = capture_spec();
        let hit = find_unknown_long_flag(&args(&["--hi"]), &spec)
            .expect("leading flag word");
        assert_eq!(hit.flag, "--hi");
        assert!(!hit.format_json);
    }

    #[test]
    fn short_format_json_with_unknown_flag_reports_json() {
        let spec = capture_spec();
        let hit = find_unknown_long_flag(&args(&["-fjson", "--bogus"]), &spec)
            .expect("unknown flag");
        assert_eq!(hit.flag, "--bogus");
        assert!(hit.format_json);
    }
}
