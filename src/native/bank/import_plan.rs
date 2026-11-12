//! Pure whole-batch planner for bank import.
//!
//! [`plan_batch`] turns resolved statements, the current vault snapshot, and
//! the folded bank journal into exact month after-images and ordered journal
//! events without touching the vault: no lock, Git, sync, journal append, or
//! filesystem creation happens here. The same plan feeds preview and apply,
//! so identical inputs under one frozen clock produce identical bytes.
//!
//! Disposition rules, in processing order `(posted, input-file order, record
//! order)`:
//!
//! - a key journaled by any host, or its reserved row ID already present in
//!   the bank notes, is seen: never rewritten, ticked, or resurrected, so a
//!   deleted row stays deleted;
//! - a seen-by-ID row without journal evidence is a crash recovery: the
//!   missing `bank.txn` event is backfilled (counted as seen) without
//!   another row;
//! - a transaction before its account start is recorded (`before_start`)
//!   without creating or matching a row;
//! - otherwise the uniquely nearest ID-less row on the same account with
//!   exactly equal signed cents inside `[posted - 1 day, posted + 3 days]`
//!   is matched by appending only the trailing bank ID; an equal-distance
//!   tie writes a new waiting row and counts `ambiguous`;
//! - otherwise a new unchecked canonical row is rendered under its posted
//!   day.
//!
//! Journal `bank.txn` events fire once per new identity (including
//! recoveries), never once per overlapping file; a repeat of a batch key
//! counts as seen. A `bank.import` event fires per file with new evidence
//! (digest, window, or balance), even when every transaction was seen. A
//! fully recorded identical replay yields no events and no edits.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;

use super::{
    config::{AccountKind, BankConfig},
    grammar::render_row,
    identity::{merchant_key, transaction_key, IdReservation},
    journal::{
        serialize_import, serialize_txn, ImportCounts, JournalSnapshot,
        NewImport, NewTxn, TxnOutcome,
    },
    model::{month_of, month_path, BankRow},
    month::{
        canonical_template, parse_month_note, plan_append_id, plan_append_row,
        plan_insert_day, LocatedRowKind,
    },
    naming::{resolve_name, LearnedName},
    scan::BankScan,
    source::{ParsedTransaction, ResolvedStatement, SourceKind},
};

/// One vault file the planner read, with the exact bytes it saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadDep {
    pub(crate) path: String,
    pub(crate) preimage: Option<Vec<u8>>,
}

/// One restaurant candidate supplied by the caller from the records
/// registry (exact title matches only; the planner never scans the vault).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestaurantEntry {
    pub(crate) title: String,
    pub(crate) path: String,
    pub(crate) emoji: Option<String>,
}

impl RestaurantEntry {
    pub(crate) fn link(&self) -> String {
        match &self.emoji {
            Some(emoji) if !emoji.is_empty() => {
                format!("[[{}|{emoji}]]", self.path)
            }
            _ => format!("[[{}]]", self.path),
        }
    }
}

/// Everything the planner needs, already read.
pub(crate) struct PlanInput<'a> {
    pub(crate) config: Option<&'a BankConfig>,
    pub(crate) hub: ReadDep,
    pub(crate) scan: &'a BankScan,
    pub(crate) journal: &'a JournalSnapshot,
    pub(crate) statements: Vec<ResolvedStatement>,
    /// Human labels for `statements` in order (file paths or display
    /// names); used only in findings.
    pub(crate) file_labels: Vec<String>,
    pub(crate) restaurants: Vec<RestaurantEntry>,
    pub(crate) restaurant_deps: Vec<ReadDep>,
    /// Single frozen clock instant for this plan (`created` stamps and
    /// journal `ts` fields), formatted `%Y-%m-%dT%H:%M:%S+00:00`.
    pub(crate) now_stamp: String,
    /// Host label stamped on new journal events.
    pub(crate) host: String,
}

/// Why the whole batch is refused before any mutation shape is produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanRefusal {
    /// No usable `bank:` configuration (missing hub or invalid mapping).
    MissingConfig,
    /// Month files that could not be read; the write would be unsafe.
    IncompleteRead { paths: Vec<String> },
    /// Both the 8- and 12-character ID prefixes name a different key.
    IdCollision { prefix: String },
    /// Immutable transaction money fields disagree across this batch or the
    /// folded journal.
    IdentityConflict {
        file: String,
        record: usize,
        field: &'static str,
    },
    /// More than one unjournaled row matches this transaction's recovery ID.
    DuplicateUnjournaledId { file: String, record: usize },
}

impl PlanRefusal {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::MissingConfig => {
                "no usable bank configuration; fix bank.md first".to_owned()
            }
            Self::IncompleteRead { paths } => format!(
                "refusing unsafe import; unreadable: {}",
                paths.join(", ")
            ),
            Self::IdCollision { prefix } => format!(
                "refusing import; row ID prefix {prefix} names two identities"
            ),
            Self::IdentityConflict { file, record, field } => format!(
                "refusing import; {file} record {record} field {field} conflicts with prior transaction evidence"
            ),
            Self::DuplicateUnjournaledId { file, record } => format!(
                "refusing import; {file} record {record} matches an unjournaled bank ID present on multiple rows"
            ),
        }
    }
}

/// One non-fatal planner note. Messages never echo raw bank identifiers or
/// description text, only vault paths, file labels, and record positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanFinding {
    pub(crate) code: &'static str,
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

/// Per-file import summary (also the shape of journal `bank.import`
/// counts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    pub(crate) file: String,
    pub(crate) account: String,
    pub(crate) source: SourceKind,
    pub(crate) start: Option<NaiveDate>,
    pub(crate) end: Option<NaiveDate>,
    pub(crate) counts: ImportCounts,
    pub(crate) out_cents: i64,
    pub(crate) in_cents: i64,
    pub(crate) balance_cents: Option<i64>,
    pub(crate) balance_asof: Option<NaiveDate>,
    /// False when this file's evidence was already fully recorded.
    pub(crate) new_evidence: bool,
}

/// One month file the plan rewrites (`before` is `None` for a new file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonthEdit {
    pub(crate) path: String,
    pub(crate) before: Option<String>,
    pub(crate) after: String,
}

/// The complete batch plan: previews, guarded inputs, and journal events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchPlan {
    pub(crate) files: Vec<PlannedFile>,
    /// New and matched rows in the after-images, ordered by note and line,
    /// ready for `model::row_json`.
    pub(crate) rows: Vec<BankRow>,
    pub(crate) month_edits: Vec<MonthEdit>,
    pub(crate) journal_events: Vec<String>,
    pub(crate) read_deps: Vec<ReadDep>,
    pub(crate) findings: Vec<PlanFinding>,
    pub(crate) counts: ImportCounts,
    /// False for an identical replay: no edits and no events.
    pub(crate) changed: bool,
}

/// Plan the whole batch, or refuse it before producing any mutation.
pub(crate) fn plan_batch(
    input: PlanInput<'_>,
) -> Result<BatchPlan, PlanRefusal> {
    let config = input.config.ok_or(PlanRefusal::MissingConfig)?;
    if !input.scan.io_errors.is_empty() {
        return Err(PlanRefusal::IncompleteRead {
            paths: input.scan.io_errors.clone(),
        });
    }
    Planner::new(&input, config).run()
}

struct Planner<'a> {
    input: &'a PlanInput<'a>,
    config: &'a BankConfig,
    rows: Vec<RowState>,
    ids: IdReservation,
    findings: Vec<PlanFinding>,
    month_texts: BTreeMap<String, String>,
    pending_rows: BTreeMap<(String, NaiveDate), Vec<PendingRow>>,
    match_edits: BTreeMap<String, Vec<(usize, String)>>,
    txn_events: Vec<NewTxn>,
    file_counts: Vec<ImportCounts>,
    counts: ImportCounts,
    batch_keys: HashSet<String>,
    batch_money: HashMap<String, (NaiveDate, i64)>,
}

#[derive(Debug, Clone)]
struct RowState {
    row: BankRow,
    consumed: bool,
}

struct PendingRow {
    cents: i64,
    payee: String,
    category: Option<String>,
    account: String,
    links: Vec<String>,
    id: String,
}

impl<'a> Planner<'a> {
    fn new(input: &'a PlanInput<'a>, config: &'a BankConfig) -> Self {
        let rows = collect_planner_rows(input, config);
        let mut ids = IdReservation::from_map(&input.journal.ids);
        // Occupy every unjournaled in-notes ID up front so a fresh
        // reservation can never hand the same ID to a different
        // identity. Journaled IDs keep their real owners; recovery
        // overwrites its own entry with the real key below.
        for row in &rows {
            if let Some(id) = row.row.id.as_deref()
                && !input.journal.ids.contains_key(id)
            {
                ids.occupy(id, &format!("in-notes:{id}"));
            }
        }
        let mut month_texts = BTreeMap::new();
        for month in &input.scan.months {
            month_texts.insert(month.path.clone(), month.contents.clone());
        }
        Self {
            input,
            config,
            rows,
            ids,
            findings: Vec::new(),
            month_texts,
            pending_rows: BTreeMap::new(),
            match_edits: BTreeMap::new(),
            txn_events: Vec::new(),
            file_counts: Vec::new(),
            counts: ImportCounts::default(),
            batch_keys: HashSet::new(),
            batch_money: HashMap::new(),
        }
    }

    fn run(mut self) -> Result<BatchPlan, PlanRefusal> {
        self.index_duplicates();
        self.surface_journal_notes();
        // Copy the shared input reference out so the loop below can call
        // `&mut self` methods while reading statements.
        let input = self.input;
        let mut ordered: Vec<(usize, usize)> = Vec::new();
        for (file_index, statement) in input.statements.iter().enumerate() {
            for (record_index, _) in
                statement.statement.transactions.iter().enumerate()
            {
                ordered.push((file_index, record_index));
            }
        }
        ordered.sort_by(|left, right| {
            let left_txn =
                &input.statements[left.0].statement.transactions[left.1];
            let right_txn =
                &input.statements[right.0].statement.transactions[right.1];
            left_txn
                .posted
                .cmp(&right_txn.posted)
                .then(left.0.cmp(&right.0))
                .then(left_txn.record_order.cmp(&right_txn.record_order))
        });
        for (file_index, record_index) in ordered {
            let statement = &input.statements[file_index];
            let txn = &statement.statement.transactions[record_index];
            self.process(input, file_index, statement, txn)?;
        }
        let mut files = Vec::new();
        for (file_index, statement) in input.statements.iter().enumerate() {
            files.push(self.finish_file(input, file_index, statement));
        }
        self.emit_months(input);
        Ok(self.finish(input, files))
    }

