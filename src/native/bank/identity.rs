//! Hashed bank identities, reserved row IDs, and masked descriptions.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

/// SHA-256 hex of `alias + "\n" + bank_id`. The bank ID (FITID or CSV
/// occurrence key) stays in this transient input and is never stored.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn transaction_key(alias: &str, bank_id: &str) -> String {
    sha256_hex(format!("{alias}\n{bank_id}").as_bytes())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Replace every run of at least four ASCII digits with `••••`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn mask_digits(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            let mut run = String::new();
            run.push(ch);
            while chars.peek().is_some_and(|next| next.is_ascii_digit()) {
                run.push(chars.next().expect("peeked digit"));
            }
            if run.len() >= 4 {
                out.push_str("••••");
            } else {
                out.push_str(&run);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Merchant key from conservatively cleaned, masked, whitespace-collapsed,
/// lowercased text. The same function handles old journal descriptions, so
/// append-only history remains comparable without rewriting stored events.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn merchant_key(text: &str) -> String {
    let cleaned = super::naming::clean_description(text);
    mask_digits(&cleaned)
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdCollision {
    pub(crate) prefix: String,
}

/// Reserve 8-character row IDs, extending to 12 on a prefix collision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IdReservation {
    by_id: BTreeMap<String, String>,
    by_key: BTreeMap<String, String>,
}

impl IdReservation {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_map(ids: &BTreeMap<String, String>) -> Self {
        let mut reservation = Self::default();
        for (id, key) in ids {
            reservation.by_id.insert(id.clone(), key.clone());
            reservation.by_key.entry(key.clone()).or_insert(id.clone());
        }
        reservation
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn occupy(&mut self, id: &str, key: &str) {
        self.by_id.insert(id.to_owned(), key.to_owned());
        self.by_key
            .entry(key.to_owned())
            .or_insert_with(|| id.to_owned());
    }

    /// Reuse a key's existing ID, else take 8 hex chars, else 12.
    /// Refuses when both prefixes already name a different key.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn reserve(&mut self, key: &str) -> Result<String, IdCollision> {
        if let Some(id) = self.by_key.get(key) {
            return Ok(id.clone());
        }
        let eight = prefix(key, 8);
        if self.id_is_free_for(&eight, key) {
            self.assign(key, eight);
            return Ok(self.by_key[key].clone());
        }
        let twelve = prefix(key, 12);
        if self.id_is_free_for(&twelve, key) {
            self.assign(key, twelve);
            return Ok(self.by_key[key].clone());
        }
        Err(IdCollision { prefix: twelve })
    }

    fn id_is_free_for(&self, id: &str, key: &str) -> bool {
        self.by_id.get(id).is_none_or(|owner| owner == key)
    }

    fn assign(&mut self, key: &str, id: String) {
        self.by_id.insert(id.clone(), key.to_owned());
        self.by_key.insert(key.to_owned(), id);
    }
}

fn prefix(key: &str, len: usize) -> String {
    key.chars().take(len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_sha256_of_alias_newline_bank_id() {
        let expected = sha256_hex(b"checking\nfitid-1");
        assert_eq!(transaction_key("checking", "fitid-1"), expected);
        assert_eq!(expected.len(), 64);
        assert!(expected.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(
            transaction_key("checking", "fitid-1"),
            transaction_key("savings", "fitid-1")
        );
    }

    #[test]
    fn mask_replaces_four_or_more_digits_only() {
        assert_eq!(mask_digits("CARD 1234"), "CARD ••••");
        assert_eq!(mask_digits("A12B345C6789D"), "A12B345C••••D");
        assert_eq!(mask_digits("room 123"), "room 123");
        assert_eq!(merchant_key("SQ *SMASH"), "smash");
        assert_eq!(merchant_key("TST*SMASH"), "smash");
        assert_ne!(merchant_key("TST*SMASH"), merchant_key("TST*MARKET"));
        assert_eq!(
            merchant_key("WHOLEFDS #4821"),
            merchant_key(&mask_digits("WHOLEFDS #4821"))
        );
    }

    #[test]
    fn qfx_merchant_keys_ignore_dates_cards_and_order_codes() {
        assert_eq!(
            merchant_key(
                "PURCHASE 11/12/26 SQ *COFFEE SHOP NEW YORK NY CARD 4321"
            ),
            merchant_key(
                "PURCHASE 11/14/26 SQ *COFFEE SHOP NEW YORK NY CARD 9876"
            )
        );
        assert_eq!(
            merchant_key("COFFEE SHOP PURCHASE 11/12/26 NEW YORK NY CARD 4321"),
            merchant_key("COFFEE SHOP PURCHASE 11/14/26 NEW YORK NY CARD 9876")
        );
        assert_eq!(
            merchant_key("GROCERY SHOP *AB12"),
            merchant_key("GROCERY SHOP *CD34")
        );
        assert_ne!(
            merchant_key("DELIVERY *SUSHI"),
            merchant_key("DELIVERY *PIZZA")
        );
    }

    #[test]
    fn reserve_reuses_and_extends_on_collision() {
        let (first, second) = colliding_bank_ids("checking");
        let key_a = transaction_key("checking", &first);
        let key_b = transaction_key("checking", &second);
        assert_eq!(&key_a[..8], &key_b[..8]);
        let mut known = BTreeMap::new();
        known.insert(key_a[..8].to_owned(), key_a.clone());
        let mut ids = IdReservation::from_map(&known);
        let id_a = ids.reserve(&key_a).expect("first");
        assert_eq!(id_a.len(), 8);
        let id_b = ids.reserve(&key_b).expect("second");
        assert_eq!(id_b.len(), 12);
        assert_eq!(ids.reserve(&key_a).expect("reuse"), id_a);
        let mut blocked = IdReservation::default();
        blocked.occupy(&key_b[..8], &key_a);
        blocked.occupy(&key_b[..12], &key_a);
        assert!(blocked.reserve(&key_b).is_err());
    }

    fn colliding_bank_ids(alias: &str) -> (String, String) {
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for n in 0..200_000u32 {
            let bank_id = format!("fitid-{n}");
            let prefix = prefix(&transaction_key(alias, &bank_id), 8);
            if let Some(other) = seen.get(&prefix)
                && other != &bank_id
            {
                return (other.clone(), bank_id);
            }
            seen.insert(prefix, bank_id);
        }
        panic!("no 8-character identity collision in search window");
    }
}
