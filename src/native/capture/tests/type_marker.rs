use super::*;

#[test]
fn capture_parse_recognizes_hash_type_marker() {
    let parsed = parse_capture_text("Fix the parser #fix", None)
        .expect("hash type marker parses");
    assert_eq!(parsed.body, "Fix the parser");
    assert_eq!(
        parsed
            .type_marker
            .as_ref()
            .map(|marker| marker.name.as_str()),
        Some("fix")
    );
}
