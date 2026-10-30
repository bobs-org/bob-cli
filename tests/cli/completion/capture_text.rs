//! Capture markers inside capture TEXT: shell completion through the
//! in-process `capture_complete` extraction.
//!
//! Every case runs the built binary against a small fixture vault with
//! `BOB_NOW` pinned, so the goldens cover the presenter, the `!prefix`
//! directive, the release gates, and the providers together.

use crate::support::*;
use std::process::Output;
use std::time::Instant;

const BOB_NOW: &str = "2026-06-01 09:10:01";

struct Fixture {
    _temp: TempDir,
    vault: std::path::PathBuf,
}

/// A vault with an area (`cash`), a project (`dev`), open tasks with block
/// IDs (plus one without, for the safe-rows gate), task sections, active
/// tasks (`/` and `*`), and a daily note with open Pomodoros.
fn fixture() -> Fixture {
    let temp = TempDir::new("bob-cli-complete-capture-text");
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "# Cash\n",
            "\n",
            "## Shopping\n",
            "\n",
            "## Ideas\n",
            "\n",
            "- [ ] #task Buy oat milk ^buy-milk\n",
            "- [ ] #task Fix the sink ^fix-sink\n",
            "  - FIRST STEPS\n",
            "    - gather parts\n",
            "  - SECOND STEPS\n",
            "- [ ] #task No ID yet\n",
            "- [/] #task Active job ^active-job\n",
            "- [*] #task Next job ^next-job\n",
            "- [x] #task Done already ^done-old\n",
        ),
    );
    write_file(
        &vault.join("dev.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "status: active\n",
            "---\n",
            "# Dev\n",
            "\n",
            "- [ ] #task Ship the release ^ship-it\n",
        ),
    );
    write_file(
        &vault.join("2026/20260601.md"),
        concat!(
            "# 2026-06-01\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (0900-0930) Morning pages\n",
            "- [ ] () — DEEP WORK\n",
            "- [x] (0700-0730) Done early\n",
        ),
    );
    Fixture { _temp: temp, vault }
}

fn complete(fixture: &Fixture, words: &[&str]) -> Output {
    let mut command = bob_completion_command();
    command
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", BOB_NOW);
    for word in words {
        command.arg(word);
    }
    command.output().expect("run bob __complete")
}

fn complete_with_config(
    fixture: &Fixture,
    config: &std::path::Path,
    words: &[&str],
) -> Output {
    let mut command = bob_completion_command();
    command
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", BOB_NOW)
        .env("BOB_CONFIG_FILE", config)
        .env("XDG_CACHE_HOME", fixture._temp.path().join("cache"))
        .args(words);
    command.output().expect("run configured bob __complete")
}

fn complete_with_suffix(
    fixture: &Fixture,
    suffix: &str,
    words: &[&str],
) -> Output {
    let mut command = bob_completion_command();
    command
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--suffix")
        .arg(suffix)
        .arg("--")
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", BOB_NOW);
    for word in words {
        command.arg(word);
    }
    command.output().expect("run bob __complete")
}

fn lines(output: &Output) -> Vec<String> {
    stdout(output).lines().map(str::to_string).collect()
}

fn values(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').next().unwrap_or_default().to_string())
        .collect()
}

fn directives(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .filter(|line| line.starts_with('!'))
        .map(str::to_string)
        .collect()
}

fn groups(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').nth(2).unwrap_or_default().to_string())
        .collect()
}

#[test]
fn routes_complete_at_end_of_text_word() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "fix", "it", "@"]);
    assert_success(&output);
    let values = values(&output);
    assert!(values.contains(&"@mac_inbox".to_string()), "{values:?}");
    assert!(values.contains(&"@cash".to_string()), "{values:?}");
    assert!(values.contains(&"@dev".to_string()), "{values:?}");
    // No prefix directive for a bare marker word.
    assert!(directives(&output).is_empty(), "{:?}", lines(&output));
    assert!(groups(&output).contains(&"inbox".to_string()));
    assert!(groups(&output).contains(&"areas".to_string()));
    assert!(groups(&output).contains(&"projects".to_string()));
}

