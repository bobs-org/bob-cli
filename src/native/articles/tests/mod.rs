//! Golden fixtures, refusals, mutation, and the ignored corpus.

use std::path::Path;

use super::{
    adopt, format, format_as_article, heading_identity, heading_plain_identity,
    strip_section_number, ArticleContext,
};
use crate::native::projects::{frontmatter_is_article, parse_frontmatter};

fn article(text: &str) -> String {
    let plan = format(text, &ArticleContext::new(Path::new("article.md")));
    match plan.formatted {
        Some(formatted) => formatted,
        None => panic!("format refused or errored: {:?}", plan.reason),
    }
}

fn assert_pair(input: &str, expected: &str) {
    let formatted = article(input);
    assert_eq!(
        formatted, expected,
        "formatted output mismatch\n=== got ===\n{formatted}=== expected ===\n{expected}"
    );
    let again = article(&formatted);
    assert_eq!(again, formatted, "second pass was not a no-op");
}

#[test]
fn blog_draft_matches_worked_example() {
    assert_pair(
        include_str!("fixtures/blog_draft.md"),
        include_str!("fixtures/blog_draft.expected.md"),
    );
}

#[test]
fn ownership_tokens() {
    assert_pair(
        include_str!("fixtures/ownership.md"),
        include_str!("fixtures/ownership.expected.md"),
    );
}

#[test]
fn protected_regions_leave_inner_headings() {
    assert_pair(
        include_str!("fixtures/protected.md"),
        include_str!("fixtures/protected.expected.md"),
    );
}

#[test]
fn setext_and_thematic_breaks() {
    assert_pair(
        include_str!("fixtures/setext.md"),
        include_str!("fixtures/setext.expected.md"),
    );
}

#[test]
fn layout_whitespace_crlf_and_bom() {
    let input = include_bytes!("fixtures/layout.md");
    let expected = include_bytes!("fixtures/layout.expected.md");
    let input = std::str::from_utf8(input).expect("utf8");
    let expected = std::str::from_utf8(expected).expect("utf8");
    assert_pair(input, expected);
}

#[test]
fn layout_crlf_file() {
    let input =
        "---\r\ntype: \"[[article]]\"\r\n---\r\n\r\n# Title\r\n\r\nbody\r\n";
    let formatted = article(input);
    assert!(formatted.contains("\r\n"), "{formatted:?}");
    assert_eq!(
        formatted,
        "---\r\ntype: \"[[article]]\"\r\n---\r\n\r\n# 1. Title\r\n\r\nbody\r\n"
    );
}

#[test]
fn layout_bom_and_final_newline() {
    let input = "\u{feff}---\ntype: [[article]]\n---\n\n# Title";
    let formatted = article(input);
    assert!(formatted.starts_with('\u{feff}'));
    assert!(formatted.ends_with('\n'));
    assert!(formatted.contains("# 1. Title"));
}

#[test]
fn fences_spacing_and_unclosed() {
    assert_pair(
        include_str!("fixtures/fences.md"),
        include_str!("fixtures/fences.expected.md"),
    );
}

#[test]
fn heading_levels_and_depth() {
    let plan = format(
        include_str!("fixtures/levels.md"),
        &ArticleContext::new(Path::new("article.md")),
    );
    let formatted = plan.formatted.expect("levels format");
    assert_eq!(formatted, include_str!("fixtures/levels.expected.md"));
    assert!(
        plan.findings.iter().any(|f| f.code == "deep-section"),
        "depth 5 should warn: {:?}",
        plan.findings
    );
    assert_eq!(plan.changes.promoted, 2);

    let first_deep =
        article("---\ntype: [[article]]\n---\n\n### First\n\n## Second\n");
    assert!(first_deep.contains("## 1. First"), "{first_deep}");
    assert!(first_deep.contains("## 2. Second"), "{first_deep}");
}

#[test]
fn numbers_off_and_round_trip() {
    let off = include_str!("fixtures/numbers_off.md");
    assert_pair(off, include_str!("fixtures/numbers_off.expected.md"));
    let stripped = article(off);
    let on =
        stripped.replace("section_numbers: false", "section_numbers: true");
    let restored = article(&on);
    assert!(restored.contains("# 1. Taxes"), "{restored}");
    assert!(restored.contains("## 1.1 30 Burger"), "{restored}");
}

