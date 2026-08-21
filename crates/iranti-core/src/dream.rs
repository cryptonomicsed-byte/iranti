//! Dream cycle: periodic reflection that distills an agent's memory into a
//! digest — namespace statistics, top terms, salient entries, drift versus the
//! previous dream, and expired-memory decay. This is the self-improvement loop:
//! memory that never reflects is a log; memory that dreams becomes wisdom.

use anyhow::Result;
use serde_json::json;

use crate::recall::tokenize;
use crate::store::Store;

/// Current UTC date as YYYY-MM-DD (local calendar day approximation is fine
/// for a daily cycle; period is just a digest key).
pub fn today() -> String {
    let d = crate::now_secs();
    // seconds since epoch -> days -> civil date via a compact algorithm
    let days = d.div_euclid(86400) as i64;
    let (y, m, day) = civil_from_days(days);
    format!("{y:04}-{m:02}-{day:02}")
}

/// Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Generate (and persist) the dream digest for an agent for today.
pub fn dream(store: &Store, agent_pubkey: &str) -> Result<serde_json::Value> {
    let now = crate::now_secs();
    let period = today();
    let all = store.list_memories(agent_pubkey, None)?;

    let mut live = Vec::new();
    let mut expired = 0usize;
    for m in all {
        if m.expired(now) {
            expired += 1;
        } else {
            live.push(m);
        }
    }

    let mut ns_counts: Vec<(String, usize)> = Vec::new();
    let mut top_terms: Vec<(String, usize)> = Vec::new();
    let mut salient_slugs: Vec<String> = Vec::new();
    let mut pinned = 0usize;
    {
        use std::collections::HashMap;
        let mut nsmap: HashMap<String, usize> = HashMap::new();
        let mut tf: HashMap<String, usize> = HashMap::new();
        for m in &live {
            *nsmap.entry(m.ns.clone()).or_insert(0) += 1;
            if m.pin {
                pinned += 1;
            }
            if m.salience >= 75 {
                salient_slugs.push(m.slug.clone());
            }
            for t in tokenize(&m.value) {
                *tf.entry(t).or_insert(0) += 1;
            }
        }
        ns_counts = nsmap.into_iter().collect();
        ns_counts.sort_by(|a, b| b.1.cmp(&a.1));
        let mut terms: Vec<(String, usize)> = tf.into_iter().collect();
        terms.sort_by(|a, b| b.1.cmp(&a.1));
        top_terms = terms.into_iter().take(12).collect();
    }

    // Drift vs previous dream: term-set overlap.
    let prev = store.get_digest(agent_pubkey, "dream", &period)?;
    let mut overlap: Option<f64> = None;
    let mut new_terms: Vec<String> = Vec::new();
    if let Some(prev_json) = prev {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&prev_json) {
            if let Some(prev_terms) = v["top_terms"].as_array() {
                let prev_set: std::collections::HashSet<String> = prev_terms
                    .iter()
                    .filter_map(|t| t[0].as_str().map(|s| s.to_string()))
                    .collect();
                let cur_set: std::collections::HashSet<String> =
                    top_terms.iter().map(|(t, _)| t.clone()).collect();
                if !prev_set.is_empty() {
                    let inter = prev_set.intersection(&cur_set).count() as f64;
                    overlap = Some(inter / prev_set.len() as f64);
                }
                new_terms = cur_set.difference(&prev_set).cloned().collect();
            }
        }
    }

    let digest = json!({
        "agent": agent_pubkey,
        "period": period,
        "generated_at": now,
        "total": live.len(),
        "expired_this_cycle": expired,
        "pinned": pinned,
        "namespaces": ns_counts,
        "top_terms": top_terms,
        "salient_slugs": salient_slugs,
        "drift": { "term_overlap": overlap, "new_terms": new_terms },
    });
    store.put_digest(agent_pubkey, "dream", &period, &digest.to_string())?;
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_is_reasonable() {
        // 2026-08-04 ~ epoch 1785888000 (approx). Just assert shape + range.
        let d = today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("2026-"));
    }

    #[test]
    fn dream_produces_digest_and_persists() {
        use crate::store::Store;
        use std::sync::atomic::{AtomicUsize, Ordering};
        static C: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "iranti-dream-{}-{}",
            std::process::id(),
            C.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).ok();
        let store = Store::open(dir.join("t.sqlite")).unwrap();
        let agent = store.birth("dreamer", None).unwrap();
        store
            .write_memory(&agent.pubkey, "mem/a/x", "a", "the buzz relay enforces kind gates on ingest", 80, true, 0, None, None)
            .unwrap();
        store
            .write_memory(&agent.pubkey, "mem/b/y", "b", "mcp servers expose tools to agents in buzz rooms", 30, false, 0, None, None)
            .unwrap();
        let d = dream(&store, &agent.pubkey).unwrap();
        assert_eq!(d["total"], 2);
        assert_eq!(d["pinned"], 1);
        assert_eq!(d["namespaces"].as_array().unwrap().len(), 2);
        assert!(d["top_terms"].as_array().unwrap().len() >= 4);
        assert_eq!(d["salient_slugs"].as_array().unwrap().len(), 1);
        // second dream has drift overlap
        let d2 = dream(&store, &agent.pubkey).unwrap();
        assert_eq!(d2["drift"]["term_overlap"], 1.0);
    }
}
