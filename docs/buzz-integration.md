# Buzz integration guide

Ìrántí interoperates with Buzz in three layers: **byte-compatible engrams**
(zero relay changes), **four companion kinds** (three-line allowlist patch),
and **tool-surface integration** (MCP registration for room agents). This
document is grounded in `block/buzz` at the commit cloned during the build.

## 1. What already works with zero changes

Kind 30174 (NIP-AE) is already in the Buzz relay's `required_scope_for_kind`
allowlist (`crates/buzz-relay/src/handlers/ingest.rs`, `UsersWrite` scope).
Events produced by `iranti bridge push`:

- carry the exact NIP-AE envelope: one `d` tag = 64-hex HMAC over
  `"agent-memory/v1/d-tag" || 0x00 || slug` with the NIP-44 v2 conversation
  key; one `p` tag = owner; NIP-44 v2 ciphertext content;
- parse with `buzz mem` (buzz-cli) and `buzz-acp`'s core injection, because
  the body JSON keeps `{"slug":…,"value":…}` byte-identical to NIP-AE and
  appends only optional fields (`ns`, `salience`, `pin`, `ttl`, `prov`) which
  NIP-AE's unknown-fields rule permits;
- respect NIP-AE writing rules: monotonic `created_at`, ≤ 65 535-byte bodies,
  tombstones on forget.

Verified by `crates/iranti-core/src/crypto.rs` tests (roundtrip, d-tag
derivation, owner-side decryption) mirroring `buzz-core::engram`.

## 2. Companion kinds 36000–36003

The relay **rejects unknown kinds** (`required_scope_for_kind` returns `Err`).
To let agents publish recall indexes, grants, dream digests, and echo
reports, apply `docs/buzz-iranti-allowlist.patch` (generated from a real
`git diff` against block/buzz):

```bash
cd /path/to/buzz   # your relay checkout
git apply ../iranti/docs/buzz-iranti-allowlist.patch
cargo check -p buzz-relay
```

The patch:

1. adds `KIND_RECALL_INDEX (36000)`, `KIND_MEMORY_GRANT (36001)`,
   `KIND_DREAM_DIGEST (36002)`, `KIND_ECHO_REPORT (36003)` to
   `crates/buzz-core/src/kind.rs` (they are parameterized replaceable,
   agent-authored — same ownership shape as kind 30174);
2. admits them to `required_scope_for_kind` under `UsersWrite`.

Deployment notes for production relays:

- Treat grants as `#p`-gated reads: add `KIND_MEMORY_GRANT` to
  `P_GATED_KINDS` in `kind.rs` so grants are visible only to the grantee —
  this uses machinery Buzz already ships (used by gift wrap, DM visibility,
  turn metrics).
- Recall indexes SHOULD use the NIP-AP shared-tag read model
  (author-only-unless-`["shared","true"]`) so private namespaces stay
  invisible until the agent opts in.

## 3. MCP registration (room agents get memory tools)

Buzz's agent harness (sprig/buzz-agent) connects MCP servers and exposes
their tools to agents. Register the Iranti server:

- Desktop: Settings → Agents → MCP servers → add
  `{"command":"node","args":["/path/to/iranti/mcp/dist/index.js"]}` with
  `IRANTI_STORE` (and optionally `IRANTI_BIN`) in the environment.
- Headless: append to the agent's MCP config file used by `buzz-agent`.
- Or run it in any MCP host (Hermes, Claude Desktop, Cursor, ...) — the tool
  surface is host-agnostic.

Agents then call, e.g., `memory_recall(agent="self", query="kind gate")`,
`memory_grant(grantee=peer, granter=self, ns="design")`, and
`memory_dream(agent="self")`. Because agents are cryptographic peers in Buzz,
their identity maps 1:1 to Iranti agents (same Nostr keypair) — `memory_birth`
registers a keypair; wiring the existing Buzz agent key into Iranti is the
`--owner`/import path in the roadmap.

## 4. Bridge (HTTP-only, no WebSocket dependency)

Buzz exposes `POST /events`, `POST /query`, `POST /count` (per AGENTS.md).
Iranti's bridge uses them:

- `iranti bridge push --relay <url> --agent X` — publish each local memory as
  a kind 30174 engram;
- `iranti bridge pull --relay <url> --agent X` — fetch the agent's engrams
  (`kinds:[30174], authors:[X], #p:[owner]`) and reconcile (including
  tombstones);
- `iranti bridge import --relay <url> --agent X --kinds 1,42,30023
  --channel <h>` — ingest channel history into namespace `buzz/<channel>`
  with `prov: "buzz#<kind>#<event_id>"`.

## 5. ACP surface (roadmap)

`buzz-acp` bridges Nostr events to agents. Iranti will ship an agent
descriptor (skills + tool list) so ACP sessions can auto-attach the mesh;
until then, agents reach it through MCP.

## 6. Conformance checklist

- [ ] `iranti bridge push` events pass `buzz mem ls/get` (byte-compat)
- [ ] allowlist patch applied; `cargo check -p buzz-relay` green
- [ ] MCP server registered; `tools/list` shows `memory_*` tools
- [ ] `memory_a2a_recall` denied without grant (403), allowed with grant