#[test]
fn strip_section_number_keeps_bare_numbers_and_escapes() {
    assert_eq!(strip_section_number("2024 taxes").token, None);
    assert_eq!(strip_section_number("30 Burger").token, None);
    assert_eq!(strip_section_number("1.4 New").token, Some("1.4"));
    assert_eq!(strip_section_number("1.4 New").rest, "New");
    assert_eq!(strip_section_number("3. Title").token, Some("3."));
    assert_eq!(strip_section_number("1.5 million users").token, Some("1.5"));
    assert_eq!(
        strip_section_number("1.16.0 changelog").token,
        Some("1.16.0")
    );
    assert_eq!(strip_section_number("1\\.5 million").token, None);
    assert_eq!(strip_section_number("1.2").token, Some("1.2"));
    assert_eq!(strip_section_number("").token, None);
    assert_eq!(heading_identity("1.4 New"), "new");
    assert_eq!(
        heading_plain_identity("1.3 Skills, **memory**"),
        "skills, memory"
    );
}

#[test]
fn heading_links_follow_new_numbers_and_keep_aliases_and_encoding() {
    let input = "---\ntype: [[article]]\n---\n\n## One\n\n## Two\n\n## Three\n\n## Four\n\n## Five\n\n## Six\n\n## Beads\n\n[[#Beads]]\n";
    let plan = format(input, &ArticleContext::new(Path::new("stem.md")));
    let output = plan.formatted.expect("formatted article");
    assert!(output.contains("[[#7. Beads]]"), "{output}");
    assert_eq!(plan.changes.links, 1);

    let input = "---\ntype: [[article]]\n---\n\n# 2024 taxes\n\n[[stem#2024 taxes|alias]]\n[encoded](stem.md#2024%20taxes)\n";
    let plan = format(input, &ArticleContext::new(Path::new("stem.md")));
    let output = plan.formatted.expect("formatted article");
    assert!(output.contains("[[stem#1. 2024 taxes|alias]]"), "{output}");
    assert!(
        output.contains("[encoded](stem.md#1.%202024%20taxes)"),
        "{output}"
    );
    assert_eq!(plan.changes.links, 2);
}

#[test]
fn heading_embeds_refresh_and_preserve_aliases_and_blocks() {
    let input = "---\ntype: [[article]]\n---\n\n# Root\n\n## Beta\n\n![[#Beta]]\n![[note#Beta|alias]]\n![[#^id]]\n";
    let plan = format(input, &ArticleContext::new(Path::new("note.md")));
    let output = plan.formatted.expect("formatted article");
    assert!(output.contains("![[#1.1 Beta]]"), "{output}");
    assert!(output.contains("![[note#1.1 Beta|alias]]"), "{output}");
    assert!(output.contains("![[#^id]]"), "{output}");
    assert_eq!(plan.changes.links, 2);
}

#[test]
fn heading_links_resolve_preformat_titles_before_ambiguous_identities() {
    let duplicate = "---\ntype: [[article]]\n---\n\n## 9. Examples\n\n## 8. Examples\n\n[[#9. Examples]]\n";
    let plan = format(duplicate, &ArticleContext::new(Path::new("note.md")));
    let output = plan.formatted.expect("duplicate titles format");
    assert!(output.contains("[[#1. Examples]]"), "{output}");
    assert!(!plan
        .findings
        .iter()
        .any(|f| f.code == "ambiguous-heading-link"));

    let ambiguous = "---\ntype: [[article]]\n---\n\n## 9. Example\n\n## 2. Example\n\n[[#Example]]\n";
    let plan = format(ambiguous, &ArticleContext::new(Path::new("note.md")));
    assert!(plan.formatted.is_some());
    assert!(plan
        .findings
        .iter()
        .any(|f| f.code == "ambiguous-heading-link"));
}

#[test]
fn heading_links_leave_blocks_and_protected_code_alone() {
    let input = "---\ntype: [[article]]\n---\n\n## Section\n\n[[#^block]]\n`[[#Section]]`\nInline $[[#Section]]$ math.\nInline %% [[#Section]] %% comment.\n\n```md\n[[#Section]]\n```\n";
    let plan = format(input, &ArticleContext::new(Path::new("note.md")));
    let output = plan.formatted.expect("formatted article");
    assert!(output.contains("[[#^block]]"), "{output}");
    assert!(output.contains("`[[#Section]]`"), "{output}");
    assert!(output.contains("Inline $[[#Section]]$ math."), "{output}");
    assert!(
        output.contains("Inline %% [[#Section]] %% comment."),
        "{output}"
    );
    assert!(output.contains("[[#Section]]\n```"), "{output}");
    assert_eq!(plan.changes.links, 0);
}

