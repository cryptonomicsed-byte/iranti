# Ìrántí — the Sovereign Agent Memory Mesh for Buzz

Ìrántí (Yoruba: *memory / recollection*) is a companion system for
[Buzz](https://github.com/block/buzz) that turns Buzz's NIP-AE agent engrams
into a **living memory**: findable, shareable, forgettable, and
self-reflective.

Buzz stores encrypted engrams (kind 30174). Iranti gives agents what Buzz's
NIP-AE deliberately leaves to companion NIPs:

- **Recall** — "what did I learn about X?" with BM25 relevance × recency
  decay × salience/pin ranking (NIP-AE d-tags are opaque HMACs; there is no
  way to search them).
- **A2A memory grants** — agents consent to share namespaces with other
  agents; recall is enforced, not requested.
- **Dream cycle** — daily reflection digests (topics, drift, salience) so
  memory becomes wisdom instead of a log.
- **Hive resonance (echo)** — cross-agent fingerprint comparison that shows
  what the hive is converging on.
- **MCP tool surface** — every capability as `memory_*` tools, so any agent
  in a Buzz room can call the mesh directly.
- **Buzz bridge** — import channel history into memory with provenance; push
  and pull engrams over Buzz's HTTP surfaces (`POST /events`, `POST /query`).

It never rebuilds Buzz. It writes **byte-compatible NIP-AE engrams** and adds
four companion kinds (36000–36003) behind a three-line relay allowlist patch.

## Quickstart

```bash
# build (Rust core)
cargo build --release
export PATH="$PWD/target/release:$PATH"

# birth two agents
iranti birth Amara --owner <your-pubkey-hex>
iranti birth Kofi

# write memory (namespaced, with salience/pin/ttl)
iranti write Amara design "the buzz relay gates custom kinds in ingest.rs" --salience 90 --pin
iranti write Amara design "mcp servers expose tools to agents in buzz rooms"
iranti write Kofi ops "deploy matrix targets three production relays"

# recall (ranked, decayed, pin-boosted)
iranti recall Amara "relay kind gate" -k 3

# A2A consent + cross-agent recall
iranti grant Kofi Amara design
iranti a2a Kofi Amara design "relay gate"

# reflection + hive intelligence
iranti dream Amara
iranti echo Amara Kofi

# status / export
iranti status Amara
iranti export Amara --out amara.json

# run the A2A daemon (HTTP RPC for remote agents)
iranti daemon --port 38400
```

### MCP (agent tool surface)

```bash
cd mcp
npm install && npm run build
npm run smoke    # boots server, asserts 13 tools
npm run e2e      # real tool calls: birth→write→recall→grant→a2a→dream→status→export
node dist/index.js   # run the server (stdio)
```

Register in Buzz: add this server to your MCP server list; room agents can
then call `memory_recall`, `memory_grant`, `memory_dream`, `memory_echo`,
etc. directly (see docs/buzz-integration.md).

## Architecture (polyglot, Buzz-aligned)

| Layer | Language | Module | Role |
|-------|----------|--------|------|
| Steward (Èṣù) | Rust | `crates/iranti-core` | Store, NIP-AE crypto, recall engine, dream cycle, bridge, A2A daemon, CLI |
| Computation (Ọ̀ṣun) | Python | `resonance/` | TF-IDF fingerprints, echo detection, convergence |
| Human/tool surface | TypeScript | `mcp/` | MCP server: 13 `memory_*` tools |
| Economic/ownership (Ṣàngó) | Move/Sui | *roadmap* | Memory ownership, Walrus blobs, Seal policies |
| Portable power (Àṣẹ) | WASM | *roadmap* | Edge identity + memory modules |

Key design decisions:

- **NIP-AE byte-compat**: engrams are built with the exact envelope of
  `buzz-core::engram` (same `nostr` crate version 0.44, same `d`-tag HMAC
  domain, same NIP-44 v2 conversation key, same `p`-tag owner). Verified by
  tests in `crates/iranti-core/src/crypto.rs`.
- **Local-first, relay-optional**: the SQLite store is the source of truth;
  `bridge push/pull/import` syncs with any Buzz relay over HTTP (no WS
  client needed).
- **Everything is a tool**: CLI parity + MCP tools + A2A RPC — three surfaces,
  one core.
- **Consent by default**: no cross-agent recall without an explicit grant;
  private engrams stay NIP-44-encrypted end to end.

## Directory map

```
crates/iranti-core/   Rust core (store, crypto, recall, dream, bridge, daemon)
crates/iranti-cli/    the `iranti` CLI
resonance/            Python resonance engine (stdlib only)
mcp/                  TypeScript MCP server + smoke/e2e tests
docs/nip-ir.md        the companion NIP spec
docs/buzz-integration.md  Buzz allowlist patch + MCP registration
docs/architecture.md  deeper architecture notes
docs/security.md      threat model + production hardening path
docs/buzz-iranti-allowlist.patch  applied to block/buzz (kind.rs + ingest.rs)
```

## Roadmap (agent-native, self-improving)

1. **Dream cron** — scheduled dream cycles per agent (`iranti daemon` poller
   or cron) publishing kind 36002.
2. **Recall index publishing** — kind 36000 manifests on the relay so recall
   services can index without content.
3. **Sui/Walrus + Seal** — encrypted private memory pinned on Walrus,
   ownership objects on Sui (Ṣàngó layer), TEE processing (Nautilus).
4. **WASM modules** — portable identity/memory for edge agents (Àṣẹ layer).
5. **Embeddings backend** — swap TF-IDF for vector recall behind the same
   interface.
6. **Buzz ACP surface** — agent descriptor so Buzz's ACP harness discovers
   the mesh as a tool-providing peer.
