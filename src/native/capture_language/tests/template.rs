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
fn template_header_accepts_case_and_ascii_padding_and_skips_blank_lines() {
    let raw = "  \t@@TeMpLaTe:\t\n\n  Task {0}  \n- child {0}\n  - deeper {0}\n\n\nvalue\n";
    let expansion = template::expand(raw).expect("valid template").unwrap();
    assert_eq!(
        expansion.text,
        "  Task value  \n- child value\n  - deeper value\n"
    );
    assert_eq!(expansion.summary.line, 1);
    assert_eq!(expansion.summary.field_count, 1);
    assert_eq!(expansion.summary.row_count, 1);
}

#[test]
fn template_header_reports_misplaced_inline_repeated_and_option_like_forms() {
    for raw in [
        "Task\n@@template:\n\n{0}\n\nvalue\n",
        "@@template: extra\nTask {0}\n\nvalue\n",
        "@@template:\n@@template:\n\nTask {0}\n\nvalue\n",
        "@@template:tsv\nTask {0}\n\nvalue\n",
    ] {
        assert!(
            issue_codes(raw).contains(&"template_header_misplaced"),
            "{raw:?}"
        );
        assert_eq!(
            first_issue(raw).message(),
            "write `@@template:` alone on the draft's first line"
        );
    }
}

#[test]
fn missing_template_or_rows_has_a_teaching_issue() {
    assert_eq!(
        first_issue("@@template:\n\n").code(),
        "template_missing_rows"
    );
    assert!(first_issue("@@template:\n\n")
        .message()
        .contains("write the template"));
    assert_eq!(
        first_issue("@@template:\nTask {0}\n\n").code(),
        "template_missing_rows"
    );
    assert_eq!(
        first_issue("@@template:\nTask {0}\n\n").message(),
        "add one value per capture, separated by spaces or new lines"
    );
    assert_eq!(
        first_issue("@@template:\nApply to {0} {1}\n\n").message(),
        "add one row per capture: 2 values separated by spaces"
    );
    assert_eq!(
        first_issue("@@template:\nA task\n\n").message(),
        "add one row per capture"
    );
    assert!(first_issue("@@template:\nTask {0}\nvalue\n")
        .message()
        .contains("add a blank line"));
}

#[test]
fn a_template_needs_a_blank_separator_before_rows() {
    let raw = "@@template:\nTask {0}\nvalue\n";
    assert!(issue_codes(raw).contains(&"template_needs_separator"));
    assert!(first_issue(raw).message().contains("add a blank line"));
}

#[test]
fn placeholders_repeat_reorder_escape_and_expand_inside_markers() {
    let raw = "@@template:\nApply {{draft}} {1} {0} {1}! @jobs^apply-{0}\n\nfoo bar\n";
    let expansion = template::expand(raw).expect("valid template").unwrap();
    assert_eq!(
        expansion.text,
        "Apply {draft} bar foo bar! @jobs^apply-foo\n"
    );
}

#[test]
fn placeholder_grammar_rejects_noncanonical_and_incomplete_braces() {
    for bad in ["{}", "{name}", "{01}", "{0:x}", "{1000}", "{", "}", "{12"] {
        let raw = format!("@@template:\nTask {bad}\n\nvalue\n");
        assert!(
            issue_codes(&raw).contains(&"template_invalid_placeholder"),
            "{bad:?}"
        );
        assert!(first_issue(&raw).message().contains("write `{{`"));
    }
}

#[test]
fn placeholder_gaps_and_one_based_indices_are_explained() {
    let gap = first_issue("@@template:\nTask {0} {2}\n\nA B C\n");
    assert_eq!(gap.code(), "template_placeholder_gap");
    assert!(gap.message().contains("not {1}"));
    assert!(gap.range().start < gap.range().end);
    let one_based = first_issue("@@template:\nTask {1} {2}\n\nA B\n");
    assert_eq!(one_based.code(), "template_placeholder_gap");
    assert!(one_based.message().contains("placeholders start at {0}"));
}

#[test]
fn missing_placeholder_is_suppressed_until_rows_exist() {
    assert_eq!(
        issue_codes("@@template:\nA task\n\n").as_slice(),
        ["template_missing_rows"]
    );
    assert!(issue_codes("@@template:\nA task\n\nvalue\n")
        .contains(&"template_missing_placeholder"));
}

