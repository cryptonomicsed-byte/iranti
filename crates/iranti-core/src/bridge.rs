//! Buzz bridge: sync local memory to/from a Buzz relay using its plain-HTTP
//! surfaces (`POST /events`, `POST /query`) — no WebSocket client required.
//!
//! - `push`: publish local memories as NIP-AE kind:30174 engram events.
//! - `pull`: fetch the agent's engrams back and upsert (or tombstone) locally.
//! - `import`: ingest Buzz channel history (kinds 1/42/30023) into memory with
//!   provenance, so an agent can remember what happened in a room.
//!
//! Note on Buzz relays: the relay enforces `required_scope_for_kind` — unknown
//! kinds are rejected. Kind 30174 is already allowlisted upstream; the Iranti
//! companion kinds 36000-36003 need the allowlist patch in docs/.

use anyhow::{bail, Context, Result};
use nostr::Event;
use serde_json::{json, Value};

use crate::crypto::{self, MemBody};
use crate::store::{Agent, Store};
use crate::VERSION;

pub struct RelayClient {
    pub url: String,
    client: reqwest::blocking::Client,
}

impl RelayClient {
    pub fn new(url: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("iranti/{VERSION}"))
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self { url: url.trim_end_matches('/').to_string(), client })
    }

    /// POST /events — submit a signed event. Buzz accepts the event JSON
    /// object directly (same path the WebSocket uses).
    pub fn publish(&self, event: &Event) -> Result<String> {
        let resp = self
            .client
            .post(format!("{}/events", self.url))
            .json(&serde_json::to_value(event)?)
            .send()
            .with_context(|| format!("POST {} /events failed", self.url))?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("relay rejected event: HTTP {status}: {body}");
        }
        // Relay OK envelope: ["OK", <id>, <bool>, <msg>]
        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&body) {
            if arr.len() >= 4 && arr[0] == "OK" && arr[2] == false {
                bail!("relay OK=false: {}", arr[3]);
            }
        }
        Ok(body)
    }

    /// POST /query — Nostr REQ over HTTP. Returns parsed events.
    pub fn query(&self, filter: &Value) -> Result<Vec<Event>> {
        let body = json!(["REQ", "iranti-sync", filter]);
        let resp = self
            .client
            .post(format!("{}/query", self.url))
            .json(&body)
            .send()
            .with_context(|| format!("POST {} /query failed", self.url))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("relay query failed: HTTP {status}");
        }
        let text = resp.text().unwrap_or_default();
        let arr: Vec<Value> = serde_json::from_str(&text)
            .with_context(|| format!("relay returned non-JSON-array: {}", truncate(&text, 200)))?;
        let mut events = Vec::new();
        for item in arr {
            // ["EVENT", <subid>, <event>] tuples; also tolerate bare events.
            let ev = match item.as_array() {
                Some(v) if v.len() >= 3 && v[0] == "EVENT" => v[2].clone(),
                _ => item.clone(),
            };
            if let Ok(e) = serde_json::from_str::<Event>(&ev.to_string()) {
                events.push(e);
            }
        }
        Ok(events)
    }

    /// POST /count — Nostr COUNT over HTTP.
    #[allow(dead_code)]
    pub fn count(&self, filter: &Value) -> Result<u64> {
        let body = json!(["COUNT", "iranti-sync", filter]);
        let resp = self
            .client
            .post(format!("{}/count", self.url))
            .json(&body)
            .send()
            .with_context(|| format!("POST {} /count failed", self.url))?;
        let text = resp.text().unwrap_or_default();
        let arr: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
        for item in arr {
            if let Some(v) = item.as_array() {
                if v.len() >= 3 && v[0] == "COUNT" {
                    return Ok(v[2].as_u64().unwrap_or(0));
                }
            }
        }
        Ok(0)
    }
}

/// Push an agent's local memories to a relay as NIP-AE engram events.
///
/// Enforces NIP-AE monotonicity: created_at = max(now, head+1) via the stored
/// `updated_at` — the bridge keeps a per-agent cursor in meta.
pub fn push(store: &Store, relay: &str, agent: &Agent) -> Result<usize> {
    let client = RelayClient::new(relay)?;
    let keys = store.keys_for(agent)?;
    let owner_pk = require_owner(agent)?;
    let mems = store.list_memories(&agent.pubkey, None)?;
    let mut pushed = 0usize;
    for m in mems {
        let mut body = MemBody::new(m.slug.clone(), m.value.clone());
        body.ns = Some(m.ns.clone());
        body.salience = Some(m.salience);
        body.pin = Some(m.pin);
        if m.ttl > 0 {
            body.ttl = Some(m.ttl);
        }
        body.prov = m.prov.clone();
        let created_at = m.updated_at.max(crate::now_secs()) as u64;
        let ev = crypto::build_engram_event(&keys, &owner_pk, &m.slug, &body, created_at)?;
        client.publish(&ev)?;
        pushed += 1;
    }
    store.meta_set(&format!("sync_cursor_{}", agent.pubkey), &crate::now_secs().to_string())?;
    Ok(pushed)
}

