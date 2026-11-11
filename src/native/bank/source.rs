//! OFX/QFX and Wells Fargo CSV parsers for bank import.

use chrono::NaiveDate;
use encoding_rs::Encoding;

use super::{
    config::{Account, AccountKind, BankConfig},
    identity::sha256_hex,
};

const USD: &str = "USD";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    Ofx,
    Csv,
}

impl SourceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ofx => "ofx",
            Self::Csv => "csv",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "ofx" => Some(Self::Ofx),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceError {
    pub(crate) file: String,
    pub(crate) record: Option<usize>,
    pub(crate) field: Option<&'static str>,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

impl SourceError {
    fn new(
        file: impl Into<String>,
        record: Option<usize>,
        field: Option<&'static str>,
        message: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        Self {
            file: file.into(),
            record,
            field,
            message: message.into(),
            hint,
        }
    }

    pub(crate) fn display(&self) -> String {
        let mut text = self.file.clone();
        if let Some(record) = self.record {
            text.push_str(&format!(" record {record}"));
        }
        if let Some(field) = self.field {
            text.push_str(&format!(" field {field}"));
        }
        text.push_str(": ");
        text.push_str(&self.message);
        text
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedTransaction {
    pub(crate) record_order: usize,
    pub(crate) posted: NaiveDate,
    pub(crate) cents: i64,
    pub(crate) bank_id: String,
    pub(crate) trntype: Option<String>,
    pub(crate) desc: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedStatement {
    pub(crate) source: SourceKind,
    pub(crate) file_digest: String,
    pub(crate) source_order: usize,
    pub(crate) inferred_kind: Option<AccountKind>,
    pub(crate) start: Option<NaiveDate>,
    pub(crate) end: Option<NaiveDate>,
    pub(crate) ledger_balance_cents: Option<i64>,
    pub(crate) ledger_balance_asof: Option<NaiveDate>,
    pub(crate) transactions: Vec<ParsedTransaction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedStatement {
    pub(crate) statement: ParsedStatement,
    pub(crate) alias: String,
    pub(crate) account_start: Option<NaiveDate>,
}

pub(crate) fn parse_export(
    label: &str,
    bytes: &[u8],
    source_order: usize,
) -> Result<ParsedStatement, SourceError> {
    let file_digest = sha256_hex(bytes);
    let (bom, rest) = strip_bom(bytes);
    let text = decode_source(label, rest, bom)?;
    if looks_like_ofx(&text) {
        parse_ofx(label, &text, file_digest, source_order)
    } else {
        parse_csv(label, &text, file_digest, source_order)
    }
}

pub(crate) fn resolve_account(
    statement: ParsedStatement,
    config: &BankConfig,
    requested: Option<&str>,
    file: &str,
) -> Result<ResolvedStatement, SourceError> {
    if let Some(alias) = requested {
        let account = require_importable(config, alias, file)?;
        return Ok(resolved(statement, account));
    }
    let Some(kind) = statement.inferred_kind else {
        return Err(SourceError::new(
            file,
            None,
            Some("account"),
            "CSV has no account type",
            Some("Pass -a ALIAS to choose an importable account.".to_owned()),
        ));
    };
    let matches: Vec<&Account> = config
        .accounts
        .iter()
        .filter(|account| account.kind == kind && account.kind.importable())
        .collect();
    match matches.as_slice() {
        [account] => Ok(resolved(statement, account)),
        [] => Err(SourceError::new(
            file,
            None,
            Some("account"),
            format!("no importable {} account is configured", kind.as_str()),
            Some("Pass -a ALIAS or add a matching account.".to_owned()),
        )),
        many => {
            let aliases = many
                .iter()
                .map(|account| account.alias.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(SourceError::new(
                file,
                None,
                Some("account"),
                format!("ambiguous {} account", kind.as_str()),
                Some(format!("Pass -a ALIAS; candidates: {aliases}.")),
            ))
        }
    }
}

fn resolved(
    statement: ParsedStatement,
    account: &Account,
) -> ResolvedStatement {
    ResolvedStatement {
        statement,
        alias: account.alias.clone(),
        account_start: account.start,
    }
}

fn require_importable<'a>(
    config: &'a BankConfig,
    alias: &str,
    file: &str,
) -> Result<&'a Account, SourceError> {
    let Some(account) = config.account(alias) else {
        return Err(SourceError::new(
            file,
            None,
            Some("account"),
            "unknown account alias",
            Some("Pass -a with a configured importable alias.".to_owned()),
        ));
    };
    if !account.kind.importable() {
        return Err(SourceError::new(
            file,
            None,
            Some("account"),
            "cash accounts are never imported",
            None,
        ));
    }
    Ok(account)
}

fn strip_bom(bytes: &[u8]) -> (Bom, &[u8]) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (Bom::Utf8, &bytes[3..])
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        (Bom::Utf16Le, &bytes[2..])
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (Bom::Utf16Be, &bytes[2..])
    } else {
        (Bom::None, bytes)
    }
}

#[derive(Clone, Copy)]
enum Bom {
    None,
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn decode_source(
    file: &str,
    bytes: &[u8],
    bom: Bom,
) -> Result<String, SourceError> {
    let encoding = match bom {
        Bom::Utf8 => encoding_rs::UTF_8,
        Bom::Utf16Le => encoding_rs::UTF_16LE,
        Bom::Utf16Be => encoding_rs::UTF_16BE,
        Bom::None => sniff_encoding(bytes),
    };
    let (text, _, _) = encoding.decode(bytes);
    let text = text.replace('\r', "");
    if text.trim().is_empty() {
        return Err(SourceError::new(
            file,
            None,
            None,
            "truncated or empty input",
            None,
        ));
    }
    Ok(text)
}

fn sniff_encoding(bytes: &[u8]) -> &'static Encoding {
    let probe = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    if let Some(label) = xml_encoding(&probe)
        && let Some(encoding) = Encoding::for_label(label.as_bytes())
    {
        return encoding;
    }
    if let Some(charset) = header_field(&probe, "CHARSET")
        && let Some(encoding) = charset_encoding(&charset)
    {
        return encoding;
    }
    if let Some(encoding_name) = header_field(&probe, "ENCODING") {
        if encoding_name.eq_ignore_ascii_case("UNICODE") {
            return encoding_rs::UTF_8;
        }
        if let Some(encoding) = Encoding::for_label(encoding_name.as_bytes()) {
            return encoding;
        }
    }
    encoding_rs::UTF_8
}

fn xml_encoding(text: &str) -> Option<String> {
    let decl = text.trim_start();
    if !decl.starts_with("<?xml") {
        return None;
    }
    let end = decl.find("?>")?;
    let attrs = &decl[..end];
    let key = "encoding=";
    let start = attrs.to_ascii_lowercase().find(key)? + key.len();
    let rest = attrs.get(start..)?;
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let inner = rest.get(1..)?;
    let end = inner.find(quote)?;
    Some(inner[..end].to_owned())
}

fn header_field(text: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}:");
    for line in text.lines().take(32) {
        let trimmed = line.trim();
        if trimmed.len() >= prefix.len()
            && trimmed[..prefix.len()].eq_ignore_ascii_case(&prefix)
        {
            let value = trimmed[prefix.len()..].trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn charset_encoding(charset: &str) -> Option<&'static Encoding> {
    if charset.eq_ignore_ascii_case("NONE") {
        return Some(encoding_rs::UTF_8);
    }
    if charset.chars().all(|ch| ch.is_ascii_digit()) {
        let label = format!("windows-{charset}");
        if let Some(encoding) = Encoding::for_label(label.as_bytes()) {
            return Some(encoding);
        }
    }
    Encoding::for_label(charset.as_bytes())
}

fn looks_like_ofx(text: &str) -> bool {
    let probe = text[..text.len().min(4096)].to_ascii_uppercase();
    probe.contains("OFXHEADER")
        || probe.contains("<OFX")
        || probe.contains("<STMTRS")
        || probe.contains("<CCSTMTRS")
}

fn parse_ofx(
    file: &str,
    text: &str,
    file_digest: String,
    source_order: usize,
) -> Result<ParsedStatement, SourceError> {
    let upper = text.to_ascii_uppercase();
    let bank = block_inners(text, &upper, "STMTRS");
    let cards = block_inners(text, &upper, "CCSTMTRS");
    if bank.len() + cards.len() > 1 {
        return Err(SourceError::new(
            file,
            None,
            Some("account"),
            "multiple statement blocks cannot be resolved safely",
            Some("Import each account as a separate export.".to_owned()),
        ));
    }
    let (block, credit) = if let Some(block) = bank.into_iter().next() {
        (block, false)
    } else if let Some(block) = cards.into_iter().next() {
        (block, true)
    } else {
        return Err(SourceError::new(
            file,
            None,
            Some("STMTRS"),
            "truncated or missing statement",
            None,
        ));
    };
    let block_upper = block.to_ascii_uppercase();
    if let Some(currency) = leaf(block, &block_upper, "CURDEF")
        && !currency.is_empty()
        && !currency.eq_ignore_ascii_case(USD)
    {
        return Err(SourceError::new(
            file,
            None,
            Some("CURDEF"),
            "unsupported currency",
            None,
        ));
    }
    let inferred_kind = if credit {
        Some(AccountKind::Credit)
    } else {
        match leaf(block, &block_upper, "ACCTTYPE") {
            Some(value) if !value.is_empty() => {
                Some(account_kind_from_ofx(&value).ok_or_else(|| {
                    SourceError::new(
                        file,
                        None,
                        Some("ACCTTYPE"),
                        "account type cannot be resolved",
                        None,
                    )
                })?)
            }
            _ => None,
        }
    };
    let list = block_inners(block, &block_upper, "BANKTRANLIST")
        .into_iter()
        .next()
        .ok_or_else(|| {
            SourceError::new(
                file,
                None,
                Some("BANKTRANLIST"),
                "truncated or missing transaction list",
                None,
            )
        })?;
    let list_upper = list.to_ascii_uppercase();
    let start = required_ofx_date(file, list, &list_upper, "DTSTART", None)?;
    let end = required_ofx_date(file, list, &list_upper, "DTEND", None)?;
    if start > end {
        return Err(SourceError::new(
            file,
            None,
            Some("DTEND"),
            "coverage end is before start",
            None,
        ));
    }
    let transactions = parse_stmttrns(file, list, &list_upper)?;
    let (ledger_balance_cents, ledger_balance_asof) =
        parse_ledger_balance(file, block, &block_upper)?;
    Ok(ParsedStatement {
        source: SourceKind::Ofx,
        file_digest,
        source_order,
        inferred_kind,
        start: Some(start),
        end: Some(end),
        ledger_balance_cents,
        ledger_balance_asof,
        transactions,
    })
}

fn parse_stmttrns(
    file: &str,
    list: &str,
    list_upper: &str,
) -> Result<Vec<ParsedTransaction>, SourceError> {
    let mut transactions = Vec::new();
    let mut seen_ids = Vec::new();
    for (index, block) in block_inners(list, list_upper, "STMTTRN")
        .into_iter()
        .enumerate()
    {
        let record = index + 1;
        let upper = block.to_ascii_uppercase();
        let bank_id =
            required_leaf(file, Some(record), block, &upper, "FITID")?;
        let posted =
            required_ofx_date(file, block, &upper, "DTPOSTED", Some(record))?;
        let amount =
            required_leaf(file, Some(record), block, &upper, "TRNAMT")?;
        let cents = parse_signed_cents(&amount)
            .map_err(|kind| cents_error(file, Some(record), "TRNAMT", kind))?;
        if seen_ids.iter().any(|id| id == &bank_id) {
            return Err(SourceError::new(
                file,
                Some(record),
                Some("FITID"),
                "conflicting repeat of a bank ID",
                None,
            ));
        }
        seen_ids.push(bank_id.clone());
        let name = leaf(block, &upper, "NAME").unwrap_or_default();
        let memo = leaf(block, &upper, "MEMO").unwrap_or_default();
        let desc = join_desc(&name, &memo);
        let trntype =
            leaf(block, &upper, "TRNTYPE").filter(|value| !value.is_empty());
        transactions.push(ParsedTransaction {
            record_order: index,
            posted,
            cents,
            bank_id,
            trntype,
            desc,
        });
    }
    if list_upper.contains("<STMTTRN")
        && transactions.is_empty()
        && !list_upper.contains("</STMTTRN")
    {
        return Err(SourceError::new(
            file,
            Some(1),
            Some("STMTTRN"),
            "truncated transaction",
            None,
        ));
    }
    Ok(transactions)
}

fn parse_ledger_balance(
    file: &str,
    block: &str,
    upper: &str,
) -> Result<(Option<i64>, Option<NaiveDate>), SourceError> {
    let Some(bal) = block_inners(block, upper, "LEDGERBAL").into_iter().next()
    else {
        return Ok((None, None));
    };
    let bal_upper = bal.to_ascii_uppercase();
    let amount = required_leaf(file, None, bal, &bal_upper, "BALAMT")?;
    let cents = parse_signed_cents(&amount)
        .map_err(|kind| cents_error(file, None, "BALAMT", kind))?;
    let asof = required_ofx_date(file, bal, &bal_upper, "DTASOF", None)?;
    Ok((Some(cents), Some(asof)))
}

fn account_kind_from_ofx(value: &str) -> Option<AccountKind> {
    match value.trim().to_ascii_uppercase().as_str() {
        "CHECKING" => Some(AccountKind::Checking),
        "SAVINGS" | "MONEYMRKT" => Some(AccountKind::Savings),
        "CREDITLINE" | "CREDITCARD" => Some(AccountKind::Credit),
        _ => None,
    }
}

fn parse_csv(
    file: &str,
    text: &str,
    file_digest: String,
    source_order: usize,
) -> Result<ParsedStatement, SourceError> {
    let rows = csv_records(file, text)?;
    let mut transactions = Vec::new();
    let mut occurrences: Vec<(NaiveDate, i64, u32)> = Vec::new();
    let mut start = None;
    let mut end = None;
    for (index, fields) in rows.iter().enumerate() {
        let record = index + 1;
        if fields.len() != 5 {
            return Err(SourceError::new(
                file,
                Some(record),
                None,
                "CSV is not the Wells Fargo five-column profile",
                Some(
                    "Expected Date, Amount, *, (blank), Description."
                        .to_owned(),
                ),
            ));
        }
        if !fields[3].is_empty() {
            return Err(SourceError::new(
                file,
                Some(record),
                None,
                "CSV is not the Wells Fargo five-column profile",
                Some(
                    "Expected Date, Amount, *, (blank), Description."
                        .to_owned(),
                ),
            ));
        }
        let posted = parse_csv_date(&fields[0]).ok_or_else(|| {
            SourceError::new(
                file,
                Some(record),
                Some("Date"),
                "invalid date",
                None,
            )
        })?;
        let cents = parse_signed_cents(&fields[1])
            .map_err(|kind| cents_error(file, Some(record), "Amount", kind))?;
        let occurrence = next_occurrence(&mut occurrences, posted, cents);
        let bank_id =
            format!("csv:{}:{cents}:{occurrence}", posted.format("%Y-%m-%d"));
        start = Some(
            start.map_or(posted, |current: NaiveDate| current.min(posted)),
        );
        end =
            Some(end.map_or(posted, |current: NaiveDate| current.max(posted)));
        transactions.push(ParsedTransaction {
            record_order: index,
            posted,
            cents,
            bank_id,
            trntype: None,
            desc: fields[4].clone(),
        });
    }
    Ok(ParsedStatement {
        source: SourceKind::Csv,
        file_digest,
        source_order,
        inferred_kind: None,
        start,
        end,
        ledger_balance_cents: None,
        ledger_balance_asof: None,
        transactions,
    })
}

fn next_occurrence(
    seen: &mut Vec<(NaiveDate, i64, u32)>,
    posted: NaiveDate,
    cents: i64,
) -> u32 {
    if let Some(entry) = seen
        .iter_mut()
        .find(|(date, amount, _)| *date == posted && *amount == cents)
    {
        entry.2 += 1;
        entry.2
    } else {
        seen.push((posted, cents, 1));
        1
    }
}

fn csv_records(
    file: &str,
    text: &str,
) -> Result<Vec<Vec<String>>, SourceError> {
    let mut rows = Vec::new();
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    let mut record = 1usize;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut current));
            }
            '\n' if !in_quotes => {
                fields.push(std::mem::take(&mut current));
                if fields.iter().any(|field| !field.is_empty()) {
                    rows.push(std::mem::take(&mut fields));
                    record += 1;
                } else {
                    fields.clear();
                }
            }
            ch => current.push(ch),
        }
    }
    if in_quotes {
        return Err(SourceError::new(
            file,
            Some(record),
            None,
            "truncated quoted field",
            None,
        ));
    }
    if !current.is_empty() || !fields.is_empty() {
        fields.push(current);
        if fields.iter().any(|field| !field.is_empty()) {
            rows.push(fields);
        }
    }
    Ok(rows)
}

