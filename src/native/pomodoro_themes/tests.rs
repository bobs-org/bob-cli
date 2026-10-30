use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::NaiveDate;
use serde::Deserialize;

use crate::native::{
    config::PlanConfig,
    env::TestEnvGuard,
    plan_budget::{normalize_component, split_components},
    pomodoro_homes::{fallback_hue, HomeResolver, Hue, IndexedHomeSource},
    task_theme::{
        clear_task_theme, parse_task_theme, set_task_theme, TaskThemeState,
    },
    vault_links::NoteIndex,
};

use super::{
    journal, rank_history, resolve_hue, scan,
    similar::{similar_to, SimilarTheme},
    suggest::{
        suggest_for_new_task, suggest_for_task, SuggestResult, ThemeSuggestion,
    },
    Catalog, HueSource, RankTheme, ThemeLookup,
};

fn isolate_catalog_env(cache: &std::path::Path) -> TestEnvGuard {
    TestEnvGuard::set(&[
        ("XDG_CACHE_HOME", Some(cache.as_os_str())),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent/bob-cli-test-config.yml")),
        ),
    ])
}

#[test]
fn catalog_store_prunes_vanished_vaults_and_sweeps_orphan_files() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let live = tempfile::tempdir().expect("live vault");
    let vanished = tempfile::tempdir().expect("vanished vault");
    let live_key = super::cache::vault_key(live.path());
    let vanished_key = super::cache::vault_key(vanished.path());
    let catalog = super::cache::CacheFile {
        version: 1,
        vaults: BTreeMap::from([
            (live_key.clone(), super::cache::VaultCache::default()),
            (vanished_key.clone(), super::cache::VaultCache::default()),
        ]),
    };
    super::cache::store(&catalog, None);

    let themes_dir = super::cache::cache_path()
        .parent()
        .expect("themes cache directory")
        .to_path_buf();
    let live_completion = super::cache::completion_path(live.path());
    let orphan_completion =
        themes_dir.join(format!("completion-v1-{}.json", "0".repeat(64)));
    let stale_temp = themes_dir.join("catalog-v1.json.123.tmp");
    let fresh_temp = themes_dir.join("catalog-v1.json.456.tmp");
    fs::write(&live_completion, "live completion").unwrap();
    fs::write(&orphan_completion, "orphan completion").unwrap();
    fs::write(&stale_temp, "stale write").unwrap();
    fs::write(&fresh_temp, "active write").unwrap();
    let stale_file = fs::File::options().write(true).open(&stale_temp).unwrap();
    stale_file
        .set_times(
            fs::FileTimes::new().set_modified(
                SystemTime::now() - Duration::from_secs(10 * 60 + 1),
            ),
        )
        .unwrap();

    drop(vanished);
    super::cache::store(&catalog, Some(&live_key));

    let stored = super::cache::load();
    assert!(stored.vaults.contains_key(&live_key));
    assert!(!stored.vaults.contains_key(&vanished_key));
    assert!(live_completion.exists(), "live completion view is kept");
    assert!(!orphan_completion.exists(), "orphan view is removed");
    assert!(!stale_temp.exists(), "stale atomic-write temp is removed");
    assert!(fresh_temp.exists(), "fresh atomic-write temp is kept");
}

#[test]
fn known_keys_store_prunes_vanished_vaults() {
    let cache_home = tempfile::tempdir().expect("cache home");
    let _guard = isolate_catalog_env(cache_home.path());
    let vanished = tempfile::tempdir().expect("vanished vault");
    let live = tempfile::tempdir().expect("live vault");
    let vanished_key = super::cache::vault_key(vanished.path());
    let live_key = super::cache::vault_key(live.path());
    let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    let years = BTreeMap::new();
    super::cache::store_known_keys(
        vanished.path(),
        today,
        super::cache::now_unix(),
        &years,
        BTreeSet::from(["vanished".to_string()]),
    );
    super::cache::store_known_keys(
        live.path(),
        today,
        super::cache::now_unix(),
        &years,
        BTreeSet::from(["live".to_string()]),
    );

    drop(vanished);
    super::cache::store_known_keys(
        live.path(),
        today,
        super::cache::now_unix(),
        &years,
        BTreeSet::from(["live".to_string()]),
    );

    let known_keys = super::cache::cache_path()
        .parent()
        .expect("themes cache directory")
        .join("known-keys-v1.json");
    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(known_keys).unwrap()).unwrap();
    let vaults = stored["vaults"].as_object().expect("vault map");
    assert!(vaults.contains_key(&live_key));
    assert!(!vaults.contains_key(&vanished_key));
}