#[test]
fn bare_plus_shell_completion_returns_only_identified_parent_markers() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "+"]);
    assert_success(&output);
    let values = values(&output);
    assert!(values.contains(&"@cash+buy-milk".to_string()), "{values:?}");
    assert!(values.contains(&"@cash+fix-sink".to_string()), "{values:?}");
    assert!(values.contains(&"@dev+ship-it".to_string()), "{values:?}");
    assert!(
        values.iter().all(|value| !value.ends_with("+")),
        "{values:?}"
    );
    assert!(
        !stdout(&output).contains("No ID yet"),
        "shell completion must omit ID-less task rows: {}",
        stdout(&output)
    );
}

#[test]
fn partial_route_returns_full_set_for_shell_filtering() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "fix", "it", "@ca"]);
    assert_success(&output);
    // Bob never prefix-filters: the shell keeps filtering, so `@ca` still
    // lists every route.
    let values = values(&output);
    assert!(values.contains(&"@mac_inbox".to_string()), "{values:?}");
    assert!(values.contains(&"@cash".to_string()), "{values:?}");
    assert!(values.contains(&"@dev".to_string()), "{values:?}");
}

#[test]
fn task_type_shell_completion_returns_configured_t_type_rows() {
    let fixture = fixture();
    let config = fixture._temp.path().join("config.yml");
    write_file(&config, "types:\n  BUILD: yellow\n  FIX: blue\n");
    let output = complete_with_config(
        &fixture,
        &config,
        &["bob", "capture", "Fix", "the", "parser", "t:"],
    );

    assert_success(&output);
    let candidates = values(&output);
    assert!(
        candidates.contains(&"t:build".to_string()),
        "{candidates:?}"
    );
    assert!(candidates.contains(&"t:fix".to_string()), "{candidates:?}");
    assert!(groups(&output).contains(&"task types".to_string()));
    assert!(stdout(&output).contains("your type · never used"));

    let typed = complete_with_config(
        &fixture,
        &config,
        &["bob", "capture", "Fix the parser t:fixs"],
    );
    assert_success(&typed);
    assert!(values(&typed).contains(&"t:fixs".to_string()));

    write_file(
        &fixture.vault.join(".bob/themes/test.jsonl"),
        "{\"v\":1,\"ts\":\"2026-06-01T08:00:00Z\",\"host\":\"test\",\"kind\":\"theme.hide\",\"key\":\"fix\",\"name\":\"FIX\",\"via\":\"test\"}\\n",
    );
    let hidden = complete_with_config(
        &fixture,
        &config,
        &["bob", "capture", "Fix the parser t:fix"],
    );
    assert_success(&hidden);
    assert!(values(&hidden).contains(&"t:fix".to_string()));
}

#[test]
fn solo_route_colon_lists_existing_tasks() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "@dev:"]);
    assert_success(&output);
    let values = values(&output);
    assert!(values.contains(&"@dev:ship-it".to_string()), "{values:?}");
    assert!(groups(&output).contains(&"tasks in dev".to_string()));
}

#[test]
fn body_bearing_route_colon_suggests_new_id() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "fix", "it", "@dev:"]);
    assert_success(&output);
    let values = values(&output);
    assert!(values.contains(&"@dev:fix".to_string()), "{values:?}");
    assert!(!values.contains(&"@dev:ship-it".to_string()), "{values:?}");
    assert!(groups(&output).contains(&"new task ID".to_string()));
    assert!(
        !groups(&output).contains(&"tasks in dev".to_string()),
        "{:?}",
        lines(&output)
    );
}