fn parse_csv_date(raw: &str) -> Option<NaiveDate> {
    let trimmed = raw.trim();
    NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .ok()
        .or_else(|| NaiveDate::parse_from_str(trimmed, "%m/%d/%Y").ok())
}

#[derive(Debug)]
enum CentsKind {
    Invalid,
    Fractional,
    Overflow,
}

fn parse_signed_cents(raw: &str) -> Result<i64, CentsKind> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CentsKind::Invalid);
    }
    let (sign, rest) = if let Some(rest) = trimmed.strip_prefix('+') {
        (1i64, rest.trim())
    } else if let Some(rest) = trimmed.strip_prefix('-') {
        (-1i64, rest.trim())
    } else if let Some(rest) = trimmed.strip_prefix('−') {
        (-1i64, rest.trim())
    } else {
        (1i64, trimmed)
    };
    let rest = rest.strip_prefix('$').unwrap_or(rest).trim();
    if let Some((_, frac)) = rest.split_once('.') {
        let digit_count = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count > 2 {
            return Err(CentsKind::Fractional);
        }
    }
    let (cents, leftover) =
        parse_cents_unsigned(rest).ok_or(CentsKind::Invalid)?;
    if !leftover.trim().is_empty() {
        return Err(CentsKind::Invalid);
    }
    let signed = cents.checked_mul(sign).ok_or(CentsKind::Overflow)?;
    Ok(signed)
}