#[test]
fn definition_validation_ignores_rows_but_checks_placeholders_without_rows() {
    assert_eq!(
        template::definition_issue("@@template:\nTask {0}\n\n")
            .map(|issue| issue.code()),
        None
    );
    assert_eq!(
        template::definition_issue("@@template:\nA task\n\n")
            .map(|issue| issue.code()),
        Some("template_missing_placeholder")
    );
    assert_eq!(
        template::definition_issue(
            "@@template:\nTask {0}\n\nvalid\n\nwrong arity\n"
        )
        .map(|issue| issue.code()),
        None
    );
    assert_eq!(
        template::definition_issue("@@template:\n").map(|issue| issue.code()),
        Some("template_missing_rows")
    );
}

#[test]
fn row_lexer_handles_quotes_escapes_and_plain_quotes() {
    let raw = "@@template:\n{0} {1} {2} {3}\n\n\"Two Sigma\" “Acme Works” \"say \\\"hi\\\"\" \"a\\\\b\"\n";
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

    let raw = "@@template:\n{0} {1}\n\n\"Two Sigma” next\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows[0].fields[0].value, "Two Sigma");

    let raw = "@@template:\n{0} {1} {2}\n\n27\" O'Reilly value\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows[0].fields[0].value, "27\"");
    assert_eq!(analysis.rows[0].fields[1].value, "O'Reilly");

    let raw = "@@template:\n{0}\t{1}\t{2}\n\n\"\"\t“”\tplain\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows[0].fields[0].value, "");
    assert_eq!(analysis.rows[0].fields[1].value, "");

    let raw = "@@template:\n{0} {1}\n\n\"close\"word next\n";
    assert_eq!(
        first_issue(raw).code(),
        "template_unterminated_quote",
        "a quote not followed by a field separator is literal"
    );
}

#[test]
fn quote_errors_and_arity_errors_are_row_scoped() {
    let unterminated = first_issue("@@template:\n{0} {1}\n\n\"Two Sigma\n");
    assert_eq!(unterminated.code(), "template_unterminated_quote");
    assert!(unterminated
        .message()
        .starts_with("template row 1 (line 4):"));

    for row in ["one", "one two three"] {
        let raw = format!("@@template:\n{{0}} {{1}}\n\n{row}\n");
        let arity = first_issue(&raw);
        assert_eq!(arity.code(), "template_row_arity");
        assert!(arity.message().starts_with("template row 1 (line 4):"));
    }
}

#[test]
fn rows_cannot_be_bullets_and_templates_cannot_use_other_globals() {
    assert!(issue_codes("@@template:\nTask {0}\n\n- value\n")
        .contains(&"template_row_bullet"));
    assert!(issue_codes("@@template:\nTask {0} @@route\n\nvalue\n")
        .contains(&"template_global_declaration"));
    assert!(issue_codes("@@template:\nTask {0}\n\nvalue @@route\n")
        .contains(&"template_global_declaration"));
    assert!(
        issue_codes("@@template:\nTask {0} {1}\n\nvalue \"@@route\"\n")
            .contains(&"template_global_declaration")
    );
    assert!(issue_codes("@@template:\nTask @{0}\n\n@route\n")
        .contains(&"template_global_declaration"));
}

#[test]
fn row_global_declarations_use_row_scope() {
    let issue = first_issue("@@template:\nTask {0} {1}\n\na @@job\n");
    assert_eq!(issue.code(), "template_global_declaration");
    assert!(
        issue.message().starts_with("template row 1 (line 4):"),
        "{}",
        issue.message()
    );
}

#[test]
fn one_field_rows_split_on_spaces_and_template_line_endings_normalize() {
    for newline in ["\n", "\r\n", "\r"] {
        let raw = ["@@template:", "Apply {0}", "", "  Montréal 🧭 role  "]
            .join(newline);
        let analysis = template::analyze(&raw);
        assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
        assert_eq!(analysis.rows.len(), 3);
        assert_eq!(
            analysis.rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            [4, 4, 4]
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
                ["@@template:", "Apply {0}", "", &format!("  {quoted}  ")]
                    .join(newline);
            let analysis = template::analyze(&raw);
            assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
            assert_eq!(analysis.rows.len(), 1);
            assert_eq!(analysis.rows[0].fields[0].value, "Montréal 🧭 role");
            let expansion =
                template::expand(&raw).expect("valid template").unwrap();
            assert_eq!(expansion.text, "Apply Montréal 🧭 role\n");
            assert_eq!(expansion.instances[0].line, 4);
        }
    }
}

