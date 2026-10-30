//! Display-only inverse phrases for relation labels
//! (`docs/task-relations.md` §10).
//!
//! Named pairs cover labels the participle rule cannot produce. Every
//! other two-word `‹participle› by` label derives a lowercase inverse.
//! The result is never stored.

use super::label::collapse_label_text;

/// Named pairs the `‹participle› by` rule cannot produce.
pub(crate) const NAMED_PAIRS: &[(&str, &str)] = &[
    ("duplicate of", "duplicated by"),
    ("follow-up of", "followed up by"),
    ("merged into", "merged from"),
    ("split from", "split into"),
];

const PARTICIPLE_BASES: &[(&str, &str)] = &[
    ("added", "add"),
    ("adhered", "adhere"),
    ("adored", "adore"),
    ("agreed", "agree"),
    ("annulled", "annul"),
    ("atoned", "atone"),
    ("augured", "augur"),
    ("awed", "awe"),
    ("banged", "bang"),
    ("basted", "baste"),
    ("bathed", "bathe"),
    ("beaten", "beat"),
    ("begun", "begin"),
    ("billeted", "billet"),
    ("binged", "binge"),
    ("bitten", "bite"),
    ("bled", "bleed"),
    ("boomeranged", "boomerang"),
    ("borne", "bear"),
    ("bought", "buy"),
    ("bound", "bind"),
    ("breathed", "breathe"),
    ("bred", "breed"),
    ("broadcast", "broadcast"),
    ("broken", "break"),
    ("brought", "bring"),
    ("built", "build"),
    ("bulleted", "bullet"),
    ("calendared", "calendar"),
    ("caroled", "carol"),
    ("cast", "cast"),
    ("caught", "catch"),
    ("caviled", "cavil"),
    ("chosen", "choose"),
    ("clanged", "clang"),
    ("clothed", "clothe"),
    ("cohered", "cohere"),
    ("collared", "collar"),
    ("combated", "combat"),
    ("competed", "compete"),
    ("concreted", "concrete"),
    ("condoned", "condone"),
    ("contravened", "contravene"),
    ("controlled", "control"),
    ("convened", "convene"),
    ("corralled", "corral"),
    ("cringed", "cringe"),
    ("cut", "cut"),
    ("dealt", "deal"),
    ("debuted", "debut"),
    ("decreed", "decree"),
    ("delineated", "delineate"),
    ("deplored", "deplore"),
    ("dethroned", "dethrone"),
    ("deviled", "devil"),
    ("devoted", "devote"),
    ("dialled", "dial"),
    ("disagreed", "disagree"),
    ("done", "do"),
    ("downscaled", "downscale"),
    ("drawn", "draw"),
    ("driven", "drive"),
    ("duelled", "duel"),
    ("dug", "dig"),
    ("dyed", "dye"),
    ("eaten", "eat"),
    ("egged", "egg"),
    ("eloped", "elope"),
    ("emceed", "emcee"),
    ("enthroned", "enthrone"),
    ("equalled", "equal"),
    ("erred", "err"),
    ("excited", "excite"),
    ("excreted", "excrete"),
    ("exhaled", "exhale"),
    ("expedited", "expedite"),
    ("explored", "explore"),
    ("extolled", "extol"),
    ("extradited", "extradite"),
    ("eyed", "eye"),
    ("fed", "feed"),
    ("felt", "feel"),
    ("filleted", "fillet"),
    ("fled", "flee"),
    ("forecast", "forecast"),
    ("foreseen", "foresee"),
    ("forgotten", "forget"),
    ("fought", "fight"),
    ("found", "find"),
    ("freed", "free"),
    ("fringed", "fringe"),
    ("frolicked", "frolic"),
    ("frozen", "freeze"),
    ("fuelled", "fuel"),
    ("ganged", "gang"),
    ("given", "give"),
    ("gotten", "get"),
    ("grown", "grow"),
    ("guaranteed", "guarantee"),
    ("hanged", "hang"),
    ("hasted", "haste"),
    ("heard", "hear"),
    ("held", "hold"),
    ("hidden", "hide"),
    ("hinged", "hinge"),
    ("hit", "hit"),
    ("hung", "hang"),
    ("hurt", "hurt"),
    ("ideated", "ideate"),
    ("ignited", "ignite"),
    ("ignored", "ignore"),
    ("impaled", "impale"),
    ("imperiled", "imperil"),
    ("imperilled", "imperil"),
    ("impinged", "impinge"),
    ("implored", "implore"),
    ("incited", "incite"),
    ("infringed", "infringe"),
    ("inhaled", "inhale"),
    ("initialled", "initial"),
    ("interfered", "interfere"),
    ("interloped", "interlope"),
    ("intervened", "intervene"),
    ("intoned", "intone"),
    ("invited", "invite"),
    ("kept", "keep"),
    ("known", "know"),
    ("laid", "lay"),
    ("lambasted", "lambaste"),
    ("led", "lead"),
    ("left", "leave"),
    ("lent", "lend"),
    ("let", "let"),
    ("loathed", "loathe"),
    ("lost", "lose"),
    ("made", "make"),
    ("marshalled", "marshal"),
    ("meant", "mean"),
    ("medalled", "medal"),
    ("met", "meet"),
    ("mimicked", "mimic"),
    ("misled", "mislead"),
    ("murmured", "murmur"),
    ("outscored", "outscore"),
    ("overcome", "overcome"),
    ("overfed", "overfeed"),
    ("overridden", "override"),
    ("overrun", "overrun"),
    ("overseen", "oversee"),
    ("overtaken", "overtake"),
    ("overwritten", "overwrite"),
    ("paid", "pay"),
    ("panicked", "panic"),
    ("pasted", "paste"),
    ("patrolled", "patrol"),
    ("pedalled", "pedal"),
    ("penciled", "pencil"),
    ("pencilled", "pencil"),
    ("periled", "peril"),
    ("permeated", "permeate"),
    ("persevered", "persevere"),
    ("pled", "plead"),
    ("postponed", "postpone"),
    ("proven", "prove"),
    ("put", "put"),
    ("quit", "quit"),
    ("read", "read"),
    ("readded", "readd"),
    ("rebuilt", "rebuild"),
    ("recited", "recite"),
    ("redone", "redo"),
    ("refereed", "referee"),
    ("refuelled", "refuel"),
    ("rehomed", "rehome"),
    ("reignited", "reignite"),
    ("reinvited", "reinvite"),
    ("remade", "remake"),
    ("rerouted", "reroute"),
    ("rerun", "rerun"),
    ("rescaled", "rescale"),
    ("reset", "reset"),
    ("restored", "restore"),
    ("resynced", "resync"),
    ("reunited", "reunite"),
    ("revered", "revere"),
    ("rewritten", "rewrite"),
    ("ridden", "ride"),
    ("rivalled", "rival"),
    ("routed", "route"),
    ("run", "run"),
    ("rung", "ring"),
    ("said", "say"),
    ("scythed", "scythe"),
    ("secreted", "secrete"),
    ("seen", "see"),
    ("seethed", "seethe"),
    ("sent", "send"),
    ("set", "set"),
    ("shaken", "shake"),
    ("sheathed", "sheathe"),
    ("shed", "shed"),
    ("shown", "show"),
    ("shut", "shut"),
    ("signalled", "signal"),
    ("singed", "singe"),
    ("sold", "sell"),
    ("soothed", "soothe"),
    ("sought", "seek"),
    ("sped", "speed"),
    ("spent", "spend"),
    ("split", "split"),
    ("spoken", "speak"),
    ("sponged", "sponge"),
    ("spread", "spread"),
    ("spun", "spin"),
    ("stenciled", "stencil"),
    ("stencilled", "stencil"),
    ("stolen", "steal"),
    ("stood", "stand"),
    ("struck", "strike"),
    ("strung", "string"),
    ("stuck", "stick"),
    ("sung", "sing"),
    ("swept", "sweep"),
    ("synced", "sync"),
    ("syringed", "syringe"),
    ("taken", "take"),
    ("tasted", "taste"),
    ("taught", "teach"),
    ("teed", "tee"),
    ("teethed", "teethe"),
    ("telephoned", "telephone"),
    ("thought", "think"),
    ("thrown", "throw"),
    ("tinged", "tinge"),
    ("tithed", "tithe"),
    ("told", "tell"),
    ("torn", "tear"),
    ("totalled", "total"),
    ("trafficked", "traffic"),
    ("trialled", "trial"),
    ("twanged", "twang"),
    ("twinged", "twinge"),
    ("underscored", "underscore"),
    ("understood", "understand"),
    ("undertaken", "undertake"),
    ("undone", "undo"),
    ("united", "unite"),
    ("upheld", "uphold"),
    ("upscaled", "upscale"),
    ("upset", "upset"),
    ("wasted", "waste"),
    ("wed", "wed"),
    ("welcomed", "welcome"),
    ("whinged", "whinge"),
    ("withheld", "withhold"),
    ("won", "win"),
    ("worn", "wear"),
    ("wreathed", "wreathe"),
    ("writhed", "writhe"),
    ("written", "write"),
];