fn parse_cents_unsigned(input: &str) -> Option<(i64, &str)> {
    let bytes = input.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() {
        return None;
    }
    let mut index = 0;
    while index < bytes.len()
        && (bytes[index].is_ascii_digit() || bytes[index] == b',')
    {
        index += 1;
    }
    let integer = &input[..index];
    if integer.contains(',') {
        let parts: Vec<&str> = integer.split(',').collect();
        if parts
            .first()
            .is_none_or(|part| !(1..=3).contains(&part.len()))
            || parts.iter().skip(1).any(|part| part.len() != 3)
            || parts.iter().any(|part| part.is_empty())
        {
            return None;
        }
    }
    let dollars: i64 = integer.replace(',', "").parse().ok()?;
    let mut rest = &input[index..];
    let mut cents = dollars.checked_mul(100)?;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digit_count =
            after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 {
            return None;
        }
        if digit_count > 2 {
            return None;
        }
        let frac: i64 = after_dot[..digit_count].parse().ok()?;
        let scale = if digit_count == 1 { 10 } else { 1 };
        cents = cents.checked_add(frac * scale)?;
        rest = &after_dot[digit_count..];
    }
    Some((cents, rest))
}

fn cents_error(
    file: &str,
    record: Option<usize>,
    field: &'static str,
    kind: CentsKind,
) -> SourceError {
    let message = match kind {
        CentsKind::Fractional => "fractional cents",
        CentsKind::Overflow => "amount overflow",
        CentsKind::Invalid => "invalid amount",
    };
    SourceError::new(file, record, Some(field), message, None)
}