    /// Record duplicate `^bank-` IDs as findings; duplicates are never
    /// matched, recovered, or chosen between.
    fn index_duplicates(&mut self) {
        let mut first: HashMap<&str, (String, usize)> = HashMap::new();
        for row in &self.rows {
            let Some(id) = row.row.id.as_deref() else {
                continue;
            };
            if let Some((first_path, first_line)) = first.get(id) {
                self.findings.push(PlanFinding {
                    code: "bank_import_duplicate_id",
                    path: row.row.path.clone(),
                    line: Some(row.row.line),
                    message: format!(
                        "duplicate ^bank-{id} (first at {first_path}:{first_line}); leaving both rows untouched"
                    ),
                });
            } else {
                first.insert(id, (row.row.path.clone(), row.row.line));
            }
        }
    }

    fn surface_journal_notes(&mut self) {
        const CAP: usize = 20;
        for line in self.input.journal.malformed.iter().take(CAP) {
            self.findings.push(PlanFinding {
                code: "bank_journal_malformed",
                path: line.path.clone(),
                line: Some(line.line),
                message: "unreadable journal line; skipped".to_owned(),
            });
        }
        if self.input.journal.malformed.len() > CAP {
            self.findings.push(PlanFinding {
                code: "bank_journal_malformed",
                path: ".bob/bank".to_owned(),
                line: None,
                message: format!(
                    "…and {} more unreadable journal lines",
                    self.input.journal.malformed.len() - CAP
                ),
            });
        }
        for conflict in self.input.journal.contradictions.iter().take(CAP) {
            self.findings.push(PlanFinding {
                code: "bank_import_journal_conflict",
                path: ".bob/bank".to_owned(),
                line: None,
                message: format!(
                    "journal identity {} disagrees on {}; keeping recorded money",
                    conflict.key.chars().take(8).collect::<String>(),
                    conflict.field,
                ),
            });
        }
    }

    fn process(
        &mut self,
        input: &'a PlanInput<'a>,
        file_index: usize,
        statement: &ResolvedStatement,
        txn: &ParsedTransaction,
    ) -> Result<(), PlanRefusal> {
        let alias = statement.alias.clone();
        let key = transaction_key(&alias, &txn.bank_id);
        let file =
            input
                .file_labels
                .get(file_index)
                .cloned()
                .unwrap_or_else(|| {
                    format!("file-{}", statement.statement.source_order)
                });
        let conflicting_field = |previous: (NaiveDate, i64)| {
            if previous.0 != txn.posted {
                Some("DTPOSTED")
            } else if previous.1 != txn.cents {
                Some("TRNAMT")
            } else {
                None
            }
        };
        if let Some(previous) = self.batch_money.get(&key).copied()
            && let Some(field) = conflicting_field(previous)
        {
            return Err(PlanRefusal::IdentityConflict {
                file,
                record: txn.record_order + 1,
                field,
            });
        }
        if let Some(previous) = input.journal.transactions.get(&key)
            && let Some(field) =
                conflicting_field((previous.posted, previous.cents))
        {
            return Err(PlanRefusal::IdentityConflict {
                file,
                record: txn.record_order + 1,
                field,
            });
        }
        self.batch_money
            .entry(key.clone())
            .or_insert((txn.posted, txn.cents));

        // Overlapping files emit one event per identity; a repeat of a
        // batch key counts as seen.
        if !self.batch_keys.insert(key.clone()) {
            self.count_seen(file_index);
            return Ok(());
        }
        if input.journal.transactions.contains_key(&key) {
            self.count_seen(file_index);
            return Ok(());
        }
        if let Some(recovered) = self.recovery_row(input, &key) {
            self.count_seen(file_index);
            self.ids.occupy(&recovered.id, &key);
            self.txn_events.push(NewTxn {
                account: alias.clone(),
                key,
                id: recovered.id,
                posted: txn.posted,
                cents: txn.cents,
                desc: txn.desc.clone(),
                outcome: TxnOutcome::Written,
                note: Some(recovered.path),
            });
            return Ok(());
        }
        if self.has_duplicate_unjournaled_recovery_id(input, &key) {
            return Err(PlanRefusal::DuplicateUnjournaledId {
                file,
                record: txn.record_order + 1,
            });
        }
        if let Some(start) = statement.account_start
            && txn.posted < start
        {
            self.count_before_start(file_index);
            let id = self.reserve_id(&key)?;
            self.txn_events.push(NewTxn {
                account: alias.clone(),
                key,
                id,
                posted: txn.posted,
                cents: txn.cents,
                desc: txn.desc.clone(),
                outcome: TxnOutcome::BeforeStart,
                note: None,
            });
            return Ok(());
        }
        match self.match_candidate(&alias, txn.posted, txn.cents) {
            MatchDecision::Unique(path, line) => {
                let id = self.reserve_id(&key)?;
                self.count_matched(file_index);
                self.match_edits
                    .entry(path.clone())
                    .or_default()
                    .push((line, id.clone()));
                self.txn_events.push(NewTxn {
                    account: alias.clone(),
                    key,
                    id,
                    posted: txn.posted,
                    cents: txn.cents,
                    desc: txn.desc.clone(),
                    outcome: TxnOutcome::Matched,
                    note: Some(path),
                });
            }
            decision => {
                let id = self.reserve_id(&key)?;
                let ambiguous = matches!(decision, MatchDecision::Ambiguous);
                if ambiguous {
                    self.count_ambiguous(file_index);
                    self.findings.push(PlanFinding {
                        code: "bank_import_ambiguous",
                        path: month_path(month_of(txn.posted)),
                        line: None,
                        message: format!(
                            "{} record {}: equidistant typed rows; wrote a waiting row",
                            input.file_labels.get(file_index).map_or_else(
                                || format!("file-{}", statement.statement.source_order),
                                Clone::clone
                            ),
                            txn.record_order,
                        ),
                    });
                }
                self.count_new(file_index);
                let merchant = merchant_key(&txn.desc);
                let learned = self.learn(input, &merchant);
                let restaurant = self.restaurant_link(input, txn);
                let named = resolve_name(
                    self.config,
                    &txn.desc,
                    txn.trntype.as_deref(),
                    learned.as_ref(),
                    restaurant,
                );
                let mut links = Vec::new();
                if let Some(link) = named.link {
                    links.push(link);
                }
                let path = month_path(month_of(txn.posted));
                self.pending_rows
                    .entry((path.clone(), txn.posted))
                    .or_default()
                    .push(PendingRow {
                        cents: txn.cents,
                        payee: named.payee,
                        category: named.category,
                        account: alias.clone(),
                        links,
                        id: id.clone(),
                    });
                self.txn_events.push(NewTxn {
                    account: alias.clone(),
                    key,
                    id,
                    posted: txn.posted,
                    cents: txn.cents,
                    desc: txn.desc.clone(),
                    outcome: TxnOutcome::Written,
                    note: Some(path),
                });
            }
        }
        Ok(())
    }

    /// A crash-recovery row: the reserved ID is already in the notes but the
    /// key was never journaled. Duplicate IDs and journal-conflicting IDs
    /// never recover.
    fn recovery_row(
        &self,
        input: &'a PlanInput<'a>,
        key: &str,
    ) -> Option<RecoveredRow> {
        let id8: String = key.chars().take(8).collect();
        let id12: String = key.chars().take(12).collect();
        let mut found = None;
        for row in &self.rows {
            let Some(id) = row.row.id.as_deref() else {
                continue;
            };
            if (id == id8 || id == id12) && key.starts_with(id) {
                if found.is_some() {
                    return None;
                }
                found = Some(RecoveredRow {
                    id: id.to_owned(),
                    path: row.row.path.clone(),
                });
            }
        }
        let recovered = found?;
        if let Some(owner) = input.journal.ids.get(&recovered.id)
            && owner != key
        {
            return None;
        }
        Some(recovered)
    }

    fn has_duplicate_unjournaled_recovery_id(
        &self,
        input: &'a PlanInput<'a>,
        key: &str,
    ) -> bool {
        let id8: String = key.chars().take(8).collect();
        let id12: String = key.chars().take(12).collect();
        let mut matches = self.rows.iter().filter(|row| {
            let Some(id) = row.row.id.as_deref() else {
                return false;
            };
            (id == id8 || id == id12)
                && key.starts_with(id)
                && !input.journal.ids.contains_key(id)
        });
        matches.next().is_some() && matches.next().is_some()
    }

    fn reserve_id(&mut self, key: &str) -> Result<String, PlanRefusal> {
        self.ids
            .reserve(key)
            .map_err(|collision| PlanRefusal::IdCollision {
                prefix: collision.prefix,
            })
    }

    /// The uniquely nearest ID-less row on the same non-cash account with
    /// exactly equal signed cents in `[posted - 1, posted + 3]`. Consumes
    /// the winner for the rest of the batch.
    fn match_candidate(
        &mut self,
        alias: &str,
        posted: NaiveDate,
        cents: i64,
    ) -> MatchDecision {
        let mut best: Option<(i64, usize)> = None;
        let mut tied = false;
        for (index, row) in self.rows.iter().enumerate() {
            if row.consumed || row.row.id.is_some() {
                continue;
            }
            if row.row.account != alias {
                continue;
            }
            if !self
                .config
                .account(&row.row.account)
                .is_some_and(|account| account.kind != AccountKind::Cash)
            {
                continue;
            }
            if row.row.cents != cents {
                continue;
            }
            let delta = (row.row.date - posted).num_days();
            if !(-1..=3).contains(&delta) {
                continue;
            }
            let distance = delta.abs();
            match best {
                None => best = Some((distance, index)),
                Some((best_distance, _)) if distance < best_distance => {
                    best = Some((distance, index));
                    tied = false;
                }
                Some((best_distance, _)) if distance == best_distance => {
                    tied = true;
                }
                _ => {}
            }
        }
        match best {
            None => MatchDecision::None,
            Some(_) if tied => MatchDecision::Ambiguous,
            Some((_, index)) => {
                let row = &mut self.rows[index];
                row.consumed = true;
                MatchDecision::Unique(row.row.path.clone(), row.row.line)
            }
        }
    }