#[test]
fn indented_code_links_and_blank_runs_are_preserved() {
    let input = "---\ntype: [[article]]\n---\n\n## Alpha\n\n    code [[#Alpha]]\n\n\n    more code\n";
    let plan = format(input, &ArticleContext::new(Path::new("note.md")));
    let output = plan.formatted.expect("formatted article");
    assert!(
        output.contains("    code [[#Alpha]]\n\n\n    more code"),
        "{output:?}"
    );
    assert_eq!(plan.changes.links, 0);
}

#[test]
fn heading_spacing_surrounds_protected_blocks() {
    let input = concat!(
        "---\ntype: [[article]]\n---\n\n",
        "# Math\n$$\nmath\n$$\n",
        "# Comment\n%%\ncomment\n%%\n",
        "# Html comment\n<!--\ncomment\n-->\n",
        "# Html block\n<div>\nbody\n</div>\n",
    );
    let output = article(input);
    for (heading, block) in [
        ("# 1. Math", "$$"),
        ("# 2. Comment", "%%"),
        ("# 3. Html comment", "<!--"),
        ("# 4. Html block", "<div>"),
    ] {
        assert!(
            output.contains(&format!("{heading}\n\n{block}")),
            "{output}"
        );
    }
}

#[test]
fn section_number_counts_and_notice_plurals_reflect_actual_changes() {
    let single = format(
        "---\ntype: [[article]]\n---\n\n## One\n",
        &ArticleContext::new(Path::new("note.md")),
    );
    assert_eq!(single.changes.numbered, 1);
    assert_eq!(single.notice, "Article formatted · 1 section numbered");

    let formatted = single.formatted.unwrap();
    let unchanged =
        format(&formatted, &ArticleContext::new(Path::new("note.md")));
    assert_eq!(unchanged.changes.numbered, 0);
    assert_eq!(unchanged.changes.renumbered, 0);
    assert_eq!(unchanged.notice, "Article formatted");

    let layout_only = format(
        "---\ntype: [[article]]\n---\n\n# 1. One   \n",
        &ArticleContext::new(Path::new("note.md")),
    );
    assert_eq!(layout_only.changes.numbered, 0);
    assert_eq!(layout_only.notice, "Article formatted");

    let promoted = format(
        "---\ntype: [[article]]\n---\n\n## Parent\n#### Child\n",
        &ArticleContext::new(Path::new("note.md")),
    );
    assert!(promoted.notice.contains("1 heading promoted (#### → ###)"));
}

#[test]
fn pseudo_footnotes_only_exclude_url_like_destinations() {
    let input = concat!(
        "---\ntype: [[article]]\n---\n\n",
        "[b]: TODO: Update commit/line counts!\n",
        "[x]: https://example.test/path\n",
        "[y]: ./a.md\n",
        "[z]: #heading\n",
        "[mail]: MAILTO:person@example.test\n",
    );
    let plan = format(input, &ArticleContext::new(Path::new("note.md")));
    let findings = plan
        .findings
        .iter()
        .filter(|finding| finding.code == "pseudo-footnote")
        .collect::<Vec<_>>();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].line, 5);
}

#[test]
fn lazy_list_continuation_is_not_a_setext_heading() {
    let input = "---\ntype: [[article]]\n---\n\n- foo\nbar\n---\n";
    let plan = format(input, &ArticleContext::new(Path::new("note.md")));
    let output = plan.formatted.expect("formatted article");
    assert!(output.contains("- foo\nbar\n---"), "{output:?}");
    assert!(plan.headings.is_empty());
}

#[test]
fn frontmatter_only_article_does_not_gain_an_extra_blank_line() {
    let input = "---\ntype: [[article]]\n---\n";
    assert_eq!(article(input), input);
}

#[test]
fn invalid_section_numbers_finding_uses_the_option_line() {
    let input = "---\ntype: [[article]]\ntitle: Example\nsection_numbers: maybe\n---\n\n# Heading\n";
    let plan = format(input, &ArticleContext::new(Path::new("note.md")));
    let finding = plan
        .findings
        .iter()
        .find(|finding| finding.code == "bad-option")
        .expect("bad option finding");
    assert_eq!(finding.line, 4);
}

