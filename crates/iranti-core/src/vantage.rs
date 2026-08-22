//! Vantage bridge: mirror local memories into a Vantage agent's memory vault
//! via its external-connector ingest endpoint (`POST /api/vault/external/iranti/ingest`).
//!
//! This is NOT a merge of the two memory systems — Iranti stays the source of
//! truth for the value; Vantage renders a read-only OKF note per (ns, slug)
//! carrying an `iranti://<pubkey>/<ns>/<slug>` resource URI back here. On
//! push, this module sets each memory's local `prov` to a `vantage://` URI
//! once Vantage confirms the note's path, so recall on either side can trace
//! back to the other.

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::store::{Agent, Store};

pub struct VantageClient {
    base_url: String,
    connector_token: String,
    client: reqwest::blocking::Client,
}

impl VantageClient {
    /// `base_url` is the Vantage backend root (e.g. https://vantage.example.com).
    /// `connector_token` is a `vconn_...` token from
    /// `POST /api/agents/{agent_name}/vault/external/connectors` with
    /// `source: "iranti"`, held by the Vantage agent this Iranti agent is
    /// bridging into.
    pub fn new(base_url: &str, connector_token: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("iranti/{}", crate::VERSION))
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            connector_token: connector_token.to_string(),
            client,
        })
    }

    /// Push one memory to Vantage. Returns the vault-relative note path
    /// Vantage rendered it to.
    pub fn push_memory(
        &self,
        agent_pubkey: &str,
        ns: &str,
        slug: &str,
        value: &str,
        salience: u8,
        pin: bool,
        prov: Option<&str>,
        updated_at: i64,
    ) -> Result<String> {
        let short_slug = slug.rsplit('/').next().unwrap_or(slug);
        let body = json!({
            "iranti_pubkey": agent_pubkey,
            "ns": ns,
            "slug": short_slug,
            "value": value,
            "salience": salience,
            "pin": pin,
            "prov": prov,
            "updated_at": updated_at,
        });
        let resp = self
            .client
            .post(format!("{}/api/vault/external/iranti/ingest", self.base_url))
            .header("X-Vault-Connector-Key", &self.connector_token)
            .json(&body)
            .send()
            .with_context(|| format!("POST {}/api/vault/external/iranti/ingest failed", self.base_url))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("vantage rejected iranti memory: HTTP {status}: {text}");
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("vantage returned non-JSON: {text}"))?;
        Ok(v.get("vault_path").and_then(|p| p.as_str()).unwrap_or_default().to_string())
    }
}

/// Push an agent's memories in a given namespace (or all namespaces if
/// `ns` is None) to a Vantage vault, recording the Vantage note path back
/// as each memory's local provenance.
pub fn push(store: &Store, client: &VantageClient, agent: &Agent, ns: Option<&str>) -> Result<usize> {
    let mems = store.list_memories(&agent.pubkey, ns)?;
    let mut pushed = 0usize;
    for m in mems {
        let vault_path = client.push_memory(
            &agent.pubkey, &m.ns, &m.slug, &m.value, m.salience, m.pin,
            m.prov.as_deref(), m.updated_at,
        )?;
        let prov = format!("vantage://{}/{}", agent.name, vault_path);
        store.write_memory(
            &agent.pubkey, &m.slug, &m.ns, &m.value,
            m.salience, m.pin, m.ttl, Some(&prov), Some(m.updated_at),
        )?;
        pushed += 1;
    }
    Ok(pushed)
}
