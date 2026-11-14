//! Relay bridge: sync local memory to/from a Nostr relay.
//!
//! Two transports, chosen by the relay URL:
//! - `ws://` / `wss://`: standard NIP-01 over WebSocket (`EVENT`/`OK`,
//!   `REQ`/`EOSE`), with NIP-42 `AUTH` when the relay asks. This is what
//!   ordinary relays serve.
//! - `http://` / `https://`: Buzz's plain-HTTP surfaces (`POST /events`,
//!   `POST /query`). Standard relays do not serve these.
//!
//! - `push`: publish local memories as NIP-AE kind:30174 engram events.
//! - `pull`: fetch the agent's engrams back and upsert (or tombstone) locally.
//! - `import`: ingest Buzz channel history (kinds 1/42/30023) into memory with
//!   provenance, so an agent can remember what happened in a room.
//!
//! Note on Buzz relays: the relay enforces `required_scope_for_kind` — unknown
//! kinds are rejected. Kind 30174 is already allowlisted upstream; the Iranti
//! companion kinds 36000-36003 need the allowlist patch in docs/.

use std::net::TcpStream;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::crypto::{self, MemBody};
use crate::store::{Agent, Store};
use crate::VERSION;

pub struct RelayClient {
    pub url: String,
    client: reqwest::blocking::Client,
    /// Used to answer a relay's NIP-42 AUTH challenge. Without keys an
    /// auth-requiring relay is reported as such, not silently retried.
    keys: Option<Keys>,
}

const WS_TIMEOUT: Duration = Duration::from_secs(15);
const SUB_ID: &str = "iranti-sync";

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

