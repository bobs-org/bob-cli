use super::super::template;

fn issue_codes(raw: &str) -> Vec<&'static str> {
    template::analyze(raw)
        .issues
        .iter()
        .map(|issue| issue.code())
        .collect()
}

fn first_issue(raw: &str) -> template::TemplateIssue {
    template::analyze(raw).issues[0].clone()
}

#[test]
fn template_directive_sits_at_either_edge_of_any_line_of_the_first_item() {
    let expected = "  Task value\n- child value\n  - deeper value\n";
    for raw in [
        "\n\n  Task {0} %t\n- child {0}\n  - deeper {0}\n\n\nvalue\n",
        "\n\n  %t Task {0}\n- child {0}\n  - deeper {0}\n\n\nvalue\n",
        "\n\n  Task {0}\n- child {0} %t\n  - deeper {0}\n\n\nvalue\n",
        "\n\n  Task {0}\n- %t child {0}\n  - deeper {0}\n\n\nvalue\n",
        "\n\n  Task {0}\n- child {0}\n  - deeper {0} %t\n\n\nvalue\n",
    ] {
        let expansion = template::expand(raw).expect("valid template").unwrap();
        assert_eq!(expansion.text, expected, "{raw:?}");
        assert_eq!(expansion.summary.field_count, 1, "{raw:?}");
        assert_eq!(expansion.summary.row_count, 1, "{raw:?}");
    }
    let trailing = template::expand("Task {0} %t\n\nvalue\n")
        .expect("valid template")
        .unwrap();
    assert_eq!(trailing.summary.line, 1);
}

#[test]
fn a_directive_after_markers_still_claims_the_template() {
    let raw = "Apply to {0}! @job+search#leads %t\n\nAcme\n";
    let expansion = template::expand(raw).expect("valid template").unwrap();
    assert_eq!(expansion.text, "Apply to Acme! @job+search#leads\n");
    let raw = "%t Apply to {0}! @job+search#leads\n\nAcme\n";
    let expansion = template::expand(raw).expect("valid template").unwrap();
    assert_eq!(expansion.text, "Apply to Acme! @job+search#leads\n");
}

#[test]
fn mid_line_and_escaped_directives_leave_an_ordinary_draft() {
    for raw in [
        "Task %t more {0}\n\nvalue\n",
        "Task {0} \\%t\n\nvalue\n",
        "Task {0} 50%t\n\nvalue\n",
    ] {
        assert!(template::analyze(raw).directive.is_none(), "{raw:?}");
        assert!(issue_codes(raw).is_empty(), "{raw:?}");
        assert_eq!(template::expand(raw).unwrap(), None, "{raw:?}");
    }
}

#[test]
fn a_directive_in_a_later_item_is_misplaced() {
    for raw in [
        "Task\n\nLater item %t\n",
        "Task {0}\n\nvalue\n\n%t more\n",
        "Task\n\nLater item\n- child %t\n",
    ] {
        assert!(
            issue_codes(raw).contains(&"template_directive_misplaced"),
            "{raw:?}"
        );
        assert_eq!(
            first_issue(raw).message(),
            "`%t` marks the first item as the template",
            "{raw:?}"
        );
        assert!(template::analyze(raw).directive.is_none(), "{raw:?}");
        assert!(template::expand(raw).is_err(), "{raw:?}");
    }
}

#[test]
fn a_repeated_directive_in_the_first_item_is_a_duplicate() {
    let raw = "Task {0} %t %t\n\nvalue\n";
    assert_eq!(first_issue(raw).code(), "duplicate_capture_marker");
}

#[test]
fn missing_template_or_rows_has_a_teaching_issue() {
    assert_eq!(first_issue("%t\n\n").code(), "template_missing_rows");
    assert_eq!(first_issue("%t\n\n").message(), "add one row per capture");
    assert_eq!(
        first_issue("Task {0} %t\n\n").code(),
        "template_missing_rows"
    );
    assert_eq!(
        first_issue("Task {0} %t\n\n").message(),
        "add one value per capture, separated by spaces or new lines"
    );
    assert_eq!(
        first_issue("Apply to {0} {1} %t\n\n").message(),
        "add one row per capture: 2 values separated by spaces"
    );
    assert_eq!(
        first_issue("A task %t\n\n").message(),
        "add one row per capture"
    );
    assert!(first_issue("Task {0} %t\nvalue\n")
        .message()
        .contains("add a blank line"));
}

