use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{
    bands, corpus, query_related_in, score, tokenize, CaptureRelatedResult,
};
use crate::native::env::TestEnvGuard;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            current_time_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("create temp dir {}: {error}", path.display())
        });
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            eprintln!("failed to remove {}: {error}", self.path.display());
        }
    }
}

fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn write_settings(root: &Path) {
    write_file(
        &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
}

fn area_note(title: &str, tasks: &str) -> String {
    format!("---\ntype: [[area]]\n---\n# {title}\n{tasks}")
}

fn terms(title: &str) -> Vec<String> {
    let display = tokenize::display_title(title, "#task", None);
    tokenize::tokenize(&display)
        .into_iter()
        .map(|token| token.term)
        .collect()
}

fn weights(title: &str) -> Vec<(String, f64)> {
    let display = tokenize::display_title(title, "#task", None);
    tokenize::tokenize(&display)
        .into_iter()
        .map(|token| (token.term, token.weight))
        .collect()
}

#[test]
fn tokenizer_table_covers_bob_aware_cases() {
    let cases: &[(&str, &[&str])] = &[
        (
            "Add `%d` short-hand for `%dispatch`!",
            &["add", "%d", "short", "hand", "%dispatch"],
        ),
        (
            "Press <ctrl+shift+m> to move",
            &["press", "ctrl+shift+m", "move"],
        ),
        (
            "Read [[path/to/Note|The Alias]] today",
            &["read", "alias", "today"],
        ),
        ("Call the bank #errand ^bank-id", &["call", "bank"]),
        (
            "Ship it [created:: 2026-10-01] [due:: 2026-10-20]",
            &["ship"],
        ),
        (
            "See https://example.com/docs for details",
            &["see", "detail"],
        ),
        (
            "Buy oat milk 🛒 [💡](https://keep.google.com/u/0/#NOTE/id)",
            &["buy", "oat", "milk"],
        ),
        (
            "parseHTTPResponse now",
            &["parse", "http", "response", "now"],
        ),
        (
            "snake_case and kebab-case names",
            &["snake", "case", "kebab", "case", "name"],
        ),
        ("templated templates", &["templat", "templat"]),
        ("The tasks are ready", &["task", "ready"]),
        (
            "Sase projects should be ready to be talked about",
            &["sase", "project", "ready", "talk"],
        ),
        (
            "Read Part 2 of Bryan's essay",
            &["read", "part", "2", "bryan", "essay"],
        ),
        (
            "Sign up for job_2026_apply_29",
            &["sign", "up", "job", "2026", "apply", "29"],
        ),
    ];
    for (title, expected) in cases {
        assert_eq!(
            terms(title),
            expected
                .iter()
                .map(|term| (*term).to_string())
                .collect::<Vec<_>>(),
            "tokens for {title:?}"
        );
    }
    let chord_weights = weights("Press <ctrl+shift+m> to move");
    assert!(
        chord_weights.iter().any(|(term, weight)| {
            term == "ctrl+shift+m" && (*weight - 2.0).abs() < f64::EPSILON
        }),
        "{chord_weights:?}"
    );
    let code_weights = weights("Add `%d` here");
    assert!(
        code_weights.iter().any(|(term, weight)| {
            term == "%d" && (*weight - 2.0).abs() < f64::EPSILON
        }),
        "{code_weights:?}"
    );
}

#[test]
fn stopwords_stay_sorted_for_binary_search() {
    assert!(
        tokenize::STOPWORDS.windows(2).all(|pair| pair[0] < pair[1]),
        "STOPWORDS must be sorted and unique"
    );
}

#[test]
fn tokenizer_records_display_spans() {
    let display =
        tokenize::display_title("Install new UPS battery", "#task", None);
    let tokens = tokenize::tokenize(&display);
    let ups = tokens.iter().find(|token| {
        display[token.start..token.end].eq_ignore_ascii_case("UPS")
    });
    let Some(ups) = ups else {
        panic!("missing UPS token in {tokens:?} for {display:?}");
    };
    assert_eq!(&display[ups.start..ups.end], "UPS");
}

#[test]
fn same_title_normalization_strips_punctuation_and_tags() {
    assert_eq!(
        tokenize::normalize_title("Sell stocks! #now"),
        tokenize::normalize_title("sell stocks")
    );
    assert_ne!(
        tokenize::normalize_title("Sell stocks now!"),
        tokenize::normalize_title("Sell stocks!")
    );
}

fn fixture_vault() -> TempDir {
    let temp = TempDir::new("bob-cli-capture-related");
    write_settings(temp.path());
    write_file(
        &temp.path().join("dev.md"),
        &area_note(
            "Dev",
            "- [ ] #task Add support for bob capture templates! ^templates\n\
- [ ] #task Make appointment for CT scan! ^ct-scan\n\
- [ ] #task Add `!<cmd>` short-hand for `%proc(\"<cmd>\")`! ^proc-shorthand\n\
- [ ] #task Install new UPS battery ^ups\n\
- [?] #task Order a replacement UPS battery for athena ^ups-order\n\
- [ ] #task Review [[docs/plan|The Plan]] ^plan-read #ref\n\
- [ ] #task Morning review #gtd #pre ^gtd-pre\n\
- [x] #task Closed yesterday [completion:: 2026-10-19] ^closed-19\n\
- [x] #task Closed 29 days ago [completion:: 2026-09-21] ^closed-29\n\
- [x] #task Closed 30 days ago [completion:: 2026-09-20] ^closed-30\n\
- [x] #task Closed 31 days ago [completion:: 2026-09-19] ^closed-31\n\
- [x] #task Recurring chore 🔁 [repeat:: every month] [completion:: 2026-10-10] ^recurring\n",
        ),
    );
    write_file(
        &temp.path().join("health.md"),
        &area_note(
            "Health",
            "- [/] #task Make appt w/ Rahway Hospital for CT scan! ^rahway\n\
- [*] #task Buy oat milk ^milk\n",
        ),
    );
    write_file(
        &temp.path().join("ship.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Ship the matcher ^prj\n",
    );
    write_file(
        &temp.path().join("mac_inbox.md"),
        "- [ ] #task Inbox only\n",
    );
    temp
}

fn query(root: &Path, text: &str) -> CaptureRelatedResult {
    let _now = TestEnvGuard::set(&[(
        "BOB_NOW",
        Some(std::ffi::OsStr::new("2026-10-20 12:00:00")),
    )]);
    query_related_in(root, text, 30, 5, None)
}

fn bands_for(result: &CaptureRelatedResult, title: &str) -> Vec<String> {
    result
        .items
        .iter()
        .flat_map(|item| item.matches.iter())
        .filter(|related| related.title.contains(title))
        .map(|related| related.band.clone())
        .collect()
}

#[test]
fn scorer_pairs_from_research() {
    let temp = fixture_vault();
    let templates = query(temp.path(), "Add support for templated capture!");
    let template_match = templates.items[0]
        .matches
        .iter()
        .find(|related| related.title.contains("capture templates"))
        .expect("templates counterpart");
    assert_eq!(
        template_match.band, "possible_duplicate",
        "{template_match:?}"
    );

    let ct = query(temp.path(), "Make appt w/ Rahway Hospital for CT scan!");
    let ct_match = ct.items[0]
        .matches
        .iter()
        .find(|related| related.title.contains("appointment for CT scan"))
        .expect("ct counterpart");
    assert!(
        matches!(
            ct_match.band.as_str(),
            "hold" | "similar" | "possible_duplicate"
        ),
        "CT pair band {} score {}",
        ct_match.band,
        ct_match.score.expect("query score")
    );

    let code = query(temp.path(), "Add `%d` short-hand for `%dispatch`!");
    let code_hit = code.items[0].matches.iter().find(|related| {
        related.title.contains("!<cmd>") || related.title.contains("%proc")
    });
    if let Some(hit) = code_hit {
        assert_ne!(hit.band, "similar");
        assert_ne!(hit.band, "possible_duplicate");
        assert_ne!(hit.band, "same_title");
        assert!(
            hit.score.expect("query score") < bands::SIMILAR,
            "code-span pair scored {}",
            hit.score.expect("query score")
        );
    }
}

#[test]
fn band_edges_and_short_query_rules() {
    let temp = fixture_vault();
    let same = query(temp.path(), "Install new UPS battery");
    assert!(
        bands_for(&same, "Install new UPS battery")
            .iter()
            .any(|band| band == "same_title"),
        "{same:?}"
    );

    let short = query(temp.path(), "battery");
    assert!(
        short.items[0].matches.iter().all(|related| {
            related.band == "same_title" || related.band == "closed_duplicate"
        }),
        "short query must not fire lexical bands: {:?}",
        short.items[0].matches
    );
}

#[test]
fn numbers_that_differ_cap_duplicates_at_similar() {
    let temp = TempDir::new("bob-cli-capture-related-numbers");
    write_settings(temp.path());
    write_file(
        &temp.path().join("reading.md"),
        &area_note(
            "Reading",
            "- [ ] #task Watch Stripe one-shot coding agents talk part 2 ^talk-2\n\
- [ ] #task Order printer paper ^paper\n\
- [ ] #task Call the dentist ^dentist\n\
- [x] #task File passport renewal forms 2025 [completion:: 2026-10-19] ^passport\n",
        ),
    );

    let control = query(
        temp.path(),
        "Watch the Stripe one-shot coding agents talk, part 2!",
    );
    assert_eq!(
        bands_for(&control, "agents talk part 2"),
        ["possible_duplicate"],
        "{control:?}"
    );

    let other_part = query(
        temp.path(),
        "Watch Stripe one-shot coding agents talk part 3",
    );
    assert_eq!(
        bands_for(&other_part, "agents talk part 2"),
        ["similar"],
        "{other_part:?}"
    );

    let unnumbered =
        query(temp.path(), "Watch Stripe one-shot coding agents talk part");
    assert_eq!(
        bands_for(&unnumbered, "agents talk part 2"),
        ["similar"],
        "{unnumbered:?}"
    );

    let closed_twin = query(temp.path(), "File passport renewal forms 2025!");
    assert_eq!(
        bands_for(&closed_twin, "passport renewal"),
        ["closed_duplicate"],
        "{closed_twin:?}"
    );
    let next_year = query(temp.path(), "File passport renewal forms 2026");
    assert!(
        bands_for(&next_year, "passport renewal").is_empty(),
        "{next_year:?}"
    );
}

#[test]
fn ordering_prefers_band_then_open_then_status() {
    let temp = fixture_vault();
    let result = query(temp.path(), "Install new UPS battery");
    let titles: Vec<&str> = result.items[0]
        .matches
        .iter()
        .map(|related| related.title.as_str())
        .collect();
    let same = titles
        .iter()
        .position(|title| *title == "Install new UPS battery")
        .expect("same title");
    if let Some(possible) = titles
        .iter()
        .position(|title| title.contains("replacement UPS battery"))
    {
        assert!(same < possible, "{titles:?}");
    }
}

#[test]
fn closed_window_includes_30_days_and_excludes_31() {
    let temp = fixture_vault();
    let result = query(temp.path(), "Closed 30 days ago");
    assert!(
        result.items[0]
            .matches
            .iter()
            .any(|related| related.title.contains("Closed 30 days ago")),
        "{result:?}"
    );
    let too_old = query(temp.path(), "Closed 31 days ago");
    assert!(
        too_old.items[0].matches.iter().all(|related| {
            !related.title.contains("Closed 31 days ago")
                || related.band == "same_title"
        }),
        "31-day closed task leaked: {:?}",
        too_old.items[0].matches
    );
    let twenty_nine = query(temp.path(), "Closed 29 days ago");
    assert!(
        twenty_nine.items[0]
            .matches
            .iter()
            .any(|related| related.title.contains("Closed 29 days ago")),
        "{twenty_nine:?}"
    );
}

#[test]
fn excludes_gtd_checklist_and_closed_recurring_rows() {
    let temp = fixture_vault();
    let gtd = query(temp.path(), "Morning review");
    assert!(
        gtd.items[0]
            .matches
            .iter()
            .all(|related| !related.title.contains("Morning review")),
        "{gtd:?}"
    );
    let recurring = query(temp.path(), "Recurring chore");
    assert!(
        recurring.items[0]
            .matches
            .iter()
            .all(|related| !related.title.contains("Recurring chore")),
        "{recurring:?}"
    );
}

#[test]
fn archive_closed_window_respects_mtime() {
    let temp = fixture_vault();
    let archive = temp.path().join("done/dev_done.md");
    write_file(
        &archive,
        "---\ntype: [[area]]\n---\n- [x] #task Archived UPS swap [completion:: 2026-10-01] ^archived-ups\n",
    );
    let recent = SystemTime::now() - Duration::from_secs(2 * 86400);
    fs::File::open(&archive)
        .expect("open archive")
        .set_modified(recent)
        .expect("set archive mtime");
    let result = query(temp.path(), "Archived UPS swap");
    assert!(
        result.items[0]
            .matches
            .iter()
            .any(|related| related.title.contains("Archived UPS swap")),
        "{result:?}"
    );

    let stale = temp.path().join("done/old_done.md");
    write_file(
        &stale,
        "---\ntype: [[area]]\n---\n- [x] #task Stale archive task [completion:: 2026-10-01] ^stale-arch\n",
    );
    let old = SystemTime::now() - Duration::from_secs(40 * 86400);
    fs::File::open(&stale)
        .expect("open stale archive")
        .set_modified(old)
        .expect("set stale mtime");
    let stale_result = query(temp.path(), "Stale archive task");
    assert!(
        stale_result.items[0]
            .matches
            .iter()
            .all(|related| !related.title.contains("Stale archive task")),
        "{stale_result:?}"
    );
}

#[test]
fn synthetic_vault_query_is_under_250ms() {
    let temp = TempDir::new("bob-cli-capture-related-perf");
    write_settings(temp.path());
    for note in 0..40 {
        let mut body = String::from("---\ntype: [[area]]\n---\n");
        for task in 0..38 {
            body.push_str(&format!(
                "- [ ] #task Perf note {note} task {task} unique-{note}-{task} ^n{note}-t{task}\n"
            ));
        }
        write_file(&temp.path().join(format!("area{note}.md")), &body);
    }
    write_file(
        &temp.path().join("done/area0_done.md"),
        "---\ntype: [[area]]\n---\n- [x] #task Archived perf [completion:: 2026-10-01] ^arch-perf\n",
    );
    let _now = TestEnvGuard::set(&[(
        "BOB_NOW",
        Some(std::ffi::OsStr::new("2026-10-20 12:00:00")),
    )]);
    let query = "Perf note 7 task 3 unique-7-3";
    let _warmup = query_related_in(temp.path(), query, 30, 5, None);
    let started = std::time::Instant::now();
    let result = query_related_in(temp.path(), query, 30, 5, None);
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 500,
        "query took {} ms (isolated debug target is 250ms; 500ms absorbs parallel just check load)",
        elapsed.as_millis()
    );
    assert!(
        result.items[0]
            .matches
            .iter()
            .any(|related| related.band == "same_title"),
        "{result:?}"
    );
}

#[test]
fn synthetic_vault_draft_contract_is_under_250ms() {
    let temp = TempDir::new("bob-cli-capture-related-draft-perf");
    write_settings(temp.path());
    for note in 0..40 {
        let mut body = String::from("---\ntype: [[area]]\n---\n");
        for task in 0..38 {
            body.push_str(&format!(
                "- [ ] #task Perf note {note} task {task} unique-{note}-{task} ^n{note}-t{task}\n"
            ));
        }
        write_file(&temp.path().join(format!("area{note}.md")), &body);
    }
    let _now = TestEnvGuard::set(&[(
        "BOB_NOW",
        Some(std::ffi::OsStr::new("2026-10-20 12:00:00")),
    )]);
    let request = super::DraftRequest {
        bob_dir: temp.path().to_path_buf(),
        days: 30,
        limit: 5,
        draft: "Perf note 7 task 3 unique-7-3 @area7".to_string(),
    };
    let _warmup = super::draft::draft_related(&request);
    let started = std::time::Instant::now();
    let result = super::draft::draft_related(&request);
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 500,
        "draft call took {} ms (debug target allows 500ms under parallel just check load)",
        elapsed.as_millis()
    );
    assert_eq!(result.items.len(), 1);
}

#[test]
fn cosine_is_symmetric_on_identical_titles() {
    let left = tokenize::tokenize("Install new UPS battery");
    let right = corpus::Candidate {
        key: "dev.md#ups".into(),
        title: "Install new UPS battery".into(),
        tokens: tokenize::tokenize("Install new UPS battery"),
        kind: corpus::CandidateKind::Task,
        status_symbol: ' ',
        status_name: "Todo".into(),
        status_type: "TODO",
        closed: None,
        route: "dev".into(),
        locator: "dev".into(),
        note_path: "dev.md".into(),
        target: PathBuf::from("/tmp/dev.md"),
        line: 1,
        task_ref: "1:abcd1234".into(),
        block_id: Some("ups".into()),
        block_id_suggestions: Vec::new(),
        created: None,
        open: true,
    };
    let scores = score::cosine_scores(&left, std::slice::from_ref(&right));
    assert!(scores[0] > 0.99, "{}", scores[0]);
}