    /// Current-text learning: the most recent acknowledged ID-bearing row
    /// with the same journal merchant. Waiting rows never teach, rows whose
    /// ID has no journal evidence cannot be attributed, and equal-date ties
    /// break by note then line.
    fn learn(
        &self,
        input: &'a PlanInput<'a>,
        merchant: &str,
    ) -> Option<LearnedName> {
        let mut best: Option<(&BankRow, NaiveDate, &str, usize)> = None;
        for row in &self.rows {
            let row = &row.row;
            if !row.acknowledged {
                continue;
            }
            let Some(id) = row.id.as_deref() else {
                continue;
            };
            let Some(key) = input.journal.ids.get(id) else {
                continue;
            };
            let Some(event) = input.journal.transactions.get(key) else {
                continue;
            };
            // Old append-only events stored the uncleaned key. Recompute
            // from their preserved masked source description so existing
            // journal bytes remain comparable without migration.
            if merchant_key(&event.desc) != merchant {
                continue;
            }
            if self
                .rows
                .iter()
                .filter(|other| other.row.id.as_deref() == Some(id))
                .count()
                != 1
            {
                continue;
            }
            let candidate = (row, row.date, row.path.as_str(), row.line);
            best = Some(match best {
                None => candidate,
                Some(current)
                    if (candidate.1, candidate.2, candidate.3)
                        > (current.1, current.2, current.3) =>
                {
                    candidate
                }
                Some(current) => current,
            });
        }
        best.map(|(row, _, _, _)| LearnedName {
            payee: row.payee.clone(),
            category: row.category.clone(),
            links: row.links.clone(),
        })
    }

    /// Exact case-insensitive restaurant title match against the cleaned,
    /// title-cased merchant core. No fuzzy guessing; ambiguous titles omit
    /// the link.
    fn restaurant_link(
        &self,
        input: &'a PlanInput<'a>,
        txn: &ParsedTransaction,
    ) -> Option<String> {
        let cleaned = super::naming::title_case(
            &super::naming::clean_description(&txn.desc),
        );
        let folded = cleaned.to_lowercase();
        let mut winner: Option<&RestaurantEntry> = None;
        for entry in &input.restaurants {
            if entry.title.to_lowercase() == folded {
                if winner.is_some() {
                    return None;
                }
                winner = Some(entry);
            }
        }
        winner.map(RestaurantEntry::link)
    }

    fn count_new(&mut self, file_index: usize) {
        self.counts.new += 1;
        self.file_counts_mut(file_index).new += 1;
    }

    fn count_matched(&mut self, file_index: usize) {
        self.counts.matched += 1;
        self.file_counts_mut(file_index).matched += 1;
    }

    fn count_seen(&mut self, file_index: usize) {
        self.counts.seen += 1;
        self.file_counts_mut(file_index).seen += 1;
    }

    fn count_ambiguous(&mut self, file_index: usize) {
        self.counts.ambiguous += 1;
        self.file_counts_mut(file_index).ambiguous += 1;
    }

    fn count_before_start(&mut self, file_index: usize) {
        self.counts.before_start += 1;
        self.file_counts_mut(file_index).before_start += 1;
    }

    fn file_counts_mut(&mut self, file_index: usize) -> &mut ImportCounts {
        if self.file_counts.len() <= file_index {
            self.file_counts
                .resize_with(file_index + 1, ImportCounts::default);
        }
        &mut self.file_counts[file_index]
    }

    fn finish_file(
        &mut self,
        input: &'a PlanInput<'a>,
        file_index: usize,
        statement: &ResolvedStatement,
    ) -> PlannedFile {
        let parsed = &statement.statement;
        let counts = if file_index < self.file_counts.len() {
            std::mem::take(&mut self.file_counts[file_index])
        } else {
            ImportCounts::default()
        };
        let mut report = PlannedFile {
            file: input
                .file_labels
                .get(file_index)
                .cloned()
                .unwrap_or_else(|| format!("file-{}", parsed.source_order)),
            account: statement.alias.clone(),
            source: parsed.source,
            start: parsed.start,
            end: parsed.end,
            counts,
            out_cents: 0,
            in_cents: 0,
            balance_cents: parsed.ledger_balance_cents,
            balance_asof: parsed.ledger_balance_asof,
            new_evidence: self.is_new_evidence(input, statement),
        };
        for txn in &parsed.transactions {
            if txn.cents < 0 {
                report.out_cents += -txn.cents;
            } else {
                report.in_cents += txn.cents;
            }
        }
        report
    }

    /// New evidence is a new file digest, window, or balance for this
    /// account; a fully recorded identical replay has none.
    fn is_new_evidence(
        &self,
        input: &'a PlanInput<'a>,
        statement: &ResolvedStatement,
    ) -> bool {
        let parsed = &statement.statement;
        !input.journal.imports.iter().any(|event| {
            event.account == statement.alias
                && event.file_sha256 == parsed.file_digest
                && event.start == parsed.start.unwrap_or(event.start)
                && event.end == parsed.end.unwrap_or(event.end)
                && event.balance_cents == parsed.ledger_balance_cents
                && event.balance_asof == parsed.ledger_balance_asof
        })
    }

    /// Apply match-ID appends first (descending original lines), then day
    /// inserts, then row appends with recomputed coordinates, so earlier
    /// inserts can never shift a typed-row match.
    fn emit_months(&mut self, input: &'a PlanInput<'a>) {
        for (path, mut edits) in std::mem::take(&mut self.match_edits) {
            let Some(text) = self.month_texts.get(&path).cloned() else {
                continue;
            };
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
            let mut current = text;
            for (line, id) in edits {
                if let Some(edit) = plan_append_id(&current, line, &id) {
                    current = edit.apply(&current);
                }
            }
            self.month_texts.insert(path, current);
        }
        let mut days_needed: BTreeMap<String, Vec<NaiveDate>> = BTreeMap::new();
        for (path, date) in self.pending_rows.keys() {
            days_needed.entry(path.clone()).or_default().push(*date);
        }
        for (path, mut dates) in days_needed {
            dates.sort();
            dates.dedup();
            if !self.month_texts.contains_key(&path) {
                self.month_texts.insert(
                    path.clone(),
                    canonical_template(
                        month_of(dates[0]),
                        &input.now_stamp,
                        dates[0],
                    ),
                );
            }
            for date in dates {
                let month = month_of(date);
                let current =
                    self.month_texts.get(&path).cloned().unwrap_or_default();
                if let Some(edit) = plan_insert_day(&current, month, date) {
                    let next = edit.apply(&current);
                    self.month_texts.insert(path.clone(), next);
                }
            }
        }
        let mut keys: Vec<(String, NaiveDate)> =
            self.pending_rows.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let rows = std::mem::take(
                self.pending_rows.get_mut(&key).expect("pending rows"),
            );
            for pending in rows {
                let path = key.0.clone();
                let current =
                    self.month_texts.get(&path).cloned().unwrap_or_default();
                let line = render_row(
                    true,
                    pending.cents,
                    &pending.payee,
                    pending.category.as_deref(),
                    Some(&pending.account),
                    &pending.links,
                    Some(&pending.id),
                );
                if let Some(edit) =
                    plan_append_row(&current, month_of(key.1), key.1, &line)
                {
                    let next = edit.apply(&current);
                    self.month_texts.insert(path, next);
                }
            }
        }
    }

    fn finish(
        self,
        input: &'a PlanInput<'a>,
        files: Vec<PlannedFile>,
    ) -> BatchPlan {
        let mut journal_events = Vec::new();
        for (statement, report) in input.statements.iter().zip(files.iter()) {
            if !report.new_evidence {
                continue;
            }
            let parsed = &statement.statement;
            journal_events.push(serialize_import(
                parse_stamp(&input.now_stamp),
                &input.host,
                &NewImport {
                    account: statement.alias.clone(),
                    source: parsed.source,
                    file_sha256: parsed.file_digest.clone(),
                    start: parsed
                        .start
                        .unwrap_or_else(|| coverage_fallback(parsed, true)),
                    end: parsed
                        .end
                        .unwrap_or_else(|| coverage_fallback(parsed, false)),
                    balance_cents: parsed.ledger_balance_cents,
                    balance_asof: parsed.ledger_balance_asof,
                    counts: report.counts.clone(),
                },
            ));
        }
        // `txn_events` already holds each identity once, in processing
        // order, so overlapping files never double-emit.
        for event in &self.txn_events {
            journal_events.push(serialize_txn(
                parse_stamp(&input.now_stamp),
                &input.host,
                event,
            ));
        }
        let mut month_edits: Vec<MonthEdit> = self
            .month_texts
            .iter()
            .filter_map(|(path, after)| {
                let before = input
                    .scan
                    .months
                    .iter()
                    .find(|month| &month.path == path)
                    .map(|month| month.contents.clone());
                let changed = before.as_deref() != Some(after.as_str());
                changed.then(|| MonthEdit {
                    path: path.clone(),
                    before,
                    after: after.clone(),
                })
            })
            .collect();
        month_edits.sort_by(|left, right| left.path.cmp(&right.path));
        let rows = self.affected_rows(input);
        let changed = !month_edits.is_empty() || !journal_events.is_empty();
        let mut read_deps = Vec::new();
        read_deps.push(input.hub.clone());
        for month in &input.scan.months {
            read_deps.push(ReadDep {
                path: month.path.clone(),
                preimage: Some(month.contents.as_bytes().to_vec()),
            });
        }
        for (path, bytes) in &input.journal.preimages {
            read_deps.push(ReadDep {
                path: path.clone(),
                preimage: Some(bytes.clone()),
            });
        }
        read_deps.extend(input.restaurant_deps.iter().cloned());
        BatchPlan {
            files,
            rows,
            month_edits,
            journal_events,
            read_deps,
            findings: self.findings,
            counts: self.counts,
            changed,
        }
    }

    /// New and matched rows with their after-image coordinates, ordered by
    /// note and line. Matched rows report their journal outcome.
    fn affected_rows(&self, input: &'a PlanInput<'a>) -> Vec<BankRow> {
        let mut wanted: HashMap<String, TxnOutcome> = HashMap::new();
        for event in &self.txn_events {
            if event.outcome != TxnOutcome::BeforeStart {
                wanted.insert(event.id.clone(), event.outcome);
            }
        }
        if wanted.is_empty() {
            return Vec::new();
        }
        let mut rows = Vec::new();
        let mut paths: Vec<&String> = self.month_texts.keys().collect();
        paths.sort();
        for path in paths {
            let after = &self.month_texts[path];
            let month =
                month_of(self.pending_month(path).unwrap_or_else(|| {
                    NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoch")
                }));
            let note = parse_month_note(path, after, month);
            for day in &note.days {
                for located in &day.rows {
                    let LocatedRowKind::Parsed(parsed) = &located.kind else {
                        continue;
                    };
                    let Some(id) = parsed.id.as_deref() else {
                        continue;
                    };
                    let Some(outcome) = wanted.get(id) else {
                        continue;
                    };
                    let account = parsed.account.clone().unwrap_or_else(|| {
                        input
                            .config
                            .and_then(|config| config.default_account())
                            .map(|account| account.alias.clone())
                            .unwrap_or_else(|| "checking".to_owned())
                    });
                    let origin = match outcome {
                        TxnOutcome::Matched => "matched",
                        _ => "imported",
                    };
                    rows.push(BankRow {
                        path: path.clone(),
                        line: located.line,
                        date: day.date,
                        heading: super::grammar::render_day_heading(day.date),
                        account,
                        cents: parsed.cents,
                        payee: parsed.payee.clone(),
                        category: parsed.category.clone(),
                        links: parsed.links.clone(),
                        note: parsed.note.clone(),
                        flagged: parsed.flagged,
                        acknowledged: parsed.acknowledged,
                        id: parsed.id.clone(),
                        origin,
                    });
                }
            }
        }
        rows.sort_by(|left, right| {
            left.path.cmp(&right.path).then(left.line.cmp(&right.line))
        });
        rows
    }

    /// Best-effort month for parsing an after-image: the pending month when
    /// known, else the month already scanned for that path.
    fn pending_month(&self, path: &str) -> Option<NaiveDate> {
        self.pending_rows
            .keys()
            .find(|(pending_path, _)| pending_path == path)
            .map(|(_, date)| *date)
            .or_else(|| {
                self.input
                    .scan
                    .months
                    .iter()
                    .find(|month| month.path == path)
                    .map(|month| {
                        NaiveDate::from_ymd_opt(
                            month.month.year,
                            month.month.month,
                            1,
                        )
                        .expect("scanned month")
                    })
            })
    }
}

