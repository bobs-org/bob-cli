//! Merchant cleaning and tiered naming for bank import.
//!
//! Naming resolves `payee` / `category` / `links` for a new bank row from four
//! tiers, highest first:
//!
//! 1. the first configured rule whose `match` text is a case-insensitive
//!    substring of the raw bank description;
//! 2. the most recent acknowledged ID-bearing row with the same journal
//!    merchant (learned from current row text, never a stale journal copy);
//! 3. Wells Fargo built-ins (ATM withdrawal, fees, transfers);
//! 4. the cleaned, title-cased description.
//!
//! Unspecified rule fields fall through to lower tiers. A rule-selected link
//! always wins over the restaurant lookup. Bank-derived payees
//! (tiers 3-with-cleaned-text and 4) pass through [`sanitize_payee`]; learned
//! payees retain their edited text unless the current cleaner removes bank
//! transaction noise.

use super::{config::BankConfig, identity::mask_digits};

/// A learned name taught by one acknowledged ID-bearing row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LearnedName {
    pub(crate) payee: String,
    pub(crate) category: Option<String>,
    pub(crate) links: Vec<String>,
}

/// The winning name for one new bank row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NameResolution {
    pub(crate) payee: String,
    pub(crate) category: Option<String>,
    pub(crate) link: Option<String>,
}

/// Resolve a name through the four tiers.
///
/// `learned` is the current text of the most recent acknowledged ID-bearing
/// row with the same journal merchant, when one exists. `restaurant` is the
/// single `[[eat/<stem>|<emoji>]]` link for an exact title match, used only
/// when no higher tier selected a link. Categories naming an undeclared key
/// are dropped; a positive amount alone never implies income or refund.
pub(crate) fn resolve_name(
    config: &BankConfig,
    raw_desc: &str,
    trntype: Option<&str>,
    learned: Option<&LearnedName>,
    restaurant: Option<String>,
) -> NameResolution {
    let rule = config
        .rules
        .iter()
        .find(|rule| contains_folded(raw_desc, &rule.match_text));
    let builtin = classify_builtin(raw_desc, trntype);
    let cleaned = clean_description(raw_desc);

    let payee = match rule.and_then(|rule| rule.payee.clone()) {
        Some(payee) => payee,
        None => match learned.map(|name| clean_learned_payee(&name.payee)) {
            Some(payee) => payee,
            None => match builtin.clone().and_then(|item| item.payee) {
                Some(payee) => payee,
                None => sanitize_payee(&title_case(&cleaned)),
            },
        },
    };

    let category = rule
        .and_then(|rule| rule.cat.clone())
        .filter(|cat| is_declared(config, cat))
        .or_else(|| {
            learned
                .and_then(|name| name.category.clone())
                .filter(|cat| is_declared(config, cat))
        })
        .or_else(|| {
            builtin
                .and_then(|item| item.category.map(str::to_owned))
                .filter(|cat| is_declared(config, cat))
        });

    let link = rule
        .and_then(|rule| rule.link.clone())
        .or_else(|| learned.and_then(|name| name.links.first().cloned()))
        .or(restaurant);

    NameResolution {
        payee,
        category,
        link,
    }
}

fn is_declared(config: &BankConfig, key: &str) -> bool {
    config.category(key).is_some()
}

