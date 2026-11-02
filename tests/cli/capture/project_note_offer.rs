//! Acceptance matrix for the `@route:id@` project-note offer form
//! (`plan:202610/project_note_offer_form.md`, phase `offer_contracts`).
//!
//! `project_note.rs` holds the basic offer, byte-equality, and
//! pick-validation tests; this module fills the remaining matrix:
//! equivalence to authored colon bullets across destinations and
//! statuses, row eligibility, blocked projects, lazy Pomodoro creation,
//! per-task types, minted-ID collisions, mixed-draft numbering, and
//! batch suppression.

use crate::support::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-09-20 14:31:07";
const DAY: &str = "## Pomodoros\n- [ ] (1330-1400) Work\n";
const TYPED_NOW: &str = "2026-07-10 13:40:00";

struct Fixture {
    _temp: TempDir,
    vault: PathBuf,
    day_file: PathBuf,
    config: Option<PathBuf>,
    now: &'static str,
}

impl Fixture {
    fn new(prefix: &str, area: &str, day: &str) -> Self {
        let temp = TempDir::new(prefix);
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_toggle_task_settings(&vault);
        write_file(&vault.join("cash.md"), area);
        write_file(&day_file, day);
        Self {
            _temp: temp,
            vault,
            day_file,
            config: None,
            now: NOW,
        }
    }

    fn plain(prefix: &str) -> Self {
        Self::new(prefix, "---\ntype: \"[[area]]\"\n---\n# Cash\n", DAY)
    }

    /// A typed vault: BUILD is the area type and the running entry.
    fn typed(prefix: &str) -> Self {
        let mut fixture = Self::new(
            prefix,
            "---\ntype: \"[[area]]\"\ntask_type: BUILD\n---\n# Cash\n",
            concat!(
                "# 2026-07-10\n",
                "## Pomodoros\n",
                "- [ ] (**1330-1400** [t:: 30m]) \u{2014} BUILD\n",
                "  - existing context\n",
                "- [ ] () \u{2014} MEET\n",
                "## Later\n",
            ),
        );
        let config = fixture._temp.path().join("config.yml");
        write_file(
            &config,
            concat!(
                "plan:\n  default_type: CHORE\n",
                "types:\n  BUILD: yellow\n  FIX: blue\n  READ: green\n",
                "  WRITE: magenta\n  PLAN: blue\n  CHORE: yellow\n",
            ),
        );
        fixture.config = Some(config);
        fixture.now = TYPED_NOW;
        fixture
    }

    fn run(&self, input: &str, flags: &[&str]) -> Output {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&self.vault)
            .arg("--no-clip");
        for flag in flags {
            command.arg(flag);
        }
        command
            .env("BOB_DAY_FILE", &self.day_file)
            .env("BOB_NOW", self.now)
            .env("TZ", "UTC0");
        if let Some(config) = &self.config {
            command.env("BOB_CONFIG_FILE", config);
        }
        run_with_stdin(&mut command, input)
    }

    fn json(&self, input: &str, flags: &[&str]) -> serde_json::Value {
        let mut all = vec!["-f", "json"];
        all.extend_from_slice(flags);
        let output = self.run(input, &all);
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        })
    }

    fn day(&self) -> String {
        fs::read_to_string(&self.day_file).expect("read day file")
    }

    /// Every file under the vault (minus Obsidian state), keyed by path.
    fn snapshot(&self) -> BTreeMap<String, String> {
        let mut files = BTreeMap::new();
        collect(&self.vault, &self.vault, &mut files);
        files
    }
}

fn collect(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(dir).expect("read vault dir") {
        let path = entry.expect("dir entry").path();
        let relative = path.strip_prefix(root).expect("relative path");
        if relative.starts_with(".obsidian") {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            files.insert(
                relative.display().to_string(),
                fs::read_to_string(&path).expect("read vault file"),
            );
        }
    }
}

fn successors(json: &serde_json::Value) -> &Vec<serde_json::Value> {
    json["project_note"]["successors"]
        .as_array()
        .unwrap_or_else(|| panic!("offer successors in {json}"))
}

