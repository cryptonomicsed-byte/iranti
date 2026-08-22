//! iranti — the agent-first command surface for the Sovereign Agent Memory Mesh.
//!
//! Every capability here is also exposed as an MCP tool (see mcp/) so agents
//! inside Buzz rooms can call the mesh directly. JSON output (`--json`) is the
//! machine contract for the MCP layer and for agent pipelines.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use iranti_core::bridge;
use iranti_core::daemon;
use iranti_core::dream;
use iranti_core::export;
use iranti_core::recall;
use iranti_core::store::{Agent, Store};
use iranti_core::{default_store_path, VERSION};
use nostr::nips::nip19::ToBech32;
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "iranti", version = VERSION, about = "Ìrántí — the sovereign agent memory mesh for Buzz (NIP-AE companion)")]
struct Cli {
    /// Path to the local store (default: ~/.iranti/store.sqlite)
    #[arg(long, global = true, env = "IRANTI_STORE")]
    store: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Birth a new agent identity (Nostr keypair + NIP-AE owner binding).
    Birth {
        name: String,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Agents on this mesh.
    Agents {
        #[arg(long)]
        json: bool,
    },
    /// Write a memory entry (namespace-scoped).
    Write {
        agent: String,
        namespace: String,
        text: String,
        #[arg(long)]
        pin: bool,
        #[arg(long)]
        salience: Option<u8>,
        #[arg(long = "ttl-days")]
        ttl_days: Option<i64>,
        #[arg(long)]
        prov: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read one memory by slug.
    Read {
        agent: String,
        slug: String,
        #[arg(long)]
        json: bool,
    },
    /// List memories (optionally by namespace).
    List {
        agent: String,
        #[arg(long)]
        ns: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Recall: BM25 x recency-decay x salience/pin ranking.
    Recall {
        agent: String,
        query: String,
        #[arg(short = 'k', default_value_t = 3)]
        k: usize,
        #[arg(long)]
        ns: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Forget a memory (local tombstone; `sync push` emits the NIP-AE head).
    Forget {
        agent: String,
        slug: String,
    },
    /// Grant an agent read access to one of your namespaces (A2A consent).
    Grant {
        grantee: String,
        granter: String,
        ns: String,
    },
    /// Revoke an A2A namespace grant.
    Revoke {
        grantee: String,
        granter: String,
        ns: String,
    },
    /// List grants touching an agent.
    Grants {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Cross-agent recall: query another agent's namespace (grant-checked).
    A2a {
        requester: String,
        owner: String,
        ns: String,
        query: String,
        #[arg(short = 'k', default_value_t = 3)]
        k: usize,
        #[arg(long)]
        json: bool,
    },
    /// Dream cycle: distill memory into a daily digest (reflection).
    Dream {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Hive resonance: cross-agent echo detection (Python).
    Echo {
        agents: Vec<String>,
        #[arg(long)]
        top: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Status: memory counts, namespaces, grants, last dream.
    Status {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Export an agent's plaintext snapshot (input for the resonance engine).
    Export {
        agent: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Export every agent (hive snapshot).
    ExportAll {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Buzz bridge: import channel history / sync engrams via relay HTTP.
    Bridge {
        #[command(subcommand)]
        sub: BridgeCmd,
    },
    /// Run the A2A daemon (HTTP RPC for cross-agent recall).
    Daemon {
        #[arg(long, default_value_t = 38400)]
        port: u16,
    },
    /// Vantage bridge: mirror memories into a Vantage agent's memory vault
    /// via its external-connector ingest endpoint. Iranti stays the source
    /// of truth; each pushed memory's `prov` is set to the resulting
    /// vantage:// note path.
    VantagePush {
        #[arg(long)]
        agent: String,
        /// Vantage backend base URL, e.g. https://vantage.example.com
        #[arg(long)]
        vantage_url: String,
        /// vconn_... connector token from
        /// POST /api/agents/{name}/vault/external/connectors (source=iranti)
        #[arg(long)]
        token: String,
        /// Only push this namespace; omit to push all namespaces.
        #[arg(long)]
        ns: Option<String>,
    },
}

#[derive(Subcommand)]
enum BridgeCmd {
    /// Import Buzz channel history (kinds 1/42/30023...) into an agent's memory.
    Import {
        relay: String,
        #[arg(long)]
        agent: String,
        #[arg(long, default_value = "1,42,30023")]
        kinds: String,
        #[arg(long)]
        channel: Option<String>,
    },
    /// Push local memories to the relay as NIP-AE kind:30174 engrams.
    Push {
        relay: String,
        #[arg(long)]
        agent: String,
    },
    /// Pull the agent's engrams back from the relay and reconcile.
    Pull {
        relay: String,
        #[arg(long)]
        agent: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store_path = cli.store.clone().unwrap_or_else(default_store_path);
    let store = Store::open(&store_path)?;

    match cli.cmd {
        Cmd::Birth { name, owner, json } => {
            let a = store.birth(&name, owner.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&agent_json(&a))?);
            } else {
                println!("born {name}");
                println!("  pubkey : {}", a.pubkey);
                println!("  npub   : {}", npub(&a.pubkey)?);
                println!("  owner  : {}", a.owner.as_deref().unwrap_or("(none — set with --owner at birth)"));
                println!("  store  : {}", store_path.display());
            }
        }
        Cmd::Agents { json } => {
            let agents = store.list_agents()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&agents.iter().map(agent_json).collect::<Vec<_>>())?);
            } else {
                for a in agents {
                    println!("{}  {}  memories: {}", a.name, a.pubkey, store.list_memories(&a.pubkey, None)?.len());
                }
            }
        }
        Cmd::Write { agent, namespace, text, pin, salience, ttl_days, prov, json } => {
            let a = store.resolve_agent(&agent)?;
            let slug = store.new_slug(&namespace);
            let ttl = ttl_days.map(|d| d * 86400).unwrap_or(0);
            let m = store.write_memory(
                &a.pubkey, &slug, &namespace, &text,
                salience.unwrap_or(50), pin, ttl, prov.as_deref(), None,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&mem_json(&m))?);
            } else {
                println!("stored {}", m.slug);
                println!("  ns        : {}", m.ns);
                println!("  salience  : {}", m.salience);
                println!("  pin       : {}", m.pin);
                if m.ttl > 0 {
                    println!("  ttl       : {}s", m.ttl);
                }
            }
        }
        Cmd::Read { agent, slug, json } => {
            let a = store.resolve_agent(&agent)?;
            let m = store.mem_by_slug(&a.pubkey, &slug)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&mem_json(&m))?);
            } else {
                println!("[{}] {}", m.ns, m.value);
                println!("slug: {}  salience: {}  pin: {}", m.slug, m.salience, m.pin);
            }
        }
        Cmd::List { agent, ns, json } => {
            let a = store.resolve_agent(&agent)?;
            let mems = store.list_memories(&a.pubkey, ns.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&mems.iter().map(mem_json).collect::<Vec<_>>())?);
            } else {
                for m in mems {
                    println!("{}  [{}]  {}", m.slug, m.ns, one_line(&m.value));
                }
            }
        }
        Cmd::Recall { agent, query, k, ns, json } => {
            let a = store.resolve_agent(&agent)?;
            let hits = recall::recall(&store, &a.pubkey, &query, k, ns.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits.iter().map(hit_json).collect::<Vec<_>>())?);
            } else {
                if hits.is_empty() {
                    println!("no recall hits for {query:?}");
                }
                for h in hits {
                    println!("[{:6.3}] {} [{}] {}", h.score, h.ns, h.slug, one_line(&h.value));
                }
            }
        }
        Cmd::Forget { agent, slug } => {
            let a = store.resolve_agent(&agent)?;
            if store.forget(&a.pubkey, &slug)? {
                println!("forgotten {slug}");
            } else {
                println!("no memory {slug} for {}", a.name);
            }
        }
        Cmd::Grant { grantee, granter, ns } => {
            let g1 = store.resolve_agent(&grantee)?;
            let g2 = store.resolve_agent(&granter)?;
            store.grant(&g2.pubkey, &g1.pubkey, &ns)?;
            println!("granted {} read access to {} / {}", g1.name, g2.name, ns);
        }
        Cmd::Revoke { grantee, granter, ns } => {
            let g1 = store.resolve_agent(&grantee)?;
            let g2 = store.resolve_agent(&granter)?;
            if store.revoke(&g2.pubkey, &g1.pubkey, &ns)? {
                println!("revoked {} access to {} / {}", g1.name, g2.name, ns);
            } else {
                println!("no such grant");
            }
        }
        Cmd::Grants { agent, json } => {
            let a = store.resolve_agent(&agent)?;
            let grants = store.list_grants(&a.pubkey)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&grants.iter().map(|g| json!({"granter": g.granter, "grantee": g.grantee, "ns": g.ns, "created_at": g.created_at})).collect::<Vec<_>>())?);
            } else {
                for g in grants {
                    println!("{} -> {} / {}", short(&g.granter), short(&g.grantee), g.ns);
                }
            }
        }
        Cmd::A2a { requester, owner, ns, query, k, json } => {
            let r = store.resolve_agent(&requester)?;
            let o = store.resolve_agent(&owner)?;
            if !store.has_grant(&o.pubkey, &r.pubkey, &ns)? {
                bail!("grant required: {} has no read access to {} / {}", r.name, o.name, ns);
            }
            let hits = recall::recall(&store, &o.pubkey, &query, k, Some(&ns))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits.iter().map(hit_json).collect::<Vec<_>>())?);
            } else {
                for h in hits {
                    println!("[{:6.3}] {} [{}] {}", h.score, h.ns, h.slug, one_line(&h.value));
                }
            }
        }
        Cmd::Dream { agent, json } => {
            let a = store.resolve_agent(&agent)?;
            let d = dream::dream(&store, &a.pubkey)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&d)?);
            } else {
                println!("dream digest for {} / {}", a.name, d["period"].as_str().unwrap_or("?"));
                println!("  memories     : {}", d["total"]);
                println!("  expired      : {}", d["expired_this_cycle"]);
                println!("  pinned       : {}", d["pinned"]);
                println!("  namespaces   : {}", d["namespaces"]);
                println!("  top terms    : {}", d["top_terms"]);
                if let Some(v) = d["drift"]["term_overlap"].as_f64() {
                    println!("  drift overlap: {:.2}", v);
                }
            }
        }
        Cmd::Echo { agents, top, json } => {
            run_echo(&store, &agents, top, json)?;
        }
        Cmd::Status { agent, json } => {
            let a = store.resolve_agent(&agent)?;
            let mems = store.list_memories(&a.pubkey, None)?;
            let nss = store.namespaces(&a.pubkey)?;
            let grants = store.list_grants(&a.pubkey)?;
            let dream_period = store.list_digests(&a.pubkey, Some("dream"))?.last().map(|(p, _)| p.clone());
            if json {
                println!("{}", serde_json::to_string_pretty(&json!({
                    "name": a.name, "pubkey": a.pubkey,
                    "memories": mems.len(),
                    "namespaces": nss.iter().map(|(n, c)| json!({"ns": n, "count": c})).collect::<Vec<_>>(),
                    "grants": grants.len(),
                    "last_dream": dream_period,
                }))?);
            } else {
                println!("{}  ({})", a.name, short(&a.pubkey));
                println!("  memories : {}", mems.len());
                for (n, c) in &nss {
                    println!("    {n}: {c}");
                }
                println!("  grants   : {}", grants.len());
                println!("  last dream: {}", dream_period.as_deref().unwrap_or("never"));
            }
        }
        Cmd::Export { agent, out } => {
            let a = store.resolve_agent(&agent)?;
            let v = export::export_agent(&store, &a)?;
            write_or_print(out, &serde_json::to_string_pretty(&v)?)?;
        }
        Cmd::ExportAll { out } => {
            let v = export::export_all(&store)?;
            write_or_print(out, &serde_json::to_string_pretty(&v)?)?;
        }
        Cmd::Bridge { sub } => match sub {
            BridgeCmd::Import { relay, agent, kinds, channel } => {
                let a = store.resolve_agent(&agent)?;
                let kinds: Vec<u32> = kinds
                    .split(',')
                    .map(|s| s.trim().parse::<u32>())
                    .collect::<Result<_, _>>()?;
                let n = bridge::import_from_buzz(&store, &relay, &a, &kinds, channel.as_deref())?;
                println!("imported {n} Buzz events into {}'s memory", a.name);
            }
            BridgeCmd::Push { relay, agent } => {
                let a = store.resolve_agent(&agent)?;
                let n = bridge::push(&store, &relay, &a)?;
                println!("pushed {n} engram events to {relay}");
            }
            BridgeCmd::Pull { relay, agent } => {
                let a = store.resolve_agent(&agent)?;
                let (up, tomb) = bridge::pull(&store, &relay, &a)?;
                println!("pulled {up} engrams (tombstones: {tomb}) from {relay}");
            }
        },
        Cmd::VantagePush { agent, vantage_url, token, ns } => {
            let a = store.resolve_agent(&agent)?;
            let client = iranti_core::vantage::VantageClient::new(&vantage_url, &token)?;
            let n = iranti_core::vantage::push(&store, &client, &a, ns.as_deref())?;
            println!("pushed {n} memories to Vantage vault at {vantage_url}");
        }
        Cmd::Daemon { port } => {
            daemon::serve(store, port)?;
        }
    }
    Ok(())
}