#[test]
fn nested_heading_paths_rewrite_every_segment_and_numbers_off_removes_them() {
    let nested = "---\ntype: [[article]]\n---\n\n# Parent\n\n## Child\n\n### Grandchild\n\n[[#Parent#Child#Grandchild]]\n";
    let plan = format(nested, &ArticleContext::new(Path::new("note.md")));
    let output = plan.formatted.expect("nested article format");
    assert!(
        output.contains("[[#1. Parent#1.1 Child#1.1.1 Grandchild]]"),
        "{output}"
    );

    let unnumbered = "---\ntype: [[article]]\nsection_numbers: false\n---\n\n## 8. Section\n\n[[#8. Section]]\n";
    let plan = format(unnumbered, &ArticleContext::new(Path::new("note.md")));
    let output = plan.formatted.expect("numbering-off format");
    assert!(output.contains("## Section"), "{output}");
    assert!(output.contains("[[#Section]]"), "{output}");
}

#[test]
fn report_only_findings_include_pseudo_footnote_broken_and_title() {
    let input = "---\ntype: [[article]]\n---\n\n[1]: TODO reference\n\n# 1. Article\n\n[[#missing]]\n";
    let plan = format(input, &ArticleContext::new(Path::new("article.md")));
    assert!(plan.findings.iter().any(|f| f.code == "pseudo-footnote"));
    assert!(plan
        .findings
        .iter()
        .any(|f| f.code == "broken-heading-link"));
    assert!(plan.findings.iter().any(|f| f.code == "title-heading"));
}

#[test]
fn outline_counts_words_and_marks_stubs() {
    let plan = format(
        include_str!("fixtures/blog_draft.md"),
        &ArticleContext::new(Path::new("sase_blog_contents.md")),
    );
    let outline = plan.outline;
    assert_eq!(
        serde_json::to_string_pretty(&outline).unwrap(),
        include_str!("fixtures/outline_blog_draft.json").trim_end()
    );
    assert_eq!(outline.title, "sase_blog_contents");
    assert!(outline.words > 0);
    assert!(outline.preamble.words > 0);
    assert!(outline.preamble.line > 0);
    assert_eq!(outline.sections.len(), 10);
    assert!(outline.sections.iter().all(|section| section.stub));
    assert_eq!(outline.sections[0].words, 0);
    assert!(outline.sections[1..]
        .iter()
        .all(|section| section.words == 1));
    assert!(outline
        .sections
        .iter()
        .all(|section| section.output_line > 0));
    assert_eq!(outline.sections[0].title, "The Abstractions");
}

#[test]
fn outline_word_counts_skip_fences_math_and_comments() {
    let input = "---\ntype: [[article]]\n---\n\nVisible words. Inline $ignored math$ prose.\n\n```md\nignored words\n```\n\n$$\nignored math words\n$$\n\n%%\nignored comment words\n%%\n\n<!-- hidden words -->\n\n# Section\n\nBody words.\n";
    let plan = format(input, &ArticleContext::new(Path::new("note.md")));
    assert_eq!(plan.outline.words, 8);
    assert_eq!(plan.outline.preamble.words, 4);
    assert_eq!(plan.outline.sections[0].words, 2);
}

#[test]
fn article_plan_json_matches_schema_v1_shape() {
    let plan = format(
        "---\ntype: [[article]]\n---\n\n## Alpha\n\nText.\n",
        &ArticleContext::new(Path::new("note.md")),
    );
    let response = super::response(vec![plan]);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(
        serde_json::to_string_pretty(&response).unwrap(),
        include_str!("fixtures/plan_v1.expected.json").trim_end()
    );
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["summary"]["articles"], 1);
    assert_eq!(json["summary"]["changed"], 1);
    let note = &json["notes"][0];
    assert_eq!(note["path"], "note.md");
    assert_eq!(note["status"], "changed");
    assert!(note["reason"].is_null());
    assert!(note["sha256"].as_str().is_some());
    assert!(note["edits"].is_array());
    assert!(note["changes"]["links"].is_number());
    assert!(note["replaced"].is_array());
    assert_eq!(note["notice"], "Article formatted · 1 section numbered");
    assert!(note["findings"].is_array());
    assert_eq!(note["outline"]["title"], "note");
    assert_eq!(note["outline"]["sections"][0]["title"], "Alpha");
    assert_eq!(note["inbound"], serde_json::json!([]));
    assert!(note.get("formatted").is_none());
}