const ELL_KEEP: &[&str] = &[
    "dwell",
    "fell",
    "foretell",
    "knell",
    "misspell",
    "outsell",
    "oversell",
    "quell",
    "resell",
    "respell",
    "retell",
    "sell",
    "shell",
    "smell",
    "spell",
    "swell",
    "tell",
    "undersell",
    "upsell",
    "well",
    "yell",
];

const DOUBLED: &[u8] = b"bdgkmnprtv";

/// Display-only inverse phrase for a stored label. Always lowercase.
pub(crate) fn inverse_phrase(label: &str) -> Option<String> {
    let key = collapse_label_text(label).to_lowercase();
    if let Some(inverse) = named_pair(&key) {
        return Some(inverse.to_string());
    }
    let mut tokens = key.split(' ');
    let first = tokens.next()?;
    let second = tokens.next()?;
    if tokens.next().is_some() || second != "by" {
        return None;
    }
    if !is_ascii_hyphen_token(first) {
        return None;
    }
    let (head, participle) = split_last_hyphen(first);
    let third = third_person_from_participle(participle)?;
    Some(format!("{head}{third}"))
}

fn named_pair(key: &str) -> Option<&'static str> {
    lookup_pair(NAMED_PAIRS, key)
}

fn lookup_pair<'a>(
    table: &'a [(&'a str, &'a str)],
    key: &str,
) -> Option<&'a str> {
    table
        .binary_search_by_key(&key, |&(candidate, _)| candidate)
        .ok()
        .map(|index| table[index].1)
}

