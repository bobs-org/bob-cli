use std::{collections::BTreeMap, ffi::OsStr, fs};

use chrono::NaiveDate;
use serde::Deserialize;

use crate::native::{
    env::TestEnvGuard,
    plan_budget::{normalize_component, split_components},
    pomodoro_homes::{fallback_hue, Hue},
};

use super::{
    journal, rank_history, resolve_hue, scan,
    similar::{similar_to, SimilarTheme},
    Catalog, HueSource, RankTheme,
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
        let areas = case
            .areas
            .iter()
            .map(|(key, hue)| {
                (
                    key.clone(),
                    Hue::parse(hue).unwrap_or_else(|| panic!("{}", case.name)),
                )
            })
            .collect();
        let (hue, source) = resolve_hue(&case.key, pin, &exempt, &areas);
        assert_eq!(
            hue.map(Hue::name),
            case.expected.hue.as_deref(),
            "hue: {}",
            case.name
        );
        let source = match source {
            HueSource::Pin => "pin",
            HueSource::Exempt => "exempt",
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
        "## Pomodoros\n- [ ] () — BOB\n- [x] (0800-0830 30m) — GTD\n- [ ] () — FIX\n",
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
    let gtd = catalog.theme_ref("gtd", "GTD");
    assert_eq!(gtd.hue, None);
    assert_eq!(gtd.hue_source, HueSource::Exempt);
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
