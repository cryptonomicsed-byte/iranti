//! JSON export: the contract between the Rust core (Èṣù) and the Python
//! resonance engine (Ọ̀ṣun). The resonance layer never touches the store or
//! keys directly — it consumes these plaintext snapshots and computes
//! cross-agent fingerprints (echo reports).

use anyhow::Result;
use serde_json::json;

use crate::store::{Agent, Store};

/// Full plaintext snapshot of one agent's memory for analysis.
pub fn export_agent(store: &Store, agent: &Agent) -> Result<serde_json::Value> {
    let now = crate::now_secs();
    let memories: Vec<serde_json::Value> = store
        .list_memories(&agent.pubkey, None)?
        .into_iter()
        .filter(|m| !m.expired(now))
        .map(|m| {
            json!({
                "slug": m.slug,
                "ns": m.ns,
                "value": m.value,
                "salience": m.salience,
                "pin": m.pin,
                "ttl": m.ttl,
                "prov": m.prov,
                "created_at": m.created_at,
            })
        })
        .collect();

    let digests: Vec<serde_json::Value> = store
        .list_digests(&agent.pubkey, None)?
        .into_iter()
        .map(|(period, body)| {
            let parsed = serde_json::from_str::<serde_json::Value>(&body).unwrap_or(json!({}));
            json!({ "period": period, "body": parsed })
        })
        .collect();

    let grants: Vec<serde_json::Value> = store
        .list_grants(&agent.pubkey)?
        .into_iter()
        .map(|g| json!({ "granter": g.granter, "grantee": g.grantee, "ns": g.ns, "created_at": g.created_at }))
        .collect();

    Ok(json!({
        "format": "iranti/export/v1",
        "name": agent.name,
        "pubkey": agent.pubkey,
        "owner": agent.owner,
        "created_at": agent.created_at,
        "memories": memories,
        "digests": digests,
        "grants": grants,
    }))
}

/// Snapshot of every agent (for hive-level echo analysis).
pub fn export_all(store: &Store) -> Result<Vec<serde_json::Value>> {
    let mut out = Vec::new();
    for agent in store.list_agents()? {
        out.push(export_agent(store, &agent)?);
    }
    Ok(out)
}