fn contains_folded(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Builtin {
    payee: Option<String>,
    category: Option<&'static str>,
}

/// Classify Wells Fargo built-ins from the raw description and OFX type.
///
/// The raw text and type are consulted before cleaning can remove a useful
/// discriminator. ATM withdrawals name a fixed payee with `cash`; fees keep
/// the cleaned payee with `fees`; card payments and online transfers keep
/// the cleaned payee with `transfer`. The caller drops categories the hub
/// does not declare.
fn classify_builtin(raw_desc: &str, trntype: Option<&str>) -> Option<Builtin> {
    let folded = raw_desc.to_ascii_uppercase();
    let tx_type = trntype.unwrap_or_default().to_ascii_uppercase();
    if is_atm_withdrawal(&folded, &tx_type) {
        return Some(Builtin {
            payee: Some("ATM withdrawal".to_owned()),
            category: Some("cash"),
        });
    }
    if is_fee(&folded, &tx_type) {
        return Some(Builtin {
            payee: None,
            category: Some("fees"),
        });
    }
    if is_transfer(&folded, &tx_type) {
        return Some(Builtin {
            payee: None,
            category: Some("transfer"),
        });
    }
    None
}

fn is_atm_withdrawal(folded: &str, tx_type: &str) -> bool {
    if folded.contains("FEE") || folded.contains("CHARGE") {
        return false;
    }
    tx_type == "ATM"
        || folded.contains("ATM WITHDRAWAL")
        || folded.contains("ATM CASH")
        || folded.contains("CASH WITHDRAWAL")
}

fn is_fee(folded: &str, tx_type: &str) -> bool {
    tx_type == "FEE"
        || tx_type == "SRVCHG"
        || folded.contains("OVERDRAFT")
        || folded.contains("ATM FEE")
        || folded.contains("SERVICE FEE")
        || folded.contains("MONTHLY FEE")
        || folded.contains("INSUFFICIENT")
}

fn is_transfer(folded: &str, tx_type: &str) -> bool {
    tx_type == "XFER"
        || tx_type == "PAYMENT"
        || folded.contains("ONLINE TRANSFER")
        || folded.contains("TRANSFER TO")
        || folded.contains("TRANSFER FROM")
        || folded.contains("BILL PAY")
        || folded.contains("ONLINE PAYMENT")
}

/// Clean a raw bank description down to a merchant core.
///
/// Stages strip authorization/QFX date prefixes, processor prefixes and
/// order codes, store numbers, holder/phone/card tails, and recognizable
/// city + US-state spans, then redact long digit runs. Every removal stage
/// keeps its input when stripping would leave nothing, so cleaning never
/// erases a merchant; the patterns stay conservative and fixture-driven.
pub(crate) fn clean_description(raw: &str) -> String {
    let mut text = collapse_spaces(raw.trim());
    for stage in [
        strip_qfx_transaction_segment as fn(&str) -> Option<String>,
        strip_auth_prefix as fn(&str) -> Option<String>,
        strip_processor_prefix,
        strip_processor_order_code,
        strip_account_holder_tail,
        strip_store_numbers,
        strip_phone_number,
        strip_card_suffix,
        strip_city_state,
        mask_long_digits,
    ] {
        if let Some(next) = stage(&text)
            && !next.trim().is_empty()
        {
            text = next;
        }
    }
    text
}

/// Remove Wells Fargo's transaction-kind/date segment, whether it precedes
/// the merchant or trails it, while retaining the merchant body.
fn strip_qfx_transaction_segment(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    for (marker, is_purchase) in
        [("PURCHASE", true), ("RECURRING PAYMENT", false)]
    {
        let mut search_from = 0;
        while let Some(relative) = upper.get(search_from..)?.find(marker) {
            let start = search_from + relative;
            let end = start + marker.len();
            let has_left_boundary = start == 0
                || upper
                    .as_bytes()
                    .get(start - 1)
                    .is_some_and(u8::is_ascii_whitespace);
            let has_right_boundary = upper
                .as_bytes()
                .get(end)
                .is_some_and(u8::is_ascii_whitespace);
            if !has_left_boundary || !has_right_boundary {
                search_from = end;
                continue;
            }

            let mut rest = text.get(end..)?.trim_start();
            if is_purchase
                && let Some(after_return) = take_leading_word(rest, "RETURN")
            {
                rest = after_return;
            }
            if let Some(after_auth) = strip_ascii_prefix(rest, "AUTHORIZED ON ")
            {
                rest = after_auth;
            }
            rest = rest.trim_start();
            let date_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let date = rest.get(..date_end)?;
            if !is_month_day_token(date) {
                search_from = end;
                continue;
            }
            let after_date =
                rest.get(date_end..).unwrap_or_default().trim_start();
            let merchant = text.get(..start)?.trim_end();
            if !merchant.is_empty() {
                return Some(collapse_spaces(merchant));
            }
            if !after_date.is_empty() {
                return Some(collapse_spaces(after_date));
            }
            search_from = end;
        }
    }
    None
}

fn strip_ascii_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &text[prefix.len()..])
}

fn take_leading_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let text = text.trim_start();
    let head = text.get(..word.len())?;
    if !head.eq_ignore_ascii_case(word) {
        return None;
    }
    let rest = text.get(word.len()..)?;
    rest.starts_with(char::is_whitespace)
        .then(|| rest.trim_start())
}