#[test]
fn body_bearing_text_agrees_with_capture_complete_intent() {
    let fixture = fixture();
    let draft = "fix it @dev:";
    let output = complete(&fixture, &["bob", "capture", "fix", "it", "@dev:"]);
    assert_success(&output);
    // `capture-complete -f json` on the same text reports `new` intent
    // with suggestions and used IDs.
    let mut command = bob_command();
    command
        .arg("capture-complete")
        .arg("-b")
        .arg(&fixture.vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", BOB_NOW);
    let completed = command.output().expect("run capture-complete");
    assert_success(&completed);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout(&completed).trim()).expect("complete JSON");
    assert_eq!(parsed["block_id"]["intent"], "new");
    let used: Vec<String> = parsed["block_id"]["used"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry["id"].as_str().map(str::to_string))
        .collect();
    assert!(used.contains(&"ship-it".to_string()), "{parsed}");
    // Shell completion offers the suggestions, never a used ID.
    let values = values(&output);
    let suggestions: Vec<String> = parsed["block_id"]["suggestions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.as_str().map(str::to_string))
        .collect();
    assert!(!suggestions.is_empty(), "{parsed}");
    for suggestion in &suggestions {
        assert!(
            values.contains(&format!("@dev:{suggestion}")),
            "{values:?} vs {parsed}"
        );
    }
    for id in &used {
        assert!(
            !values.iter().any(|value| value == &format!("@dev:{id}")),
            "{values:?} lists used {id}"
        );
    }
}

#[test]
fn sections_complete_after_hash() {
    let fixture = fixture();
    let output =
        complete(&fixture, &["bob", "capture", "jot", "idea", "@cash#"]);
    assert_success(&output);
    let values = values(&output);
    assert!(
        values.iter().any(|value| value.starts_with("@cash#")),
        "{values:?}"
    );
    assert!(groups(&output).contains(&"sections in cash".to_string()));
}

#[test]
fn task_sections_complete_after_id_hash() {
    let fixture = fixture();
    let output = complete(
        &fixture,
        &["bob", "capture", "fix", "it", "@cash+fix-sink#"],
    );
    assert_success(&output);
    let values = values(&output);
    assert!(
        values.contains(&"@cash+fix-sink#first-steps".to_string()),
        "{values:?}"
    );
    assert!(groups(&output).contains(&"task sections".to_string()));
}

#[test]
fn caret_completes_active_tasks() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "^"]);
    assert_success(&output);
    let values = values(&output);
    assert!(
        values.contains(&"^cash:active-job".to_string()),
        "{values:?}"
    );
    assert!(values.contains(&"^cash:next-job".to_string()), "{values:?}");
    assert!(groups(&output).contains(&"active tasks".to_string()));
}

#[test]
fn named_start_completes_after_hash() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "=#"]);
    assert_success(&output);
    let values = values(&output);
    assert!(!values.is_empty(), "expected start names");
    assert!(groups(&output).contains(&"open Pomodoros".to_string()));
}

#[test]
fn quoted_word_keeps_prefix_with_prefix_directive() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "fix it @dev:"]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!prefix 7".to_string()]);
    // Body-bearing text suggests a new ID, never the existing task.
    assert!(values(&output).contains(&"@dev:fix".to_string()));
    assert!(!values(&output).contains(&"@dev:ship-it".to_string()));
}

#[test]
fn unicode_prefix_counts_chars_not_bytes() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "café @dev:"]);
    assert_success(&output);
    // `café ` is five Unicode scalar values (é is one), six bytes.
    assert_eq!(directives(&output), vec!["!prefix 5".to_string()]);
    assert!(values(&output).contains(&"@dev:caf".to_string()));
}

#[test]
fn suffix_returns_nothing() {
    let fixture = fixture();
    let output = complete_with_suffix(
        &fixture,
        "x",
        &["bob", "capture", "fix", "it", "@dev:"],
    );
    assert_success(&output);
    assert!(lines(&output).is_empty());
}

#[test]
fn newline_in_word_returns_nothing() {
    let fixture = fixture();
    let output =
        complete(&fixture, &["bob", "capture", "fix", "it", "@dev:\nfoo"]);
    assert_success(&output);
    assert!(lines(&output).is_empty());
}

#[test]
fn options_stay_hidden_once_text_starts() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "fix", "--r"]);
    assert_success(&output);
    assert!(
        !values(&output).iter().any(|value| value == "--route"),
        "{:?}",
        lines(&output)
    );
}