/// One bullet of an offer draft.
enum Row {
    /// An ID-less task the planner offers; a pick mints its ID.
    Offered(&'static str),
    /// A task named with ` ^<id>`: offered with its authored ID, and
    /// authored as ` :<id>` in the equivalent Create draft.
    Named(&'static str, &'static str),
    /// A line authored verbatim in both drafts (` :` links, `#hide`).
    Literal(&'static str),
}

fn offer_draft(header: &str, rows: &[Row]) -> String {
    let mut draft = format!("{header}\n");
    for row in rows {
        match row {
            Row::Offered(text) => draft.push_str(&format!("- {text}\n")),
            Row::Named(text, id) => {
                draft.push_str(&format!("- {text} ^{id}\n"));
            }
            Row::Literal(line) => draft.push_str(&format!("{line}\n")),
        }
    }
    draft
}

/// The Create draft whose ` :<id>` bullets are what `-p all` picks.
fn authored_draft(header: &str, rows: &[Row], picked: &[String]) -> String {
    let mut picked = picked.iter();
    let mut draft = format!("{header}\n");
    for row in rows {
        match row {
            Row::Offered(text) => draft.push_str(&format!(
                "- {text} :{}\n",
                picked.next().expect("picked ID per offered row")
            )),
            Row::Named(text, id) => {
                assert_eq!(picked.next().map(String::as_str), Some(*id));
                draft.push_str(&format!("- {text} :{id}\n"));
            }
            Row::Literal(line) => draft.push_str(&format!("{line}\n")),
        }
    }
    assert!(picked.next().is_none(), "every picked ID is used");
    draft
}

/// Invariant 1: Offer `-p all` writes the same bytes as Create with
/// the equivalent authored colon bullets, in separate identical vaults.
fn assert_offer_matches_authored(
    make: impl Fn(&str) -> Fixture,
    offer_header: &str,
    create_header: &str,
    rows: &[Row],
    expected_status: &str,
) {
    let offer = make("bob-cli-offer-equiv-offer");
    let offered = offer.json(&offer_draft(offer_header, rows), &["-p", "all"]);
    assert_eq!(offered["ok"], true, "{offered}");
    let picked = successors(&offered);
    let minted: Vec<String> = picked
        .iter()
        .map(|row| {
            assert_eq!(row["offer"]["selected"], true, "{row}");
            assert_eq!(row["status_symbol"], expected_status, "{row}");
            assert!(row.get("link").is_some(), "{row}");
            row["block_id"].as_str().expect("picked ID").to_string()
        })
        .collect();
    let offered_count = rows
        .iter()
        .filter(|row| !matches!(row, Row::Literal(_)))
        .count();
    assert_eq!(minted.len(), offered_count, "{offered}");

    let authored = make("bob-cli-offer-equiv-authored");
    let created =
        authored.json(&authored_draft(create_header, rows, &minted), &[]);
    assert_eq!(created["ok"], true, "{created}");

    assert_eq!(
        offer.snapshot(),
        authored.snapshot(),
        "every written file matches the authored colon-bullet capture"
    );
    // The dual representation: a pick is also a normal `task_links`
    // entry, so clients must not count it twice.
    let offered_links = offered["project_note"]["task_links"]
        .as_array()
        .expect("offer task_links");
    for id in &minted {
        assert!(
            offered_links.iter().any(|link| link["block_id"] == *id),
            "pick {id} is also a task link: {offered}"
        );
    }
    assert_eq!(
        offered["project_note"]["task_links"],
        created["project_note"]["task_links"],
        "picked links report exactly like authored links"
    );
    for (path, contents) in offer.snapshot() {
        assert!(
            !contents.contains("[fresh::"),
            "creation never stamps freshness: {path}\n{contents}"
        );
    }
    let note = &offer.snapshot()["cash_goog_exit.md"];
    assert!(!note.contains("REPLACE WITH TASK DESCRIPTION"), "{note}");
}

const MIXED_ROWS: &[Row] = &[
    Row::Offered("Draft the memo"),
    Row::Named("Collect paperwork", "paperwork"),
    Row::Literal("- Call the bank :call-bank"),
    Row::Literal("- Private #hide reminder"),
    Row::Offered("Book the interview"),
];

#[test]
fn offer_all_matches_authored_colon_bullets_implicitly() {
    assert_offer_matches_authored(
        Fixture::plain,
        "Finish the exit packet @cash:goog-exit@",
        "Finish the exit packet @cash^goog-exit@",
        MIXED_ROWS,
        "*",
    );
}

#[test]
fn offer_all_matches_authored_colon_bullets_with_an_explicit_name() {
    assert_offer_matches_authored(
        Fixture::plain,
        "Finish the exit packet @cash:goog-exit@#admin",
        "Finish the exit packet @cash^goog-exit@#admin",
        MIXED_ROWS,
        "*",
    );
}

#[test]
fn offer_all_matches_authored_colon_bullets_with_per_task_types() {
    assert_offer_matches_authored(
        Fixture::typed,
        "Finish the exit packet @cash:goog-exit@",
        "Finish the exit packet @cash^goog-exit@",
        &[
            Row::Offered("Draft the memo"),
            Row::Offered("Fix the bug [type:: FIX]"),
        ],
        "*",
    );
}

#[test]
fn offer_all_matches_authored_colon_bullets_under_a_resolved_schedule() {
    // `s:0` resolves a scheduled date that is not in the future: picks
    // render `[?]` exactly like authored ` :` tasks do.
    assert_offer_matches_authored(
        Fixture::plain,
        "Finish the exit packet s:0 @cash:goog-exit@",
        "Finish the exit packet s:0 @cash^goog-exit@",
        &[
            Row::Offered("Draft the memo"),
            Row::Named("Collect paperwork", "paperwork"),
        ],
        "?",
    );
}

#[test]
fn offer_none_and_default_match_create_without_the_offered_links() {
    // `-p none` and no `-p` both behave like Create with only the
    // explicit colon bullets; the offers are reported, never written.
    let draft =
        offer_draft("Finish the exit packet @cash:goog-exit@", MIXED_ROWS);
    let default = Fixture::plain("bob-cli-offer-none-default");
    let none = Fixture::plain("bob-cli-offer-none-explicit");
    let create = Fixture::plain("bob-cli-offer-none-create");
    let default_json = default.json(&draft, &[]);
    let none_json = none.json(&draft, &["-p", "none"]);
    let create_json = create
        .json(&draft.replace("@cash:goog-exit@", "@cash^goog-exit@"), &[]);
    assert_eq!(successors(&default_json).len(), 3, "{default_json}");
    assert_eq!(default_json["pull_in"]["choice"], serde_json::Value::Null);
    assert_eq!(none_json["pull_in"]["choice"], "none");
    assert_eq!(none_json["pull_in"]["picked_count"], 0);
    assert!(
        successors(&none_json)
            .iter()
            .all(|row| row["offer"]["selected"] == false),
        "{none_json}"
    );
    assert_eq!(default.snapshot(), create.snapshot());
    assert_eq!(none.snapshot(), create.snapshot());
    // Only the explicit ` :call-bank` bullet links.
    assert_eq!(
        create_json["project_note"]["task_links"],
        default_json["project_note"]["task_links"],
        "{default_json}"
    );
}

#[test]
fn offer_lists_only_open_ready_next_and_pending_task_rows() {
    // Done, canceled, and blocked rows, section headings, note-only
    // bullets, `^prj`, and the generated placeholder are never offered;
    // `#hide` and explicit ` :` rows are folded instead.
    let fixture = Fixture::plain("bob-cli-offer-eligibility");
    let json = fixture.json(
        concat!(
            "Finish packet @cash:pkt@\n",
            "- [x] Done thing\n",
            "- [-] Canceled thing\n",
            "- [?] Blocked thing\n",
            "- [/] Pending thing\n",
            "- [*] Next thing\n",
            "- Ready thing\n",
            "- Hidden #hide thing\n",
            "- Linked thing :linked\n",
            "- FUTURE WORK\n",
            "  - Revisit later\n",
        ),
        &[],
    );
    assert_eq!(json["ok"], true, "{json}");
    let rows = successors(&json);
    let offered: Vec<(&str, u64)> = rows
        .iter()
        .map(|row| {
            (
                row["text"].as_str().expect("text"),
                row["offer"]["n"].as_u64().expect("n"),
            )
        })
        .collect();
    assert_eq!(
        offered,
        vec![("Pending thing", 1), ("Next thing", 2), ("Ready thing", 3)],
        "{json}"
    );
    let statuses: Vec<&str> = rows
        .iter()
        .map(|row| row["previous_status_symbol"].as_str().expect("status"))
        .collect();
    assert_eq!(statuses, vec!["/", "*", " "]);
    assert!(
        rows.iter().all(|row| row["block_id"] != "prj"),
        "the ^prj task is never offered: {json}"
    );
    let folded = &json["pull_in"]["groups"][0]["folded"];
    assert_eq!(folded["hidden"], 1, "{json}");
    assert_eq!(folded["already_planned"], 1, "{json}");
    assert_eq!(folded["sub_projects"], 0, "{json}");

    // A project with no task bullets keeps the placeholder, offers
    // nothing, and never reads (or needs) the daily note.
    let bare = Fixture::plain("bob-cli-offer-placeholder");
    fs::remove_file(&bare.day_file).expect("remove day note");
    let bare_json = bare.json("Finish packet @cash:pkt@\n", &["-p", "all"]);
    assert_eq!(bare_json["ok"], true, "{bare_json}");
    assert_eq!(bare_json["project_note"]["form"], "offer");
    assert_eq!(
        bare_json["project_note"]["successors"],
        serde_json::json!([])
    );
    assert_eq!(bare_json["pull_in"]["offer_count"], 0);
    let note = &bare.snapshot()["cash_pkt.md"];
    assert!(note.contains("(REPLACE WITH TASK DESCRIPTION)"), "{note}");
    assert!(!bare.day_file.exists());

    // Only closed rows: still nothing to offer, still no daily note.
    let closed = Fixture::plain("bob-cli-offer-all-closed");
    fs::remove_file(&closed.day_file).expect("remove day note");
    let closed_json = closed.json(
        "Finish packet @cash:pkt@\n- [x] Done thing\n- [-] Canceled thing\n",
        &[],
    );
    assert_eq!(closed_json["ok"], true, "{closed_json}");
    assert_eq!(
        closed_json["project_note"]["successors"],
        serde_json::json!([])
    );
}

#[test]
fn offer_blocked_project_offers_nothing_and_links_nothing() {
    // A future project schedule blocks the project: the existing
    // `project_schedule` link guard applies and no offers are made,
    // even without an explicit colon bullet and with `-p all`.
    let fixture = Fixture::plain("bob-cli-offer-blocked");
    let day_before = fixture.day();
    let json = fixture.json(
        concat!(
            "Finish packet s:3 @cash:pkt@\n",
            "- Draft memo\n",
            "- Collect docs :collect-docs\n",
        ),
        &["-p", "all"],
    );
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["project_note"]["form"], "offer");
    assert_eq!(json["project_note"]["successors"], serde_json::json!([]));
    assert_eq!(json["pull_in"]["offer_count"], 0, "{json}");
    assert_eq!(json["pull_in"]["picked_count"], 0, "{json}");
    assert_eq!(
        json["link_guard"]["reasons"],
        serde_json::json!(["project_schedule"]),
        "{json}"
    );
    assert_eq!(
        json["project_note"]["task_links"],
        serde_json::json!([]),
        "{json}"
    );
    assert_eq!(fixture.day(), day_before, "a blocked project links nothing");
    let note = &fixture.snapshot()["cash_pkt.md"];
    assert!(note.contains("#prj_is_blocked"), "{note}");
}

#[test]
fn offer_future_project_without_colon_bullet_still_reports_schedule_guard() {
    // A future project schedule blocks the whole note even when the draft
    // has only ordinary and caret-ID bullets: no offers, no links, and the
    // `project_schedule` guard applies for default selection and `-p all`.
    // The project is still created with its blocked residents and the daily
    // note is untouched.
    for flags in [&[] as &[&str], &["-p", "all"]] {
        let fixture = Fixture::plain("bob-cli-offer-blocked-no-colon");
        let day_before = fixture.day();
        let json = fixture.json(
            concat!(
                "Finish packet s:3 @cash:pkt@\n",
                "- Draft memo\n",
                "- Collect docs ^collect-docs\n",
            ),
            flags,
        );
        assert_eq!(json["ok"], true, "{json}");
        assert_eq!(json["project_note"]["form"], "offer");
        assert_eq!(
            json["project_note"]["successors"],
            serde_json::json!([]),
            "{json}"
        );
        assert_eq!(json["pull_in"]["offer_count"], 0, "{json}");
        assert_eq!(json["pull_in"]["picked_count"], 0, "{json}");
        assert_eq!(
            json["link_guard"]["reasons"],
            serde_json::json!(["project_schedule"]),
            "{json}"
        );
        assert_eq!(
            json["project_note"]["task_links"],
            serde_json::json!([]),
            "{json}"
        );
        assert_eq!(
            fixture.day(),
            day_before,
            "a blocked project links nothing"
        );
        let snapshot = fixture.snapshot();
        let note = &snapshot["cash_pkt.md"];
        assert!(note.contains("#prj_is_blocked"), "{note}");
        assert!(note.contains("Draft memo"), "{note}");
        assert!(note.contains("Collect docs"), "{note}");
    }

    // This path stages neither offers nor actual links, so it never reads
    // the daily note: it succeeds when the day file is absent.
    let bare = Fixture::plain("bob-cli-offer-blocked-no-day");
    fs::remove_file(&bare.day_file).expect("remove day note");
    let bare_json = bare.json(
        concat!(
            "Finish packet s:3 @cash:pkt@\n",
            "- Draft memo\n",
            "- Collect docs ^collect-docs\n",
        ),
        &["-p", "all"],
    );
    assert_eq!(bare_json["ok"], true, "{bare_json}");
    assert_eq!(
        bare_json["link_guard"]["reasons"],
        serde_json::json!(["project_schedule"]),
        "{bare_json}"
    );
    assert!(!bare.day_file.exists());
    let bare_note = &bare.snapshot()["cash_pkt.md"];
    assert!(bare_note.contains("#prj_is_blocked"), "{bare_note}");
}

#[test]
fn offer_named_pomodoro_is_created_only_when_a_pick_lands_there() {
    let draft =
        "Finish packet @cash:pkt@#admin\n- Draft memo\n- Collect docs\n";

    // Nothing picked: the preview names the destination as a Pomodoro
    // that would be created, but no placeholder is ever written.
    let none = Fixture::plain("bob-cli-offer-named-none");
    let day_before = none.day();
    let preview = none.json(draft, &["--dry-run"]);
    let roles: Vec<&str> = successors(&preview)
        .iter()
        .map(|row| {
            assert_eq!(row["offer"]["destination"]["entry_name"], "ADMIN");
            assert_eq!(row["offer"]["destination"]["fallback"], false);
            row["offer"]["destination"]["entry_role"]
                .as_str()
                .expect("role")
        })
        .collect();
    // Placement is sequential: the first landing would create ADMIN and
    // the next one joins it.
    assert_eq!(roles, vec!["created", "typed"], "{preview}");
    assert!(preview.get("pomodoro_blocks").is_none(), "{preview}");
    let committed = none.json(draft, &["-p", "none"]);
    assert_eq!(committed["ok"], true, "{committed}");
    assert_eq!(none.day(), day_before, "no placeholder without a landing");
    assert!(!none.day().contains("ADMIN"));
    assert!(none.vault.join("cash_pkt.md").exists());

    // One pick lands: only then does the `ADMIN` placeholder appear,
    // holding exactly the picked link.
    let one = Fixture::plain("bob-cli-offer-named-one");
    let picked = one.json(draft, &["-p", "2"]);
    assert_eq!(picked["ok"], true, "{picked}");
    assert_eq!(picked["pomodoro_name"], "ADMIN", "{picked}");
    assert_eq!(picked["creates_pomodoro"], true, "{picked}");
    let id = picked["project_note"]["task_links"][0]["block_id"]
        .as_str()
        .expect("picked ID")
        .to_string();
    assert_eq!(
        picked["project_note"]["task_links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        one.day(),
        format!("{DAY}- [ ] () \u{2014} ADMIN\n  - [[cash_pkt#^{id}]]\n"),
        "the named placeholder exists with the picked link only"
    );
    let note = &one.snapshot()["cash_pkt.md"];
    assert!(
        note.contains(&format!("[created::2026-09-20] ^{id}")),
        "{note}"
    );
    assert_eq!(note.matches("[type:: ADMIN]").count(), 0, "{note}");
}

#[test]
fn offer_typed_tasks_report_and_place_their_own_destinations() {
    let draft =
        "Finish packet @cash:pkt@\n- Draft memo\n- Fix the bug [type:: FIX]\n";
    let fixture = Fixture::typed("bob-cli-offer-typed");
    let day_before = fixture.day();
    let preview = fixture.json(draft, &["--dry-run"]);
    let destinations: Vec<(&str, &str)> = successors(&preview)
        .iter()
        .map(|row| {
            let destination = &row["offer"]["destination"];
            (
                destination["entry_name"].as_str().expect("entry"),
                destination["entry_role"].as_str().expect("role"),
            )
        })
        .collect();
    assert_eq!(destinations, vec![("BUILD", "typed"), ("FIX", "created")]);

    // Picking only the FIX task creates FIX and leaves BUILD alone.
    let picked = fixture.json(draft, &["-p", "2"]);
    assert_eq!(picked["ok"], true, "{picked}");
    let day = fixture.day();
    assert!(
        day.contains("- [ ] () \u{2014} FIX\n  - [[cash_pkt#^"),
        "{day}"
    );
    assert_eq!(day.matches("[[cash_pkt#").count(), 1, "{day}");
    assert_ne!(day, day_before);
    let build = day
        .split("- [ ] () \u{2014} FIX")
        .next()
        .expect("BUILD block");
    assert!(!build.contains("cash_pkt"), "{day}");
}

#[test]
fn offer_minted_ids_avoid_authored_ids() {
    let fixture = Fixture::plain("bob-cli-offer-mint-collision");
    let json = fixture.json(
        concat!(
            "Finish packet @cash:pkt@\n",
            "- Keep this id ^draft-memo\n",
            "- Draft memo\n",
            "- Call the bank :call-bank\n",
        ),
        &["-p", "all"],
    );
    assert_eq!(json["ok"], true, "{json}");
    let rows = successors(&json);
    assert_eq!(rows.len(), 2, "{json}");
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row["block_id"].as_str().expect("ID"))
        .collect();
    assert_eq!(ids[0], "draft-memo", "authored IDs stay put");
    assert_ne!(ids[1], "draft-memo", "the mint reserved the authored ID");
    assert_ne!(ids[1], "prj");
    assert_ne!(ids[1], "call-bank");
    assert!(!ids[1].is_empty());
    let note = &fixture.snapshot()["cash_pkt.md"];
    for id in &ids {
        assert_eq!(note.matches(&format!("^{id}\n")).count(), 1, "{note}");
    }
    assert!(note.contains("^prj"), "{note}");
}

#[test]
fn offer_numbering_continues_after_a_done_completion_in_the_same_draft() {
    let fixture = Fixture::plain("bob-cli-offer-mixed-numbering");
    write_file(
        &fixture.vault.join("sase.md"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n",
    );
    write_file(
        &fixture.day_file,
        "## Pomodoros\n- [ ] (**1330-1400** [t:: 30m]) Work\n  - [[sase#^p]]\n",
    );
    let draft = concat!(
        "!sase:p\n\n",
        "Finish packet @cash:pkt@\n",
        "- Draft memo\n",
        "- Collect docs ^docs\n",
    );
    let preview = fixture.json(draft, &["--dry-run"]);
    assert_eq!(preview["ok"], true, "{preview}");
    let captures = preview["captures"].as_array().expect("captures");
    let done = captures[0]["task_complete"]["unblocked"]
        .as_array()
        .expect("done offers");
    let project = successors(&captures[1]);
    assert_eq!(done.len(), 1, "{preview}");
    assert_eq!(done[0]["offer"]["n"], 1);
    assert_eq!(done[0]["offer"]["group"], "hand_off");
    let numbers: Vec<u64> = project
        .iter()
        .map(|row| row["offer"]["n"].as_u64().expect("n"))
        .collect();
    assert_eq!(numbers, vec![2, 3], "project offers follow the Done offers");
    assert_eq!(preview["pull_in"]["offer_count"], 3, "{preview}");

    // A numeric pick reaches the project offer across the whole draft.
    let picked = fixture.json(draft, &["--dry-run", "-p", "3"]);
    assert_eq!(picked["ok"], true, "{picked}");
    let picked_rows = successors(&picked["captures"][1]);
    assert_eq!(picked_rows[0]["offer"]["selected"], false);
    assert_eq!(picked_rows[1]["offer"]["selected"], true);
    assert_eq!(picked["pull_in"]["picked_count"], 1);
}

#[test]
fn offer_is_dropped_when_a_later_item_closes_or_links_the_created_task() {
    let base = concat!(
        "Finish packet @cash:pkt@\n",
        "- Draft memo ^draft\n",
        "- Collect docs ^docs\n",
        "- Plain memo\n",
    );

    let closed = Fixture::plain("bob-cli-offer-net-close");
    let closed_json =
        closed.json(&format!("{base}\n!cash_pkt:draft\n"), &["--dry-run"]);
    assert_eq!(closed_json["ok"], true, "{closed_json}");
    let remaining: Vec<&str> = successors(&closed_json["captures"][0])
        .iter()
        .map(|row| row["text"].as_str().expect("text"))
        .collect();
    assert_eq!(
        remaining,
        vec!["Collect docs", "Plain memo"],
        "{closed_json}"
    );
    assert_eq!(closed_json["pull_in"]["offer_count"], 2, "{closed_json}");

    let linked = Fixture::plain("bob-cli-offer-net-link");
    let linked_json =
        linked.json(&format!("{base}\n@cash_pkt:draft\n"), &["--dry-run"]);
    assert_eq!(linked_json["ok"], true, "{linked_json}");
    let remaining: Vec<(&str, u64)> = successors(&linked_json["captures"][0])
        .iter()
        .map(|row| {
            (
                row["text"].as_str().expect("text"),
                row["offer"]["n"].as_u64().expect("n"),
            )
        })
        .collect();
    assert_eq!(
        remaining,
        vec![("Collect docs", 1), ("Plain memo", 2)],
        "the surviving ID-less row keeps its place: {linked_json}"
    );

    // A pick of a task a later item closes does not survive either.
    let picked = Fixture::plain("bob-cli-offer-net-picked");
    let picked_json = picked.json(
        &format!("{base}\n!cash_pkt:draft\n"),
        &["--dry-run", "-p", "all"],
    );
    assert_eq!(picked_json["ok"], true, "{picked_json}");
    let picked_texts: Vec<&str> = successors(&picked_json["captures"][0])
        .iter()
        .map(|row| row["text"].as_str().expect("text"))
        .collect();
    assert_eq!(picked_texts, vec!["Collect docs", "Plain memo"]);
}

fn assert_offer_lines_match_final_note(
    fixture: &Fixture,
    json: &serde_json::Value,
) {
    let snapshot = fixture.snapshot();
    let note = snapshot
        .get("cash_pkt.md")
        .unwrap_or_else(|| panic!("cash_pkt.md missing: {json}"));
    let lines: Vec<&str> = note.lines().collect();
    for row in successors(json) {
        let line = row["line"].as_u64().expect("line") as usize;
        let text = row["text"].as_str().expect("text");
        let block_id = row["block_id"].as_str().expect("block id");
        let actual = lines
            .get(line - 1)
            .unwrap_or_else(|| panic!("line {line} missing in {note}"));
        assert!(
            actual.contains("#task"),
            "line {line} is not a task for {text}: {actual}\n{note}\n{json}"
        );
        // The bare text (without checkbox) must appear on its own line.
        // For typed rows the `[type:: ...]` field stays in the text.
        let first_token = text.split_whitespace().next().expect("token");
        assert!(
            actual.contains(first_token),
            "line {line} does not address its own task {text}: {actual}\n{note}\n{json}"
        );
        assert!(
            actual.contains(text),
            "line {line} does not contain its full text {text}: {actual}\n{note}\n{json}"
        );
        if block_id.is_empty() {
            assert!(
                !actual.contains('^'),
                "unselected row must not acquire a committed ID: {actual}\n{json}"
            );
        } else {
            assert!(
                actual.contains(&format!("^{block_id}")),
                "line {line} does not carry its block ID ^{block_id}: {actual}\n{json}"
            );
        }
    }
}

#[test]
fn offer_line_shift_updates_final_lines_instead_of_trusting_old_lines() {
    // A later item that adds nested context under `^anchor` shifts the
    // ID-less task down two lines. The net pass must refresh the reported
    // final line; the old line now holds plain context, not the task.
    let draft = concat!(
        "Finish packet @cash:pkt@\n",
        "- Anchor ^anchor\n",
        "- Draft memo\n",
        "\n",
        "Add context @cash_pkt+anchor\n",
        "- extra detail\n",
    );
    let fixture = Fixture::plain("bob-cli-offer-line-shift");
    let json = fixture.json(draft, &[]);
    assert_eq!(json["ok"], true, "{json}");
    let rows = successors(&json);
    assert_eq!(rows.len(), 2, "{json}");
    assert_offer_lines_match_final_note(&fixture, &json);
    let note = &fixture.snapshot()["cash_pkt.md"];
    // The ID-less task actually lives at line 16 after the two inserted
    // context lines (13 anchor, 14-15 context, 16 draft).
    let draft_row = rows
        .iter()
        .find(|row| row["text"] == "Draft memo")
        .expect("draft row");
    assert_eq!(draft_row["line"], 16, "{json}\n{note}");
    assert_eq!(draft_row["block_id"], "", "{json}");
}

#[test]
fn offer_typed_line_shift_keeps_typed_offer_with_fields() {
    // The same shift with a `[type:: FIX]` ID-less task: cleaning must
    // strip inline fields on both sides, or the still-valid typed offer
    // is dropped.
    let draft = concat!(
        "Finish packet @cash:pkt@\n",
        "- Anchor ^anchor\n",
        "- Fix the bug [type:: FIX]\n",
        "\n",
        "Add context @cash_pkt+anchor\n",
        "- extra detail\n",
    );
    let fixture = Fixture::typed("bob-cli-offer-typed-shift");
    let json = fixture.json(draft, &[]);
    assert_eq!(json["ok"], true, "{json}");
    let rows = successors(&json);
    assert_eq!(rows.len(), 2, "{json}");
    assert_offer_lines_match_final_note(&fixture, &json);
    assert!(
        rows.iter()
            .any(|row| row["text"].as_str() == Some("Fix the bug [type:: FIX]")),
        "{json}"
    );
}

#[test]
fn offer_duplicate_descriptions_keep_distinct_final_lines() {
    // Two ID-less tasks share one cleaned description. The net pass must
    // use the frozen duplicate ordinal, not the stale line, or the second
    // row matches the first task's line.
    let draft = concat!(
        "Finish packet @cash:pkt@\n",
        "- Anchor ^anchor\n",
        "- Draft memo\n",
        "- Draft memo\n",
        "\n",
        "Add context @cash_pkt+anchor\n",
        "- extra detail\n",
    );
    let fixture = Fixture::plain("bob-cli-offer-duplicate-shift");
    let json = fixture.json(draft, &[]);
    assert_eq!(json["ok"], true, "{json}");
    let rows = successors(&json);
    assert_eq!(rows.len(), 3, "{json}");
    assert_offer_lines_match_final_note(&fixture, &json);
    let mut reported: Vec<u64> = rows
        .iter()
        .map(|row| row["line"].as_u64().expect("line"))
        .collect();
    reported.sort_unstable();
    assert_eq!(reported, vec![13, 16, 17], "{json}");
}

#[test]
fn offer_selected_pick_keeps_frozen_identity_across_line_shift() {
    // Picks keep the same frozen identity in preview/submit and across
    // numeric/key selection; unselected rows never acquire committed IDs.
    let draft = concat!(
        "Finish packet @cash:pkt@\n",
        "- Anchor ^anchor\n",
        "- Draft memo\n",
        "\n",
        "Add context @cash_pkt+anchor\n",
        "- extra detail\n",
    );
    let preview = Fixture::plain("bob-cli-offer-pick-preview");
    let preview_json = preview.json(draft, &["--dry-run", "-p", "all"]);
    let committed = Fixture::plain("bob-cli-offer-pick-commit");
    let committed_json = committed.json(draft, &["-p", "all"]);
    for json in [&preview_json, &committed_json] {
        let rows = successors(json);
        assert_eq!(rows.len(), 2, "{json}");
        assert!(
            rows.iter().all(|row| row["offer"]["selected"] == true),
            "{json}"
        );
    }
    // Only the committed run writes the note; the dry-run preview has no
    // file to check lines against, but both must share frozen keys.
    assert_offer_lines_match_final_note(&committed, &committed_json);
    let preview_keys: Vec<&str> = successors(&preview_json)
        .iter()
        .map(|row| row["offer"]["key"].as_str().expect("key"))
        .collect();
    let committed_keys: Vec<&str> = successors(&committed_json)
        .iter()
        .map(|row| row["offer"]["key"].as_str().expect("key"))
        .collect();
    assert_eq!(preview_keys, committed_keys, "preview/submit keep keys");

    // A partial pick leaves the unselected ID-less row without an ID.
    let partial = Fixture::plain("bob-cli-offer-pick-partial");
    let partial_json = partial.json(draft, &["-p", "1"]);
    let rows = successors(&partial_json);
    assert_eq!(rows.len(), 2, "{partial_json}");
    assert_eq!(rows[0]["offer"]["selected"], true);
    assert_eq!(rows[1]["offer"]["selected"], false);
    assert_eq!(rows[1]["block_id"], "");
    assert_offer_lines_match_final_note(&partial, &partial_json);
}

#[test]
fn offer_pending_blocked_by_later_dependency_is_dropped_like_ready_and_next() {
    // A later batch item that adds an open prerequisite blocks the created
    // task. Ready, Next, and Pending offers must all disappear; the
    // pre-fix filter only covered Ready/Next, leaving Pending offered.
    let fixture = Fixture::plain("bob-cli-offer-pending-blocked");
    write_file(
        &fixture.vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\n---\n# Cash\n- [ ] #task Gate ^gate\n",
    );
    write_file(
        &fixture
            .vault
            .join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        &blocked_tasks_settings_json(""),
    );
    let draft = concat!(
        "Finish packet @cash:pkt@\n",
        "- Ready work ^ready\n",
        "- [*] Next work ^next\n",
        "- [/] Pending work ^pending\n",
        "\n",
        "@cash_pkt+ready &cash:gate\n",
        "\n",
        "@cash_pkt+next &cash:gate\n",
        "\n",
        "@cash_pkt+pending &cash:gate\n",
    );
    let json = fixture.json(draft, &["--dry-run"]);
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(
        json["project_note"]["successors"],
        serde_json::json!([]),
        "{json}"
    );
    assert_eq!(json["pull_in"]["offer_count"], 0, "{json}");
    assert_eq!(json["pull_in"]["picked_count"], 0, "{json}");
}

#[test]
fn offer_human_output_names_the_project_and_cleans_the_minted_caption() {
    let fixture = Fixture::plain("bob-cli-offer-human");
    let output = fixture.run(
        "Finish packet @cash:pkt@\n- Draft memo\n- Collect docs ^docs\n",
        &["--dry-run", "-p", "1"],
    );
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("New project cash_pkt.md"), "{out}");
    assert!(
        out.contains("offer 1  would link [ ] \u{2192} [*] Draft memo"),
        "{out}"
    );
    assert!(
        out.contains("\u{b7} added ^draft-memo\n"),
        "no stray ]]: {out}"
    );
    assert!(
        out.contains("offer 2  unblocked [ ] \u{2192} [ ] Collect docs"),
        "{out}"
    );

    let refusal = fixture.run(
        "Finish packet @cash:pkt@\n- Draft memo\n",
        &["--dry-run", "--require-pull-in-choice"],
    );
    assert_eq!(
        refusal.status.code(),
        Some(1),
        "{}",
        format_output(&refusal)
    );
    let message = format_output(&refusal);
    assert!(message.contains("New project cash_pkt.md"), "{message}");
    assert!(message.contains("offer 1"), "{message}");
    assert!(!fixture.vault.join("cash_pkt.md").exists());
}

/// One offer of a pick-free preview: its frozen key, text, and authored ID.
struct BaselineOffer {
    key: String,
    text: String,
    block_id: String,
}

fn baseline_offers(draft: &str) -> Vec<BaselineOffer> {
    let fixture = Fixture::plain("bob-cli-offer-identity-baseline");
    let json = fixture.json(draft, &["--dry-run"]);
    assert_eq!(json["ok"], true, "{json}");
    successors(&json)
        .iter()
        .enumerate()
        .map(|(index, row)| {
            assert_eq!(row["offer"]["n"], index + 1, "{json}");
            assert_eq!(row["offer"]["selected"], false, "{json}");
            BaselineOffer {
                key: row["offer"]["key"].as_str().expect("key").to_string(),
                text: row["text"].as_str().expect("text").to_string(),
                block_id: row["block_id"].as_str().expect("id").to_string(),
            }
        })
        .collect()
}

/// Picks each offer of `draft` alone, by number and by opaque key, in a
/// preview and in a commit. Every original offer must survive with its
/// frozen key and number, the unpicked ones must stay ID-less (or keep
/// their authored ID), and each reported line must be its own task row.
fn assert_each_single_pick_keeps_every_offer(draft: &str, duplicate: &str) {
    let baseline = baseline_offers(draft);
    assert!(baseline.len() >= 3, "{draft}");
    for pick in 0..baseline.len() {
        let selectors = [(pick + 1).to_string(), baseline[pick].key.clone()];
        let mut seen: Vec<Vec<serde_json::Value>> = Vec::new();
        for selector in &selectors {
            for dry_run in [true, false] {
                let fixture = Fixture::plain("bob-cli-offer-identity-pick");
                let mut flags = vec!["-p", selector.as_str()];
                if dry_run {
                    flags.push("--dry-run");
                }
                let json = fixture.json(draft, &flags);
                let context = format!(
                    "pick {pick} via {selector} dry_run={dry_run}\n{json}"
                );
                assert_eq!(json["ok"], true, "{context}");
                let rows = successors(&json);
                assert_eq!(rows.len(), baseline.len(), "{context}");
                assert_eq!(
                    json["pull_in"]["offer_count"],
                    baseline.len(),
                    "{context}"
                );
                assert_eq!(json["pull_in"]["picked_count"], 1, "{context}");
                let mut previous_line = 0;
                for (index, (row, original)) in
                    rows.iter().zip(&baseline).enumerate()
                {
                    assert_eq!(row["offer"]["n"], index + 1, "{context}");
                    assert_eq!(row["offer"]["key"], original.key, "{context}");
                    assert_eq!(row["text"], original.text, "{context}");
                    assert_eq!(
                        row["offer"]["selected"],
                        index == pick,
                        "{context}"
                    );
                    let id = row["block_id"].as_str().expect("block id");
                    if index == pick {
                        assert!(!id.is_empty(), "{context}");
                        if !original.block_id.is_empty() {
                            assert_eq!(id, original.block_id, "{context}");
                        }
                    } else {
                        assert_eq!(id, original.block_id, "{context}");
                    }
                    let line = row["line"].as_u64().expect("line") as usize;
                    assert!(
                        line > previous_line,
                        "each offer owns its own later line: {context}"
                    );
                    previous_line = line;
                }
                if !dry_run {
                    assert_offer_lines_match_final_note(&fixture, &json);
                    let note = &fixture.snapshot()["cash_pkt.md"];
                    let picked_duplicates = usize::from(
                        baseline[pick].text == duplicate
                            && baseline[pick].block_id.is_empty(),
                    );
                    let minted_duplicates = note
                        .lines()
                        .filter(|line| {
                            line.contains(duplicate)
                                && line.contains(" ^")
                                && !line.contains("^memo")
                        })
                        .count();
                    assert_eq!(
                        minted_duplicates, picked_duplicates,
                        "only a picked duplicate mints an ID: {context}\n{note}"
                    );
                    for (path, contents) in fixture.snapshot() {
                        assert!(
                            !contents.contains("[fresh::"),
                            "creation never stamps: {path}\n{contents}"
                        );
                    }
                }
                seen.push(rows.clone());
            }
        }
        for rows in &seen[1..] {
            assert_eq!(
                *rows, seen[0],
                "number, key, preview, and commit report the same offers"
            );
        }
    }
}

#[test]
fn offer_two_equal_descriptions_survive_picking_the_first_or_last() {
    // Reproduces the lander's scratch vault: `-p 1` used to report one
    // offer because the second duplicate was looked up among the shrunken
    // ID-less population.
    assert_each_single_pick_keeps_every_offer(
        concat!(
            "Finish packet @cash:pkt@\n",
            "- Anchor ^anchor\n",
            "- Draft memo\n",
            "- Draft memo\n",
            "\n",
            "Add context @cash_pkt+anchor\n",
            "- extra detail\n",
        ),
        "Draft memo",
    );
}

#[test]
fn offer_three_equal_descriptions_survive_picking_first_middle_or_last() {
    assert_each_single_pick_keeps_every_offer(
        concat!(
            "Finish packet @cash:pkt@\n",
            "- Anchor ^anchor\n",
            "- Draft memo\n",
            "- Draft memo\n",
            "- Draft memo\n",
            "\n",
            "Add context @cash_pkt+anchor\n",
            "- extra detail\n",
        ),
        "Draft memo",
    );
}

#[test]
fn offer_equal_descriptions_stay_distinct_from_an_authored_id_twin() {
    // The authored-ID twin sits between the ID-less duplicates. It is a
    // separate identity domain: it never counts toward an ID-less
    // ordinal, and picking it never shifts one.
    assert_each_single_pick_keeps_every_offer(
        concat!(
            "Finish packet @cash:pkt@\n",
            "- Anchor ^anchor\n",
            "- Draft memo\n",
            "- Draft memo ^memo\n",
            "- Draft memo\n",
            "\n",
            "Add context @cash_pkt+anchor\n",
            "- extra detail\n",
        ),
        "Draft memo",
    );
}

#[test]
fn offer_duplicate_picks_without_later_context_keep_every_offer() {
    // The two-duplicate case with nothing shifting lines at all, so the
    // identity repair stands on its own.
    assert_each_single_pick_keeps_every_offer(
        concat!(
            "Finish packet @cash:pkt@\n",
            "- Draft memo\n",
            "- Draft memo\n",
            "- Draft memo\n",
        ),
        "Draft memo",
    );
}

#[test]
fn offer_picking_several_duplicates_keeps_the_rest_and_matches_authored() {
    // Two of three duplicates picked: the survivor is the unpicked one,
    // and the files equal the authored ` :` equivalent of the same picks.
    let draft = concat!(
        "Finish packet @cash:pkt@\n",
        "- Draft memo\n",
        "- Draft memo\n",
        "- Draft memo\n",
        "- Other work\n",
    );
    let baseline = baseline_offers(draft);
    assert_eq!(baseline.len(), 4);
    let picks = format!("1,{}", baseline[2].key);
    let offer = Fixture::plain("bob-cli-offer-identity-many");
    let json = offer.json(draft, &["-p", picks.as_str()]);
    assert_eq!(json["ok"], true, "{json}");
    let rows = successors(&json);
    let selected: Vec<bool> = rows
        .iter()
        .map(|row| row["offer"]["selected"] == true)
        .collect();
    assert_eq!(selected, vec![true, false, true, false], "{json}");
    let keys: Vec<&str> = rows
        .iter()
        .map(|row| row["offer"]["key"].as_str().expect("key"))
        .collect();
    let expected: Vec<&str> =
        baseline.iter().map(|offer| offer.key.as_str()).collect();
    assert_eq!(keys, expected, "{json}");
    assert_eq!(json["pull_in"]["offer_count"], 4, "{json}");
    assert_eq!(json["pull_in"]["picked_count"], 2, "{json}");
    assert_offer_lines_match_final_note(&offer, &json);

    let authored = Fixture::plain("bob-cli-offer-identity-many-authored");
    let first = rows[0]["block_id"].as_str().expect("first ID");
    let third = rows[2]["block_id"].as_str().expect("third ID");
    let created = authored.json(
        &format!(
            concat!(
                "Finish packet @cash^pkt@\n",
                "- Draft memo :{}\n",
                "- Draft memo\n",
                "- Draft memo :{}\n",
                "- Other work\n",
            ),
            first, third
        ),
        &[],
    );
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(offer.snapshot(), authored.snapshot());
}

/// A fixture whose default destination is an existing `mac_inbox.md`, so a
/// route-less draft would write an unfinished marker there if it ever fell
/// through to prose.
fn inbox_fixture(prefix: &str) -> Fixture {
    let fixture = Fixture::plain(prefix);
    write_file(&fixture.vault.join("inbox.md"), "# Inbox\n");
    write_file(
        &fixture.vault.join("mac_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Route me ^route-task\n",
        ),
    );
    fixture
}

const ROUTE_ERROR: &str =
    "Pomodoro capture route must contain only A-Z, a-z, 0-9, '_' or '-'";

#[test]
fn route_less_sealed_offers_refuse_dry_run_and_submit_without_writing() {
    // The editor reports a route need for `@:pkt@`; execution must refuse it
    // with the Pomodoro route error instead of capturing the unfinished
    // marker (and its children) as inbox task text. Leading and trailing
    // positions, named and quoted names, authored children, and multibyte
    // body text all refuse; the caret form keeps its own route error.
    for draft in [
        "Finish packet @:pkt@\n",
        "Finish packet @:pkt@\n- Draft memo\n",
        "Finish packet @:pkt@#admin\n- Draft memo\n- Collect docs\n",
        "Finish packet @:pkt@#\"Lit Name\"\n- Draft memo\n",
        "@:pkt@ Finish packet\n- Draft memo\n",
        "@:pkt@#admin Finish packet\n- Draft memo\n",
        "@:pkt@#\"Lit Name\" Finish packet\n- Draft memo\n",
        "Ünï packet 日本 @:pkt@#admin\n- Draft memo\n",
        "@:pkt@\n",
    ] {
        for flags in [&["--dry-run"][..], &[][..], &["-p", "all"][..]] {
            let fixture = inbox_fixture("bob-cli-route-less-offer-refusal");
            let before = fixture.snapshot();
            let day_before = fixture.day();
            let output = fixture.run(draft, flags);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{draft:?} {flags:?}\n{}",
                format_output(&output)
            );
            let message = format_output(&output);
            assert!(
                message.contains(ROUTE_ERROR),
                "{draft:?} {flags:?}\n{message}"
            );
            assert_eq!(fixture.snapshot(), before, "{draft:?} {flags:?}");
            assert_eq!(fixture.day(), day_before, "{draft:?} {flags:?}");
            assert!(!fixture.vault.join("cash_pkt.md").exists());
            assert!(!fixture.vault.join("_pkt.md").exists());
        }
    }