fn is_month_day_token(token: &str) -> bool {
    let parts: Vec<&str> = token.split('/').collect();
    matches!(parts.len(), 2 | 3)
        && parts[0].len() <= 2
        && !parts[0].is_empty()
        && parts[0].bytes().all(|byte| byte.is_ascii_digit())
        && parts[1].len() <= 2
        && !parts[1].is_empty()
        && parts[1].bytes().all(|byte| byte.is_ascii_digit())
        && (parts.len() == 2
            || (matches!(parts[2].len(), 2 | 4)
                && parts[2].bytes().all(|byte| byte.is_ascii_digit())))
}

/// Keep clean, user-edited learned names byte-for-byte. Re-run the cleaner
/// only when it removes substantive transaction noise, not just whitespace.
fn clean_learned_payee(payee: &str) -> String {
    let cleaned = clean_description(payee);
    if cleaned != collapse_spaces(payee.trim()) {
        cleaned
    } else {
        payee.to_owned()
    }
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_auth_prefix(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    for prefix in [
        "PURCHASE AUTHORIZED ON ",
        "AUTHORIZED ON ",
        "PENDING ",
        "RECURRING ",
    ] {
        if let Some(rest) = upper
            .strip_prefix(prefix)
            .and_then(|_| text.get(prefix.len()..))
        {
            // Skip the date/reference token that follows the prefix.
            let rest = rest.trim_start();
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let rest = rest.get(end..).unwrap_or_default().trim_start();
            if rest.is_empty() {
                return None;
            }
            return Some(collapse_spaces(rest));
        }
    }
    None
}

fn strip_processor_prefix(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    for prefix in ["SQ *", "SQ*", "TST*", "TST *", "PAYPAL *", "PAYPAL*"] {
        if let Some(rest) = upper
            .strip_prefix(prefix)
            .and_then(|_| text.get(prefix.len()..))
        {
            let rest = rest.trim_start().trim_start_matches('*').trim_start();
            if rest.is_empty() {
                return None;
            }
            let code_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let code = rest.get(..code_end).unwrap_or_default();
            if is_short_digit_code(code) {
                let after_code =
                    rest.get(code_end..).unwrap_or_default().trim_start();
                return Some(collapse_spaces(after_code));
            }
            return Some(collapse_spaces(rest));
        }
    }
    None
}

/// Drop short, digit-bearing processor/order tokens introduced by `*`.
/// Word-only sub-merchants after `*` are deliberately preserved.
fn strip_processor_order_code(text: &str) -> Option<String> {
    let mut words = Vec::new();
    let mut changed = false;
    for word in text.split_whitespace() {
        let Some((before, after)) = word.split_once('*') else {
            words.push(word);
            continue;
        };
        if !is_short_digit_code(after) {
            words.push(word);
            continue;
        }
        if !before.is_empty() {
            words.push(before);
        }
        changed = true;
    }
    changed.then(|| words.join(" "))
}

fn is_short_digit_code(token: &str) -> bool {
    let code = token
        .strip_prefix('*')
        .unwrap_or(token)
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '•');
    let length = code.chars().count();
    (1..=12).contains(&length)
        && code
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '•')
        && code.chars().any(|ch| ch.is_ascii_digit() || ch == '•')
}

fn strip_store_numbers(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '#'
            && let Some((_, next)) = chars.peek()
            && next.is_ascii_digit()
        {
            // Drop `#1234` plus the digits, and one preceding space.
            if out.ends_with(' ') {
                out.pop();
            }
            for (_, digit) in chars.by_ref() {
                if !digit.is_ascii_digit() {
                    out.push(digit);
                    break;
                }
            }
            changed = true;
        } else if ch == '#'
            && text
                .get(index + ch.len_utf8()..)
                .is_some_and(|tail| tail.starts_with("••••"))
        {
            // Historical journal descriptions mask store digits before
            // writing. Remove only this exact redaction token when it is
            // preceded by the store-number marker.
            if out.ends_with(' ') {
                out.pop();
            }
            for _ in 0..4 {
                chars.next();
            }
            changed = true;
        } else {
            out.push(ch);
        }
    }
    changed.then(|| collapse_spaces(out.trim()))
}