#[test]
fn a_template_needs_a_blank_separator_before_rows() {
    let raw = "Task {0} %t\nvalue\n";
    assert!(issue_codes(raw).contains(&"template_needs_separator"));
    assert!(first_issue(raw).message().contains("add a blank line"));
}

#[test]
fn placeholders_repeat_reorder_escape_and_expand_inside_markers() {
    let raw = "Apply {{draft}} {1} {0} {1}! @jobs^apply-{0} %t\n\nfoo bar\n";
    let expansion = template::expand(raw).expect("valid template").unwrap();
    assert_eq!(
        expansion.text,
        "Apply {draft} bar foo bar! @jobs^apply-foo\n"
    );
}

#[test]
fn placeholder_grammar_rejects_noncanonical_and_incomplete_braces() {
    for bad in ["{}", "{name}", "{01}", "{0:x}", "{1000}", "{", "}", "{12"] {
        let raw = format!("Task {bad} %t\n\nvalue\n");
        assert!(
            issue_codes(&raw).contains(&"template_invalid_placeholder"),
            "{bad:?}"
        );
        assert!(first_issue(&raw).message().contains("write `{{`"));
    }
}

#[test]
fn placeholder_gaps_and_one_based_indices_are_explained() {
    let gap = first_issue("Task {0} {2} %t\n\nA B C\n");
    assert_eq!(gap.code(), "template_placeholder_gap");
    assert!(gap.message().contains("not {1}"));
    assert!(gap.range().start < gap.range().end);
    let one_based = first_issue("Task {1} {2} %t\n\nA B\n");
    assert_eq!(one_based.code(), "template_placeholder_gap");
    assert!(one_based.message().contains("placeholders start at {0}"));
}

#[test]
fn missing_placeholder_is_suppressed_until_rows_exist() {
    assert_eq!(
        issue_codes("A task %t\n\n").as_slice(),
        ["template_missing_rows"]
    );
    assert!(issue_codes("A task %t\n\nvalue\n")
        .contains(&"template_missing_placeholder"));
}

#[test]
fn definition_validation_ignores_rows_but_checks_placeholders_without_rows() {
    assert_eq!(
        template::definition_issue("Task {0} %t\n\n").map(|issue| issue.code()),
        None
    );
    assert_eq!(
        template::definition_issue("A task %t\n\n").map(|issue| issue.code()),
        Some("template_missing_placeholder")
    );
    assert_eq!(
        template::definition_issue("Task {0} %t\n\nvalid\n\nwrong arity\n")
            .map(|issue| issue.code()),
        None
    );
    assert_eq!(
        template::definition_issue("%t\n").map(|issue| issue.code()),
        Some("template_missing_placeholder")
    );
    assert_eq!(
        template::definition_issue("Task {0}\n\nlater %t\n")
            .map(|issue| issue.code()),
        Some("template_directive_misplaced")
    );
}

#[test]
fn row_lexer_handles_quotes_escapes_and_plain_quotes() {
    let raw = "{0} {1} {2} {3} %t\n\n\"Two Sigma\" “Acme Works” \"say \\\"hi\\\"\" \"a\\\\b\"\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(
        analysis.rows[0]
            .fields
            .iter()
            .map(|field| field.value.as_str())
            .collect::<Vec<_>>(),
        ["Two Sigma", "Acme Works", "say \"hi\"", "a\\b"]
    );

    let raw = "{0} {1} %t\n\n\"Two Sigma” next\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows[0].fields[0].value, "Two Sigma");

    let raw = "{0} {1} {2} %t\n\n27\" O'Reilly value\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows[0].fields[0].value, "27\"");
    assert_eq!(analysis.rows[0].fields[1].value, "O'Reilly");

    let raw = "{0}\t{1}\t{2} %t\n\n\"\"\t“”\tplain\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows[0].fields[0].value, "");
    assert_eq!(analysis.rows[0].fields[1].value, "");

    let raw = "{0} {1} %t\n\n\"close\"word next\n";
    assert_eq!(
        first_issue(raw).code(),
        "template_unterminated_quote",
        "a quote not followed by a field separator is literal"
    );
}