fn block_inners<'a>(text: &'a str, upper: &str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let open_u = open.to_ascii_uppercase();
    let close_u = close.to_ascii_uppercase();
    let mut inners = Vec::new();
    let mut from = 0;
    while let Some(rel) = upper[from..].find(&open_u) {
        let start = from + rel + open.len();
        let Some(rel_end) =
            upper.get(start..).and_then(|rest| rest.find(&close_u))
        else {
            inners.push(&text[start..]);
            break;
        };
        inners.push(&text[start..start + rel_end]);
        from = start + rel_end + close.len();
    }
    inners
}

fn leaf(text: &str, upper: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let open_u = open.to_ascii_uppercase();
    let pos = upper.find(&open_u)?;
    let start = pos + open.len();
    let end = upper[start..].find('<').unwrap_or(upper.len() - start);
    Some(decode_entities(text[start..start + end].trim()))
}

fn required_leaf(
    file: &str,
    record: Option<usize>,
    text: &str,
    upper: &str,
    tag: &'static str,
) -> Result<String, SourceError> {
    match leaf(text, upper, tag) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(SourceError::new(
            file,
            record,
            Some(tag),
            "missing required field",
            None,
        )),
    }
}

fn required_ofx_date(
    file: &str,
    text: &str,
    upper: &str,
    tag: &'static str,
    record: Option<usize>,
) -> Result<NaiveDate, SourceError> {
    let raw = required_leaf(file, record, text, upper, tag)?;
    parse_ofx_date(&raw).ok_or_else(|| {
        SourceError::new(file, record, Some(tag), "invalid date", None)
    })
}