/// Drop an ACH/P2P account-holder suffix such as `REF 123456 DOE, JANE M`.
/// A formatted name is removed only after reference digits and a recognized
/// payment-rail marker, so an actual counterparty name remains part of the
/// merchant.
fn strip_account_holder_tail(text: &str) -> Option<String> {
    let (before_comma, after_comma) = text.rsplit_once(',')?;
    let given: Vec<&str> = after_comma.split_whitespace().collect();
    if given.len() != 2 || !is_name_word(given[0]) || !is_name_initial(given[1])
    {
        return None;
    }
    let surname = before_comma.split_whitespace().next_back()?;
    if !is_name_word(surname) {
        return None;
    }
    let surname_start = before_comma
        .rfind(char::is_whitespace)
        .map_or(0, |index| index + 1);
    let prefix = before_comma.get(..surname_start)?.trim_end();
    let upper = prefix.to_ascii_uppercase();
    let has_rail = ["ACH", "P2P", "ZELLE", "VENMO", "CASHAPP", "PAYPAL"]
        .iter()
        .any(|rail| upper.split_whitespace().any(|word| word == *rail))
        || upper.contains("CASH APP");
    let reference_digits = prefix
        .split_whitespace()
        .map(|word| word.bytes().filter(u8::is_ascii_digit).count())
        .sum::<usize>()
        + prefix.matches("••••").count() * 4;
    if !has_rail || reference_digits < 4 || prefix.is_empty() {
        return None;
    }
    Some(prefix.to_owned())
}

fn is_name_word(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|ch| ch.is_alphabetic() || matches!(ch, '\'' | '’' | '-'))
}

fn is_name_initial(word: &str) -> bool {
    let initial = word.strip_suffix('.').unwrap_or(word);
    initial.chars().count() == 1 && initial.chars().all(char::is_alphabetic)
}

/// Remove complete phone-number tokens, including common punctuation and
/// space-separated formats. Other long numeric identifiers are redacted by
/// `mask_long_digits` below.
fn strip_phone_number(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut output = Vec::new();
    let mut changed = false;
    let mut index = 0;
    while index < words.len() {
        if !is_phone_piece(words[index]) {
            output.push(words[index]);
            index += 1;
            continue;
        }
        let start = index;
        let mut digits = 0;
        while index < words.len() && is_phone_piece(words[index]) {
            digits += words[index]
                .chars()
                .filter(|ch| ch.is_ascii_digit() || *ch == '•')
                .count();
            index += 1;
        }
        let has_phone_shape = index - start > 1
            || words[start]
                .chars()
                .any(|ch| !ch.is_ascii_digit() && ch != '•');
        if digits >= 7 && has_phone_shape {
            changed = true;
        } else {
            output.extend_from_slice(&words[start..index]);
        }
    }
    changed.then(|| output.join(" "))
}

fn is_phone_piece(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|ch| {
            ch.is_ascii_digit()
                || ch == '•'
                || matches!(ch, '-' | '(' | ')' | '.' | '+' | ',')
        })
}

fn mask_long_digits(text: &str) -> Option<String> {
    let masked = mask_digits(text);
    (masked != text).then_some(masked)
}

fn strip_card_suffix(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 3 {
        return None;
    }
    let digits = |word: &str| {
        (word.len() == 4 && word.bytes().all(|byte| byte.is_ascii_digit()))
            || word == "••••"
    };
    let len = words.len();
    if words[len - 2].eq_ignore_ascii_case("CARD") && digits(words[len - 1]) {
        return Some(words[..len - 2].join(" "));
    }
    if len >= 4
        && words[len - 3].starts_with('S')
        && words[len - 3].len() > 1
        && words[len - 3][1..]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        && words[len - 2].eq_ignore_ascii_case("CARD")
        && digits(words[len - 1])
    {
        return Some(words[..len - 3].join(" "));
    }
    None
}

fn strip_city_state(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 3 {
        return None;
    }
    let state = words[words.len() - 1];
    if !is_state_code(state) {
        return None;
    }
    if words.len() >= 4 {
        let city = words[words.len() - 3..words.len() - 1]
            .join(" ")
            .to_ascii_uppercase();
        if is_known_two_word_city(&city) {
            return Some(words[..words.len() - 3].join(" "));
        }
    }
    let city = words[words.len() - 2];
    if is_city_word(city) {
        return Some(words[..words.len() - 2].join(" "));
    }
    None
}