#[test]
fn quote_errors_and_arity_errors_are_row_scoped() {
    let unterminated = first_issue("{0} {1} %t\n\n\"Two Sigma\n");
    assert_eq!(unterminated.code(), "template_unterminated_quote");
    assert!(unterminated
        .message()
        .starts_with("template row 1 (line 3):"));

    for row in ["one", "one two three"] {
        let raw = format!("{{0}} {{1}} %t\n\n{row}\n");
        let arity = first_issue(&raw);
        assert_eq!(arity.code(), "template_row_arity");
        assert!(arity.message().starts_with("template row 1 (line 3):"));
    }
}

#[test]
fn rows_cannot_be_bullets_and_templates_cannot_use_other_globals() {
    assert!(issue_codes("Task {0} %t\n\n- value\n")
        .contains(&"template_row_bullet"));
    assert!(issue_codes("Task {0} @@route %t\n\nvalue\n")
        .contains(&"template_global_declaration"));
    assert!(issue_codes("Task {0} %t\n\nvalue @@route\n")
        .contains(&"template_global_declaration"));
    assert!(issue_codes("Task {0} {1} %t\n\nvalue \"@@route\"\n")
        .contains(&"template_global_declaration"));
    assert!(issue_codes("Task @{0} %t\n\n@route\n")
        .contains(&"template_global_declaration"));
}

#[test]
fn row_global_declarations_use_row_scope() {
    let issue = first_issue("Task {0} {1} %t\n\na @@job\n");
    assert_eq!(issue.code(), "template_global_declaration");
    assert!(
        issue.message().starts_with("template row 1 (line 3):"),
        "{}",
        issue.message()
    );
}

#[test]
fn one_field_rows_split_on_spaces_and_template_line_endings_normalize() {
    for newline in ["\n", "\r\n", "\r"] {
        let raw = ["Apply {0} %t", "", "  Montréal 🧭 role  "].join(newline);
        let analysis = template::analyze(&raw);
        assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
        assert_eq!(analysis.rows.len(), 3);
        assert_eq!(
            analysis.rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            [3, 3, 3]
        );
        assert_eq!(
            analysis
                .rows
                .iter()
                .map(|row| row.fields[0].value.as_str())
                .collect::<Vec<_>>(),
            ["Montréal", "🧭", "role"]
        );
        let expansion =
            template::expand(&raw).expect("valid template").unwrap();
        assert_eq!(
            expansion.text,
            "Apply Montréal\n\nApply 🧭\n\nApply role\n"
        );
        assert_eq!(expansion.summary.row_count, 3);

        for quoted in ["\"Montréal 🧭 role\"", "“Montréal 🧭 role”"] {
            let raw =
                ["Apply {0} %t", "", &format!("  {quoted}  ")].join(newline);
            let analysis = template::analyze(&raw);
            assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
            assert_eq!(analysis.rows.len(), 1);
            assert_eq!(analysis.rows[0].fields[0].value, "Montréal 🧭 role");
            let expansion =
                template::expand(&raw).expect("valid template").unwrap();
            assert_eq!(expansion.text, "Apply Montréal 🧭 role\n");
            assert_eq!(expansion.instances[0].line, 3);
        }
    }
}

#[test]
fn expansion_matches_the_plan_example_byte_for_byte() {
    let raw = concat!(
        "Apply to {0}! @job_2026_apply_29:{1} %t\n",
        "- **URL**: {2}\n",
        "\n",
        "Foobar foo http://foobar.com/career/some_foo_job\n",
        "Bazbuz baz http://bazbuz.com/career/some_baz_job\n",
        "\"Two Sigma\" two-sigma https://www.twosigma.com/careers/some_role\n",
    );
    let expanded = template::expand(raw).expect("valid example").unwrap();
    assert_eq!(
        expanded.text,
        concat!(
            "Apply to Foobar! @job_2026_apply_29:foo\n",
            "- **URL**: http://foobar.com/career/some_foo_job\n",
            "\n",
            "Apply to Bazbuz! @job_2026_apply_29:baz\n",
            "- **URL**: http://bazbuz.com/career/some_baz_job\n",
            "\n",
            "Apply to Two Sigma! @job_2026_apply_29:two-sigma\n",
            "- **URL**: https://www.twosigma.com/careers/some_role\n",
        )
    );
}