#[test]
fn wikilinks_return_nothing_quickly() {
    let fixture = fixture();
    let wiki_words = ["bob", "capture", "fix", "[["];
    let baseline_words = ["bob", "capture", "--format", ""];

    // Warm both paths, then compare paired process runs. The static format
    // completion includes process startup and scheduler delay but avoids the
    // vault index, so subtracting it keeps the 150 ms fast-path budget useful
    // under parallel test load.
    let baseline = complete(&fixture, &baseline_words);
    assert_success(&baseline);
    let wiki = complete(&fixture, &wiki_words);
    assert_success(&wiki);
    assert!(lines(&wiki).is_empty());

    let mut extra = Vec::with_capacity(7);
    for _ in 0..7 {
        let started = Instant::now();
        let wiki = complete(&fixture, &wiki_words);
        let wiki_elapsed = started.elapsed();
        assert_success(&wiki);
        assert!(lines(&wiki).is_empty());

        let started = Instant::now();
        let baseline = complete(&fixture, &baseline_words);
        let baseline_elapsed = started.elapsed();
        assert_success(&baseline);

        extra.push(wiki_elapsed.saturating_sub(baseline_elapsed));
    }
    let fastest_extra = extra.iter().min().expect("paired completion samples");
    assert!(
        fastest_extra.as_millis() < 150,
        "wikilink added {fastest_extra:?} over the static completion baseline; paired samples: {extra:?}"
    );
}

#[test]
fn safe_rows_only_omit_missing_ids() {
    let fixture = fixture();
    // `@cash+` completes parent tasks: the `No ID yet` row carries
    // `requires_block_id` and must stay out.
    let output = complete(&fixture, &["bob", "capture", "@cash+"]);
    assert_success(&output);
    for line in lines(&output) {
        assert!(!line.contains("No ID yet"), "{line}");
    }
}

#[test]
fn capture_parse_and_rewrite_share_the_text_slot() {
    let fixture = fixture();
    for command in ["capture-parse", "capture-rewrite"] {
        let output =
            complete(&fixture, &["bob", command, "fix", "it", "@dev:"]);
        assert_success(&output);
        assert!(
            values(&output).contains(&"@dev:fix".to_string()),
            "{command}: {:?}",
            lines(&output)
        );
    }
}

#[test]
fn close_shorthands_offer_nothing() {
    let fixture = fixture();
    // Action items offer nothing, exactly as in `capture-complete`:
    // closes (`x…`, `x*`, `x!`), starts (`=`, `=<X>`), adjustments,
    // shifts, and Work Log text on or below the `x` line.
    for words in [
        vec!["bob", "capture", "x done @"],
        vec!["bob", "capture", "x*"],
        vec!["bob", "capture", "x!"],
    ] {
        let output = complete(&fixture, &words);
        assert_success(&output);
        assert!(lines(&output).is_empty(), "{words:?}: {:?}", lines(&output));
    }
}

#[test]
fn named_start_after_inline_close_keeps_prefix() {
    let fixture = fixture();
    let output = complete(&fixture, &["bob", "capture", "x wired it =#"]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!prefix 11".to_string()]);
    assert!(values(&output).contains(&"=#deep-work".to_string()));
    assert!(groups(&output).contains(&"open Pomodoros".to_string()));
}

#[test]
fn relation_labels_complete_after_double_pipe() {
    let fixture = fixture();
    write_file(
        &fixture.vault.join("tasks.md"),
        "- [ ] #task Target ^target\n  - 🪢 **LINKS:** see [[cash#^buy-milk]]\n",
    );
    // A `||` probe completes label rows with the one-off spelling.
    let output = complete(
        &fixture,
        &["bob", "capture", "Fix", "it", "~tasks:target||s"],
    );
    assert_success(&output);
    let vals = values(&output);
    assert!(vals.contains(&"~tasks:target||see".to_string()), "{vals:?}");
    assert!(groups(&output).contains(&"relation labels".to_string()));
    // A `|` probe keeps the single-pipe spelling on the same rows.
    let output = complete(
        &fixture,
        &["bob", "capture", "Fix", "it", "~tasks:target|s"],
    );
    assert_success(&output);
    let vals = values(&output);
    assert!(vals.contains(&"~tasks:target|see".to_string()), "{vals:?}");
}