fn is_ascii_hyphen_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut index = 0;
    let mut saw_letter = false;
    while index < bytes.len() && bytes[index].is_ascii_lowercase() {
        saw_letter = true;
        index += 1;
    }
    if !saw_letter {
        return false;
    }
    while index < bytes.len() {
        if bytes[index] != b'-' {
            return false;
        }
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_lowercase() {
            index += 1;
        }
        if index == start {
            return false;
        }
    }
    true
}

fn split_last_hyphen(token: &str) -> (&str, &str) {
    match token.rfind('-') {
        Some(index) => (&token[..=index], &token[index + 1..]),
        None => ("", token),
    }
}

fn third_person_from_participle(participle: &str) -> Option<String> {
    if let Some(base) = lookup_pair(PARTICIPLE_BASES, participle) {
        return Some(third_person(base));
    }
    if !participle.ends_with("ed") || participle.len() < 4 {
        return None;
    }
    let stem = &participle[..participle.len() - 2];
    if vowel_group_count(stem) == 0 {
        return None;
    }
    if stem.ends_with('e') {
        return None;
    }
    if stem.ends_with('i') {
        return Some(format!("{stem}es"));
    }
    if stem.ends_with('y') {
        return Some(third_person(stem));
    }
    let bytes = stem.as_bytes();
    let len = bytes.len();
    if len >= 3
        && bytes[len - 1] == bytes[len - 2]
        && DOUBLED.contains(&bytes[len - 1])
    {
        return Some(third_person(&stem[..len - 1]));
    }
    if stem.ends_with("ll") {
        let drop_last = stem.ends_with("ell")
            && ELL_KEEP.binary_search(&stem).is_err()
            && vowel_group_count(stem) >= 2;
        let base = if drop_last { &stem[..len - 1] } else { stem };
        return Some(third_person(base));
    }
    if stem.ends_with("ss") || stem.ends_with("zz") || stem.ends_with("ff") {
        return Some(third_person(stem));
    }
    let base = if needs_e(stem) {
        format!("{stem}e")
    } else {
        stem.to_string()
    };
    Some(third_person(&base))
}