fn is_known_two_word_city(city: &str) -> bool {
    matches!(
        city,
        "FORT WORTH"
            | "LAS VEGAS"
            | "LOS ANGELES"
            | "NEW YORK"
            | "PALO ALTO"
            | "SAN DIEGO"
            | "SAN FRANCISCO"
            | "ST LOUIS"
    )
}

fn is_city_word(word: &str) -> bool {
    word.len() >= 3 && word.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn is_state_code(word: &str) -> bool {
    matches!(
        word,
        "AL" | "AK"
            | "AZ"
            | "AR"
            | "CA"
            | "CO"
            | "CT"
            | "DE"
            | "DC"
            | "FL"
            | "GA"
            | "HI"
            | "ID"
            | "IL"
            | "IN"
            | "IA"
            | "KS"
            | "KY"
            | "LA"
            | "ME"
            | "MD"
            | "MA"
            | "MI"
            | "MN"
            | "MS"
            | "MO"
            | "MT"
            | "NE"
            | "NV"
            | "NH"
            | "NJ"
            | "NM"
            | "NY"
            | "NC"
            | "ND"
            | "OH"
            | "OK"
            | "OR"
            | "PA"
            | "RI"
            | "SC"
            | "SD"
            | "TN"
            | "TX"
            | "UT"
            | "VT"
            | "VA"
            | "WA"
            | "WV"
            | "WI"
            | "WY"
    )
}

/// Title-case a cleaned merchant core: first letter up, the rest down.
pub(crate) fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut out = String::with_capacity(word.len());
                    out.extend(first.to_uppercase());
                    out.push_str(&chars.as_str().to_lowercase());
                    out
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Keep an untrusted bank-derived payee to one grammar-safe line.
///
/// Strips headings, checkboxes, brackets, carets, and inline-field/link
/// syntax so a hostile description cannot inject headings, block IDs,
/// fields, or links into canonical output. Returns `"Unnamed transaction"`
/// when nothing safe remains.
pub(crate) fn sanitize_payee(text: &str) -> String {
    let first = text
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .next()
        .unwrap_or_default()
        .trim();
    let unmarked = first.trim_start_matches(['#', '>', ' ', '\t']).trim_start();
    let unboxed = strip_leading_checkbox(unmarked);
    let mut out = String::with_capacity(unboxed.len());
    for ch in unboxed.chars() {
        match ch {
            '[' | ']' | '^' => {}
            ch if ch.is_control() => out.push(' '),
            ch => out.push(ch),
        }
    }
    let collapsed = collapse_spaces(out.trim());
    if collapsed.is_empty() {
        return "Unnamed transaction".to_owned();
    }
    collapsed
}

fn strip_leading_checkbox(text: &str) -> &str {
    let after_dash = text.strip_prefix("- ").unwrap_or(text);
    let after_open = after_dash.strip_prefix('[').unwrap_or(after_dash);
    if after_dash.len() == after_open.len() {
        return text;
    }
    let mut chars = after_open.chars();
    let status = chars.next();
    let rest = chars.as_str();
    if !matches!(status, Some(' ' | 'x' | 'X' | '/' | '-'))
        || !rest.starts_with(']')
    {
        return text;
    }
    rest[1..].trim_start()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(extra: &str) -> BankConfig {
        let text = format!(
            "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n  categories:\n    groceries: \u{1f6d2} Groceries\n    dining: \u{1f37d}\u{fe0f} Dining\n    fees: \u{1f9fe} Fees\n    transfer: \u{2194}\u{fe0f} Transfer\n    cash: \u{1f3e7} Cash out\n{extra}---\n# Bank\n"
        );
        match super::super::config::parse_bank_config(&text) {
            super::super::config::ConfigParse::Ok(config) => config,
            other => panic!("hub should parse: {other:?}"),
        }
    }

    fn plain_hub() -> BankConfig {
        hub("")
    }

    #[test]
    fn cleaning_stages_compose_without_erasing_short_merchants() {
        assert_eq!(
            clean_description("SQ *SMASHBURGER ARLINGTON VA"),
            "SMASHBURGER"
        );
        assert_eq!(
            clean_description("TST* BLUE RIBBON BBQ BOSTON MA"),
            "BLUE RIBBON BBQ"
        );
        assert_eq!(
            clean_description("PAYPAL *STEAM GAMES #1234"),
            "STEAM GAMES"
        );
        assert_eq!(
            clean_description("WHOLEFDS #4821 MONTCLAIR NJ"),
            "WHOLEFDS"
        );
        assert_eq!(
            clean_description("COSTCO S123 CARD 4321 WHSE"),
            "COSTCO S123 CARD •••• WHSE"
        );
        assert_eq!(clean_description("SHOPRITE S987 CARD 1234"), "SHOPRITE");
        // Two-word cores survive even when the tail looks strippable.
        assert_eq!(
            clean_description("BANK OF AMERICA NA"),
            "BANK OF AMERICA NA"
        );
        assert_eq!(clean_description("A B"), "A B");
        assert_eq!(title_case("BLUE RIBBON BBQ"), "Blue Ribbon Bbq");
    }

    #[test]
    fn qfx_shapes_clean_dates_places_cards_and_processor_codes() {
        assert_eq!(
            clean_description(
                "PURCHASE 11/12/26 SQ *COFFEE SHOP NEW YORK NY CARD 4321"
            ),
            "COFFEE SHOP"
        );
        assert_eq!(
            clean_description(
                "COFFEE SHOP PURCHASE 11/12/26 NEW YORK NY CARD 4321"
            ),
            "COFFEE SHOP"
        );
        assert_eq!(
            clean_description(
                "PURCHASE 11/12/26 SQ *COFFEE SHOP NEW YORK NY CARD ••••"
            ),
            "COFFEE SHOP"
        );
        assert_eq!(
            clean_description(
                "PURCHASE RETURN AUTHORIZED ON 11/12/26 STORE RETURN BOSTON MA CARD 4321"
            ),
            "STORE RETURN"
        );
        assert_eq!(
            clean_description(
                "RECURRING PAYMENT AUTHORIZED ON 11/12/26 STREAMING SERVICE"
            ),
            "STREAMING SERVICE"
        );
        assert_eq!(
            clean_description(
                "STREAMING SERVICE RECURRING PAYMENT AUTHORIZED ON 11/12/26"
            ),
            "STREAMING SERVICE"
        );
        assert_eq!(
            clean_description("PURCHASE AUTHORIZED ON 11/12/26 COFFEE SHOP"),
            "COFFEE SHOP"
        );
        assert_eq!(
            clean_description("AUTHORIZED ON 11/12/26 COFFEE SHOP"),
            "COFFEE SHOP"
        );
        assert_eq!(clean_description("GROCERY SHOP *AB12"), "GROCERY SHOP");
        assert_eq!(clean_description("GROCERY SHOP *7"), "GROCERY SHOP");
        assert_eq!(clean_description("GROCERY SHOP *••••"), "GROCERY SHOP");
        assert_eq!(clean_description("GROCERY SHOP *AB••••"), "GROCERY SHOP");
        assert_eq!(clean_description("DELIVERY *SUSHI"), "DELIVERY *SUSHI");
        assert_eq!(clean_description("DELIVERY *PIZZA"), "DELIVERY *PIZZA");
    }

    #[test]
    fn account_holder_and_phone_noise_do_not_become_payees() {
        assert_eq!(
            clean_description("ACH PAYMENT REF 938271 DOE, JANE Q"),
            "ACH PAYMENT REF ••••"
        );
        assert_eq!(
            clean_description("ACH PAYMENT REF •••• DOE, JANE Q"),
            "ACH PAYMENT REF ••••"
        );
        assert_eq!(
            clean_description("P2P PAYMENT FROM 555-123-4567"),
            "P2P PAYMENT FROM"
        );
        assert_eq!(
            clean_description("P2P PAYMENT FROM 555-123-••••"),
            "P2P PAYMENT FROM"
        );
        assert_eq!(clean_description("MERCHANT 12345678"), "MERCHANT ••••");
        assert_eq!(
            clean_description("ACH PAYMENT TO DOE, JANE"),
            "ACH PAYMENT TO DOE, JANE"
        );
    }

    #[test]
    fn builtins_cover_atm_fees_and_transfers() {
        let config = plain_hub();
        let atm = resolve_name(
            &config,
            "ATM WITHDRAWAL 009821 NEW YORK NY",
            Some("ATM"),
            None,
            None,
        );
        assert_eq!(atm.payee, "ATM withdrawal");
        assert_eq!(atm.category.as_deref(), Some("cash"));

        let fee =
            resolve_name(&config, "OVERDRAFT FEE", Some("FEE"), None, None);
        assert_eq!(fee.category.as_deref(), Some("fees"));
        assert_eq!(fee.payee, "Overdraft Fee");

        let xfer = resolve_name(
            &config,
            "ONLINE TRANSFER TO SAVINGS 4821",
            Some("XFER"),
            None,
            None,
        );
        assert_eq!(xfer.category.as_deref(), Some("transfer"));

        // Undeclared categories never leak into a name.
        let minimal_text = "---\nparent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-01\n  categories:\n    groceries: \u{1f6d2} Groceries\n---\n# Bank\n";
        let bare = match super::super::config::parse_bank_config(minimal_text) {
            super::super::config::ConfigParse::Ok(config) => config,
            other => panic!("minimal hub should parse: {other:?}"),
        };
        let fee_bare =
            resolve_name(&bare, "OVERDRAFT FEE", Some("FEE"), None, None);
        assert_eq!(fee_bare.category, None);
        assert_eq!(fee_bare.payee, "Overdraft Fee");
    }

    #[test]
    fn rule_and_learned_tiers_fill_downward() {
        let config =
            hub("  rules:\n    - match: VENMO\n      payee: Venmo cashout\n");
        let named = resolve_name(
            &config,
            "VENMO CASHOUT 5551234567",
            Some("POS"),
            Some(&LearnedName {
                payee: "Old name".to_owned(),
                category: Some("dining".to_owned()),
                links: vec!["[[eat/old|🍽️]]".to_owned()],
            }),
            Some("[[eat/new|🍽️]]".to_owned()),
        );
        // Rule payee wins; rule leaves cat/link unspecified so the learned
        // row fills them and the restaurant link stays out.
        assert_eq!(named.payee, "Venmo cashout");
        assert_eq!(named.category.as_deref(), Some("dining"));
        assert_eq!(named.link.as_deref(), Some("[[eat/old|🍽️]]"));

        // Case-insensitive substring match; first rule wins.
        let multi = hub(
            "  rules:\n    - match: first\n      payee: First\n    - match: FIRST SECOND\n      payee: Second\n",
        );
        let named =
            resolve_name(&multi, "a first second spot", None, None, None);
        assert_eq!(named.payee, "First");
    }

    #[test]
    fn learned_payees_are_cleaned_only_when_they_still_carry_noise() {
        let config = plain_hub();
        let noisy = LearnedName {
            payee: "PURCHASE 11/12/26 COFFEE SHOP CARD 4321".to_owned(),
            category: None,
            links: Vec::new(),
        };
        assert_eq!(
            resolve_name(&config, "new transaction", None, Some(&noisy), None)
                .payee,
            "COFFEE SHOP"
        );

        let edited = "my preferred café";
        let clean = LearnedName {
            payee: edited.to_owned(),
            category: None,
            links: Vec::new(),
        };
        let resolved =
            resolve_name(&config, "new transaction", None, Some(&clean), None);
        assert_eq!(resolved.payee.as_bytes(), edited.as_bytes());
    }

    #[test]
    fn positive_amounts_never_imply_income() {
        let config = plain_hub();
        let inflow = resolve_name(
            &config,
            "CHECK 1001 DEPOSIT",
            Some("CREDIT"),
            None,
            None,
        );
        assert_eq!(inflow.category, None);
        assert_eq!(inflow.payee, "Check •••• Deposit");
    }

    #[test]
    fn sanitize_keeps_payees_to_one_safe_line() {
        assert_eq!(
            sanitize_payee("## Evil\n- [ ] $5 [[eat/x]] ^bank-abcd [cat:: y]"),
            "Evil"
        );
        assert_eq!(sanitize_payee("- [x] $5 Legit"), "$5 Legit");
        assert_eq!(sanitize_payee("[[only]]"), "only");
        assert_eq!(sanitize_payee("^^^"), "Unnamed transaction");
        assert_eq!(sanitize_payee("AT&T Store"), "AT&T Store");
    }
}