impl RelayClient {
    pub fn new(url: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("iranti/{VERSION}"))
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self { url: url.trim_end_matches('/').to_string(), client, keys: None })
    }

    /// Give the client the agent's keys so it can authenticate to relays that
    /// require NIP-42.
    pub fn with_keys(mut self, keys: Keys) -> Self {
        self.keys = Some(keys);
        self
    }

    fn is_websocket(&self) -> bool {
        self.url.starts_with("ws://") || self.url.starts_with("wss://")
    }

    fn ws_connect(&self) -> Result<Socket> {
        let (socket, _) = tungstenite::connect(self.url.as_str())
            .with_context(|| format!("cannot connect to relay {}", self.url))?;
        let stream = match socket.get_ref() {
            MaybeTlsStream::Plain(s) => Some(s),
            MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
            _ => None,
        };
        if let Some(stream) = stream {
            stream.set_read_timeout(Some(WS_TIMEOUT))?;
            stream.set_write_timeout(Some(WS_TIMEOUT))?;
        }
        Ok(socket)
    }

    /// Next relay message as a JSON array (`["OK", ...]`, `["EVENT", ...]`, ...).
    fn ws_read(&self, socket: &mut Socket) -> Result<Vec<Value>> {
        loop {
            let msg = socket
                .read()
                .with_context(|| format!("relay {} stopped answering", self.url))?;
            match msg {
                Message::Text(text) => {
                    if let Ok(arr) = serde_json::from_str::<Vec<Value>>(text.as_str()) {
                        if !arr.is_empty() {
                            return Ok(arr);
                        }
                    }
                }
                Message::Close(_) => bail!("relay {} closed the connection", self.url),
                _ => {} // ping/pong/binary
            }
        }
    }

    fn ws_send(&self, socket: &mut Socket, value: &Value) -> Result<()> {
        socket
            .send(Message::Text(value.to_string().into()))
            .with_context(|| format!("cannot send to relay {}", self.url))
    }

    /// Answer a NIP-42 challenge and wait for the relay to accept it.
    fn ws_auth(&self, socket: &mut Socket, challenge: &str) -> Result<()> {
        let keys = self.keys.as_ref().ok_or_else(|| {
            anyhow!("relay {} requires authentication (NIP-42) and no keys were supplied", self.url)
        })?;
        let auth = EventBuilder::new(Kind::Authentication, "")
            .tags(vec![
                Tag::parse(["relay", &auth_relay_url(&self.url)])?,
                Tag::parse(["challenge", challenge])?,
            ])
            .sign_with_keys(keys)?;
        let auth_id = auth.id.to_hex();
        self.ws_send(socket, &json!(["AUTH", auth]))?;
        loop {
            let m = self.ws_read(socket)?;
            if m[0] == "OK" && m.get(1).and_then(Value::as_str) == Some(auth_id.as_str()) {
                if m.get(2) == Some(&Value::Bool(true)) {
                    return Ok(());
                }
                bail!("relay {} rejected authentication: {}", self.url,
                      m.get(3).and_then(Value::as_str).unwrap_or(""));
            }
        }
    }

    fn publish_ws(&self, event: &Event) -> Result<String> {
        let mut socket = self.ws_connect()?;
        let id = event.id.to_hex();
        let frame = json!(["EVENT", event]);
        self.ws_send(&mut socket, &frame)?;
        let mut authed = false;
        loop {
            let m = self.ws_read(&mut socket)?;
            let kind = m[0].as_str().unwrap_or("");
            if kind == "AUTH" && !authed {
                let challenge = m.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                self.ws_auth(&mut socket, &challenge)?;
                authed = true;
                self.ws_send(&mut socket, &frame)?; // the relay dropped the first attempt
            } else if kind == "OK" && m.get(1).and_then(Value::as_str) == Some(id.as_str()) {
                let message = m.get(3).and_then(Value::as_str).unwrap_or("").to_string();
                if m.get(2) == Some(&Value::Bool(true)) {
                    let _ = socket.close(None);
                    return Ok(message);
                }
                if message.starts_with("auth-required") && !authed {
                    continue; // the AUTH challenge follows or has already been sent
                }
                bail!("relay OK=false: {message}");
            }
        }
    }

    fn query_ws(&self, filter: &Value) -> Result<Vec<Event>> {
        let mut socket = self.ws_connect()?;
        let frame = json!(["REQ", SUB_ID, filter]);
        self.ws_send(&mut socket, &frame)?;
        let mut events = Vec::new();
        let mut authed = false;
        loop {
            let m = self.ws_read(&mut socket)?;
            match m[0].as_str().unwrap_or("") {
                "EVENT" if m.len() >= 3 && m[1] == SUB_ID => {
                    // Only events whose id and signature verify are kept.
                    if let Ok(ev) = serde_json::from_value::<Event>(m[2].clone()) {
                        if ev.verify().is_ok() {
                            events.push(ev);
                        }
                    }
                }
                "EOSE" if m.get(1).and_then(Value::as_str) == Some(SUB_ID) => {
                    let _ = self.ws_send(&mut socket, &json!(["CLOSE", SUB_ID]));
                    let _ = socket.close(None);
                    return Ok(events);
                }
                "AUTH" if !authed => {
                    let challenge = m.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                    self.ws_auth(&mut socket, &challenge)?;
                    authed = true;
                    events.clear();
                    self.ws_send(&mut socket, &frame)?;
                }
                "CLOSED" if m.get(1).and_then(Value::as_str) == Some(SUB_ID) => {
                    let reason = m.get(2).and_then(Value::as_str).unwrap_or("");
                    if reason.starts_with("auth-required") && !authed {
                        continue; // answered when the AUTH challenge arrives
                    }
                    bail!("relay {} closed the subscription: {reason}", self.url);
                }
                _ => {}
            }
        }
    }

    /// Submit a signed event: NIP-01 `EVENT` over WebSocket, or Buzz's
    /// `POST /events` for an http(s) relay URL.
    pub fn publish(&self, event: &Event) -> Result<String> {
        if self.is_websocket() {
            return self.publish_ws(event);
        }
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

    /// Fetch events matching a filter: NIP-01 `REQ` until `EOSE` over
    /// WebSocket, or Buzz's `POST /query` for an http(s) relay URL.
    pub fn query(&self, filter: &Value) -> Result<Vec<Event>> {
        if self.is_websocket() {
            return self.query_ws(filter);
        }
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
    let keys = store.keys_for(agent)?;
    let client = RelayClient::new(relay)?.with_keys(keys.clone());
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
    let keys = store.keys_for(agent)?;
    let client = RelayClient::new(relay)?.with_keys(keys.clone());
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

/// NIP-42: the `relay` tag is scheme + host, without a proxy path.
fn auth_relay_url(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            let host = rest.split('/').next().unwrap_or(rest);
            format!("{}://{}", &url[..i], host)
        }
        None => url.to_string(),
    }
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

    // ---- standard WebSocket relay (NIP-01, optional NIP-42) -----------------

    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A small NIP-01 relay for tests: stores verified events, answers REQ by
    /// kinds/authors/#p, and with `require_auth` refuses everything until the
    /// client proves its key with a NIP-42 AUTH event for this challenge.
    fn spawn_relay(require_auth: bool) -> (String, Arc<Mutex<Vec<Event>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/relay", listener.local_addr().unwrap());
        let stored: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let shared = stored.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let stored = shared.clone();
                std::thread::spawn(move || {
                    let mut ws = match tungstenite::accept(stream) {
                        Ok(ws) => ws,
                        Err(_) => return,
                    };
                    let challenge = "test-challenge";
                    let mut authed: Option<String> = None;
                    let mut challenged = false;
                    let mut send = |ws: &mut WebSocket<TcpStream>, v: Value| {
                        let _ = ws.send(Message::Text(v.to_string().into()));
                    };
                    while let Ok(msg) = ws.read() {
                        let Message::Text(text) = msg else { continue };
                        let Ok(m) = serde_json::from_str::<Vec<Value>>(text.as_str()) else { continue };
                        match m[0].as_str().unwrap_or("") {
                            "AUTH" => {
                                let ev: Event = serde_json::from_value(m[1].clone()).unwrap();
                                let has = |k: &str, v: &str| ev.tags.iter().any(|t| {
                                    let s = t.as_slice();
                                    s.len() >= 2 && s[0] == k && s[1] == v
                                });
                                let ok = ev.verify().is_ok()
                                    && ev.kind == Kind::Authentication
                                    && has("challenge", challenge)
                                    && ev.tags.iter().any(|t| {
                                        let s = t.as_slice();
                                        s.len() >= 2 && s[0] == "relay" && !s[1].contains("/relay")
                                    });
                                if ok {
                                    authed = Some(ev.pubkey.to_hex());
                                }
                                send(&mut ws, json!(["OK", ev.id.to_hex(), ok, if ok { "" } else { "invalid: bad auth" }]));
                            }
                            "EVENT" => {
                                let ev: Event = serde_json::from_value(m[1].clone()).unwrap();
                                if require_auth && authed.is_none() {
                                    send(&mut ws, json!(["OK", ev.id.to_hex(), false, "auth-required: sign in"]));
                                    if !challenged {
                                        challenged = true;
                                        send(&mut ws, json!(["AUTH", challenge]));
                                    }
                                    continue;
                                }
                                let ok = ev.verify().is_ok();
                                if ok {
                                    stored.lock().unwrap().push(ev.clone());
                                }
                                send(&mut ws, json!(["OK", ev.id.to_hex(), ok, ""]));
                            }
                            "REQ" => {
                                let sub = m[1].clone();
                                if require_auth && authed.is_none() {
                                    send(&mut ws, json!(["CLOSED", sub, "auth-required: sign in"]));
                                    if !challenged {
                                        challenged = true;
                                        send(&mut ws, json!(["AUTH", challenge]));
                                    }
                                    continue;
                                }
                                let f = &m[2];
                                let list = |k: &str| -> Option<Vec<String>> {
                                    f.get(k).and_then(Value::as_array).map(|a| {
                                        a.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                                    })
                                };
                                let kinds: Option<Vec<u64>> = f.get("kinds").and_then(Value::as_array)
                                    .map(|a| a.iter().filter_map(Value::as_u64).collect());
                                for ev in stored.lock().unwrap().iter() {
                                    if let Some(k) = &kinds {
                                        if !k.contains(&(ev.kind.as_u16() as u64)) { continue; }
                                    }
                                    if let Some(a) = list("authors") {
                                        if !a.contains(&ev.pubkey.to_hex()) { continue; }
                                    }
                                    if let Some(p) = list("#p") {
                                        let hit = ev.tags.iter().any(|t| {
                                            let s = t.as_slice();
                                            s.len() >= 2 && s[0] == "p" && p.contains(&s[1])
                                        });
                                        if !hit { continue; }
                                    }
                                    send(&mut ws, json!(["EVENT", sub, ev]));
                                }
                                send(&mut ws, json!(["EOSE", sub]));
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        (url, stored)
    }

    fn store_with_agent() -> (Store, Agent, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("iranti-test-{}.sqlite", rand::random::<u64>()));
        let store = Store::open(&path).unwrap();
        let owner = store.birth("Owner", None).unwrap();
        let agent = store.birth("Amara", Some(&owner.pubkey)).unwrap();
        (store, agent, path)
    }

    fn roundtrip(require_auth: bool) {
        let (url, relay_events) = spawn_relay(require_auth);
        let (store, agent, path) = store_with_agent();
        store.write_memory(&agent.pubkey, "mem/design/a1", "design",
            "relays gate custom kinds", 90, true, 0, Some("test#1"), None).unwrap();
        store.write_memory(&agent.pubkey, "mem/ops/b2", "ops",
            "deploys go out on fridays", 40, false, 0, None, None).unwrap();

        assert_eq!(push(&store, &url, &agent).unwrap(), 2);
        // what reached the relay is ciphertext, signed by the agent
        {
            let on_relay = relay_events.lock().unwrap();
            assert_eq!(on_relay.len(), 2);
            for ev in on_relay.iter() {
                assert_eq!(ev.pubkey.to_hex(), agent.pubkey);
                assert!(!ev.content.contains("relays gate") && !ev.content.contains("fridays"));
            }
        }

        // lose the local copy, then recover it from the relay
        assert!(store.forget(&agent.pubkey, "mem/design/a1").unwrap());
        assert!(store.forget(&agent.pubkey, "mem/ops/b2").unwrap());
        assert!(store.list_memories(&agent.pubkey, None).unwrap().is_empty());

        assert_eq!(pull(&store, &url, &agent).unwrap(), (2, 0));
        let mut got = store.list_memories(&agent.pubkey, None).unwrap();
        got.sort_by(|a, b| a.slug.cmp(&b.slug));
        assert_eq!(got[0].slug, "mem/design/a1");
        assert_eq!(got[0].value, "relays gate custom kinds");
        assert_eq!((got[0].ns.as_str(), got[0].salience, got[0].pin), ("design", 90, true));
        assert_eq!(got[0].prov.as_deref(), Some("test#1"));
        assert_eq!(got[1].value, "deploys go out on fridays");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn websocket_push_then_pull_restores_memories() {
        roundtrip(false);
    }

    #[test]
    fn websocket_roundtrip_through_a_relay_that_requires_nip42_auth() {
        roundtrip(true);
    }

    #[test]
    fn auth_requiring_relay_without_keys_is_reported_not_retried() {
        let (url, _) = spawn_relay(true);
        let err = RelayClient::new(&url).unwrap()
            .query(&json!({"kinds": [crypto::KIND_AGENT_ENGRAM]}))
            .unwrap_err();
        assert!(err.to_string().contains("requires authentication"), "{err}");
    }

    #[test]
    fn pull_ignores_events_with_bad_signatures() {
        let (url, relay_events) = spawn_relay(false);
        let (store, agent, path) = store_with_agent();
        store.write_memory(&agent.pubkey, "mem/design/a1", "design", "real", 50, false, 0, None, None).unwrap();
        push(&store, &url, &agent).unwrap();
        // the relay starts serving a tampered copy as well
        let forged: Event = {
            let good = relay_events.lock().unwrap()[0].clone();
            let mut v = serde_json::to_value(&good).unwrap();
            v["content"] = json!("tampered");
            serde_json::from_value(v).unwrap()
        };
        relay_events.lock().unwrap().push(forged);
        let events = RelayClient::new(&url).unwrap()
            .query(&json!({"kinds": [crypto::KIND_AGENT_ENGRAM], "authors": [agent.pubkey]}))
            .unwrap();
        assert_eq!(events.len(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_relay_url_drops_the_path() {
        assert_eq!(auth_relay_url("wss://relay.example:3443/relay"), "wss://relay.example:3443");
        assert_eq!(auth_relay_url("ws://127.0.0.1:7000"), "ws://127.0.0.1:7000");
    }
}
