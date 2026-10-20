//! Per-call TF-IDF cosine over the candidate corpus.

use std::collections::HashMap;

use super::corpus::Candidate;
use super::tokenize::Token;

pub(super) fn cosine_scores(
    query: &[Token],
    candidates: &[Candidate],
) -> Vec<f64> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let n = candidates.len();
    let mut df: HashMap<&str, usize> = HashMap::new();
    let doc_tfs: Vec<HashMap<&str, f64>> = candidates
        .iter()
        .map(|candidate| {
            let tf = term_weights(&candidate.tokens);
            for term in tf.keys() {
                *df.entry(*term).or_insert(0) += 1;
            }
            tf
        })
        .collect();
    let query_tf = term_weights(query);
    let query_vec = weighted_vector(&query_tf, n, &df);
    let query_norm = l2_norm(&query_vec);
    if query_norm == 0.0 {
        return vec![0.0; n];
    }
    doc_tfs
        .iter()
        .map(|tf| {
            let doc_vec = weighted_vector(tf, n, &df);
            let doc_norm = l2_norm(&doc_vec);
            if doc_norm == 0.0 {
                0.0
            } else {
                dot(&query_vec, &doc_vec) / (query_norm * doc_norm)
            }
        })
        .collect()
}

fn term_weights(tokens: &[Token]) -> HashMap<&str, f64> {
    let mut weights = HashMap::new();
    for token in tokens {
        *weights.entry(token.term.as_str()).or_insert(0.0) += token.weight;
    }
    weights
}

fn weighted_vector<'a>(
    tf: &HashMap<&'a str, f64>,
    n: usize,
    df: &HashMap<&str, usize>,
) -> HashMap<&'a str, f64> {
    tf.iter()
        .map(|(term, weight)| {
            let df = df.get(term).copied().unwrap_or(0);
            (*term, sublinear_tf(*weight) * idf(n, df))
        })
        .collect()
}

fn sublinear_tf(tf: f64) -> f64 {
    if tf <= 0.0 {
        0.0
    } else {
        1.0 + tf.ln()
    }
}

fn idf(n: usize, df: usize) -> f64 {
    ((n as f64 + 1.0) / (df as f64 + 0.5)).ln()
}

fn dot(left: &HashMap<&str, f64>, right: &HashMap<&str, f64>) -> f64 {
    left.iter().fold(0.0, |acc, (term, weight)| {
        acc + weight * right.get(term).copied().unwrap_or(0.0)
    })
}

fn l2_norm(vector: &HashMap<&str, f64>) -> f64 {
    vector
        .values()
        .map(|weight| weight * weight)
        .sum::<f64>()
        .sqrt()
}

pub(super) fn round_score(score: f64) -> f64 {
    (score * 1000.0).round() / 1000.0
}

pub(super) fn round_sort_score(score: f64) -> i32 {
    (score * 100.0).round() as i32
}