#[test]
fn both_template_bounds_are_checked_without_truncation() {
    let rows = std::iter::repeat_n("x", 1_001)
        .collect::<Vec<_>>()
        .join("\n");
    let raw = format!("{{0}} %t\n\n{rows}\n");
    assert!(issue_codes(&raw).contains(&"template_too_large"));

    let row = "x".repeat(1024 * 1024);
    let raw = format!("{{0}} %t\n\n{row}\n");
    assert!(issue_codes(&raw).contains(&"template_too_large"));

    let repeated_placeholder = "{0}".repeat(1_000);
    let row = "x".repeat(1_100);
    let raw = format!("{repeated_placeholder} %t\n\n{row}\n");
    assert!(issue_codes(&raw).contains(&"template_too_large"));
}

#[test]
fn non_template_expansion_is_an_identity() {
    let raw = "plain draft\r\nkeeps its exact bytes";
    assert_eq!(template::expand(raw).unwrap(), None);
}

#[test]
fn template_mask_preserves_byte_offsets_and_hides_directive_rows_and_braces() {
    let raw = "Task {0} {{literal}} @job:{1} %t\r\n\r\n\"Two Sigma\" role\r\n";
    let masked = template::masked(raw);
    assert_eq!(masked.len(), raw.len());
    assert!(masked.starts_with("Task xxx xxliteralxx @job:xxx   \r\n"));
    let row_start = raw.find("\"Two Sigma\"").unwrap();
    let row_len = "\"Two Sigma\" role".len();
    assert_eq!(&masked[row_start..row_start + row_len], " ".repeat(row_len));
    assert!(masked.ends_with("\r\n"));
    assert_eq!(
        masked
            .as_bytes()
            .iter()
            .filter(|byte| **byte == b'\n')
            .count(),
        3
    );
}

#[test]
fn template_mask_hides_only_the_directive_token_wherever_it_sits() {
    for raw in [
        "%t Task {0}\n- child\n\nvalue\n",
        "Task {0}\n- child %t\n\nvalue\n",
    ] {
        let masked = template::masked(raw);
        assert_eq!(masked.len(), raw.len(), "{raw:?}");
        let token = raw.find("%t").unwrap();
        assert_eq!(&masked[token..token + 2], "  ", "{raw:?}");
        assert!(!masked.contains("%t"), "{raw:?}");
        assert!(masked.contains("child"), "{raw:?}");
    }
}

#[test]
fn row_values_cannot_inject_the_template_directive() {
    let raw = "Note {0} %t\n\n\"later %t\"\n";
    let issue = first_issue(raw);
    assert_eq!(issue.code(), "template_directive_misplaced");
    assert!(
        issue.message().starts_with("template row 1 (line 3):"),
        "{}",
        issue.message()
    );
    // A mid-line occurrence is prose, so it expands normally.
    let raw = "Note {0} then more %t\n\n\"later %t x\"\n";
    assert!(template::analyze(raw).issues.is_empty());
}

#[test]
fn every_directive_placement_expands_to_the_same_text() {
    let expected = "Apply to Acme! @job+search#leads\n- **URL**: https://acme.example/jobs\n";
    for raw in [
        "Apply to {0}! @job+search#leads %t\n- **URL**: {1}\n\nAcme https://acme.example/jobs\n",
        "%t Apply to {0}! @job+search#leads\n- **URL**: {1}\n\nAcme https://acme.example/jobs\n",
        "Apply to {0}! @job+search#leads\n- **URL**: {1} %t\n\nAcme https://acme.example/jobs\n",
    ] {
        let expansion = template::expand(raw).expect("valid template").unwrap();
        assert_eq!(expansion.text, expected, "{raw:?}");
    }
}

