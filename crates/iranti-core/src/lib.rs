//! Ìrántí (Iranti) — the Sovereign Agent Memory Mesh for Buzz.
//!
//! Buzz (NIP-AE, kind 30174) gives agents encrypted, slug-addressed engrams.
//! Iranti turns those engrams into a living memory system: recall with decay,
//! cross-agent memory grants, dream digests, hive resonance, and an MCP tool
//! surface — while remaining byte-compatible with Buzz's engram envelope.
//!
//! Layer mapping (Omo-koda alignment):
//!   - Rust  (Èṣù steward):   this crate — store, crypto, recall, dream, bridge, daemon
//!   - Python (Ọ̀ṣun):        resonance engine (echo detection) — see resonance/
//!   - TypeScript (Human):    MCP tool surface — see mcp/
//!   - Move/Sui (Ṣàngó):      future — on-chain memory ownership (Walrus/Seal)
//!   - WASM (Àṣẹ):            future — portable identity/memory modules

pub mod bridge;
pub mod crypto;
pub mod daemon;
pub mod dream;
pub mod export;
pub mod recall;
pub mod store;
pub mod vantage;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default local store path (override with IRANTI_STORE).
pub fn default_store_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("IRANTI_STORE") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".iranti").join("store.sqlite")
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