    // JSON mode reports the same error and leaves the same files alone.
    let fixture = inbox_fixture("bob-cli-route-less-offer-refusal-json");
    let before = fixture.snapshot();
    let output = fixture.run(
        "Finish packet @:pkt@#admin\n- Draft memo\n",
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], false, "{json}");
    assert!(
        json["error"].as_str().is_some_and(|error| {
            error.contains(ROUTE_ERROR) && error.contains("capture item 1")
        }),
        "{json}"
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn route_less_sealed_offer_refuses_a_mixed_draft_atomically() {
    // A valid earlier item must not be written when a later item is a
    // route-less sealed offer: the refusal is atomic across the batch.
    for draft in [
        "Earlier task @cash\n\nFinish packet @:pkt@\n- Draft memo\n",
        "Finish packet @cash:pkt@\n- Draft memo\n\nOther packet @:other@#admin\n- Collect docs\n",
        "Earlier task @cash\n\n@:pkt@#\"Lit Name\" Finish packet\n",
    ] {
        for flags in [&["--dry-run"][..], &[][..]] {
            let fixture = inbox_fixture("bob-cli-route-less-offer-mixed");
            let before = fixture.snapshot();
            let day_before = fixture.day();
            let output = fixture.run(draft, flags);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{draft:?} {flags:?}\n{}",
                format_output(&output)
            );
            let message = format_output(&output);
            assert!(
                message.contains(ROUTE_ERROR),
                "{draft:?} {flags:?}\n{message}"
            );
            assert!(
                message.contains("capture item 2"),
                "{draft:?} {flags:?}\n{message}"
            );
            assert_eq!(fixture.snapshot(), before, "{draft:?} {flags:?}");
            assert_eq!(fixture.day(), day_before, "{draft:?} {flags:?}");
        }
    }
}