// ---- helpers ----

fn run_echo(store: &Store, agents: &[String], top: Option<usize>, json: bool) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("iranti-echo-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let targets: Vec<Agent> = if agents.is_empty() {
        store.list_agents()?
    } else {
        agents.iter().map(|r| store.resolve_agent(r)).collect::<Result<Vec<_>>>()?
    };
    if targets.len() < 2 {
        bail!("echo needs at least 2 agents (have {})", targets.len());
    }
    for a in &targets {
        let v = export::export_agent(store, a)?;
        let name = a.name.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
        std::fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(&v)?)?;
    }
    // locate the resonance engine
    let script = std::env::var("IRANTI_RESONANCE").unwrap_or_else(|_| {
        let base = env!("CARGO_MANIFEST_DIR"); // .../iranti/crates/iranti-cli
        format!("{base}/../../resonance/iranti_resonance.py")
    });
    if !std::path::Path::new(&script).exists() {
        bail!("resonance engine not found at {script} (set IRANTI_RESONANCE)");
    }
    let mut cmd = Command::new("python3");
    cmd.arg(&script).arg(dir.to_string_lossy().to_string());
    if let Some(t) = top {
        cmd.arg("--top").arg(t.to_string());
    }
    let out = cmd.output()?;
    if !out.status.success() {
        bail!("resonance engine failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if json {
        // already JSON from the engine; pass through
        println!("{stdout}");
    } else {
        // pretty-print the JSON report
        if let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) {
            println!("{}", serde_json::to_string_pretty(&v)?);
        } else {
            println!("{stdout}");
        }
    }
    Ok(())
}

