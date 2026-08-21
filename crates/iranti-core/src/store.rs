//! Local memory store (SQLite). Source of truth for the mesh; relay sync is
//! an optional transport (see `bridge`). Private keys live in this store in
//! the prototype — production runs seal keys in TEE/HSM (see docs/security.md).

use anyhow::{bail, Context, Result};
use rand::RngCore;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

use crate::{now_secs, VERSION};

#[derive(Debug, Clone)]
pub struct Agent {
    pub pubkey: String,
    pub name: String,
    pub owner: Option<String>,
    pub nsec: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct MemRecord {
    pub slug: String,
    pub ns: String,
    pub value: String,
    pub salience: u8,
    pub pin: bool,
    pub ttl: i64,
    pub prov: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl MemRecord {
    /// TTL expiry: ttl>0 and now - created_at > ttl.
    pub fn expired(&self, now: i64) -> bool {
        self.ttl > 0 && (now - self.created_at) > self.ttl
    }
}

#[derive(Debug, Clone)]
pub struct GrantRow {
    pub granter: String,
    pub grantee: String,
    pub ns: String,
    pub created_at: i64,
}

pub struct Store {
    conn: Connection,
    #[allow(dead_code)]
    path: PathBuf,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("opening store {}", path.display()))?;
        let store = Store { conn, path: path.clone() };
        store.migrate()?;
        store.meta_set("store_path", &path.to_string_lossy())?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS agents(
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                sk TEXT NOT NULL,
                owner TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS memories(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                slug TEXT NOT NULL,
                ns TEXT NOT NULL DEFAULT 'general',
                value TEXT NOT NULL,
                salience INTEGER NOT NULL DEFAULT 50,
                pin INTEGER NOT NULL DEFAULT 0,
                ttl INTEGER NOT NULL DEFAULT 0,
                prov TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(agent, slug)
            );
            CREATE INDEX IF NOT EXISTS idx_memories_agent_ns ON memories(agent, ns);
            CREATE TABLE IF NOT EXISTS grants(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                granter TEXT NOT NULL,
                grantee TEXT NOT NULL,
                ns TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE(granter, grantee, ns)
            );
            CREATE TABLE IF NOT EXISTS digests(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                kind TEXT NOT NULL,
                period TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE(agent, kind, period)
            );
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            "#,
        )?;
        self.meta_set("schema_version", "1");
        self.meta_set("iranti_version", VERSION);
        Ok(())
    }