#[test]
fn routed_sealed_offers_and_literal_route_less_forms_are_unchanged() {
    // The route-less guard is scoped to the sealed family: a routed offer
    // still creates its project note, and unsealed `@:` / `@:id` or a
    // mid-prose sealed token stay literal inbox task text.
    let routed = inbox_fixture("bob-cli-route-less-offer-routed");
    let json = routed.json(
        "Finish packet @cash:pkt@#admin\n- Draft memo\n",
        &["-p", "all"],
    );
    assert_eq!(json["ok"], true, "{json}");
    assert!(routed.snapshot().contains_key("cash_pkt.md"));

    for (draft, task_line) in [
        (
            "Finish packet @:pkt\n",
            "- [ ] #task Finish packet @:pkt [created::2026-09-20]",
        ),
        (
            "Finish packet @:\n",
            "- [ ] #task Finish packet @: [created::2026-09-20]",
        ),
        (
            "Finish @:pkt@ later\n",
            "- [ ] #task Finish @:pkt@ later [created::2026-09-20]",
        ),
    ] {
        let fixture = inbox_fixture("bob-cli-route-less-offer-literal");
        let json = fixture.json(draft, &[]);
        assert_eq!(json["ok"], true, "{draft:?}\n{json}");
        assert_eq!(json["kind"], "task", "{draft:?}\n{json}");
        assert_eq!(json["task_line"], task_line, "{draft:?}\n{json}");
        let inbox = &fixture.snapshot()["mac_inbox.md"];
        assert!(inbox.contains(task_line), "{draft:?}\n{inbox}");
    }
}