struct RecoveredRow {
    id: String,
    path: String,
}

enum MatchDecision {
    None,
    Ambiguous,
    Unique(String, usize),
}

fn coverage_fallback(
    parsed: &super::source::ParsedStatement,
    start: bool,
) -> NaiveDate {
    let dates = parsed.transactions.iter().map(|txn| txn.posted);
    let fallback = NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoch");
    if start {
        dates.min().unwrap_or(fallback)
    } else {
        dates.max().unwrap_or(fallback)
    }
}

/// Collect planner rows from every scanned month. Rows whose account is
/// unknown to the configuration keep the default-account fallback so
/// matching sees the same attribution the vault shows.
fn collect_planner_rows(
    input: &PlanInput<'_>,
    config: &BankConfig,
) -> Vec<RowState> {
    let mut rows = Vec::new();
    for month in &input.scan.months {
        let note = parse_month_note(&month.path, &month.contents, month.month);
        for day in &note.days {
            for located in &day.rows {
                let LocatedRowKind::Parsed(parsed) = &located.kind else {
                    continue;
                };
                let account = parsed.account.clone().unwrap_or_else(|| {
                    config
                        .default_account()
                        .map(|account| account.alias.clone())
                        .unwrap_or_else(|| "checking".to_owned())
                });
                rows.push(RowState {
                    row: BankRow {
                        path: month.path.clone(),
                        line: located.line,
                        date: day.date,
                        heading: String::new(),
                        account,
                        cents: parsed.cents,
                        payee: parsed.payee.clone(),
                        category: parsed.category.clone(),
                        links: parsed.links.clone(),
                        note: parsed.note.clone(),
                        flagged: parsed.flagged,
                        acknowledged: parsed.acknowledged,
                        id: parsed.id.clone(),
                        origin: "typed",
                    },
                    consumed: false,
                });
            }
        }
    }
    rows.sort_by(|left, right| {
        left.row
            .date
            .cmp(&right.row.date)
            .then(left.row.path.cmp(&right.row.path))
            .then(left.row.line.cmp(&right.row.line))
    });
    rows
}