    // ---- meta ----

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let mut st = self
            .conn
            .prepare("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = st.query_map(params![key], |r| r.get::<_, String>(0))?;
        Ok(rows.next().transpose()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- agents ----

    /// Birth an agent identity (Nostr keypair). Returns the agent row.
    pub fn birth(&self, name: &str, owner: Option<&str>) -> Result<Agent> {
        let keys = nostr::Keys::generate();
        let pubkey = keys.public_key().to_hex();
        let sk = keys.secret_key().display_secret().to_string();
        let now = now_secs();
        self.conn.execute(
            "INSERT INTO agents(id,name,sk,owner,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![pubkey, name, sk, owner, now],
        )?;
        Ok(Agent {
            pubkey,
            name: name.to_string(),
            owner: owner.map(|s| s.to_string()),
            nsec: sk,
            created_at: now,
        })
    }

    /// Resolve an agent reference: exact pubkey, exact name, or pubkey prefix.
    pub fn resolve_agent(&self, ref_: &str) -> Result<Agent> {
        if let Some(a) = self.agent_by_pubkey(ref_)? {
            return Ok(a);
        }
        if let Some(a) = self.agent_by_name(ref_)? {
            return Ok(a);
        }
        // prefix match on pubkey
        let mut st = self
            .conn
            .prepare("SELECT id,name,sk,owner,created_at FROM agents WHERE id LIKE ?1 || '%'")?;
        let rows: Vec<Agent> = st
            .query_map(params![ref_], |r| {
                Ok(Agent {
                    pubkey: r.get(0)?,
                    name: r.get(1)?,
                    nsec: r.get(2)?,
                    owner: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        match rows.len() {
            0 => bail!("unknown agent: {ref_}"),
            1 => Ok(rows.into_iter().next().unwrap()),
            _ => bail!("ambiguous agent reference: {ref_} matches {} agents", rows.len()),
        }
    }

    pub fn agent_by_pubkey(&self, pubkey: &str) -> Result<Option<Agent>> {
        let mut st = self
            .conn
            .prepare("SELECT id,name,sk,owner,created_at FROM agents WHERE id = ?1")?;
        let mut rows = st.query_map(params![pubkey], |r| {
            Ok(Agent {
                pubkey: r.get(0)?,
                name: r.get(1)?,
                nsec: r.get(2)?,
                owner: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn agent_by_name(&self, name: &str) -> Result<Option<Agent>> {
        let mut st = self
            .conn
            .prepare("SELECT id,name,sk,owner,created_at FROM agents WHERE name = ?1")?;
        let mut rows = st.query_map(params![name], |r| {
            Ok(Agent {
                pubkey: r.get(0)?,
                name: r.get(1)?,
                nsec: r.get(2)?,
                owner: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_agents(&self) -> Result<Vec<Agent>> {
        let mut st = self
            .conn
            .prepare("SELECT id,name,sk,owner,created_at FROM agents ORDER BY created_at")?;
        let rows = st
            .query_map([], |r| {
                Ok(Agent {
                    pubkey: r.get(0)?,
                    name: r.get(1)?,
                    nsec: r.get(2)?,
                    owner: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Rebuild a `nostr::Keys` from a stored agent (its nsec).
    pub fn keys_for(&self, agent: &Agent) -> Result<nostr::Keys> {
        nostr::Keys::parse(&agent.nsec)
            .map_err(|e| anyhow::anyhow!("bad stored secret for {}: {e}", agent.name))
    }

    // ---- memories ----

    #[allow(clippy::too_many_arguments)]
    pub fn write_memory(
        &self,
        agent_pubkey: &str,
        slug: &str,
        ns: &str,
        value: &str,
        salience: u8,
        pin: bool,
        ttl: i64,
        prov: Option<&str>,
        created_at: Option<i64>,
    ) -> Result<MemRecord> {
        if salience > 100 {
            bail!("salience must be 0-100");
        }
        if !slug.starts_with("mem/") {
            bail!("slug must start with mem/");
        }
        let now = created_at.unwrap_or_else(now_secs);
        self.conn.execute(
            "INSERT INTO memories(agent,slug,ns,value,salience,pin,ttl,prov,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(agent,slug) DO UPDATE SET
               ns=excluded.ns, value=excluded.value, salience=excluded.salience,
               pin=excluded.pin, ttl=excluded.ttl, prov=excluded.prov, updated_at=excluded.updated_at",
            params![agent_pubkey, slug, ns, value, salience, pin as i64, ttl, prov, now, now],
        )?;
        self.mem_by_slug(agent_pubkey, slug)
    }

    pub fn new_slug(&self, ns: &str) -> String {
        let mut b = [0u8; 6];
        rand::thread_rng().fill_bytes(&mut b);
        format!("mem/{}/{}", ns.trim_matches('/'), hex::encode(b))
    }

    pub fn mem_by_slug(&self, agent_pubkey: &str, slug: &str) -> Result<MemRecord> {
        let mut st = self.conn.prepare(
            "SELECT slug,ns,value,salience,pin,ttl,prov,created_at,updated_at
             FROM memories WHERE agent=?1 AND slug=?2",
        )?;
        let mut rows = st.query_map(params![agent_pubkey, slug], row_mem)?;
        match rows.next().transpose()? {
            Some(m) => Ok(m),
            None => bail!("no memory {slug} for agent {agent_pubkey}"),
        }
    }

    pub fn list_memories(&self, agent_pubkey: &str, ns: Option<&str>) -> Result<Vec<MemRecord>> {
        let mut st = self.conn.prepare(
            "SELECT slug,ns,value,salience,pin,ttl,prov,created_at,updated_at
             FROM memories WHERE agent=?1 AND (?2 IS NULL OR ns=?2) ORDER BY updated_at DESC",
        )?;
        let rows = st
            .query_map(params![agent_pubkey, ns], row_mem)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn forget(&self, agent_pubkey: &str, slug: &str) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM memories WHERE agent=?1 AND slug=?2",
            params![agent_pubkey, slug],
        )?;
        Ok(n > 0)
    }

    /// All memories of an agent, including namespace list (for status/export).
    pub fn namespaces(&self, agent_pubkey: &str) -> Result<Vec<(String, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT ns, COUNT(*) FROM memories WHERE agent=?1 GROUP BY ns ORDER BY COUNT(*) DESC",
        )?;
        let rows = st
            .query_map(params![agent_pubkey], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    // ---- grants (A2A memory consent) ----

    pub fn grant(&self, granter: &str, grantee: &str, ns: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO grants(granter,grantee,ns,created_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(granter,grantee,ns) DO NOTHING",
            params![granter, grantee, ns, now_secs()],
        )?;
        Ok(())
    }

    pub fn revoke(&self, granter: &str, grantee: &str, ns: &str) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM grants WHERE granter=?1 AND grantee=?2 AND ns=?3",
            params![granter, grantee, ns],
        )?;
        Ok(n > 0)
    }

    pub fn list_grants(&self, agent_pubkey: &str) -> Result<Vec<GrantRow>> {
        let mut st = self.conn.prepare(
            "SELECT granter,grantee,ns,created_at FROM grants
             WHERE granter=?1 OR grantee=?1 ORDER BY created_at DESC",
        )?;
        let rows = st
            .query_map(params![agent_pubkey], |r| {
                Ok(GrantRow { granter: r.get(0)?, grantee: r.get(1)?, ns: r.get(2)?, created_at: r.get(3)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn has_grant(&self, granter: &str, grantee: &str, ns: &str) -> Result<bool> {
        let mut st = self.conn.prepare(
            "SELECT 1 FROM grants WHERE granter=?1 AND grantee=?2 AND ns=?3",
        )?;
        let mut rows = st.query_map(params![granter, grantee, ns], |_| Ok(()))?;
        Ok(rows.next().transpose()?.is_some())
    }

    // ---- digests (dream / echo) ----

    pub fn put_digest(&self, agent_pubkey: &str, kind: &str, period: &str, body: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO digests(agent,kind,period,body,created_at) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(agent,kind,period) DO UPDATE SET body=excluded.body, created_at=excluded.created_at",
            params![agent_pubkey, kind, period, body, now_secs()],
        )?;
        Ok(())
    }

    pub fn get_digest(&self, agent_pubkey: &str, kind: &str, period: &str) -> Result<Option<String>> {
        let mut st = self.conn.prepare(
            "SELECT body FROM digests WHERE agent=?1 AND kind=?2 AND period=?3",
        )?;
        let mut rows = st.query_map(params![agent_pubkey, kind, period], |r| r.get::<_, String>(0))?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_digests(&self, agent_pubkey: &str, kind: Option<&str>) -> Result<Vec<(String, String)>> {
        let mut st = self.conn.prepare(
            "SELECT period,body FROM digests WHERE agent=?1 AND (?2 IS NULL OR kind=?2) ORDER BY period",
        )?;
        let rows = st
            .query_map(params![agent_pubkey, kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
}

fn row_mem(r: &rusqlite::Row) -> rusqlite::Result<MemRecord> {
    Ok(MemRecord {
        slug: r.get(0)?,
        ns: r.get(1)?,
        value: r.get(2)?,
        salience: r.get::<_, i64>(3)? as u8,
        pin: r.get::<_, i64>(4)? != 0,
        ttl: r.get(5)?,
        prov: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}