#[derive(Debug, Deserialize)]
struct Vectors {
    components: Vec<ComponentCase>,
    hue: Vec<HueCase>,
    fold: Vec<FoldCase>,
    rank: Vec<RankCase>,
    similar: Vec<SimilarCase>,
}

#[derive(Debug, Deserialize)]
struct SimilarCase {
    name: String,
    query: String,
    themes: Vec<SimilarFixture>,
    expected: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SimilarFixture {
    key: String,
    name: String,
    last_used: String,
    hidden: bool,
}

#[derive(Debug, Deserialize)]
struct ComponentCase {
    name: String,
    input: String,
    expected: Vec<ComponentExpected>,
}

#[derive(Debug, Deserialize)]
struct ComponentExpected {
    raw: String,
    key: String,
}

#[derive(Debug, Deserialize)]
struct HueCase {
    name: String,
    key: String,
    pin: Option<String>,
    exempt: Vec<String>,
    areas: BTreeMap<String, String>,
    #[serde(default)]
    configured: Option<BTreeMap<String, Option<String>>>,
    expected: HueExpected,
}

#[derive(Debug, Deserialize)]
struct HueExpected {
    hue: Option<String>,
    hue_source: String,
}

#[derive(Debug, Deserialize)]
struct FoldCase {
    name: String,
    events: Vec<FoldEvent>,
    expected: BTreeMap<String, FoldExpected>,
}

#[derive(Debug, Deserialize)]
struct FoldEvent {
    raw: String,
}

#[derive(Debug, Deserialize)]
struct FoldExpected {
    hidden: bool,
    pin: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RankCase {
    name: String,
    query: String,
    exclusions: Vec<String>,
    themes: Vec<RankFixture>,
    expected: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RankFixture {
    key: String,
    name: String,
    last_used: String,
    days_used: u32,
}

fn load_vectors() -> Vectors {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/pomodoro_themes/vectors.json"
    ))
    .expect("theme vectors parse")
}

#[test]
fn shared_vectors_cover_components_hue_fold_rank_and_similar() {
    let vectors = load_vectors();
    assert!(!vectors.components.is_empty());
    assert!(!vectors.hue.is_empty());
    assert!(!vectors.fold.is_empty());
    assert!(!vectors.rank.is_empty());
    assert!(!vectors.similar.is_empty());

    for case in &vectors.components {
        let got: Vec<(String, String)> = split_components(&case.input)
            .into_iter()
            .map(|raw| {
                let key = normalize_component(&raw);
                (raw, key)
            })
            .collect();
        let expected: Vec<(String, String)> = case
            .expected
            .iter()
            .map(|item| (item.raw.clone(), item.key.clone()))
            .collect();
        assert_eq!(got, expected, "components: {}", case.name);
    }

    for case in &vectors.hue {
        let pin = case.pin.as_deref().and_then(Hue::parse);
        let exempt = case
            .exempt
            .iter()
            .map(|value| normalize_component(value))
            .collect();
        let areas: BTreeMap<String, Hue> = case
            .areas
            .iter()
            .map(|(key, hue)| {
                (
                    key.clone(),
                    Hue::parse(hue).unwrap_or_else(|| panic!("{}", case.name)),
                )
            })
            .collect();
        let configured: BTreeMap<String, Option<Hue>> = case
            .configured
            .as_ref()
            .map(|entries| {
                entries
                    .iter()
                    .map(|(key, hue)| {
                        (key.clone(), hue.as_deref().and_then(Hue::parse))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let has_configured = case.configured.is_some();
        let config_color = configured.get(&case.key).copied().flatten();
        let unconfigured =
            has_configured && !configured.contains_key(&case.key);
        let (hue, source) = resolve_hue(
            &case.key,
            pin,
            &exempt,
            &areas,
            config_color,
            unconfigured,
        );
        assert_eq!(
            hue.map(Hue::name),
            case.expected.hue.as_deref(),
            "hue: {}",
            case.name
        );
        let source = match source {
            HueSource::Config => "config",
            HueSource::Pin => "pin",
            HueSource::Exempt => "exempt",
            HueSource::Unconfigured => "unconfigured",
            HueSource::Area => "area",
            HueSource::Hash => "hash",
        };
        assert_eq!(
            source, case.expected.hue_source,
            "hue_source: {}",
            case.name
        );
    }

    for case in &vectors.fold {
        let mut events = Vec::new();
        for (index, event) in case.events.iter().enumerate() {
            if let Some(parsed) = journal::parse_event(
                &event.raw,
                "fixture.jsonl",
                u32::try_from(index + 1).unwrap(),
            ) {
                events.push(parsed);
            }
        }
        let prefs = journal::fold_events(&events);
        let mut got: BTreeMap<String, (bool, Option<String>)> = BTreeMap::new();
        for (key, pref) in &prefs.by_key {
            got.insert(
                key.clone(),
                (pref.hidden, pref.pin.map(Hue::name).map(str::to_string)),
            );
        }
        let expected: BTreeMap<String, (bool, Option<String>)> = case
            .expected
            .iter()
            .map(|(key, value)| {
                (key.clone(), (value.hidden, value.pin.clone()))
            })
            .collect();
        assert_eq!(got, expected, "fold: {}", case.name);
    }

    for case in &vectors.rank {
        let dates: Vec<NaiveDate> = case
            .themes
            .iter()
            .map(|theme| {
                NaiveDate::parse_from_str(&theme.last_used, "%Y-%m-%d")
                    .expect(theme.last_used.as_str())
            })
            .collect();
        let ranked: Vec<RankTheme<'_>> = case
            .themes
            .iter()
            .zip(dates.iter())
            .map(|(theme, date)| RankTheme {
                key: &theme.key,
                name: &theme.name,
                last_used: *date,
                days_used: theme.days_used,
            })
            .collect();
        let exclusions = case.exclusions.iter().cloned().collect();
        let names: Vec<String> =
            rank_history(&ranked, &case.query, &exclusions)
                .into_iter()
                .map(|index| ranked[index].name.to_string())
                .collect();
        assert_eq!(names, case.expected, "rank: {}", case.name);
    }

    for case in &vectors.similar {
        let dates: Vec<NaiveDate> = case
            .themes
            .iter()
            .map(|theme| {
                NaiveDate::parse_from_str(&theme.last_used, "%Y-%m-%d")
                    .expect(theme.last_used.as_str())
            })
            .collect();
        let themes: Vec<SimilarTheme<'_>> = case
            .themes
            .iter()
            .zip(dates.iter())
            .map(|(theme, date)| SimilarTheme {
                key: &theme.key,
                name: &theme.name,
                last_used: *date,
                hidden: theme.hidden,
            })
            .collect();
        let got = similar_to(&case.query, &themes);
        assert_eq!(got, case.expected, "similar: {}", case.name);
    }
}

#[derive(Debug, Deserialize)]
struct ThemeCoreVectors {
    suggest: SuggestVectors,
    task_field: Vec<TaskFieldCase>,
}

#[derive(Debug, Deserialize)]
struct SuggestVectors {
    notes: BTreeMap<PathBuf, String>,
    cases: Vec<SuggestCase>,
}

#[derive(Debug, Deserialize)]
struct SuggestCase {
    id: String,
    residence: String,
    #[serde(default)]
    line: Option<String>,
    #[serde(default)]
    new_task: bool,
    #[serde(default)]
    config: Option<SuggestConfig>,
    #[serde(default)]
    live: Vec<String>,
    expected: Option<SuggestExpected>,
    #[serde(default)]
    warnings: Vec<SuggestWarningExpected>,
}

#[derive(Debug, Deserialize)]
struct SuggestConfig {
    reading_theme: String,
    exempt: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SuggestExpected {
    source: String,
    label: String,
    theme_key: String,
    theme_name: String,
    known: bool,
    #[serde(default)]
    repair: Option<String>,
    #[serde(default)]
    from: Option<FromExpected>,
    #[serde(default)]
    inherited: Option<Box<SuggestExpected>>,
}

#[derive(Debug, Deserialize)]
struct FromExpected {
    path: String,
    stem: String,
    kind: String,
    chain: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SuggestWarningExpected {
    code: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TaskFieldCase {
    id: String,
    op: String,
    #[serde(default)]
    line: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    repair: Option<String>,
}

#[test]
fn shared_vectors_cover_suggest_and_task_field() {
    let vectors: ThemeCoreVectors = serde_json::from_str(include_str!(
        "../../../tests/fixtures/pomodoro_themes/vectors.json"
    ))
    .expect("theme core vectors parse");
    assert!(!vectors.suggest.cases.is_empty());
    assert!(!vectors.task_field.is_empty());

    let note_index =
        NoteIndex::from_paths(vectors.suggest.notes.keys().cloned());
    let known = [
        ("write".to_string(), "WRITE".to_string()),
        ("build".to_string(), "BUILD".to_string()),
        ("read".to_string(), "READ".to_string()),
        ("fix".to_string(), "FIX".to_string()),
        ("plan".to_string(), "PLAN".to_string()),
    ];
    let configured = [("chore".to_string(), "CHORE".to_string())];
    let areas = BTreeMap::from([
        ("dev".to_string(), Hue::Blue),
        ("job".to_string(), Hue::Aqua),
    ]);
    let hidden = ["chore".to_string()];
    let exempt = ["plan".to_string()];

    for case in &vectors.suggest.cases {
        let catalog = super::Catalog::for_suggest(
            &known,
            &configured,
            &case.live,
            areas.clone(),
            &hidden,
            &exempt,
        );
        let mut cfg = PlanConfig::default();
        if let Some(config) = &case.config {
            if config.reading_theme.eq_ignore_ascii_case("off") {
                cfg.reading_theme = None;
            } else {
                cfg.reading_theme =
                    Some(config.reading_theme.to_ascii_uppercase());
            }
            cfg.exempt = config.exempt.clone();
        }
        let mut resolver = HomeResolver::new(IndexedHomeSource::new(
            &note_index,
            &vectors.suggest.notes,
        ));
        let result: SuggestResult = if case.new_task {
            suggest_for_new_task(
                &mut resolver,
                &catalog,
                &cfg,
                Path::new(&case.residence),
            )
        } else {
            suggest_for_task(
                &mut resolver,
                &catalog,
                &cfg,
                Path::new(&case.residence),
                case.line.as_deref().expect(case.id.as_str()),
            )
        };
        assert_suggestion(
            &case.id,
            case.expected.as_ref(),
            result.suggestion.as_ref(),
        );
        let got_warnings: Vec<(String, Option<String>)> = result
            .warnings
            .iter()
            .map(|warning| (warning.code.clone(), warning.path.clone()))
            .collect();
        let expected_warnings: Vec<(String, Option<String>)> = case
            .warnings
            .iter()
            .map(|warning| (warning.code.clone(), warning.path.clone()))
            .collect();
        assert_eq!(got_warnings, expected_warnings, "warnings: {}", case.id);
        if let Some(suggestion) = result.suggestion.as_ref() {
            let value =
                serde_json::to_value(suggestion).expect(case.id.as_str());
            assert_eq!(value["source"], case.expected.as_ref().unwrap().source);
            if suggestion.source == super::suggest::SuggestionSource::Task {
                assert!(value.get("inherited").is_some(), "{}", case.id);
            } else {
                assert!(value.get("inherited").is_none(), "{}", case.id);
            }
        }
    }

    for case in &vectors.task_field {
        match case.op.as_str() {
            "parse" => {
                let state =
                    parse_task_theme(case.line.as_deref().expect(&case.id));
                match case.state.as_deref() {
                    Some("none") => assert!(
                        matches!(state, TaskThemeState::None),
                        "{}",
                        case.id
                    ),
                    Some("set") => match state {
                        TaskThemeState::Set { value, .. } => {
                            assert_eq!(
                                value,
                                case.value.as_deref().unwrap(),
                                "{}",
                                case.id
                            );
                        }
                        other => panic!("{}: {other:?}", case.id),
                    },
                    Some("repair") => match state {
                        TaskThemeState::Repair { kind } => {
                            let got = match kind {
                                crate::native::task_theme::RepairKind::Duplicate => {
                                    "duplicate"
                                }
                                crate::native::task_theme::RepairKind::Invalid => {
                                    "invalid"
                                }
                            };
                            assert_eq!(
                                got,
                                case.repair.as_deref().unwrap(),
                                "{}",
                                case.id
                            );
                        }
                        other => panic!("{}: {other:?}", case.id),
                    },
                    other => panic!("unknown parse state {other:?}"),
                }
            }
            "set" => {
                let got = set_task_theme(
                    case.before.as_deref().expect(&case.id),
                    case.name.as_deref().expect(&case.id),
                );
                assert_eq!(got, case.after.as_deref().unwrap(), "{}", case.id);
            }
            "clear" => {
                let got =
                    clear_task_theme(case.before.as_deref().expect(&case.id));
                assert_eq!(got, case.after.as_deref().unwrap(), "{}", case.id);
            }
            other => panic!("unknown task_field op {other}"),
        }
    }

}

fn assert_suggestion(
    id: &str,
    expected: Option<&SuggestExpected>,
    got: Option<&ThemeSuggestion>,
) {
    match (expected, got) {
        (None, None) => {}
        (Some(expected), Some(got)) => {
            let source = match got.source {
                super::suggest::SuggestionSource::Task => "task",
                super::suggest::SuggestionSource::Reading => "reading",
                super::suggest::SuggestionSource::Note => "note",
                super::suggest::SuggestionSource::Default => "default",
            };
            assert_eq!(source, expected.source, "{id}");
            assert_eq!(got.label, expected.label, "{id}");
            assert_eq!(got.theme.key, expected.theme_key, "{id}");
            assert_eq!(got.theme.name, expected.theme_name, "{id}");
            assert_eq!(got.theme.known, expected.known, "{id}");
            let repair = got.repair.map(|kind| match kind {
                crate::native::task_theme::RepairKind::Duplicate => "duplicate",
                crate::native::task_theme::RepairKind::Invalid => "invalid",
            });
            assert_eq!(repair, expected.repair.as_deref(), "{id}");
            match (&expected.from, &got.from) {
                (None, None) => {}
                (Some(expected), Some(got)) => {
                    assert_eq!(got.path, expected.path, "{id}");
                    assert_eq!(got.stem, expected.stem, "{id}");
                    assert_eq!(got.kind, expected.kind, "{id}");
                    assert_eq!(got.chain, expected.chain, "{id}");
                }
                other => panic!("{id}: from mismatch {other:?}"),
            }
            match (&expected.inherited, &got.inherited) {
                (None, None) | (None, Some(None)) => {}
                (Some(expected), Some(Some(got))) => {
                    assert_suggestion(id, Some(expected), Some(got));
                }
                (Some(_), Some(None)) => panic!("{id}: inherited null"),
                other => panic!("{id}: inherited mismatch {other:?}"),
            }
        }
        other => panic!("{id}: suggestion mismatch {other:?}"),
    }
}

#[test]
fn hash_hues_match_known_collisions() {
    assert_eq!(fallback_hue("bob"), Hue::Magenta);
    assert_eq!(fallback_hue("job"), Hue::Magenta);
    assert_eq!(fallback_hue("dev"), Hue::Magenta);
    assert_eq!(fallback_hue("blog"), Hue::Orange);
    assert_eq!(fallback_hue("sase"), Hue::Orange);
}

#[test]
fn scan_splits_named_entries_and_ignores_unnamed() {
    let contents = "\
## Pomodoros\n\
- [x] (0800-0830 30m) — BOB + DECKS\n\
- [ ] () — GTD\n\
- [ ] ()\n\
```\n\
- [ ] () — FENCED\n\
```\n";
    let themes = scan::scan_contents(contents);
    let by_key: BTreeMap<_, _> = themes
        .into_iter()
        .map(|theme| (theme.key.clone(), theme))
        .collect();
    assert_eq!(by_key["bob"].name, "BOB");
    assert_eq!(by_key["bob"].entries, 1);
    assert!(!by_key["bob"].open);
    assert_eq!(by_key["decks"].name, "DECKS");
    assert!(by_key["gtd"].open);
    assert!(!by_key.contains_key("fenced"));
}

#[test]
fn catalog_loads_scratch_vault_and_applies_area_hue() {
    let temp = tempfile::tempdir().expect("scratch vault");
    let vault = temp.path();
    fs::write(
        vault.join("job.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: aqua\n---\n",
    )
    .unwrap();
    fs::write(
        vault.join("gtd.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: orange\n---\n",
    )
    .unwrap();
    fs::create_dir_all(vault.join("2026")).unwrap();
    fs::write(
        vault.join("2026/20261020.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — BOB\n- [x] (0900-0930 30m) — JOB\n- [x] (1000-1030 30m) — FIXES\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — BOB\n- [x] (0800-0830 30m) — PLAN\n- [ ] () — FIX\n",
    )
    .unwrap();
    let cache = tempfile::tempdir().expect("cache");
    let _guard = isolate_catalog_env(cache.path());
    let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    let catalog = Catalog::load(vault, today, true).expect("load catalog");

    let bob = catalog.get("bob").expect("bob");
    assert_eq!(bob.days_used, 2);
    assert_eq!(bob.today, Some(super::TodayPresence::Open));
    let job = catalog.theme_ref("job", "JOB");
    assert_eq!(job.hue, Some(Hue::Aqua));
    assert_eq!(job.hue_source, HueSource::Area);
    let plan = catalog.theme_ref("plan", "PLAN");
    assert_eq!(plan.hue, None);
    assert_eq!(plan.hue_source, HueSource::Exempt);
    let ranked = catalog.rank_for_query("fix", &Default::default());
    assert_eq!(
        ranked
            .iter()
            .map(|theme| theme.name.as_str())
            .collect::<Vec<_>>(),
        ["FIX", "FIXES"]
    );
    let cached = Catalog::load(vault, today, false).expect("cached load");
    assert_eq!(cached.known_count(), catalog.known_count());
    assert_eq!(
        cached.get("bob").map(|theme| theme.entries),
        catalog.get("bob").map(|theme| theme.entries)
    );

    fs::write(
        vault.join("new.md"),
        "---\ntype: [[area]]\nparent: [[area]]\ncolor: red\n---\n",
    )
    .unwrap();
    let completion = Catalog::load_for_completion(
        vault,
        today,
        BTreeSet::from(["new".to_string()]),
        None,
    )
    .unwrap();
    for theme in catalog.records() {
        assert_eq!(
            completion.theme_ref(&theme.key, &theme.name),
            catalog.theme_ref(&theme.key, &theme.name)
        );
    }
    let new = completion.theme_ref("new", "NEW");
    assert_eq!(new.hue, Some(Hue::Red));
    assert_eq!(new.hue_source, HueSource::Area);
    assert!(!new.known);
}

#[test]
fn future_days_do_not_count_toward_recency() {
    let temp = tempfile::tempdir().expect("scratch vault");
    let vault = temp.path();
    fs::create_dir_all(vault.join("2026")).unwrap();
    fs::write(
        vault.join("2026/20261024.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — BOB\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/20261030.md"),
        "## Pomodoros\n- [ ] () — FUTURE\n",
    )
    .unwrap();
    let cache = tempfile::tempdir().expect("cache");
    let _guard = isolate_catalog_env(cache.path());
    let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    let catalog = Catalog::load(vault, today, true).expect("load");
    assert!(catalog.get("bob").is_some());
    assert!(catalog.get("future").is_none());
}

#[test]
fn collision_hints_move_the_less_used_unpinned_pair() {
    let temp = tempfile::tempdir().expect("scratch vault");
    let vault = temp.path();
    fs::create_dir_all(vault.join("2026")).unwrap();
    for day in ["20261001", "20261002"] {
        fs::write(
            vault.join(format!("2026/{day}.md")),
            "## Pomodoros\n\
- [x] (0800-0830 30m) — SASE\n\
- [x] (0900-0930 30m) — BLOG\n",
        )
        .unwrap();
    }
    fs::write(
        vault.join("2026/20261003.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — SASE\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/20261024.md"),
        "## Pomodoros\n- [ ] () — SASE\n",
    )
    .unwrap();
    let cache = tempfile::tempdir().expect("cache");
    let _guard = isolate_catalog_env(cache.path());
    let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    let catalog = Catalog::load(vault, today, true).expect("load");
    let hints = catalog.collision_hints();
    assert_eq!(hints.len(), 1, "{hints:?}");
    assert_eq!(hints[0].code, "theme_hue_collision");
    assert_eq!(hints[0].themes, ["SASE", "BLOG"]);
    assert_eq!(hints[0].hue, Hue::Orange);
    assert_eq!(hints[0].days_together, 2);
    assert_eq!(hints[0].suggest.name, "BLOG");
    assert_ne!(hints[0].suggest.hue, Hue::Orange);
}

#[test]
fn theme_refs_for_unknown_name_are_marked_unknown() {
    let temp = tempfile::tempdir().expect("scratch vault");
    let cache = tempfile::tempdir().expect("cache");
    let _guard = isolate_catalog_env(cache.path());
    let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    let catalog = Catalog::load(temp.path(), today, true).expect("load");
    let refs = catalog.theme_refs_for_name("BRAND NEW");
    assert_eq!(refs.len(), 1);
    assert!(!refs[0].known);
    assert_eq!(refs[0].key, "brand new");
    assert_eq!(refs[0].hue_source, HueSource::Hash);
}

#[test]
fn completion_cache_keeps_today_live_and_obeys_catalog_invalidation() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path();
    let cache = tempfile::tempdir().unwrap();
    let _guard = isolate_catalog_env(cache.path());
    let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    fs::create_dir_all(vault.join("2026")).unwrap();
    let past = vault.join("2026/20261023.md");
    let live = vault.join("2026/20261024.md");
    fs::write(&past, "## Pomodoros\n- [x] () — BOB\n").unwrap();
    fs::write(&live, "## Pomodoros\n").unwrap();
    let load = || {
        Catalog::load_for_completion(vault, today, BTreeSet::new(), None)
            .unwrap()
    };
    assert!(load().get("bob").is_some());
    fs::write(&live, "## Pomodoros\n- [ ] () — NEW\n").unwrap();
    let warm = load();
    assert!(warm.get("new").is_some());
    assert!(warm.get("bob").is_some());
    fs::write(&past, "## Pomodoros\n- [x] () — RENAMED\n").unwrap();
    assert!(load().get("bob").is_some(), "past edit waits for sweep");
    Catalog::load(vault, today, true).unwrap();
    assert!(
        load().get("renamed").is_some(),
        "fresh refresh invalidates warm view"
    );
    assert!(load().get("bob").is_none());
    fs::write(&past, "## Pomodoros\n- [x] () — REBUILT\n").unwrap();
    fs::write(super::cache::cache_path(), "corrupt").unwrap();
    assert!(
        load().get("rebuilt").is_some(),
        "corrupt source cache rebuilds"
    );
    super::cache::invalidate_vault(vault);
    assert!(
        load().get("rebuilt").is_some(),
        "rename invalidation rebuilds"
    );
}

#[test]
fn theme_lookup_uses_cached_history_and_refreshes_changed_today() {
    let temp = tempfile::tempdir().expect("scratch vault");
    let vault = temp.path();
    fs::create_dir_all(vault.join("2026")).unwrap();
    fs::write(
        vault.join("2026/20261023.md"),
        "## Pomodoros\n- [x] (0800-0830 30m) — BOB\n",
    )
    .unwrap();
    let today_path = vault.join("2026/20261024.md");
    fs::write(&today_path, "## Pomodoros\n- [ ] () — BOB\n").unwrap();
    let cache = tempfile::tempdir().expect("cache");
    let _guard = isolate_catalog_env(cache.path());
    let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
    Catalog::load(vault, today, true).expect("prime theme cache");

    let keys = BTreeSet::from(["bob".to_string(), "new".to_string()]);
    let lookup = ThemeLookup::load_for_theme_keys(vault, today, &keys)
        .expect("cached lookup");
    assert!(lookup.theme_ref("bob", "BOB").known);
    assert!(!lookup.theme_ref("new", "NEW").known);

    fs::write(
        &today_path,
        "## Pomodoros\n- [ ] () — BOB\n- [ ] () — NEW\n",
    )
    .unwrap();
    let refreshed = ThemeLookup::load_for_theme_keys(vault, today, &keys)
        .expect("lookup after today's note changed");
    assert!(refreshed.theme_ref("new", "NEW").known);
}