fn parse_stamp(stamp: &str) -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339(stamp).unwrap_or_else(|_| {
        chrono::DateTime::parse_from_rfc3339("1970-01-01T00:00:00+00:00")
            .expect("epoch stamp")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::bank::config::{parse_bank_config, ConfigParse};
    use crate::native::bank::grammar::{render_day_heading, YearMonth};
    use crate::native::bank::identity::{mask_digits, sha256_hex};
    use crate::native::bank::journal::TxnEvent;
    use crate::native::bank::scan::{HubFile, MonthFile};
    use crate::native::bank::source::{
        parse_export, resolve_account, ParsedStatement,
    };

    const STAMP: &str = "2026-11-12T12:00:00+00:00";

    fn nov(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn october(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn hub_with(rules_yaml: &str, start: &str) -> BankConfig {
        let text = format!(
            "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: {start}\n    savings:\n      label: WF Savings\n      kind: savings\n      start: {start}\n  categories:\n    groceries: Groceries\n    dining: Dining\n    fees: Fees\n    transfer: Transfer\n    cash: Cash out\n{rules_yaml}---\n# Bank\n"
        );
        match parse_bank_config(&text) {
            ConfigParse::Ok(config) => config,
            other => panic!("hub should parse: {other:?}"),
        }
    }

    fn hub() -> BankConfig {
        hub_with("", "2026-11-01")
    }

    fn month_path_for(year: i32, month: u32) -> (String, YearMonth) {
        let ym = YearMonth { year, month };
        (format!("bank/{}.md", ym.as_str()), ym)
    }

    fn month_file(
        path: &str,
        month: YearMonth,
        title: &str,
        days: &[(NaiveDate, Vec<&str>)],
    ) -> MonthFile {
        let mut contents = format!(
            "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: {}\ncreated: 2026-11-01T00:00:00+00:00\n---\n\n{title}\n\n```bob-bank\nview: month\n```\n\n",
            month.as_str()
        );
        for (date, rows) in days {
            contents.push_str(&render_day_heading(*date));
            contents.push('\n');
            contents.push('\n');
            for row in rows {
                contents.push_str(row);
                contents.push('\n');
            }
            contents.push('\n');
        }
        MonthFile {
            path: path.to_owned(),
            month,
            contents,
        }
    }

    fn november(days: &[(u32, Vec<&str>)]) -> MonthFile {
        let (path, month) = month_path_for(2026, 11);
        let dated: Vec<(NaiveDate, Vec<&str>)> = days
            .iter()
            .map(|(day, rows)| (nov(*day), rows.clone()))
            .collect();
        month_file(&path, month, "# November 2026", &dated)
    }

    fn scan_with(months: Vec<MonthFile>) -> BankScan {
        BankScan {
            hub: Some(HubFile {
                path: "bank.md".to_owned(),
                contents: "hub".to_owned(),
            }),
            bank_dir_exists: true,
            months,
            month_entries: Vec::new(),
            io_errors: Vec::new(),
        }
    }

    fn txn(
        posted: NaiveDate,
        cents: i64,
        bank_id: &str,
        desc: &str,
        trntype: Option<&str>,
    ) -> ParsedTransaction {
        ParsedTransaction {
            record_order: 0,
            posted,
            cents,
            bank_id: bank_id.to_owned(),
            trntype: trntype.map(str::to_owned),
            desc: desc.to_owned(),
        }
    }

    fn stmt_as(
        alias: &str,
        order: usize,
        txns: Vec<ParsedTransaction>,
        digest: &str,
        account_start: Option<NaiveDate>,
    ) -> ResolvedStatement {
        let start = txns.iter().map(|txn| txn.posted).min();
        let end = txns.iter().map(|txn| txn.posted).max();
        ResolvedStatement {
            statement: ParsedStatement {
                source: SourceKind::Ofx,
                file_digest: digest.to_owned(),
                source_order: order,
                inferred_kind: Some(AccountKind::Checking),
                start,
                end,
                ledger_balance_cents: None,
                ledger_balance_asof: None,
                transactions: txns,
            },
            alias: alias.to_owned(),
            account_start,
        }
    }

    fn stmt(
        alias: &str,
        order: usize,
        txns: Vec<ParsedTransaction>,
        digest: &str,
    ) -> ResolvedStatement {
        stmt_as(alias, order, txns, digest, Some(nov(1)))
    }

    fn digest(label: &str) -> String {
        sha256_hex(label.as_bytes())
    }

    fn journal_event(
        account: &str,
        bank_id: &str,
        outcome: TxnOutcome,
        note: Option<&str>,
        posted: NaiveDate,
        cents: i64,
        desc: &str,
    ) -> TxnEvent {
        let key = transaction_key(account, bank_id);
        TxnEvent {
            ts: parse_stamp(STAMP),
            host: "athena".to_owned(),
            path: ".bob/bank/athena.jsonl".to_owned(),
            line: 1,
            account: account.to_owned(),
            key: key.clone(),
            id: key.chars().take(8).collect(),
            posted,
            cents,
            desc: mask_digits(desc),
            merchant: merchant_key(desc),
            outcome,
            note: note.map(str::to_owned),
        }
    }

    fn snapshot_with(events: Vec<TxnEvent>) -> JournalSnapshot {
        let mut snapshot = JournalSnapshot::default();
        for event in events {
            snapshot.ids.insert(event.id.clone(), event.key.clone());
            snapshot.transactions.insert(event.key.clone(), event);
        }
        snapshot
    }

    #[allow(clippy::too_many_arguments)]
    fn run_plan(
        config: &BankConfig,
        scan: &BankScan,
        journal: &JournalSnapshot,
        statements: Vec<ResolvedStatement>,
        labels: Vec<&str>,
        restaurants: Vec<RestaurantEntry>,
    ) -> Result<BatchPlan, PlanRefusal> {
        plan_batch(PlanInput {
            config: Some(config),
            hub: ReadDep {
                path: "bank.md".to_owned(),
                preimage: Some(b"hub".to_vec()),
            },
            scan,
            journal,
            statements,
            file_labels: labels.into_iter().map(str::to_owned).collect(),
            restaurants,
            restaurant_deps: Vec::new(),
            now_stamp: STAMP.to_owned(),
            host: "athena".to_owned(),
        })
    }

    fn two_txn_statements() -> Vec<ResolvedStatement> {
        vec![stmt(
            "checking",
            0,
            vec![
                txn(
                    nov(12),
                    -1371,
                    "fitid-1",
                    "SMASHBURGER CARD 1234",
                    Some("DEBIT"),
                ),
                txn(nov(11), -6120, "fitid-2", "SHOPRITE", Some("POS")),
            ],
            &digest("file-a"),
        )]
    }

    #[test]
    fn new_rows_render_deterministically() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let first = run_plan(
            &config,
            &scan,
            &journal,
            two_txn_statements(),
            vec!["a.qfx"],
            Vec::new(),
        )
        .expect("plan");
        let second = run_plan(
            &config,
            &scan,
            &journal,
            two_txn_statements(),
            vec!["a.qfx"],
            Vec::new(),
        )
        .expect("plan again");
        assert_eq!(first, second);
        assert!(first.changed);
        assert_eq!(first.counts.new, 2);
        assert_eq!(first.counts.ambiguous, 0);
        assert_eq!(first.files.len(), 1);
        let file = &first.files[0];
        assert_eq!(file.file, "a.qfx");
        assert_eq!(file.account, "checking");
        assert_eq!(file.source, SourceKind::Ofx);
        assert_eq!(file.start, Some(nov(11)));
        assert_eq!(file.end, Some(nov(12)));
        assert!(file.new_evidence);
        assert_eq!(file.out_cents, 1371 + 6120);
        assert_eq!(file.in_cents, 0);
        assert_eq!(file.balance_cents, None);
        assert_eq!(file.balance_asof, None);

        // Two new rows, one import event plus one txn event per identity.
        assert_eq!(first.month_edits.len(), 1);
        let edit = &first.month_edits[0];
        assert_eq!(edit.path, "bank/2026-11.md");
        assert!(edit.before.is_some());
        let id1: String = transaction_key("checking", "fitid-1")
            .chars()
            .take(8)
            .collect();
        let id2: String = transaction_key("checking", "fitid-2")
            .chars()
            .take(8)
            .collect();
        assert!(
            edit.after.contains(&format!(
                "- [ ] $13.71 Smashburger Card 1234 [acct:: checking] ^bank-{id1}"
            )),
            "after: {}",
            edit.after
        );
        assert!(
            edit.after.contains(&format!(
                "- [ ] $61.20 Shoprite [acct:: checking] ^bank-{id2}"
            )),
            "after: {}",
            edit.after
        );
        assert_eq!(first.journal_events.len(), 3);
        assert!(first.journal_events[0].contains("\"kind\":\"bank.import\""));
        assert!(first.journal_events[1].contains("\"kind\":\"bank.txn\""));
        // Affected rows carry after-image coordinates and import origins.
        assert_eq!(first.rows.len(), 2);
        assert_eq!(first.rows[0].path, "bank/2026-11.md");
        assert!(first.rows[0].line < first.rows[1].line);
        assert!(first.rows.iter().all(|row| row.origin == "imported"));
        let row_json =
            super::super::model::row_json(&first.rows[0], "checking");
        assert_eq!(row_json["origin"], "imported");
        assert!(row_json["id"].as_str().is_some_and(|id| !id.is_empty()));
        // Private identifiers never reach serialized output.
        for event in &first.journal_events {
            assert!(!event.contains("fitid"), "{event}");
        }
        // Read dependencies cover hub, months, and journal preimages.
        let deps: Vec<&str> = first
            .read_deps
            .iter()
            .map(|dep| dep.path.as_str())
            .collect();
        assert!(deps.contains(&"bank.md"));
        assert!(deps.contains(&"bank/2026-11.md"));
        // No journal files were read in this empty case beyond the hub.
        assert!(first.findings.is_empty());
    }

    #[test]
    fn identical_replay_is_a_noop() {
        use std::fs;
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let empty = JournalSnapshot::default();
        let first = run_plan(
            &config,
            &scan,
            &empty,
            two_txn_statements(),
            vec!["a.qfx"],
            Vec::new(),
        )
        .expect("plan");
        // Record the plan's events through the real journal loader so the
        // replay snapshot carries preimages and masking exactly as written.
        let root = std::env::temp_dir().join(format!(
            "bob-bank-replay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join(".bob/bank");
        fs::create_dir_all(&dir).expect("journal dir");
        let body = first.journal_events.join("");
        assert!(body.contains("••••"));
        assert!(!body.contains("fitid"));
        fs::write(dir.join("athena.jsonl"), &body).expect("journal");
        let recorded =
            crate::native::bank::journal::load(&root).expect("load journal");
        assert_eq!(recorded.transactions.len(), 2);
        assert_eq!(recorded.imports.len(), 1);
        assert_eq!(recorded.preimages.len(), 1);

        let replay = run_plan(
            &config,
            &scan,
            &recorded,
            two_txn_statements(),
            vec!["a.qfx"],
            Vec::new(),
        )
        .expect("replay");
        assert!(!replay.changed);
        assert!(replay.month_edits.is_empty());
        assert!(replay.journal_events.is_empty());
        assert_eq!(replay.counts.seen, 2);
        assert_eq!(replay.counts.new, 0);
        assert!(!replay.files[0].new_evidence);
        let deps: Vec<&str> = replay
            .read_deps
            .iter()
            .map(|dep| dep.path.as_str())
            .collect();
        assert!(deps.contains(&".bob/bank/athena.jsonl"));
    }

    #[test]
    fn typed_row_match_appends_only_the_id() {
        let config = hub();
        let body = "- [ ] $13.71 Smashburger Card 1234\n";
        let scan = scan_with(vec![november(&[(12, vec![body])])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -1371,
                    "fitid-9",
                    "SMASHBURGER CARD 1234",
                    Some("DEBIT"),
                )],
                &digest("match-file"),
            )],
            vec!["m.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.matched, 1);
        assert_eq!(plan.counts.new, 0);
        assert_eq!(plan.month_edits.len(), 1);
        let edit = &plan.month_edits[0];
        let id: String = transaction_key("checking", "fitid-9")
            .chars()
            .take(8)
            .collect();
        assert_eq!(
            edit.after,
            edit.before.as_ref().expect("before").replace(
                "- [ ] $13.71 Smashburger Card 1234\n",
                &format!("- [ ] $13.71 Smashburger Card 1234 ^bank-{id}\n"),
            )
        );
        // Exactly one byte span changed: the trailing ID.
        assert_eq!(
            edit.after.len(),
            edit.before.as_ref().expect("before").len() + id.len() + 7
        );
        // The matched journal outcome reports through row origin.
        let matched_event = journal_event(
            "checking",
            "fitid-9",
            TxnOutcome::Matched,
            Some("bank/2026-11.md"),
            nov(12),
            -1371,
            "SMASHBURGER CARD 1234",
        );
        let recorded = snapshot_with(vec![matched_event]);
        let (after_path, after_month) = month_path_for(2026, 11);
        let after_scan = scan_with(vec![MonthFile {
            path: after_path,
            month: after_month,
            contents: edit.after.clone(),
        }]);
        let rows = super::super::model::collect_rows(
            &after_scan,
            Some(&config),
            Some(&recorded),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, "matched");
        // The plan's own affected rows agree.
        assert_eq!(plan.rows.len(), 1);
        assert_eq!(plan.rows[0].origin, "matched");
        assert_eq!(plan.rows[0].line, rows[0].line);
    }

    #[test]
    fn id_without_journal_evidence_keeps_imported_origin() {
        let config = hub();
        let scan = scan_with(vec![november(&[(
            12,
            vec!["- [x] $13.71 Old row [acct:: checking] ^bank-deadbeef\n"],
        )])]);
        let rows = super::super::model::collect_rows(
            &scan,
            Some(&config),
            Some(&JournalSnapshot::default()),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, "imported");
    }

    #[test]
    fn window_edges_match_and_amounts_stay_separate() {
        let config = hub();
        // Candidates at posted-1 (match), posted+3 (later), posted-2 and
        // posted+4 (out of window), plus a $24.00 lookalike.
        let scan = scan_with(vec![november(&[
            (10, vec!["- [ ] $20.00 Too early\n"]),
            (11, vec!["- [ ] $20.00 Near past\n"]),
            (15, vec!["- [ ] $20.00 Near future\n"]),
            (16, vec!["- [ ] $20.00 Too late\n"]),
            (
                12,
                vec!["- [ ] $24.00 Lookalike\n", "- [ ] $20.00 Same day\n"],
            ),
        ])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(nov(12), -2000, "w-1", "COFFEE SHOP", Some("POS"))],
                &digest("window"),
            )],
            vec!["w.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.matched, 1);
        let edit = &plan.month_edits[0];
        // Same-day distance 0 is the unique minimum, so the 11-12 row
        // matches; the -2/+4 rows sit outside the window and $24.00 is a
        // different amount.
        assert!(
            edit.after.contains("- [ ] $20.00 Same day ^bank-"),
            "after: {}",
            edit.after
        );
        assert!(!edit.after.contains("Too early ^bank-"));
        assert!(!edit.after.contains("Near past ^bank-"));
        assert!(!edit.after.contains("Near future ^bank-"));
        assert!(!edit.after.contains("Too late ^bank-"));
        assert!(!edit.after.contains("Lookalike ^bank-"));
    }

    #[test]
    fn equidistant_tie_writes_a_waiting_row() {
        let config = hub();
        let scan = scan_with(vec![november(&[
            (11, vec!["- [ ] $20.00 Past tie\n"]),
            (13, vec!["- [ ] $20.00 Future tie\n"]),
        ])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(nov(12), -2000, "tie-1", "COFFEE SHOP", Some("POS"))],
                &digest("tie"),
            )],
            vec!["t.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.matched, 0);
        assert_eq!(plan.counts.new, 1);
        assert_eq!(plan.counts.ambiguous, 1);
        assert!(plan.findings.iter().any(|finding| {
            finding.code == "bank_import_ambiguous"
                && finding.message.contains("t.qfx record 0")
        }));
        let edit = &plan.month_edits[0];
        assert!(!edit.after.contains("Past tie ^bank-"));
        assert!(!edit.after.contains("Future tie ^bank-"));
        assert!(
            edit.after
                .contains("- [ ] $20.00 Coffee Shop [acct:: checking] ^bank-"),
            "after: {}",
            edit.after
        );
    }

    #[test]
    fn before_start_records_without_a_row() {
        let config = hub_with("", "2026-11-12");
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt_as(
                "checking",
                0,
                vec![txn(nov(1), -500, "early-1", "OLD PURCHASE", Some("POS"))],
                &digest("early"),
                Some(nov(12)),
            )],
            vec!["e.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.before_start, 1);
        assert_eq!(plan.counts.new, 0);
        assert!(plan.month_edits.is_empty());
        assert_eq!(plan.journal_events.len(), 2);
        assert!(plan.journal_events[1].contains("\"outcome\":\"before_start\""));
        assert!(plan.journal_events[1].contains("\"note\":null"));
    }

    #[test]
    fn deleted_rows_stay_deleted() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        // The journal already knows this identity; the vault row is gone.
        let recorded = snapshot_with(vec![journal_event(
            "checking",
            "gone-1",
            TxnOutcome::Written,
            Some("bank/2026-11.md"),
            nov(12),
            -1371,
            "SMASHBURGER CARD 1234",
        )]);
        let plan = run_plan(
            &config,
            &scan,
            &recorded,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -1371,
                    "gone-1",
                    "SMASHBURGER CARD 1234",
                    Some("DEBIT"),
                )],
                &digest("new-file-same-txn"),
            )],
            vec!["gone.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.seen, 1);
        assert_eq!(plan.counts.new, 0);
        assert!(plan.month_edits.is_empty());
        // New file evidence still journals an import, but no txn event:
        // the identity is fully recorded.
        assert_eq!(plan.journal_events.len(), 1);
        assert!(plan.journal_events[0].contains("\"kind\":\"bank.import\""));
        assert!(plan.journal_events[0].contains("\"seen\":1"));
    }

    #[test]
    fn crash_recovery_backfills_the_missing_event() {
        let config = hub();
        let key = transaction_key("checking", "rec-1");
        let id: String = key.chars().take(8).collect();
        let row = format!(
            "- [x] $13.71 Smashburger Card 1234 [acct:: checking] ^bank-{id}\n"
        );
        let scan = scan_with(vec![november(&[(12, vec![row.as_str()])])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -1371,
                    "rec-1",
                    "SMASHBURGER CARD 1234",
                    Some("DEBIT"),
                )],
                &digest("crash-file"),
            )],
            vec!["crash.qfx"],
            Vec::new(),
        )
        .expect("plan");
        // The row is already there: seen, no second row, no month edits.
        assert_eq!(plan.counts.seen, 1);
        assert_eq!(plan.counts.new, 0);
        assert!(plan.month_edits.is_empty());
        // The missing journal evidence is backfilled with the row's ID.
        assert_eq!(plan.journal_events.len(), 2);
        let event = &plan.journal_events[1];
        assert!(event.contains("\"kind\":\"bank.txn\""));
        assert!(event.contains("\"outcome\":\"written\""));
        assert!(event.contains(&format!("\"id\":\"{id}\"")));
        assert!(event.contains("\"note\":\"bank/2026-11.md\""));
    }

    #[test]
    fn duplicate_unjournaled_ids_refuse_without_rewriting_existing_rows() {
        let config = hub();
        // The bank ID of "dup-key" already sits on two different rows, so
        // neither recovery nor matching may choose between them.
        let key = transaction_key("checking", "dup-key");
        let id8: String = key.chars().take(8).collect();
        let row_a =
            format!("- [ ] $20.00 Dup one [acct:: checking] ^bank-{id8}\n");
        let row_b =
            format!("- [ ] $20.00 Dup two [acct:: checking] ^bank-{id8}\n");
        let scan = scan_with(vec![november(&[(
            12,
            vec![row_a.as_str(), row_b.as_str()],
        )])]);
        let journal = JournalSnapshot::default();
        let original = scan.months[0].contents.clone();
        let refused = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(nov(12), -2000, "dup-key", "DUP SHOP", Some("POS"))],
                &digest("dup-file"),
            )],
            vec!["dup.qfx"],
            Vec::new(),
        );
        assert_eq!(
            refused,
            Err(PlanRefusal::DuplicateUnjournaledId {
                file: "dup.qfx".to_owned(),
                record: 1,
            })
        );
        assert_eq!(scan.months[0].contents, original);
        assert_eq!(id8.len(), 8);
    }

    #[test]
    fn conflicting_identity_money_is_refused_across_batch_files() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let refused = run_plan(
            &config,
            &scan,
            &journal,
            vec![
                stmt(
                    "checking",
                    0,
                    vec![txn(
                        nov(12),
                        -1000,
                        "repeat",
                        "SQ *TEST CAFE",
                        Some("POS"),
                    )],
                    &digest("money-a"),
                ),
                stmt(
                    "checking",
                    1,
                    vec![txn(
                        nov(12),
                        -2000,
                        "repeat",
                        "TST*TEST CAFE",
                        Some("POS"),
                    )],
                    &digest("money-b"),
                ),
            ],
            vec!["first.qfx", "second.qfx"],
            Vec::new(),
        );
        assert_eq!(
            refused,
            Err(PlanRefusal::IdentityConflict {
                file: "second.qfx".to_owned(),
                record: 1,
                field: "TRNAMT",
            })
        );
        assert!(!PlanRefusal::IdentityConflict {
            file: "second.qfx".to_owned(),
            record: 1,
            field: "TRNAMT",
        }
        .message()
        .contains("repeat"));
    }

    #[test]
    fn journaled_identity_money_conflict_is_refused_but_description_change_is_not(
    ) {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = snapshot_with(vec![journal_event(
            "checking",
            "old-money",
            TxnOutcome::Written,
            Some("bank/2026-11.md"),
            nov(11),
            -1000,
            "SQ *TEST CAFE",
        )]);
        let conflict = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(11),
                    -2000,
                    "old-money",
                    "TST*TEST CAFE",
                    Some("POS"),
                )],
                &digest("journal-money-conflict"),
            )],
            vec!["conflict.qfx"],
            Vec::new(),
        );
        assert_eq!(
            conflict,
            Err(PlanRefusal::IdentityConflict {
                file: "conflict.qfx".to_owned(),
                record: 1,
                field: "TRNAMT",
            })
        );

        let changed_description = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(11),
                    -1000,
                    "old-money",
                    "changed source text",
                    Some("POS"),
                )],
                &digest("journal-description-change"),
            )],
            vec!["changed.qfx"],
            Vec::new(),
        )
        .expect("source description is not immutable money evidence");
        assert_eq!(changed_description.counts.seen, 1);
        assert_eq!(changed_description.counts.new, 0);
    }

    #[test]
    fn overlapping_files_emit_one_event_per_identity() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let shared =
            txn(nov(12), -1371, "shared-1", "SMASHBURGER", Some("DEBIT"));
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![
                stmt(
                    "checking",
                    0,
                    vec![
                        shared.clone(),
                        txn(nov(11), -100, "only-a", "A SHOP", Some("POS")),
                    ],
                    &digest("file-a"),
                ),
                stmt(
                    "checking",
                    1,
                    vec![
                        shared,
                        txn(nov(10), -200, "only-b", "B SHOP", Some("POS")),
                    ],
                    &digest("file-b"),
                ),
            ],
            vec!["a.qfx", "b.qfx"],
            Vec::new(),
        )
        .expect("plan");
        // Shared identity: new under the first file, seen under the second.
        assert_eq!(plan.files[0].counts.new, 2);
        assert_eq!(plan.files[1].counts.new, 1);
        assert_eq!(plan.files[1].counts.seen, 1);
        assert_eq!(plan.counts.new, 3);
        assert_eq!(plan.counts.seen, 1);
        // Two import events plus three txn events, never four.
        assert_eq!(plan.journal_events.len(), 5);
        let txn_events = plan.journal_events[2..].to_vec();
        assert_eq!(txn_events.len(), 3);
        for event in &plan.journal_events {
            assert!(!event.contains("shared-1"), "{event}");
        }
        // Three new waiting rows, one per identity.
        let edit = &plan.month_edits[0];
        assert_eq!(edit.after.matches("^bank-").count(), 3);
    }

    #[test]
    fn same_day_identical_purchases_stay_distinct() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![
                    txn(nov(12), -500, "cup-1", "COFFEE SHOP", Some("POS")),
                    txn(nov(12), -500, "cup-2", "COFFEE SHOP", Some("POS")),
                ],
                &digest("cups"),
            )],
            vec!["cups.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.new, 2);
        assert_eq!(plan.counts.matched, 0);
        let edit = &plan.month_edits[0];
        let id1: String = transaction_key("checking", "cup-1")
            .chars()
            .take(8)
            .collect();
        let id2: String = transaction_key("checking", "cup-2")
            .chars()
            .take(8)
            .collect();
        assert_ne!(id1, id2);
        assert!(edit.after.contains(&format!("^bank-{id1}")));
        assert!(edit.after.contains(&format!("^bank-{id2}")));
    }

    #[test]
    fn one_use_matching_across_files() {
        let config = hub();
        let scan = scan_with(vec![november(&[(
            12,
            vec!["- [ ] $5.00 Coffee Shop\n"],
        )])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![
                stmt(
                    "checking",
                    0,
                    vec![txn(
                        nov(12),
                        -500,
                        "use-1",
                        "COFFEE SHOP",
                        Some("POS"),
                    )],
                    &digest("use-a"),
                ),
                stmt(
                    "checking",
                    1,
                    vec![txn(
                        nov(12),
                        -500,
                        "use-2",
                        "COFFEE SHOP",
                        Some("POS"),
                    )],
                    &digest("use-b"),
                ),
            ],
            vec!["use-a.qfx", "use-b.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.files[0].counts.matched, 1);
        assert_eq!(plan.files[1].counts.matched, 0);
        assert_eq!(plan.files[1].counts.new, 1);
        assert_eq!(plan.counts.matched, 1);
        assert_eq!(plan.counts.new, 1);
        // One ID append plus one fresh waiting row.
        let edit = &plan.month_edits[0];
        assert_eq!(edit.after.matches("^bank-").count(), 2);
    }

    #[test]
    fn id_collision_extends_then_refuses() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let key = transaction_key("checking", "ext-1");
        let eight: String = key.chars().take(8).collect();
        let twelve: String = key.chars().take(12).collect();
        // Only the 8-prefix is taken: the planner extends to twelve.
        let mut blocked = JournalSnapshot::default();
        blocked.ids.insert(eight.clone(), "other-key".to_owned());
        let plan = run_plan(
            &config,
            &scan,
            &blocked,
            vec![stmt(
                "checking",
                0,
                vec![txn(nov(12), -500, "ext-1", "CORNER STORE", Some("POS"))],
                &digest("ext"),
            )],
            vec!["ext.qfx"],
            Vec::new(),
        )
        .expect("plan extends");
        let edit = &plan.month_edits[0];
        assert!(
            edit.after.contains(&format!("^bank-{twelve}")),
            "after: {}",
            edit.after
        );
        // Both prefixes taken: the whole batch refuses.
        blocked.ids.insert(twelve.clone(), "third-key".to_owned());
        let refused = run_plan(
            &config,
            &scan,
            &blocked,
            vec![stmt(
                "checking",
                0,
                vec![txn(nov(12), -500, "ext-1", "CORNER STORE", Some("POS"))],
                &digest("ext"),
            )],
            vec!["ext.qfx"],
            Vec::new(),
        );
        assert_eq!(
            refused,
            Err(PlanRefusal::IdCollision {
                prefix: twelve.clone()
            })
        );
        assert!(PlanRefusal::IdCollision { prefix: twelve }
            .message()
            .contains("refusing import"));
    }

    fn learned_fixture(
        acknowledged: bool,
    ) -> (BankConfig, BankScan, JournalSnapshot, String) {
        let config = hub();
        let old_key = transaction_key("checking", "old-1");
        let old_id: String = old_key.chars().take(8).collect();
        let box_mark = if acknowledged { "x" } else { " " };
        let row = format!(
            "- [{box_mark}] $42.00 Whole Foods Market [cat:: groceries] [acct:: checking] ^bank-{old_id}\n"
        );
        let scan = scan_with(vec![november(&[(12, vec![row.as_str()])])]);
        // The journal taught this merchant under the old wording; the
        // vault row now carries the edited name.
        let journal = snapshot_with(vec![journal_event(
            "checking",
            "old-1",
            TxnOutcome::Written,
            Some("bank/2026-11.md"),
            nov(12),
            -4200,
            "WHOLEFDS #4821",
        )]);
        (config, scan, journal, old_id)
    }

    #[test]
    fn learns_from_edited_acknowledged_text() {
        let (config, scan, journal, _) = learned_fixture(true);
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -4200,
                    "new-1",
                    "WHOLEFDS #4821",
                    Some("POS"),
                )],
                &digest("learn"),
            )],
            vec!["learn.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.new, 1);
        let edit = &plan.month_edits[0];
        assert!(
            edit.after.contains(
                "- [ ] $42.00 Whole Foods Market [cat:: groceries] [acct:: checking] ^bank-"
            ),
            "after: {}",
            edit.after
        );
    }

    #[test]
    fn waiting_rows_never_teach() {
        let (config, scan, journal, _) = learned_fixture(false);
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -4200,
                    "new-1",
                    "WHOLEFDS #4821",
                    Some("POS"),
                )],
                &digest("learn-waiting"),
            )],
            vec!["learn.qfx"],
            Vec::new(),
        )
        .expect("plan");
        // Tier 4 falls back to the cleaned description, not the waiting
        // row's edited name: the edited line appears exactly once (the
        // fixture row itself), never as a second taught row.
        let edit = &plan.month_edits[0];
        assert!(
            edit.after
                .contains("- [ ] $42.00 Wholefds [acct:: checking] ^bank-"),
            "after: {}",
            edit.after
        );
        assert_eq!(
            edit.after
                .matches("Whole Foods Market [cat:: groceries] [acct:: checking] ^bank-")
                .count(),
            1
        );
    }

    #[test]
    fn learned_merchant_uses_cleaned_masked_description_from_old_journal() {
        let config = hub();
        let bank_id = "old-cafe";
        let key = transaction_key("checking", bank_id);
        let id: String = key.chars().take(8).collect();
        let row = format!(
            "- [x] $10.00 My preferred cafe [acct:: checking] ^bank-{id}\n"
        );
        let scan = scan_with(vec![november(&[(11, vec![row.as_str()])])]);
        let mut old_event = journal_event(
            "checking",
            bank_id,
            TxnOutcome::Written,
            Some("bank/2026-11.md"),
            nov(11),
            -1000,
            "SQ *TEST CAFE",
        );
        // This is the value written by the previous raw-text key algorithm.
        old_event.merchant = "sq *test cafe".to_owned();
        let journal = snapshot_with(vec![old_event]);

        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -1100,
                    "new-cafe",
                    "TST*TEST CAFE",
                    Some("POS"),
                )],
                &digest("cleaned-cafe"),
            )],
            vec!["new.qfx"],
            Vec::new(),
        )
        .expect("cleaned merchant match");
        assert!(plan.month_edits[0].after.contains(
            "- [ ] $11.00 My preferred cafe [acct:: checking] ^bank-"
        ));
    }

    #[test]
    fn most_recent_teaches_with_deterministic_tiebreak() {
        let config = hub();
        let id_for = |bank_id: &str| {
            transaction_key("checking", bank_id)
                .chars()
                .take(8)
                .collect::<String>()
        };
        let older_id = id_for("old-3");
        let first_id = id_for("old-1");
        let second_id = id_for("old-2");
        let older = format!(
            "- [x] $42.00 Older Name [cat:: groceries] [acct:: checking] ^bank-{older_id}\n"
        );
        let first = format!(
            "- [x] $42.00 First Tie [cat:: dining] [acct:: checking] ^bank-{first_id}\n"
        );
        let second = format!(
            "- [x] $42.00 Second Tie [cat:: dining] [acct:: checking] ^bank-{second_id}\n"
        );
        let scan = scan_with(vec![november(&[
            (10, vec![older.as_str()]),
            (12, vec![first.as_str(), second.as_str()]),
        ])]);
        // All three IDs share one journal merchant: the most recent day
        // wins, and the same-day tie breaks toward the later line.
        let journal = snapshot_with(vec![
            journal_event(
                "checking",
                "old-3",
                TxnOutcome::Written,
                Some("bank/2026-11.md"),
                nov(10),
                -4200,
                "WHOLEFDS #4821",
            ),
            journal_event(
                "checking",
                "old-1",
                TxnOutcome::Written,
                Some("bank/2026-11.md"),
                nov(12),
                -4200,
                "WHOLEFDS #4821",
            ),
            journal_event(
                "checking",
                "old-2",
                TxnOutcome::Written,
                Some("bank/2026-11.md"),
                nov(12),
                -4200,
                "WHOLEFDS #4821",
            ),
        ]);
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -4200,
                    "new-9",
                    "WHOLEFDS #4821",
                    Some("POS"),
                )],
                &digest("learn-tie"),
            )],
            vec!["learn.qfx"],
            Vec::new(),
        )
        .expect("plan");
        let edit = &plan.month_edits[0];
        assert!(
            edit.after.contains(
                "- [ ] $42.00 Second Tie [cat:: dining] [acct:: checking] ^bank-"
            ),
            "after: {}",
            edit.after
        );
    }

    fn restaurant(title: &str, stem: &str) -> RestaurantEntry {
        RestaurantEntry {
            title: title.to_owned(),
            path: format!("eat/{stem}"),
            emoji: Some("🍔".to_owned()),
        }
    }

    #[test]
    fn restaurant_link_cutover() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -999,
                    "eat-1",
                    "SMASHBURGER ARLINGTON VA",
                    Some("POS"),
                )],
                &digest("eat"),
            )],
            vec!["eat.qfx"],
            vec![restaurant("Smashburger", "smashburger")],
        )
        .expect("plan");
        let edit = &plan.month_edits[0];
        assert!(
            edit.after.contains(
                "- [ ] $9.99 Smashburger [acct:: checking] [[eat/smashburger|🍔]] ^bank-"
            ),
            "after: {}",
            edit.after
        );
    }

    #[test]
    fn ambiguous_restaurant_titles_omit_the_link() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -999,
                    "eat-2",
                    "SMASHBURGER ARLINGTON VA",
                    Some("POS"),
                )],
                &digest("eat-amb"),
            )],
            vec!["eat.qfx"],
            vec![
                restaurant("Smashburger", "smashburger"),
                restaurant("SMASHBURGER", "smashburger_2"),
            ],
        )
        .expect("plan");
        let edit = &plan.month_edits[0];
        assert!(
            edit.after
                .contains("- [ ] $9.99 Smashburger [acct:: checking] ^bank-"),
            "after: {}",
            edit.after
        );
        assert!(!edit.after.contains("[[eat/"));
    }

    #[test]
    fn rule_link_beats_restaurant_lookup() {
        let config = hub_with(
            "  rules:\n    - match: SMASH\n      link: '[[recipes/smash]]'\n",
            "2026-11-01",
        );
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![txn(
                    nov(12),
                    -999,
                    "eat-3",
                    "SMASHBURGER ARLINGTON VA",
                    Some("POS"),
                )],
                &digest("eat-rule"),
            )],
            vec!["eat.qfx"],
            vec![restaurant("Smashburger", "smashburger")],
        )
        .expect("plan");
        let edit = &plan.month_edits[0];
        assert!(
            edit.after.contains(
                "- [ ] $9.99 Smashburger [acct:: checking] [[recipes/smash]] ^bank-"
            ),
            "after: {}",
            edit.after
        );
        assert!(!edit.after.contains("[[eat/"));
    }

    #[test]
    fn late_months_open_new_files_with_frozen_stamps() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt_as(
                "checking",
                0,
                vec![txn(
                    october(28),
                    -2500,
                    "late-1",
                    "CORNER STORE",
                    Some("POS"),
                )],
                &digest("late"),
                Some(october(1)),
            )],
            vec!["late.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.month_edits.len(), 1);
        let edit = &plan.month_edits[0];
        assert_eq!(edit.path, "bank/2026-10.md");
        assert_eq!(edit.before, None);
        assert!(edit.after.contains("month: 2026-10"));
        assert!(edit.after.contains(STAMP));
        assert!(
            edit.after
                .contains("- [ ] $25.00 Corner Store [acct:: checking] ^bank-"),
            "after: {}",
            edit.after
        );
    }

    #[test]
    fn bom_crlf_children_and_prose_survive() {
        let config = hub();
        let row = "- [ ] $13.71 Smashburger Card 1234";
        let mut contents = String::from("---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: 2026-11\ncreated: 2026-11-01T00:00:00+00:00\n---\n\n# November 2026\n\n```bob-bank\nview: month\n```\n\n## Thu Nov 12\n\n");
        contents.push_str(row);
        contents.push_str(
            "\n  - receipt saved\n\nFree prose stays.\n\nAn isolated note.\n\
- [ ] call the bank about the fee\n- [ ] $\n\n```text\n## Fri Nov 13\n```\n\
\n## Sat Nov 14\n\n- [ ] $8.00 Later day\n",
        );
        let contents = contents.replace('\n', "\r\n");
        let contents = format!("\u{feff}{contents}");
        let (path, month) = month_path_for(2026, 11);
        let scan = scan_with(vec![MonthFile {
            path: path.clone(),
            month,
            contents: contents.clone(),
        }]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt(
                "checking",
                0,
                vec![
                    txn(
                        nov(12),
                        -1371,
                        "bom-1",
                        "SMASHBURGER CARD 1234",
                        Some("DEBIT"),
                    ),
                    txn(nov(13), -200, "bom-2", "NEWSSTAND", Some("POS")),
                ],
                &digest("bom"),
            )],
            vec!["bom.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.matched, 1);
        assert_eq!(plan.counts.new, 1);
        assert_eq!(plan.month_edits.len(), 1);
        let edit = &plan.month_edits[0];
        assert!(edit.after.starts_with('\u{feff}'));
        // The match appends only the ID; children and prose are untouched.
        assert!(edit.after.contains("  - receipt saved\r\n"));
        assert!(edit.after.contains("Free prose stays.\r\n"));
        let id: String = transaction_key("checking", "bom-1")
            .chars()
            .take(8)
            .collect();
        assert!(edit.after.contains(&format!("{row} ^bank-{id}\r\n")));
        // The new row lands under its own day with CRLF endings.
        assert!(edit.after.contains("## Fri Nov 13\r\n"));
        let inserted = edit.after.find("## Fri Nov 13\r\n").unwrap();
        for content in [
            "Free prose stays.",
            "An isolated note.",
            "- [ ] call the bank about the fee",
            "- [ ] $",
            "```text",
        ] {
            assert!(edit.after.find(content).unwrap() < inserted, "{content}");
        }
        assert!(inserted < edit.after.find("## Sat Nov 14\r\n").unwrap());
        assert!(edit
            .after
            .contains("- [ ] $2.00 Newsstand [acct:: checking] ^bank-"));
    }

    #[test]
    fn refuses_without_config_or_with_unreadable_months() {
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let refused = plan_batch(PlanInput {
            config: None,
            hub: ReadDep {
                path: "bank.md".to_owned(),
                preimage: None,
            },
            scan: &scan,
            journal: &journal,
            statements: two_txn_statements(),
            file_labels: vec!["a.qfx".to_owned()],
            restaurants: Vec::new(),
            restaurant_deps: Vec::new(),
            now_stamp: STAMP.to_owned(),
            host: "athena".to_owned(),
        });
        assert_eq!(refused, Err(PlanRefusal::MissingConfig));
        assert!(PlanRefusal::MissingConfig
            .message()
            .contains("bank configuration"));
        let mut broken = scan_with(vec![november(&[])]);
        broken.io_errors.push("bank/2026-11.md: denied".to_owned());
        let config = hub();
        let refused = run_plan(
            &config,
            &broken,
            &journal,
            two_txn_statements(),
            vec!["a.qfx"],
            Vec::new(),
        );
        assert_eq!(
            refused,
            Err(PlanRefusal::IncompleteRead {
                paths: vec!["bank/2026-11.md: denied".to_owned()]
            })
        );
    }

    #[test]
    fn empty_statement_journals_only_its_evidence() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![stmt("checking", 0, Vec::new(), &digest("empty"))],
            vec!["empty.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert!(plan.changed);
        assert!(plan.month_edits.is_empty());
        assert_eq!(plan.journal_events.len(), 1);
        assert!(plan.journal_events[0].contains("\"new\":0"));
        assert!(plan.journal_events[0].contains("\"seen\":0"));
        assert_eq!(plan.counts, ImportCounts::default());
    }

    #[test]
    fn parses_real_ofx_end_to_end() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let body = ofx_fixture("CHECKING", "-13.71", "qfx-1");
        let parsed =
            parse_export("real.qfx", body.as_bytes(), 0).expect("parse OFX");
        let resolved = resolve_account(parsed, &config, None, "real.qfx")
            .expect("resolve");
        assert_eq!(resolved.alias, "checking");
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            vec![resolved],
            vec!["real.qfx"],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(plan.counts.new, 1);
        assert!(plan.files[0].new_evidence);
        assert_eq!(plan.files[0].balance_cents, Some(100_000));
        let edit = &plan.month_edits[0];
        assert!(
            edit.after.contains(
                "- [ ] $13.71 Smashburger Card 1234 [acct:: checking] ^bank-"
            ),
            "after: {}",
            edit.after
        );
    }

    #[test]
    fn journal_conflicts_and_malformed_lines_surface() {
        use crate::native::bank::journal::{
            JournalContradiction, MalformedLine,
        };
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let mut journal = JournalSnapshot::default();
        journal.malformed.push(MalformedLine {
            path: ".bob/bank/athena.jsonl".to_owned(),
            line: 7,
        });
        journal.contradictions.push(JournalContradiction {
            key: transaction_key("checking", "old-1"),
            field: "cents",
        });
        let plan = run_plan(
            &config,
            &scan,
            &journal,
            two_txn_statements(),
            vec!["a.qfx"],
            Vec::new(),
        )
        .expect("plan notes never refuse");
        assert_eq!(plan.counts.new, 2);
        assert!(plan.findings.iter().any(|finding| {
            finding.code == "bank_journal_malformed"
                && finding.path == ".bob/bank/athena.jsonl"
                && finding.line == Some(7)
        }));
        assert!(plan.findings.iter().any(|finding| {
            finding.code == "bank_import_journal_conflict"
                && finding.message.contains("cents")
                && !finding.message.contains("old-1")
        }));
    }

    #[test]
    fn restaurant_inputs_are_guarded_read_dependencies() {
        let config = hub();
        let scan = scan_with(vec![november(&[])]);
        let journal = JournalSnapshot::default();
        let dep = ReadDep {
            path: "eat/smashburger.md".to_owned(),
            preimage: Some(b"note".to_vec()),
        };
        let plan = plan_batch(PlanInput {
            config: Some(&config),
            hub: ReadDep {
                path: "bank.md".to_owned(),
                preimage: Some(b"hub".to_vec()),
            },
            scan: &scan,
            journal: &journal,
            statements: two_txn_statements(),
            file_labels: vec!["a.qfx".to_owned(), "b.qfx".to_owned()],
            restaurants: vec![restaurant("Smashburger", "smashburger")],
            restaurant_deps: vec![dep],
            now_stamp: STAMP.to_owned(),
            host: "apollo".to_owned(),
        })
        .expect("plan");
        let deps: Vec<&str> =
            plan.read_deps.iter().map(|dep| dep.path.as_str()).collect();
        assert!(deps.contains(&"eat/smashburger.md"));
        // Extra labels fall back to source order without failing.
        assert_eq!(plan.files[0].file, "a.qfx");
    }

    fn ofx_fixture(acct: &str, amount: &str, fitid: &str) -> String {
        format!(
            "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<BANKMSGSRSV1>\n<STMTTRNRS>\n<STMTRS>\n<CURDEF>USD\n<BANKACCTFROM>\n<BANKID>121000248\n<ACCTID>1234567890\n<ACCTTYPE>{acct}\n</BANKACCTFROM>\n<BANKTRANLIST>\n<DTSTART>20261101\n<DTEND>20261112\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20261112040000.000[-8:PST]\n<TRNAMT>{amount}\n<FITID>{fitid}\n<NAME>SMASHBURGER\n<MEMO>CARD 1234\n</STMTTRN>\n</BANKTRANLIST>\n<LEDGERBAL>\n<BALAMT>1000.00\n<DTASOF>20261112\n</LEDGERBAL>\n</STMTRS>\n</STMTTRNRS>\n</BANKMSGSRSV1>\n</OFX>\n"
        )
    }
}