#[test]
fn refusals() {
    let skip = |text: &str, path: &str| {
        format(text, &ArticleContext::new(Path::new(path)))
            .reason
            .map(|r| r.code)
    };
    assert_eq!(
        skip("---\ntype: [[project]]\n---\n# Hi\n", "note.md").as_deref(),
        Some("not-article")
    );
    assert_eq!(
        skip("---\ntype: [[article]]\n# Hi\n", "note.md").as_deref(),
        Some("frontmatter-unclosed")
    );
    assert_eq!(
        skip(
            "---\ntype: [\"[[article]]\", \"[[project]]\"]\n---\n# Hi\n",
            "note.md"
        )
        .as_deref(),
        Some("mixed-type")
    );
    assert_eq!(
        skip(
            "---\ntype: [[article]]\n---\n\n- [ ] #task do it ^x\n",
            "note.md"
        )
        .as_deref(),
        Some("holds-tasks")
    );
    assert_eq!(
        skip(
            "---\ntype: [[article]]\n---\n\nThis article mentions #task in prose.\n",
            "note.md"
        )
        .as_deref(),
        None
    );
    assert_eq!(
        skip(
            "---\ntype: [[article]]\n---\n\n```\n- [ ] #task example\n```\n\n# Title\n",
            "note.md"
        )
        .as_deref(),
        None
    );
    assert_eq!(
        skip(
            "---\ntype: [[article]]\n---\n\n- [ ] finish ^prj\n",
            "note.md"
        )
        .as_deref(),
        Some("holds-anchor")
    );
    assert_eq!(
        skip(
            "---\ntype: [[article]]\n---\n\n<<<<<<< HEAD\n# Title\n>>>>>>>\n",
            "note.md"
        )
        .as_deref(),
        Some("conflict-markers")
    );
    assert_eq!(
        skip(
            "---\ntype: [[article]]\n---\n\n# Title\n",
            "_templates/note.md"
        )
        .as_deref(),
        Some("excluded-directory")
    );
    assert_eq!(
        skip(
            "---\ntype: [[article]]\n---\n\n# Title\n",
            ".bob/ref-sources/chat/x--0123456789ab/source.md"
        )
        .as_deref(),
        Some("excluded-directory")
    );
}

#[test]
fn adopt_inserts_type_after_parent_and_formats() {
    let input = "---\nparent: \"[[sase_blog]]\"\ncreated: 2026-10-15\n---\n\n# Draft\n\n## First\n";
    let plan = adopt(input, &ArticleContext::new(Path::new("draft.md")));
    let formatted = plan.formatted.expect("adopt formats");
    assert!(formatted.contains("parent: \"[[sase_blog]]\""));
    assert!(formatted.contains("type: \"[[article]]\""));
    assert!(formatted.contains("# 1. Draft"));
    assert!(formatted.contains("## 1.1 First"));
    assert_eq!(plan.status, super::ArticleStatus::Changed);
    assert!(plan.notice.starts_with("Adopted as an article"));
    assert!(!plan.edits.is_empty());
    let type_line = formatted
        .lines()
        .position(|line| line.contains("type: \"[[article]]\""))
        .expect("type line");
    let parent_line = formatted
        .lines()
        .position(|line| line.contains("parent:"))
        .expect("parent line");
    assert_eq!(type_line, parent_line + 1);
}

#[test]
fn adopt_creates_frontmatter_when_missing_and_inserts_before_close() {
    let no_frontmatter =
        adopt("# Title\n", &ArticleContext::new(Path::new("note.md")));
    let formatted = no_frontmatter.formatted.expect("adopted");
    assert!(formatted.starts_with("---\ntype: \"[[article]]\"\n---\n"));
    assert!(formatted.contains("# 1. Title"));

    let no_parent = adopt(
        "---\ncreated: 2026-10-15\n---\n\n# Title\n",
        &ArticleContext::new(Path::new("note.md")),
    );
    let formatted = no_parent.formatted.expect("adopted");
    let lines: Vec<&str> = formatted.lines().collect();
    assert_eq!(lines[0], "---");
    assert_eq!(lines[1], "created: 2026-10-15");
    assert_eq!(lines[2], "type: \"[[article]]\"");
    assert_eq!(lines[3], "---");
}