#[test]
fn one_placeholder_one_line_equals_three_line_expansion() {
    let one_line = "foo: {0}! @sase:foo-{0} %t\n\nbar baz bam\n";
    let three_line = "foo: {0}! @sase:foo-{0} %t\n\nbar\nbaz\nbam\n";
    let one = template::expand(one_line).expect("valid one-line").unwrap();
    let three = template::expand(three_line)
        .expect("valid three-line")
        .unwrap();
    assert_eq!(one.text, three.text);
    assert_eq!(one.summary.row_count, 3);
}

#[test]
fn one_placeholder_rows_share_line_with_per_value_ranges() {
    let raw = "foo: {0}! @sase:foo-{0} %t\n\nbar baz bam\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows.len(), 3);
    for (index, row) in analysis.rows.iter().enumerate() {
        assert_eq!(row.row, index + 1);
        assert_eq!(row.line, 3);
        assert_eq!(row.fields.len(), 1);
        assert_eq!(row.fields[0].field, 0);
        assert_eq!(row.range, row.fields[0].range);
        assert_eq!(&raw[row.range.clone()], ["bar", "baz", "bam"][index]);
    }
}

#[test]
fn one_placeholder_tabs_and_repeated_spaces_split_cleanly() {
    let raw = "Task {0} %t\n\none\t\ttwo   three\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(
        analysis
            .rows
            .iter()
            .map(|row| row.fields[0].value.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three"]
    );
}

#[test]
fn one_placeholder_mixed_lines_number_left_to_right_then_top_to_bottom() {
    let raw = "Task {0} %t\n\na b\nc\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows.len(), 3);
    assert_eq!(
        analysis
            .rows
            .iter()
            .map(|row| (row.row, row.line))
            .collect::<Vec<_>>(),
        [(1, 3), (2, 3), (3, 4)]
    );
}

#[test]
fn one_placeholder_unterminated_quote_names_value_row_and_keeps_leading_row() {
    let raw = "Task {0} %t\n\na \"b c\n";
    let issue = first_issue(raw);
    assert_eq!(issue.code(), "template_unterminated_quote");
    assert!(
        issue.message().starts_with("template row 2 (line 3):"),
        "{}",
        issue.message()
    );
    let analysis = template::analyze(raw);
    assert_eq!(analysis.rows.len(), 2);
    let expanded = template::expand_rows_for_editor(raw, &analysis);
    assert_eq!(expanded.len(), 2);
    assert!(expanded[0].is_some());
    assert!(expanded[1].is_none());
}

#[test]
fn one_placeholder_global_declaration_names_containing_row() {
    let issue = first_issue("Task {0} %t\n\na @@route\n");
    assert_eq!(issue.code(), "template_global_declaration");
    assert!(
        issue.message().starts_with("template row 2 (line 3):"),
        "{}",
        issue.message()
    );
}

#[test]
fn one_placeholder_thousand_and_first_value_is_too_large() {
    let row = std::iter::repeat_n("x", 1_001)
        .collect::<Vec<_>>()
        .join(" ");
    let raw = format!("{{0}} %t\n\n{row}\n");
    let analysis = template::analyze(&raw);
    assert!(issue_codes(&raw).contains(&"template_too_large"));
    let issue = analysis
        .issues
        .iter()
        .find(|issue| issue.code() == "template_too_large")
        .expect("too-large issue");
    assert!(issue.message().contains("template row 1001 (line 3)"));
    assert_eq!(analysis.rows.len(), 1_001);
    assert_eq!(analysis.rows[1_000].row, 1_001);
}

#[test]
fn one_placeholder_templates_never_report_arity() {
    for raw in [
        "Task {0} %t\n\na b c\n",
        "Task {0} %t\n\na\n",
        "Task {0} {0} %t\n\na b\n",
    ] {
        assert!(
            !issue_codes(raw).contains(&"template_row_arity"),
            "{raw:?}: {:?}",
            template::analyze(raw).issues
        );
    }
}

#[test]
fn multi_placeholder_extra_values_still_report_arity() {
    let raw = "Task {0} {1} %t\n\none two three\n";
    let issue = first_issue(raw);
    assert_eq!(issue.code(), "template_row_arity");
    assert!(issue.message().starts_with("template row 1 (line 3):"));
}
