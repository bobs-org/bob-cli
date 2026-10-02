//! Protocol 1: the contract between bob and every shell adapter.
//!
//! Request: `bob __complete <SHELL> --protocol <N> [--suffix <TEXT>] --
//! <WORD>...`, where `SHELL` is `zsh` or `bash`, `WORD`s are the command
//! line's words up to the cursor (shell-unquoted, first word ignored, last
//! word the cursor word's text before the cursor), and `--suffix` is the
//! cursor word's text after the cursor.
//!
//! Response: UTF-8 lines on stdout, directives first, then candidates as
//! `value<TAB>description<TAB>group<TAB>space|nospace`. Exit 0, including
//! on internal failure (which yields empty output). A malformed request
//! exits 2 with a message on stderr.

use std::ffi::{OsStr, OsString};

/// The protocol version this binary speaks.
pub(crate) const PROTOCOL: u32 = 1;
/// The oldest protocol version this binary still answers.
pub(crate) const MIN_PROTOCOL: u32 = 1;

/// Emitted when the adapter speaks a protocol older than [`MIN_PROTOCOL`].
pub(crate) const BELOW_MIN_MESSAGE: &str =
    "bob shell completion is out of date \u{2014} run: bob completion install";
/// Emitted when the adapter speaks a protocol newer than [`PROTOCOL`].
pub(crate) const ABOVE_MAX_MESSAGE: &str =
    "this bob is older than its shell completion \u{2014} reinstall bob (just install)";

/// The shell asking for completion. It changes only presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shell {
    Zsh,
    Bash,
}

/// A parsed `__complete` request.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// The requesting shell.
    pub shell: Shell,
    /// The adapter's protocol version (already range-checked by the caller).
    pub protocol: u32,
    /// Text of the cursor word after the cursor, if any.
    pub suffix: Option<OsString>,
    /// Command-line words up to the cursor; first word ignored, last word
    /// is the cursor word's text before the cursor.
    pub words: Vec<OsString>,
}

/// Parse the argv after `__complete`. The error string is the stderr
/// message for the exit-2 malformed-request path.
pub(crate) fn parse_request(argv: &[OsString]) -> Result<Request, String> {
    let usage = "usage: bob __complete <zsh|bash> --protocol <N> [--suffix <TEXT>] -- <WORD>...";
    let mut rest = argv.iter();
    let shell_word = rest.next().ok_or_else(|| {
        format!("bob __complete: malformed request: missing shell\n{usage}")
    })?;
    let shell = match shell_word.to_string_lossy().as_ref() {
        "zsh" => Shell::Zsh,
        "bash" => Shell::Bash,
        other => {
            return Err(format!(
                "bob __complete: malformed request: \
                 unknown shell {other:?}, expected zsh or bash\n{usage}"
            ));
        }
    };
    let mut protocol: Option<u32> = None;
    let mut suffix: Option<OsString> = None;
    let mut words: Option<Vec<OsString>> = None;
    let mut args = rest;
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if words.is_none() && text == "--" {
            words = Some(args.cloned().collect());
            break;
        }
        if words.is_some() {
            break;
        }
        let (flag, inline) = match text.split_once('=') {
            Some((flag, inline)) => (flag, Some(inline)),
            None => (text.as_ref(), None),
        };
        match flag {
            "--protocol" => {
                let raw = match inline {
                    Some(inline) => inline.to_string(),
                    None => match args.next() {
                        Some(next) => next.to_string_lossy().into_owned(),
                        None => {
                            return Err(format!(
                                "bob __complete: malformed request: \
                                 --protocol needs a value\n{usage}"
                            ));
                        }
                    },
                };
                match raw.parse::<u32>() {
                    Ok(number) => protocol = Some(number),
                    Err(_) => {
                        return Err(format!(
                            "bob __complete: malformed request: \
                             bad --protocol value {raw:?}\n{usage}"
                        ));
                    }
                }
            }
            "--suffix" => {
                suffix = Some(match inline {
                    Some(inline) => OsString::from(inline),
                    None => match args.next() {
                        Some(next) => next.clone(),
                        None => {
                            return Err(format!(
                                "bob __complete: malformed request: \
                                 --suffix needs a value\n{usage}"
                            ));
                        }
                    },
                });
            }
            other => {
                return Err(format!(
                    "bob __complete: malformed request: \
                     unknown option {other:?}\n{usage}"
                ));
            }
        }
    }
    let words = words.ok_or_else(|| {
        format!(
            "bob __complete: malformed request: missing -- separator\n{usage}"
        )
    })?;
    if words.is_empty() {
        return Err(format!(
            "bob __complete: malformed request: no words after --\n{usage}"
        ));
    }
    let protocol = protocol.ok_or_else(|| {
        format!(
            "bob __complete: malformed request: missing --protocol\n{usage}"
        )
    })?;
    Ok(Request {
        shell,
        protocol,
        suffix,
        words,
    })
}