#[test]
fn adopt_already_article_is_noop_and_other_type_is_refused() {
    let already = adopt(
        "---\ntype: \"[[article]]\"\n---\n\n# Title\n",
        &ArticleContext::new(Path::new("note.md")),
    );
    assert_eq!(already.status, super::ArticleStatus::Unchanged);
    assert_eq!(already.notice, "already an article");
    assert!(already.edits.is_empty());

    let restaurant = adopt(
        "---\ntype: \"[[restaurant]]\"\n---\n\n# Title\n",
        &ArticleContext::new(Path::new("note.md")),
    );
    assert_eq!(restaurant.status, super::ArticleStatus::Skipped);
    assert_eq!(
        restaurant
            .reason
            .as_ref()
            .map(|reason| reason.code.as_str()),
        Some("mixed-type")
    );
    assert_eq!(
        restaurant
            .reason
            .as_ref()
            .map(|reason| reason.message.as_str()),
        Some("note already has type [[restaurant]]")
    );

    let tasks = adopt(
        "---\nparent: \"[[home]]\"\n---\n\n- [ ] #task move me\n",
        &ArticleContext::new(Path::new("note.md")),
    );
    assert_eq!(
        tasks.reason.as_ref().map(|reason| reason.code.as_str()),
        Some("holds-tasks")
    );
}

#[test]
fn bad_option_and_number_replaced_findings() {
    let plan = format(
        "---\ntype: [[article]]\nsection_numbers: maybe\n---\n\n## 1.5 Title\n",
        &ArticleContext::new(Path::new("article.md")),
    );
    assert!(
        plan.findings.iter().any(|f| f.code == "bad-option"),
        "{:?}",
        plan.findings
    );
    assert!(
        plan.findings.iter().any(|f| f.code == "number-replaced"),
        "{:?}",
        plan.findings
    );
    assert!(plan.formatted.unwrap().contains("## 1. Title"));
}

#[test]
fn article_type_forms_are_detected() {
    let fm = parse_frontmatter("---\ntype: [[article]]\n---\n").unwrap();
    assert!(frontmatter_is_article(&fm));
}

#[test]
fn mutation_idempotent_and_words() {
    let seed = "---\ntype: [[article]]\n---\n\n# Alpha\n\nhello world\n\n## Beta\n\nmore words\n\n### Gamma\n\nstill words\n";
    let mut rng = Lcg(0xC0FF_EE42);
    for _ in 0..32 {
        let mutated = mutate(seed, &mut rng);
        let plan = format_as_article(&mutated);
        assert!(
            plan.reason.as_ref().is_none_or(
                |r| r.code != "words-changed" && r.code != "not-idempotent"
            ),
            "mutation failed: {:?}",
            plan.reason
        );
        if let Some(formatted) = plan.formatted {
            let again = format_as_article(&formatted);
            assert_eq!(again.formatted.as_deref(), Some(formatted.as_str()));
        }
    }
}

struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }
    fn choose(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

fn mutate(seed: &str, rng: &mut Lcg) -> String {
    let mut lines: Vec<String> = seed.lines().map(ToOwned::to_owned).collect();
    for line in &mut lines {
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes)
            && line.as_bytes().get(hashes).is_some_and(|b| *b == b' ')
        {
            let title = &line[hashes..];
            let level = 1 + rng.choose(6) as usize;
            *line = format!("{}{title}", "#".repeat(level));
        }
    }
    let mut out = Vec::new();
    for line in lines {
        if line.is_empty() {
            for _ in 0..=rng.choose(3) {
                out.push(String::new());
            }
        } else {
            out.push(line);
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[test]
#[ignore = "set BOB_ARTICLES_CORPUS to a vault of Markdown notes"]
fn corpus_no_panic_idempotent_and_words() {
    let Some(root) = std::env::var_os("BOB_ARTICLES_CORPUS") else {
        return;
    };
    let root = Path::new(&root);
    let mut files = Vec::new();
    collect_markdown(root, &mut files);
    let mut failures = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let relative = path.strip_prefix(root).unwrap_or(path);
        let plan = format(&text, &ArticleContext::bypass_marker(relative));
        if let Some(reason) = &plan.reason
            && matches!(
                reason.code.as_str(),
                "words-changed" | "not-idempotent"
            )
        {
            failures.push(format!(
                "{}: {}",
                relative.display(),
                reason.message
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} corpus failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn collect_markdown(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if crate::native::is_always_excluded_note_directory_name(
                &entry.file_name(),
            ) {
                continue;
            }
            collect_markdown(&path, files);
        } else if file_type.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("md")
        {
            files.push(path);
        }
    }
}
