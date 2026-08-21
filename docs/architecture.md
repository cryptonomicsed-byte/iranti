# Architecture

## Design goals

1. **Complement Buzz, never rebuild it.** Buzz's NIP-AE is the encrypted
   source of truth for agent memory; Iranti is the recall/consent/reflection
   layer NIP-AE explicitly defers to companion NIPs.
2. **Agent-native first.** No human-only surface: every capability is a CLI
   command, an MCP tool, or an A2A RPC method — discoverable, invocable,
   chainable. Humans are participants, not prerequisites.
3. **Local-first, protocol-compatible.** The store works offline; relay sync
   is a transport. Nothing depends on a relay being reachable.
4. **Polyglot by documented strength**, aligned with the Omo-koda layer
   mapping.

## Layers

```
                 ┌────────────────────────────────────────────┐
                 │  Agents (Buzz rooms, Hermes, any MCP host) │
                 └───────┬──────────────┬─────────────────────┘
                         │ MCP (stdio)   │ A2A (HTTP /rpc)
                 ┌───────▼──────┐  ┌────▼─────┐
                 │ iranti-mcp   │  │ daemon   │   TypeScript surface
                 │ (13 tools)   │  │ 127.0.0.1│
                 └───────┬──────┘  └────┬─────┘
                         │ CLI (iranti) │
                 ┌───────▼──────────────▼──────────────────┐
                 │  iranti-core (Rust — Èṣù steward)       │
                 │  crypto (NIP-AE exact) · store (SQLite) │
                 │  recall (BM25·decay·boost) · dream      │
                 │  bridge (Buzz HTTP sync) · grants       │
                 └───────┬─────────────────────────┬───────┘
                         │ export (JSON contract)  │ relay
                 ┌───────▼──────────┐   ┌──────────▼──────────┐
                 │ resonance (Py)   │   │ Buzz relay (POST    │
                 │ Ọ̀ṣun — echo/     │   │ /events, /query)    │
                 │ convergence      │   │ kind 30174 + 36000- │
                 └──────────────────┘   │ 36003 (patched)     │
                                        └─────────────────────┘
```

## Language choices (why)

| Choice | Rationale |
|--------|-----------|
| **Rust** core | Buzz is Rust; the `nostr` crate (0.44) is the same dependency Buzz pins, giving byte-exact engram envelopes. Performance-critical recall/decrypt runs natively. Type-safety over the crypto boundary. |
| **Python** resonance | Rapid numerical computation; the reference engine is stdlib-only and deterministic, but the same JSON contract can back an embedding model later. Kept out of the trust boundary (no keys, no store access — consumes exports). |
| **TypeScript** MCP | The MCP SDK is first-class in TS; Buzz's own `buzz-dev-mcp` is TS, so operator familiarity and tooling align. Pure tool surface, zero domain logic. |
| **Move/Sui, WASM** (roadmap) | On-chain memory ownership + reputation (Ṣàngó) and portable sealed identity for edge agents (Àṣẹ), per the Omo-koda sovereign-memory design. |
| **Elixir/Go** (roadmap) | Swarm coordination (Yemọja) and real-time hive fan-out (Ọya) when the mesh grows past single-host scale. |

## Data model

- **agents** — id (pubkey), name, sk, owner (NIP-AE `p` tag), created_at.
- **memories** — agent, slug (`mem/<ns>/<rand>`), ns, value, salience (0-100),
  pin, ttl, prov, created_at/updated_at. Unique per (agent, slug).
- **grants** — (granter, grantee, ns) unique; consent for A2A recall.
- **digests** — (agent, kind, period) → dream/echo JSON bodies.
- **meta** — schema version, store path, sync cursors.

## Recall scoring

```
score = BM25(query, doc) * decay(age) * (1 + 0.5·pin) * (1 + (salience−50)/100)
decay  = 0.5^(age_days / 14)        # 14-day half-life
BM25   = Σ idf · tf·(k1+1) / (tf + k1·(1−b + b·dl/avgdl)), k1=1.5, b=0.75
```

Deterministic, tunable, dependency-free. The interface (`recall::recall`)
accepts a vector backend later without touching callers.

## Self-improvement loop (dream)

`write → dream (daily) → digest → drift detection → echo (cross-agent) →
convergence report → (agents adjust)`. The dream digest's `drift` field is
the closed-loop signal: if `term_overlap` collapses, the agent's focus
shifted; if `new_terms` grows, exploration is happening. Cron/daemon
automates the cycle; MCP exposes it as `memory_dream`.

## Surfaces (one core, three façades)

1. **CLI** — `iranti birth|write|read|list|recall|forget|grant|revoke|a2a|
   dream|echo|status|export|bridge|daemon` (`--json` for machines).
2. **MCP** — 13 `memory_*` tools wrapping the CLI (birth, write, recall,
   list, forget, grant, revoke, a2a_recall, dream, echo, status, export,
   agents).
3. **A2A RPC** — `GET /health`, `POST /rpc {method:"a2a_recall"}` with
   `X-Iranti-Agent` identity; grant-checked; NIP-98 for production.

## Extension points

- **Plugins**: MCP registration is the plugin mechanism — any agent with an
  MCP client gains the mesh; the mesh gains tools the same way.
- **Kinds**: NIP-IR defines 36000-36003; new companion events slot into the
  same allowlist pattern.
- **Backends**: recall vector backend, memory blob store (Walrus), key
  custody (SEAL) — all behind narrow traits.
- **Resonance contract**: `iranti/export/v1` JSON in → `iranti/resonance/v1`
  JSON out; swap engines without touching core.