#[test]
fn expansion_matches_the_plan_example_byte_for_byte() {
    let raw = concat!(
        "@@template:\n",
        "Apply to {0}! @job_2026_apply_29:{1}\n",
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
    let raw = format!("@@template:\n{{0}}\n\n{rows}\n");
    assert!(issue_codes(&raw).contains(&"template_too_large"));

    let row = "x".repeat(1024 * 1024);
    let raw = format!("@@template:\n{{0}}\n\n{row}\n");
    assert!(issue_codes(&raw).contains(&"template_too_large"));

    let repeated_placeholder = "{0}".repeat(1_000);
    let row = "x".repeat(1_100);
    let raw = format!("@@template:\n{repeated_placeholder}\n\n{row}\n");
    assert!(issue_codes(&raw).contains(&"template_too_large"));
}

#[test]
fn non_template_expansion_is_an_identity() {
    let raw = "plain draft\r\nkeeps its exact bytes";
    assert_eq!(template::expand(raw).unwrap(), None);
}

#[test]
fn template_mask_preserves_byte_offsets_and_hides_header_rows_and_braces() {
    let raw = "@@template:\r\nTask {0} {{literal}} @job:{1}\r\n\r\n\"Two Sigma\" role\r\n";
    let masked = template::masked(raw);
    assert_eq!(masked.len(), raw.len());
    assert!(masked.starts_with("           \r\n"));
    assert!(masked.contains("Task xxx xxliteralxx @job:xxx\r\n"));
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
        4
    );
}

#[test]
fn one_placeholder_one_line_equals_three_line_expansion() {
    let one_line = "@@template:\nfoo: {0}! @sase:foo-{0}\n\nbar baz bam\n";
    let three_line = "@@template:\nfoo: {0}! @sase:foo-{0}\n\nbar\nbaz\nbam\n";
    let one = template::expand(one_line).expect("valid one-line").unwrap();
    let three = template::expand(three_line)
        .expect("valid three-line")
        .unwrap();
    assert_eq!(one.text, three.text);
    assert_eq!(one.summary.row_count, 3);
}

#[test]
fn one_placeholder_rows_share_line_with_per_value_ranges() {
    let raw = "@@template:\nfoo: {0}! @sase:foo-{0}\n\nbar baz bam\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows.len(), 3);
    for (index, row) in analysis.rows.iter().enumerate() {
        assert_eq!(row.row, index + 1);
        assert_eq!(row.line, 4);
        assert_eq!(row.fields.len(), 1);
        assert_eq!(row.fields[0].field, 0);
        assert_eq!(row.range, row.fields[0].range);
        assert_eq!(&raw[row.range.clone()], ["bar", "baz", "bam"][index]);
    }
}

#[test]
fn one_placeholder_tabs_and_repeated_spaces_split_cleanly() {
    let raw = "@@template:\nTask {0}\n\none\t\ttwo   three\n";
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
    let raw = "@@template:\nTask {0}\n\na b\nc\n";
    let analysis = template::analyze(raw);
    assert!(analysis.issues.is_empty(), "{:?}", analysis.issues);
    assert_eq!(analysis.rows.len(), 3);
    assert_eq!(
        analysis
            .rows
            .iter()
            .map(|row| (row.row, row.line))
            .collect::<Vec<_>>(),
        [(1, 4), (2, 4), (3, 5)]
    );
}

#[test]
fn one_placeholder_unterminated_quote_names_value_row_and_keeps_leading_row() {
    let raw = "@@template:\nTask {0}\n\na \"b c\n";
    let issue = first_issue(raw);
    assert_eq!(issue.code(), "template_unterminated_quote");
    assert!(
        issue.message().starts_with("template row 2 (line 4):"),
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
    let issue = first_issue("@@template:\nTask {0}\n\na @@route\n");
    assert_eq!(issue.code(), "template_global_declaration");
    assert!(
        issue.message().starts_with("template row 2 (line 4):"),
        "{}",
        issue.message()
    );
}

#[test]
fn one_placeholder_thousand_and_first_value_is_too_large() {
    let row = std::iter::repeat_n("x", 1_001)
        .collect::<Vec<_>>()
        .join(" ");
    let raw = format!("@@template:\n{{0}}\n\n{row}\n");
    let analysis = template::analyze(&raw);
    assert!(issue_codes(&raw).contains(&"template_too_large"));
    let issue = analysis
        .issues
        .iter()
        .find(|issue| issue.code() == "template_too_large")
        .expect("too-large issue");
    assert!(issue.message().contains("template row 1001 (line 4)"));
    assert_eq!(analysis.rows.len(), 1_001);
    assert_eq!(analysis.rows[1_000].row, 1_001);
}

#[test]
fn one_placeholder_templates_never_report_arity() {
    for raw in [
        "@@template:\nTask {0}\n\na b c\n",
        "@@template:\nTask {0}\n\na\n",
        "@@template:\nTask {0} {0}\n\na b\n",
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
    let raw = "@@template:\nTask {0} {1}\n\none two three\n";
    let issue = first_issue(raw);
    assert_eq!(issue.code(), "template_row_arity");
    assert!(issue.message().starts_with("template row 1 (line 4):"));
}