/// Check the request protocol against [`MIN_PROTOCOL`]..=[`PROTOCOL`].
/// Returns the skew message line when the adapter is out of range.
pub(crate) fn skew_message(protocol: u32) -> Option<String> {
    if protocol < MIN_PROTOCOL {
        Some(message_line(BELOW_MIN_MESSAGE))
    } else if protocol > PROTOCOL {
        Some(message_line(ABOVE_MAX_MESSAGE))
    } else {
        None
    }
}

/// A `!prefix <N>` directive: keep the first N Unicode scalar values of
/// the cursor-word prefix; candidates replace only the rest.
pub(crate) fn prefix_line(keep: usize) -> String {
    format!("!prefix {keep}")
}

/// The `!dirs` directive: complete directories natively.
pub(crate) fn dirs_line() -> String {
    "!dirs".to_string()
}

/// The `!files` directive, optionally filtered by glob.
pub(crate) fn files_line(glob: Option<&str>) -> String {
    match glob {
        Some(glob) => format!("!files {glob}"),
        None => "!files".to_string(),
    }
}

/// A `!message <text>` directive: show a hint, offer no candidates.
pub(crate) fn message_line(text: &str) -> String {
    format!("!message {}", sanitize_field(text))
}

/// Encode one candidate line. Returns `None` for values that cannot be
/// encoded (empty, containing TAB/LF/CR, or starting with `!`, which
/// would parse as a directive); bob drops such candidates.
pub(crate) fn candidate_line(
    value: &OsStr,
    description: &str,
    group: &str,
    nospace: bool,
) -> Option<String> {
    let value = value.to_string_lossy();
    if value.is_empty()
        || value.contains(['\t', '\n', '\r'])
        || value.starts_with('!')
    {
        return None;
    }
    let group = sanitize_field(group);
    let group = if group.is_empty() { "values" } else { &group };
    Some(format!(
        "{value}\t{}\t{group}\t{}",
        sanitize_field(description),
        if nospace { "nospace" } else { "space" },
    ))
}

/// Descriptions and groups travel in TAB-separated fields, so TAB, LF,
/// and CR all become spaces.
fn sanitize_field(text: &str) -> String {
    text.chars()
        .map(|char| {
            if char == '\t' || char == '\n' || char == '\r' {
                ' '
            } else {
                char
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_full_request() {
        let request = parse_request(&argv(&[
            "zsh",
            "--protocol",
            "1",
            "--suffix",
            "fix",
            "--",
            "bob",
            "capture",
            "",
        ]))
        .expect("full request parses");
        assert_eq!(request.shell, Shell::Zsh);
        assert_eq!(request.protocol, 1);
        assert_eq!(request.suffix, Some(OsString::from("fix")));
        assert_eq!(request.words, argv(&["bob", "capture", ""]));
    }

    #[test]
    fn parses_attached_forms() {
        let request =
            parse_request(&argv(&["bash", "--protocol=1", "--", "bob", ""]))
                .expect("attached protocol parses");
        assert_eq!(request.shell, Shell::Bash);
        assert_eq!(request.protocol, 1);
        assert_eq!(request.suffix, None);
    }

    #[test]
    fn rejects_malformed_requests() {
        for words in [
            vec![],
            vec!["fish"],
            vec!["zsh"],
            vec!["zsh", "--protocol", "1"],
            vec!["zsh", "--protocol", "1", "--"],
            vec!["zsh", "--bogus", "1", "--", "bob"],
            vec!["zsh", "--protocol", "x", "--", "bob"],
            vec!["zsh", "--protocol"],
            vec!["zsh", "--suffix"],
        ] {
            let argv = argv(&words);
            assert!(
                parse_request(&argv).is_err(),
                "expected malformed: {words:?}"
            );
        }
    }

    #[test]
    fn skew_messages_cover_both_directions() {
        assert_eq!(skew_message(0), Some(message_line(BELOW_MIN_MESSAGE)));
        assert_eq!(skew_message(1), None);
        assert_eq!(skew_message(99), Some(message_line(ABOVE_MAX_MESSAGE)));
    }

    #[test]
    fn encoder_sanitizes_fields_and_drops_bad_values() {
        assert_eq!(
            candidate_line(
                OsStr::new("human"),
                "Colored text for people",
                "format",
                false
            ),
            Some("human\tColored text for people\tformat\tspace".to_string())
        );
        assert_eq!(
            candidate_line(OsStr::new("x"), "a\tb\nc\rd", "", true),
            Some("x\ta b c d\tvalues\tnospace".to_string())
        );
        for bad in ["", "a\tb", "a\nb", "a\rb", "!dirs"] {
            assert_eq!(
                candidate_line(OsStr::new(bad), "d", "g", false),
                None,
                "expected drop: {bad:?}"
            );
        }
    }
}