fn is_vowel_at(stem: &str, index: usize) -> bool {
    let bytes = stem.as_bytes();
    match bytes[index] {
        b'a' | b'e' | b'i' | b'o' => true,
        b'u' => {
            let prev = if index == 0 { 0 } else { bytes[index - 1] };
            if prev == b'q' {
                return false;
            }
            if prev == b'g' && index + 1 < bytes.len() {
                let next = bytes[index + 1];
                if matches!(next, b'a' | b'e' | b'i' | b'o' | b'u' | b'y') {
                    return false;
                }
            }
            true
        }
        b'y' => index > 0 && !is_vowel_at(stem, index - 1),
        _ => false,
    }
}

fn vowel_group_count(stem: &str) -> usize {
    let mut count = 0;
    let mut in_vowel = false;
    for index in 0..stem.len() {
        let vowel = is_vowel_at(stem, index);
        if vowel && !in_vowel {
            count += 1;
        }
        in_vowel = vowel;
    }
    count
}

fn needs_e(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    let len = bytes.len();
    let last = bytes[len - 1];
    let prev = bytes[len - 2];
    if last == b'u' {
        return true;
    }
    if matches!(last, b'a' | b'e' | b'i' | b'o' | b'y') {
        return false;
    }
    if matches!(last, b'v' | b'c' | b'z' | b's') {
        return true;
    }
    if matches!(last, b'w' | b'x' | b'h') {
        return false;
    }
    if last == b'l' && !is_vowel_at(stem, len - 2) {
        return !matches!(prev, b'r' | b'l' | b'w');
    }
    if last == b'r' && !is_vowel_at(stem, len - 2) {
        return prev != b'r';
    }
    if last == b'g' {
        if matches!(prev, b'd' | b'r' | b'l') {
            return true;
        }
        if prev == b'n' {
            let before = if len >= 3 { bytes[len - 3] } else { 0 };
            return matches!(before, b'a' | b'e' | b'u');
        }
        return is_vowel_at(stem, len - 2);
    }
    if !is_vowel_at(stem, len - 2) {
        return false;
    }
    if len >= 3 && is_vowel_at(stem, len - 3) {
        return stem.ends_with("iat")
            || stem.ends_with("uat")
            || stem.ends_with("uad")
            || stem.ends_with("creat");
    }
    if vowel_group_count(stem) == 1 {
        return true;
    }
    let pair = &stem[len - 2..];
    match pair {
        "at" => true,
        "et" => stem.ends_with("let"),
        "ot" => {
            stem.ends_with("mot")
                || stem.ends_with("not")
                || stem.ends_with("quot")
        }
        "op" => stem.ends_with("scop"),
        "ab" | "eb" | "ib" | "ob" | "ub" | "ad" | "ed" | "id" | "od" | "ud"
        | "ak" | "ek" | "ik" | "ok" | "uk" | "am" | "em" | "im" | "um"
        | "ap" | "yp" | "ar" | "ir" | "ur" | "il" | "ol" | "ul" | "in"
        | "un" | "ut" => true,
        _ => false,
    }
}

