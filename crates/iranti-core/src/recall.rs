//! Recall engine: BM25 relevance × recency decay × salience/pin boost.
//!
//! This is the "what did I learn about X?" surface NIP-AE does not provide —
//! engram slugs are opaque HMACs, so content search must happen over a local
//! index. Deterministic, dependency-free scoring; production can swap in
//! vector embeddings behind the same interface (see docs/architecture.md).

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::store::{MemRecord, Store};

#[derive(Debug, Clone)]
pub struct RecallHit {
    pub slug: String,
    pub ns: String,
    pub value: String,
    pub salience: u8,
    pub pin: bool,
    pub created_at: i64,
    pub score: f64,
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was", "one",
    "our", "out", "day", "get", "has", "him", "his", "how", "man", "new", "now", "old", "see",
    "two", "way", "who", "boy", "did", "its", "let", "put", "say", "she", "too", "use", "that",
    "with", "have", "this", "will", "your", "from", "they", "know", "want", "been", "good",
    "much", "some", "time", "very", "when", "come", "here", "just", "like", "long", "make",
    "many", "more", "only", "over", "such", "take", "than", "them", "well", "were", "what",
    "which", "would", "into", "about", "could", "other", "these", "there", "their", "where",
    "think", "thing", "things", "also", "back", "even", "first", "found", "give", "great",
    "help", "keep", "last", "left", "might", "must", "need", "never", "next", "once", "own",
    "still", "tell", "told", "under", "until", "while", "again", "away", "done", "down",
    "each", "else", "ever", "going", "idea", "life", "live", "mean", "most", "name", "open",
    "part", "right", "same", "said", "says", "should", "show", "small", "start", "state",
    "those", "three", "through", "took", "tried", "trying", "use", "used", "whole", "yes",
];

pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .filter(|w| !STOPWORDS.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// Recall across an agent's (optionally namespace-scoped) memories.
///
/// score = BM25(query, doc) * decay(age) * (1 + 0.5*pin) * (1 + (salience-50)/100)
pub fn recall(
    store: &Store,
    agent_pubkey: &str,
    query: &str,
    k: usize,
    ns: Option<&str>,
) -> Result<Vec<RecallHit>> {
    let now = crate::now_secs();
    let docs: Vec<MemRecord> = store
        .list_memories(agent_pubkey, ns)?
        .into_iter()
        .filter(|m| !m.expired(now))
        .collect();
    if docs.is_empty() {
        return Ok(vec![]);
    }
    let qterms = tokenize(query);
    if qterms.is_empty() {
        return Ok(vec![]);
    }

    // BM25 over the doc set.
    let n = docs.len() as f64;
    let avg_len = docs.iter().map(|d| tokenize(&d.value).len() as f64).sum::<f64>() / n;
    let k1 = 1.5f64;
    let b = 0.75f64;

    // Pre-tokenize docs.
    let mut tokenized: Vec<Vec<String>> = Vec::with_capacity(docs.len());
    let mut df: HashMap<String, usize> = HashMap::new();
    for d in &docs {
        let toks = tokenize(&d.value);
        let uniq: HashSet<&String> = toks.iter().collect();
        for t in uniq {
            *df.entry(t.clone()).or_insert(0) += 1;
        }
        tokenized.push(toks);
    }

    let mut hits: Vec<(usize, f64)> = Vec::new();
    for (i, toks) in tokenized.iter().enumerate() {
        let mut score = 0.0f64;
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for t in toks {
            *tf.entry(t).or_insert(0) += 1;
        }
        let dl = toks.len() as f64;
        for qt in &qterms {
            let df_t = *df.get(qt).unwrap_or(&0) as f64;
            if df_t == 0.0 {
                continue;
            }
            let idf = (1.0 + (n - df_t + 0.5) / (df_t + 0.5)).ln();
            let tf_t = *tf.get(qt.as_str()).unwrap_or(&0) as f64;
            if tf_t == 0.0 {
                continue;
            }
            score += idf * (tf_t * (k1 + 1.0)) / (tf_t + k1 * (1.0 - b + b * dl / avg_len.max(1.0)));
        }
        if score > 0.0 {
            hits.push((i, score));
        }
    }

    // Decay + boosts.
    let mut out: Vec<RecallHit> = Vec::new();
    for (i, bm25) in hits {
        let m = &docs[i];
        let age_days = ((now - m.created_at).max(0) as f64) / 86400.0;
        let decay = 0.5f64.powf(age_days / 14.0); // 14-day half-life
        let pin_boost = 1.0 + if m.pin { 0.5 } else { 0.0 };
        let salience_boost = 1.0 + (m.salience as f64 - 50.0) / 100.0;
        let score = bm25 * decay * pin_boost * salience_boost;
        out.push(RecallHit {
            slug: m.slug.clone(),
            ns: m.ns.clone(),
            value: m.value.clone(),
            salience: m.salience,
            pin: m.pin,
            created_at: m.created_at,
            score,
        });
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(k);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn test_store() -> Store {
        // Unique path per call so parallel tests never share a SQLite file.
        let dir = std::env::temp_dir().join(format!(
            "iranti-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).ok();
        Store::open(dir.join("t.sqlite")).unwrap()
    }

    #[test]
    fn recall_finds_and_ranks() {
        let store = test_store();
        let agent = store.birth("tester", None).unwrap();
        store
            .write_memory(&agent.pubkey, "mem/design/a", "design", "the buzz relay gates custom event kinds in ingest.rs", 60, false, 0, None, None)
            .unwrap();
        store
            .write_memory(&agent.pubkey, "mem/ops/b", "ops", "deploy matrix targets three production relays", 30, false, 0, None, None)
            .unwrap();
        let hits = recall(&store, &agent.pubkey, "relay gates", 5, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "mem/design/a");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn recall_respects_namespace_and_pin_boost() {
        let store = test_store();
        let agent = store.birth("tester2", None).unwrap();
        store
            .write_memory(&agent.pubkey, "mem/design/x", "design", "nip44 conversation keys are symmetric", 50, false, 0, None, None)
            .unwrap();
        store
            .write_memory(&agent.pubkey, "mem/ops/y", "ops", "nip44 conversation keys are symmetric", 50, true, 0, None, None)
            .unwrap();
        let in_design = recall(&store, &agent.pubkey, "conversation keys", 5, Some("design")).unwrap();
        assert_eq!(in_design.len(), 1);
        assert_eq!(in_design[0].ns, "design");
        let all = recall(&store, &agent.pubkey, "conversation keys", 5, None).unwrap();
        assert_eq!(all.len(), 2);
        // pinned wins ties
        assert_eq!(all[0].slug, "mem/ops/y");
        assert_eq!(all[1].slug, "mem/design/x");
    }

    #[test]
    fn tokenize_strips_stopwords() {
        let toks = tokenize("The relay gates kinds AND handlers");
        assert!(!toks.contains(&"the".to_string()));
        assert!(toks.contains(&"relay".to_string()));
        assert!(toks.contains(&"handlers".to_string()));
    }
}
