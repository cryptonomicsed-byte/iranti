//! A2A daemon: exposes the memory mesh as an HTTP RPC so agents inside (or
//! beside) Buzz can query each other's shared memory without a shared store.
//!
//! Endpoints:
//!   GET  /health        -> {"status":"ok","service":"iranti","version":...}
//!   POST /rpc           -> { "method": "a2a_recall", "params": {...} }
//!                          header X-Iranti-Agent: <requester pubkey>
//!
//! `a2a_recall` enforces memory grants: the requester may only see namespaces
//! the owner explicitly granted. Prototype auth is a self-declared header;
//! production requires NIP-98 HTTP auth (signed event) — see docs/security.md.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use crate::recall;
use crate::store::Store;
use crate::VERSION;

pub fn serve(store: Store, port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)?;
    eprintln!("iranti daemon listening on http://{addr} (pid {})", std::process::id());
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let store = store.clone_handle();
                std::thread::spawn(move || {
                    if let Err(e) = handle(s, &store) {
                        eprintln!("iranti daemon: {e}");
                    }
                });
            }
            Err(e) => eprintln!("iranti daemon accept error: {e}"),
        }
    }
    Ok(())
}

impl Store {
    /// Cheap clone for thread dispatch: reopens the same DB file.
    pub fn clone_handle(&self) -> Store {
        // We stash the path; reopen lazily is overkill — clone the connection
        // by reopening the file.
        let path = self
            .meta_get("store_path")
            .ok()
            .flatten()
            .unwrap_or_else(|| crate::default_store_path().to_string_lossy().to_string());
        Store::open(path).expect("reopen store for daemon thread")
    }
}

fn handle(mut stream: TcpStream, store: &Store) -> Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 1_048_576 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let mut content_length = 0usize;
    let mut requester: Option<String> = None;
    for line in lines {
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            content_length = line.split(':').nth(1).unwrap_or("0").trim().parse().unwrap_or(0);
        }
        if lower.starts_with("x-iranti-agent:") {
            requester = Some(line.split(':').nth(1).unwrap_or("").trim().to_string());
        }
    }

    let body = if content_length > 0 {
        // We read headers + (possibly part of) body; take the tail.
        let header_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[header_end..]).to_string()
    } else {
        String::new()
    };

    let resp = match (method, path) {
        ("GET", "/health") => respond_ok(json!({"status":"ok","service":"iranti","version":VERSION})),
        ("POST", "/rpc") => {
            let req: Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(_) => return respond(&mut stream, 400, json!({"error":"bad json"})),
            };
            let method_name = req["method"].as_str().unwrap_or("");
            let params = req["params"].clone();
            match method_name {
                "a2a_recall" => {
                    let grant_ok = check_grant(store, &params, requester.as_deref());
                    if !grant_ok {
                        return respond(&mut stream, 403, json!({"error":"grant required","hint":"owner must `iranti grant <grantee> <owner> <ns>`"}));
                    }
                    match a2a_recall(store, &params) {
                        Ok(hits) => respond_ok(json!({"hits": hits})),
                        Err(e) => respond_err(&e.to_string()),
                    }
                }
                "status" => match status(store, &params) {
                    Ok(v) => respond_ok(v),
                    Err(e) => respond_err(&e.to_string()),
                },
                _ => respond_err(&format!("unknown method {method_name}")),
            }
        }
        _ => respond_err(&format!("not found: {method} {path}")),
    };
    respond(&mut stream, 200, resp)
}

fn respond(stream: &mut TcpStream, code: u16, body: Value) -> Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let payload = body.to_string();
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn respond_ok(v: Value) -> Value {
    v
}
fn respond_err(msg: &str) -> Value {
    json!({"error": msg})
}

fn check_grant(store: &Store, params: &Value, requester: Option<&str>) -> bool {
    let Some(rq) = requester else { return false };
    let owner = params["owner"].as_str().unwrap_or("");
    let ns = params["ns"].as_str().unwrap_or("");
    if owner.is_empty() || ns.is_empty() {
        return false;
    }
    // requester == owner? owners always pass; otherwise grant must exist.
    if rq == owner {
        return true;
    }
    store.has_grant(owner, rq, ns).unwrap_or(false)
}

fn a2a_recall(store: &Store, params: &Value) -> Result<Value> {
    let owner = params["owner"].as_str().ok_or_else(|| anyhow::anyhow!("missing owner"))?;
    let query = params["query"].as_str().unwrap_or("");
    let k = params["k"].as_u64().unwrap_or(3) as usize;
    let ns = params["ns"].as_str();
    let hits = recall::recall(store, owner, query, k, ns)?;
    Ok(json!(hits
        .into_iter()
        .map(|h| json!({"slug": h.slug, "ns": h.ns, "value": h.value, "score": h.score, "created_at": h.created_at}))
        .collect::<Vec<_>>()))
}

fn status(store: &Store, params: &Value) -> Result<Value> {
    let agent = params["agent"].as_str().ok_or_else(|| anyhow::anyhow!("missing agent"))?;
    let a = store.resolve_agent(agent)?;
    let mems = store.list_memories(&a.pubkey, None)?;
    Ok(json!({
        "name": a.name,
        "pubkey": a.pubkey,
        "memories": mems.len(),
        "grants": store.list_grants(&a.pubkey)?.len(),
    }))
}