fn write_or_print(out: Option<PathBuf>, contents: &str) -> Result<()> {
    match out {
        Some(p) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&p, contents)?;
            println!("wrote {}", p.display());
            Ok(())
        }
        None => {
            println!("{contents}");
            Ok(())
        }
    }
}

fn agent_json(a: &Agent) -> Value {
    json!({ "name": a.name, "pubkey": a.pubkey, "owner": a.owner, "created_at": a.created_at })
}

fn mem_json(m: &iranti_core::store::MemRecord) -> Value {
    json!({
        "slug": m.slug, "ns": m.ns, "value": m.value, "salience": m.salience,
        "pin": m.pin, "ttl": m.ttl, "prov": m.prov, "created_at": m.created_at,
        "updated_at": m.updated_at,
    })
}

fn hit_json(h: &recall::RecallHit) -> Value {
    json!({
        "slug": h.slug, "ns": h.ns, "value": h.value, "score": h.score,
        "salience": h.salience, "pin": h.pin, "created_at": h.created_at,
    })
}

fn npub(pubkey_hex: &str) -> Result<String> {
    let pk = nostr::PublicKey::from_hex(pubkey_hex)?;
    Ok(pk.to_bech32()?)
}

fn short(s: &str) -> String {
    if s.len() <= 12 {
        s.to_string()
    } else {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    }
}

fn one_line(s: &str) -> String {
    let t = s.replace('\n', " ");
    if t.len() > 120 {
        format!("{}…", &t[..120])
    } else {
        t
    }
}
