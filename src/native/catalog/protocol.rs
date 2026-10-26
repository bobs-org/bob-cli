//! Protocol 1: the contract between bob and catalog clients.
//!
//! Request: `bob __catalog --protocol <N>`, also accepting
//! `--protocol=N`. A missing or malformed request prints usage to
//! stderr and exits 2. An unsupported protocol prints one JSON error
//! document to stdout and exits 2.

use std::ffi::OsString;

/// The protocol version this binary speaks.
pub(crate) const PROTOCOL: u32 = 1;
/// The oldest protocol version this binary still answers.
pub(crate) const MIN_PROTOCOL: u32 = 1;

const USAGE: &str = "usage: bob __catalog --protocol <N>";

/// A parsed `__catalog` request. The protocol is already a number;
/// range-checking is a separate step so the JSON error path stays
/// distinct from malformed usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Request {
    pub protocol: u32,
}

/// Parse the argv after `__catalog`. The error string is the stderr
/// message for the exit-2 malformed-request path.
pub(crate) fn parse_request(argv: &[OsString]) -> Result<Request, String> {
    let mut protocol: Option<u32> = None;
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
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
                                "bob __catalog: malformed request: \
                                 --protocol needs a value\n{USAGE}"
                            ));
                        }
                    },
                };
                match raw.parse::<u32>() {
                    Ok(number) => protocol = Some(number),
                    Err(_) => {
                        return Err(format!(
                            "bob __catalog: malformed request: \
                             bad --protocol value {raw:?}\n{USAGE}"
                        ));
                    }
                }
            }
            other => {
                return Err(format!(
                    "bob __catalog: malformed request: \
                     unknown option {other:?}\n{USAGE}"
                ));
            }
        }
    }
    let protocol = protocol.ok_or_else(|| {
        format!("bob __catalog: malformed request: missing --protocol\n{USAGE}")
    })?;
    Ok(Request { protocol })
}

/// JSON error body when the request names a protocol outside
/// [`MIN_PROTOCOL`]..=[`PROTOCOL`]. `None` when the request is in range.
pub(crate) fn unsupported_json(protocol: u32) -> Option<String> {
    if (MIN_PROTOCOL..=PROTOCOL).contains(&protocol) {
        return None;
    }
    let message = if protocol < MIN_PROTOCOL {
        format!(
            "bob command catalog protocol {protocol} is no longer supported \
             (min {MIN_PROTOCOL})"
        )
    } else {
        format!(
            "this bob is older than the catalog client \
             (protocol {protocol}; max {PROTOCOL})"
        )
    };
    Some(format!(
        "{{\"schema_version\":1,\"ok\":false,\"code\":\"unsupported_protocol\",\
         \"min\":{MIN_PROTOCOL},\"max\":{PROTOCOL},\"message\":{}}}",
        json_string(&message)
    ))
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_space_and_equals_forms() {
        let space = parse_request(&argv(&["--protocol", "1"]))
            .expect("space form parses");
        assert_eq!(space.protocol, 1);
        let attached = parse_request(&argv(&["--protocol=1"]))
            .expect("equals form parses");
        assert_eq!(attached.protocol, 1);
    }

    #[test]
    fn rejects_malformed_requests() {
        for words in [
            vec![],
            vec!["--protocol"],
            vec!["--protocol", "x"],
            vec!["--bogus", "1"],
            vec!["--protocol", "1", "--extra"],
            vec!["1"],
        ] {
            assert!(
                parse_request(&argv(&words)).is_err(),
                "expected malformed: {words:?}"
            );
        }
    }

    #[test]
    fn unsupported_covers_both_directions() {
        assert!(unsupported_json(1).is_none());
        let below = unsupported_json(0).expect("below min");
        assert!(below.contains("\"ok\":false"));
        assert!(below.contains("unsupported_protocol"));
        assert!(below.contains("\"min\":1"));
        assert!(below.contains("\"max\":1"));
        let above = unsupported_json(2).expect("above max");
        assert!(above.contains("unsupported_protocol"));
        let parsed: serde_json::Value =
            serde_json::from_str(&above).expect("error json parses");
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["code"], "unsupported_protocol");
    }
}