fn third_person(base: &str) -> String {
    if base.ends_with('s')
        || base.ends_with('x')
        || base.ends_with('z')
        || base.ends_with("ch")
        || base.ends_with("sh")
    {
        return format!("{base}es");
    }
    let bytes = base.as_bytes();
    let len = bytes.len();
    if len >= 2 && bytes[len - 1] == b'y' && !is_vowel_at(base, len - 2) {
        return format!("{}ies", &base[..len - 1]);
    }
    if len >= 2 && bytes[len - 1] == b'o' && !is_vowel_at(base, len - 2) {
        return format!("{base}es");
    }
    format!("{base}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrase(label: &str) -> Option<String> {
        inverse_phrase(label)
    }

    #[test]
    fn tables_are_sorted() {
        let mut named = NAMED_PAIRS.to_vec();
        named.sort_by(|left, right| left.0.cmp(right.0));
        assert_eq!(named, NAMED_PAIRS);
        let mut bases = PARTICIPLE_BASES.to_vec();
        bases.sort_by(|left, right| left.0.cmp(right.0));
        assert_eq!(bases, PARTICIPLE_BASES);
        let mut keep = ELL_KEEP.to_vec();
        keep.sort_unstable();
        assert_eq!(keep, ELL_KEEP);
    }

    #[test]
    fn named_pairs_win_before_the_participle_rule() {
        assert_eq!(phrase("Merged Into").as_deref(), Some("merged from"));
        assert_eq!(phrase("duplicate of").as_deref(), Some("duplicated by"));
        assert_eq!(phrase("follow-up of").as_deref(), Some("followed up by"));
        assert_eq!(phrase("Split From").as_deref(), Some("split into"));
        assert_eq!(phrase("split by").as_deref(), Some("splits"));
    }

    #[test]
    fn ied_participles_take_ies() {
        assert_eq!(phrase("verified by").as_deref(), Some("verifies"));
        assert_eq!(phrase("copied by").as_deref(), Some("copies"));
        assert_eq!(phrase("tied by").as_deref(), Some("ties"));
    }

    #[test]
    fn y_stems_keep_y() {
        assert_eq!(phrase("played by").as_deref(), Some("plays"));
        assert_eq!(phrase("deployed by").as_deref(), Some("deploys"));
        assert_eq!(phrase("keyed by").as_deref(), Some("keys"));
    }

    #[test]
    fn doubled_consonants_drop_the_last_letter() {
        assert_eq!(phrase("planned by").as_deref(), Some("plans"));
        assert_eq!(phrase("committed by").as_deref(), Some("commits"));
        assert_eq!(phrase("embedded by").as_deref(), Some("embeds"));
        assert_eq!(phrase("revved by").as_deref(), Some("revs"));
    }

    #[test]
    fn ll_rule_drops_british_ell_and_keeps_the_rest() {
        assert_eq!(phrase("cancelled by").as_deref(), Some("cancels"));
        assert_eq!(phrase("labelled by").as_deref(), Some("labels"));
        assert_eq!(phrase("compelled by").as_deref(), Some("compels"));
        assert_eq!(phrase("installed by").as_deref(), Some("installs"));
        assert_eq!(phrase("spelled by").as_deref(), Some("spells"));
        assert_eq!(phrase("called by").as_deref(), Some("calls"));
    }

    #[test]
    fn ss_zz_ff_keep_the_double() {
        assert_eq!(phrase("passed by").as_deref(), Some("passes"));
        assert_eq!(phrase("buzzed by").as_deref(), Some("buzzes"));
        assert_eq!(phrase("stuffed by").as_deref(), Some("stuffs"));
    }

    #[test]
    fn needs_e_covers_the_documented_cases() {
        assert_eq!(phrase("queued by").as_deref(), Some("queues"));
        assert_eq!(phrase("echoed by").as_deref(), Some("echoes"));
        assert_eq!(phrase("radioed by").as_deref(), Some("radios"));
        assert_eq!(phrase("solved by").as_deref(), Some("solves"));
        assert_eq!(phrase("replaced by").as_deref(), Some("replaces"));
        assert_eq!(phrase("showed by").as_deref(), Some("shows"));
        assert_eq!(phrase("enabled by").as_deref(), Some("enables"));
        assert_eq!(phrase("curled by").as_deref(), Some("curls"));
        assert_eq!(phrase("centred by").as_deref(), Some("centres"));
        assert_eq!(phrase("judged by").as_deref(), Some("judges"));
        assert_eq!(phrase("changed by").as_deref(), Some("changes"));
        assert_eq!(phrase("pinged by").as_deref(), Some("pings"));
        assert_eq!(phrase("engaged by").as_deref(), Some("engages"));
        assert_eq!(phrase("extended by").as_deref(), Some("extends"));
        assert_eq!(phrase("created by").as_deref(), Some("creates"));
        assert_eq!(phrase("treated by").as_deref(), Some("treats"));
        assert_eq!(phrase("hoped by").as_deref(), Some("hopes"));
        assert_eq!(phrase("updated by").as_deref(), Some("updates"));
        assert_eq!(phrase("deleted by").as_deref(), Some("deletes"));
        assert_eq!(phrase("targeted by").as_deref(), Some("targets"));
        assert_eq!(phrase("promoted by").as_deref(), Some("promotes"));
        assert_eq!(phrase("pivoted by").as_deref(), Some("pivots"));
        assert_eq!(phrase("rescoped by").as_deref(), Some("rescopes"));
        assert_eq!(phrase("developed by").as_deref(), Some("develops"));
        assert_eq!(phrase("described by").as_deref(), Some("describes"));
        assert_eq!(phrase("edited by").as_deref(), Some("edits"));
        assert_eq!(phrase("needed by").as_deref(), Some("needs"));
        assert_eq!(phrase("proceeded by").as_deref(), Some("proceeds"));
    }

    #[test]
    fn exception_tables_and_irregulars() {
        assert_eq!(phrase("agreed by").as_deref(), Some("agrees"));
        assert_eq!(phrase("added by").as_deref(), Some("adds"));
        assert_eq!(phrase("invited by").as_deref(), Some("invites"));
        assert_eq!(phrase("murmured by").as_deref(), Some("murmurs"));
        assert_eq!(phrase("hanged by").as_deref(), Some("hangs"));
        assert_eq!(phrase("hinged by").as_deref(), Some("hinges"));
        assert_eq!(phrase("controlled by").as_deref(), Some("controls"));
        assert_eq!(phrase("fuelled by").as_deref(), Some("fuels"));
        assert_eq!(phrase("led by").as_deref(), Some("leads"));
        assert_eq!(phrase("written by").as_deref(), Some("writes"));
        assert_eq!(phrase("overridden by").as_deref(), Some("overrides"));
        assert_eq!(phrase("done by").as_deref(), Some("does"));
        assert_eq!(phrase("undone by").as_deref(), Some("undoes"));
        assert_eq!(phrase("caught by").as_deref(), Some("catches"));
        assert_eq!(phrase("read by").as_deref(), Some("reads"));
    }

    #[test]
    fn hyphens_split_on_the_last_hyphen() {
        assert_eq!(phrase("co-authored by").as_deref(), Some("co-authors"));
        assert_eq!(phrase("fine-tuned by").as_deref(), Some("fine-tunes"));
        assert_eq!(
            phrase("auto-generated by").as_deref(),
            Some("auto-generates")
        );
        assert_eq!(phrase("copy-pasted by").as_deref(), Some("copy-pastes"));
        assert_eq!(phrase("re-opened by").as_deref(), Some("re-opens"));
    }

    #[test]
    fn rejects_non_two_word_by_labels_and_non_ascii() {
        assert_eq!(phrase("see"), None);
        assert_eq!(phrase("idea for"), None);
        assert_eq!(phrase("polishes"), None);
        assert_eq!(phrase("implements"), None);
        assert_eq!(phrase("followed up by"), None);
        assert_eq!(phrase("spec.md comment was inspired by"), None);
        assert_eq!(phrase("need by"), None);
        assert_eq!(phrase("red by"), None);
        assert_eq!(phrase("inspired"), None);
        assert_eq!(phrase("by"), None);
        assert_eq!(phrase("inspired by me"), None);
        assert_eq!(phrase("créé by"), None);
        assert_eq!(phrase("-ed by"), None);
        assert_eq!(phrase(" INSPIRED   by:").as_deref(), Some("inspires"));
    }
}