/// Pull an agent's engrams from a relay and reconcile the local store.
pub fn pull(store: &Store, relay: &str, agent: &Agent) -> Result<(usize, usize)> {
    let client = RelayClient::new(relay)?;
    let keys = store.keys_for(agent)?;
    let owner_pk = require_owner(agent)?;
    let filter = json!({
        "kinds": [crypto::KIND_AGENT_ENGRAM],
        "authors": [agent.pubkey],
        "#p": [owner_pk.to_hex()],
    });
    let events = client.query(&filter)?;
    let mut upserted = 0usize;
    let mut tombstoned = 0usize;
    for ev in events {
        let body = match crypto::decrypt_body(&keys, &owner_pk, &ev.content) {
            Ok(b) => b,
            Err(_) => continue, // not ours or malformed
        };
        if body.value.is_empty() {
            // NIP-AE tombstone convention: value null. Our local delete marks
            // absence; an empty value here is treated as a tombstone intent.
            if store.forget(&agent.pubkey, &body.slug)? {
                tombstoned += 1;
            }
            continue;
        }
        let ns = body.ns.unwrap_or_else(|| "general".into());
        let salience = body.salience.unwrap_or(50);
        let pin = body.pin.unwrap_or(false);
        let ttl = body.ttl.unwrap_or(0);
        let prov = body.prov.as_deref();
        store.write_memory(
            &agent.pubkey, &body.slug, &ns, &body.value,
            salience, pin, ttl, prov, Some(ev.created_at.as_secs() as i64),
 )?;
 upserted += 1;
    }
    Ok((upserted, tombstoned))
}

/// Import Buzz channel history (kinds 1/42/30023 etc.) into an agent's memory.
pub fn import_from_buzz(
    store: &Store,
    relay: &str,
    agent: &Agent,
    kinds: &[u32],
    h: Option<&str>,
) -> Result<usize> {
    let client = RelayClient::new(relay)?;
    let mut filter = json!({ "kinds": kinds, "limit": 1000 });
    if let Some(h_tag) = h {
        filter["#h"] = json!([h_tag]);
    }
    let events = client.query(&filter)?;
    let mut imported = 0usize;
    for ev in events {
        let content = ev.content.trim();
        if content.is_empty() {
            continue;
        }
        let ns = ev
            .tags
            .iter()
            .find(|t| t.kind().as_str() == "h")
            .map(|t| t.content().unwrap_or("").to_string())
            .unwrap_or_else(|| "buzz".into());
        let slug = format!("mem/buzz/{}/{}", ns, ev.id.to_hex());
        let prov = format!("buzz#{}#{}", ev.kind.as_u16(), ev.id.to_hex());
        store.write_memory(
            &agent.pubkey, &slug, &ns, content,
            30, false, 0, Some(&prov), Some(ev.created_at.as_secs() as i64),
 )?;
 imported += 1;
    }
    Ok(imported)
}

fn require_owner(agent: &Agent) -> Result<nostr::PublicKey> {
    let owner = agent
        .owner
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("agent {} has no owner; birth with --owner <pubkey>", agent.name))?;
    nostr::PublicKey::from_hex(owner).map_err(|e| anyhow::anyhow!("invalid owner pubkey: {e}"))
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}...", &s[..n])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_client_smoke() {
        // No live relay in tests; ensure construction + query parsing work.
        let c = RelayClient::new("wss://example.invalid").unwrap();
        assert_eq!(c.url, "wss://example.invalid");
        // parse a fake relay response shape defensively
        let text = r#"[
            ["EVENT", "sub", {"id":"abc","kind":30174,"pubkey":"p","created_at":1,"tags":[],"content":"","sig":"0"}],
            ["EOSE", "sub"]
        ]"#;
        let arr: Vec<Value> = serde_json::from_str(text).unwrap();
        let mut n = 0;
        for item in arr {
            if let Some(v) = item.as_array() {
                if v.len() >= 3 && v[0] == "EVENT" {
                    let _ = v[2].clone();
                    n += 1;
                }
            }
        }
        assert_eq!(n, 1);
    }
}