fn parse_ofx_date(raw: &str) -> Option<NaiveDate> {
    let digits: String =
        raw.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return None;
    }
    let year: i32 = digits[0..4].parse().ok()?;
    let month: u32 = digits[4..6].parse().ok()?;
    let day: u32 = digits[6..8].parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[1..semi];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "apos" => out.push('\''),
            "quot" => out.push('"'),
            "nbsp" => out.push(' '),
            other if let Some(decoded) = decode_numeric_entity(other) => {
                out.push(decoded);
            }
            _ => {
                out.push('&');
                out.push_str(entity);
                out.push(';');
            }
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_numeric_entity(entity: &str) -> Option<char> {
    let value = if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        entity.strip_prefix('#')?.parse().ok()?
    };
    char::from_u32(value)
}

fn join_desc(name: &str, memo: &str) -> String {
    match (name.is_empty(), memo.is_empty()) {
        (true, true) => String::new(),
        (false, true) => name.to_owned(),
        (true, false) => memo.to_owned(),
        (false, false) if name.contains(memo) => name.to_owned(),
        (false, false) => format!("{name} {memo}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn checking() -> BankConfig {
        config(&[("checking", AccountKind::Checking)])
    }

    fn config(accounts: &[(&str, AccountKind)]) -> BankConfig {
        BankConfig {
            accounts: accounts
                .iter()
                .map(|(alias, kind)| Account {
                    alias: (*alias).to_owned(),
                    label: alias.to_string(),
                    kind: *kind,
                    start: Some(date(2026, 11, 1)),
                })
                .collect(),
            categories: Vec::new(),
            rules: Vec::new(),
            stale_after: 10,
        }
    }

    fn sgml() -> String {
        ofx_body(false, "CHECKING", "-13.71", "20261112001371")
    }

    fn xml() -> String {
        ofx_xml("CHECKING", "-13.71", "20261112001371")
    }

    fn ofx_body(crlf: bool, acct: &str, amount: &str, fitid: &str) -> String {
        let nl = if crlf { "\r\n" } else { "\n" };
        format!(
            "OFXHEADER:100{nl}DATA:OFXSGML{nl}VERSION:102{nl}SECURITY:NONE{nl}\
ENCODING:USASCII{nl}CHARSET:1252{nl}COMPRESSION:NONE{nl}OLDFILEUID:NONE{nl}\
NEWFILEUID:NONE{nl}{nl}<OFX>{nl}<BANKMSGSRSV1>{nl}<STMTTRNRS>{nl}<STMTRS>{nl}\
<CURDEF>USD{nl}<BANKACCTFROM>{nl}<BANKID>121000248{nl}<ACCTID>1234567890{nl}\
<ACCTTYPE>{acct}{nl}</BANKACCTFROM>{nl}<BANKTRANLIST>{nl}<DTSTART>20261101{nl}\
<DTEND>20261112{nl}<STMTTRN>{nl}<TRNTYPE>DEBIT{nl}\
<DTPOSTED>20261112040000.000[-8:PST]{nl}<TRNAMT>{amount}{nl}<FITID>{fitid}{nl}\
<NAME>SMASHBURGER{nl}<MEMO>CARD 1234{nl}</STMTTRN>{nl}</BANKTRANLIST>{nl}\
<LEDGERBAL>{nl}<BALAMT>1000.00{nl}<DTASOF>20261112{nl}</LEDGERBAL>{nl}\
<AVAILBAL>{nl}<BALAMT>900.00{nl}<DTASOF>20261112{nl}</AVAILBAL>{nl}\
</STMTRS>{nl}</STMTTRNRS>{nl}</BANKMSGSRSV1>{nl}</OFX>{nl}"
        )
    }

    fn ofx_xml(acct: &str, amount: &str, fitid: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<?OFX OFXHEADER=\"200\" VERSION=\"220\" SECURITY=\"NONE\" OLDFILEUID=\"NONE\" NEWFILEUID=\"NONE\"?>\n\
<OFX><BANKMSGSRSV1><STMTTRNRS><STMTRS>\n\
<CURDEF>USD</CURDEF>\n\
<BANKACCTFROM><BANKID>121000248</BANKID><ACCTID>1234567890</ACCTID>\
<ACCTTYPE>{acct}</ACCTTYPE></BANKACCTFROM>\n\
<BANKTRANLIST><DTSTART>20261101</DTSTART><DTEND>20261112</DTEND>\n\
<STMTTRN><TRNTYPE>DEBIT</TRNTYPE>\
<DTPOSTED>20261112040000.000[-8:PST]</DTPOSTED>\
<TRNAMT>{amount}</TRNAMT><FITID>{fitid}</FITID>\
<NAME>SMASHBURGER</NAME><MEMO>CARD 1234</MEMO></STMTTRN>\n\
</BANKTRANLIST>\n\
<LEDGERBAL><BALAMT>1000.00</BALAMT><DTASOF>20261112</DTASOF></LEDGERBAL>\n\
<AVAILBAL><BALAMT>900.00</BALAMT><DTASOF>20261112</DTASOF></AVAILBAL>\n\
</STMTRS></STMTTRNRS></BANKMSGSRSV1></OFX>\n"
        )
    }

    #[test]
    fn sgml_and_xml_are_equivalent() {
        let left = parse_export("a.qfx", sgml().as_bytes(), 0).unwrap();
        let right = parse_export("b.ofx", xml().as_bytes(), 0).unwrap();
        assert_eq!(left.source, SourceKind::Ofx);
        assert_eq!(left.inferred_kind, Some(AccountKind::Checking));
        assert_eq!(left.start, Some(date(2026, 11, 1)));
        assert_eq!(left.end, Some(date(2026, 11, 12)));
        assert_eq!(left.ledger_balance_cents, Some(100_000));
        assert_eq!(left.ledger_balance_asof, Some(date(2026, 11, 12)));
        assert_eq!(left.transactions.len(), 1);
        assert_eq!(left.transactions[0].posted, date(2026, 11, 12));
        assert_eq!(left.transactions[0].cents, -1371);
        assert_eq!(left.transactions[0].bank_id, right.transactions[0].bank_id);
        assert_eq!(left.transactions[0].desc, right.transactions[0].desc);
        assert_eq!(left.inferred_kind, right.inferred_kind);
        assert_eq!(left.ledger_balance_cents, right.ledger_balance_cents);
        assert!(!left.file_digest.is_empty());
        assert_ne!(left.file_digest, right.file_digest);
    }

    #[test]
    fn header_noise_encoding_and_crlf() {
        let mut noisy = b"garbage CHARSET noise\n".to_vec();
        noisy.extend(ofx_body(true, "CHECKING", "-13.71", "id-1").bytes());
        let parsed = parse_export("noise.qfx", &noisy, 2).unwrap();
        assert_eq!(parsed.source_order, 2);
        assert_eq!(parsed.transactions[0].cents, -1371);

        let mut win = String::from(
            "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nENCODING:USASCII\nCHARSET:1252\n\n",
        );
        win.push_str("<OFX><STMTRS><CURDEF>USD</CURDEF><BANKACCTFROM><ACCTTYPE>CHECKING</ACCTTYPE></BANKACCTFROM>");
        win.push_str(
            "<BANKTRANLIST><DTSTART>20261101</DTSTART><DTEND>20261101</DTEND>",
        );
        win.push_str("<STMTTRN><DTPOSTED>20261101</DTPOSTED><TRNAMT>-1.00</TRNAMT><FITID>e1</FITID><NAME>");
        let mut bytes = win.into_bytes();
        bytes.push(0x43); // C
        bytes.push(0x61); // a
        bytes.push(0x66); // f
        bytes.push(0xE9); // é in windows-1252
        bytes.extend(b"</STMTTRN></BANKTRANLIST></STMTRS></OFX>\n");
        let decoded = parse_export("win.qfx", &bytes, 0).unwrap();
        assert!(
            decoded.transactions[0].desc.contains('é'),
            "{}",
            decoded.transactions[0].desc
        );

        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend(xml().bytes());
        parse_export("bom.ofx", &bom, 0).unwrap();
    }

    #[test]
    fn csv_quoting_and_repeated_occurrences() {
        let csv = "11/12/2026,\"-1,234.56\",*,,\"Foo, Bar\"\n\
11/12/2026,-13.71,*,,Coffee\n\
11/12/2026,-13.71,*,,\"Say \"\"hi\"\"\"\n";
        let parsed = parse_export("a.csv", csv.as_bytes(), 0).unwrap();
        assert_eq!(parsed.source, SourceKind::Csv);
        assert_eq!(parsed.inferred_kind, None);
        assert_eq!(parsed.ledger_balance_cents, None);
        assert_eq!(parsed.transactions.len(), 3);
        assert_eq!(parsed.transactions[0].cents, -123456);
        assert_eq!(parsed.transactions[0].desc, "Foo, Bar");
        assert_eq!(parsed.transactions[1].bank_id, "csv:2026-11-12:-1371:1");
        assert_eq!(parsed.transactions[2].bank_id, "csv:2026-11-12:-1371:2");
        assert_eq!(parsed.transactions[2].desc, "Say \"hi\"");
        let overlap = parse_export("b.csv", csv.as_bytes(), 1).unwrap();
        assert_eq!(
            parsed.transactions[1].bank_id,
            overlap.transactions[1].bank_id
        );
    }

    #[test]
    fn invalid_money_date_and_truncated_input() {
        let bad_money = ofx_body(false, "CHECKING", "-13.711", "id-1");
        let err = parse_export("bad.qfx", bad_money.as_bytes(), 0).unwrap_err();
        assert_eq!(err.field, Some("TRNAMT"));
        assert_eq!(err.message, "fractional cents");
        assert!(err.display().contains("record 1"));
        assert!(!err.display().contains("id-1"));

        let bad_date = sgml().replace("20261112040000.000[-8:PST]", "20261399");
        let err = parse_export("date.qfx", bad_date.as_bytes(), 0).unwrap_err();
        assert_eq!(err.field, Some("DTPOSTED"));

        let truncated =
            "<OFX><STMTRS><BANKTRANLIST><DTSTART>20261101</DTSTART>\
<DTEND>20261112</DTEND><STMTTRN><TRNTYPE>DEBIT";
        let err =
            parse_export("short.qfx", truncated.as_bytes(), 0).unwrap_err();
        assert!(
            err.message.contains("truncated") || err.field == Some("FITID")
        );
        assert!(!err.display().contains("1234567890"));
    }

    #[test]
    fn empty_statement_keeps_coverage_and_balance() {
        let empty = "<OFX><STMTRS><CURDEF>USD</CURDEF>\
<BANKACCTFROM><ACCTTYPE>SAVINGS</ACCTTYPE></BANKACCTFROM>\
<BANKTRANLIST><DTSTART>20261101</DTSTART><DTEND>20261112</DTEND></BANKTRANLIST>\
<LEDGERBAL><BALAMT>12.34</BALAMT><DTASOF>20261112</DTASOF></LEDGERBAL>\
</STMTRS></OFX>";
        let parsed = parse_export("empty.qfx", empty.as_bytes(), 0).unwrap();
        assert!(parsed.transactions.is_empty());
        assert_eq!(parsed.inferred_kind, Some(AccountKind::Savings));
        assert_eq!(parsed.start, Some(date(2026, 11, 1)));
        assert_eq!(parsed.end, Some(date(2026, 11, 12)));
        assert_eq!(parsed.ledger_balance_cents, Some(1234));
        assert!(parse_export("empty.csv", b"", 0)
            .unwrap_err()
            .message
            .contains("empty"));
        let empty_csv = parse_export("empty.csv", b"\n\n", 0);
        assert!(empty_csv.is_err());
    }

    #[test]
    fn inferred_explicit_and_ambiguous_accounts() {
        let statement = parse_export("a.qfx", sgml().as_bytes(), 0).unwrap();
        let resolved =
            resolve_account(statement.clone(), &checking(), None, "a.qfx")
                .unwrap();
        assert_eq!(resolved.alias, "checking");

        let two = config(&[
            ("checking", AccountKind::Checking),
            ("joint", AccountKind::Checking),
            ("card", AccountKind::Credit),
            ("pocket", AccountKind::Cash),
        ]);
        let err = resolve_account(statement.clone(), &two, None, "a.qfx")
            .unwrap_err();
        assert!(err.hint.as_deref().unwrap().contains("-a"));
        assert!(err.hint.as_deref().unwrap().contains("checking"));
        assert!(err.hint.as_deref().unwrap().contains("joint"));

        let picked =
            resolve_account(statement.clone(), &two, Some("joint"), "a.qfx")
                .unwrap();
        assert_eq!(picked.alias, "joint");

        assert!(
            resolve_account(statement, &two, Some("pocket"), "a.qfx").is_err()
        );

        let csv =
            parse_export("a.csv", b"11/12/2026,-1.00,*,,Coffee\n", 0).unwrap();
        assert!(resolve_account(csv.clone(), &two, None, "a.csv").is_err());
        let csv_picked =
            resolve_account(csv, &two, Some("card"), "a.csv").unwrap();
        assert_eq!(csv_picked.alias, "card");

        let credit = ofx_xml("CHECKING", "-5.00", "cc-1").replace(
            "<BANKMSGSRSV1><STMTTRNRS><STMTRS>",
            "<CREDITCARDMSGSRSV1><CCSTMTTRNRS><CCSTMTRS>",
        ).replace(
            "</STMTRS></STMTTRNRS></BANKMSGSRSV1>",
            "</CCSTMTRS></CCSTMTTRNRS></CREDITCARDMSGSRSV1>",
        ).replace(
            "<BANKACCTFROM><BANKID>121000248</BANKID><ACCTID>1234567890</ACCTID><ACCTTYPE>CHECKING</ACCTTYPE></BANKACCTFROM>",
            "<CCACCTFROM><ACCTID>9999</ACCTID></CCACCTFROM>",
        );
        let parsed = parse_export("card.qfx", credit.as_bytes(), 0).unwrap();
        assert_eq!(parsed.inferred_kind, Some(AccountKind::Credit));
    }

    #[test]
    fn duplicate_fitid_and_unsupported_currency_refuse() {
        let dup = xml().replace("</BANKTRANLIST>", "<STMTTRN><DTPOSTED>20261112</DTPOSTED><TRNAMT>-1.00</TRNAMT><FITID>20261112001371</FITID><NAME>X</NAME></STMTTRN></BANKTRANLIST>");
        let err = parse_export("dup.qfx", dup.as_bytes(), 0).unwrap_err();
        assert_eq!(err.field, Some("FITID"));
        assert!(!err.display().contains("20261112001371"));

        let eur = xml().replace("USD", "EUR");
        let err = parse_export("eur.qfx", eur.as_bytes(), 0).unwrap_err();
        assert_eq!(err.field, Some("CURDEF"));
        assert!(!err.display().contains("EUR"));
    }

    #[test]
    fn entities_and_two_statement_blocks_refuse() {
        let amp = xml().replace("SMASHBURGER", "A &amp; B");
        let parsed = parse_export("amp.qfx", amp.as_bytes(), 0).unwrap();
        assert_eq!(parsed.transactions[0].desc, "A & B CARD 1234");

        let two = format!("{}{}", sgml(), sgml());
        let err = parse_export("two.qfx", two.as_bytes(), 0).unwrap_err();
        assert!(err.message.contains("multiple statement"));
    }
}
