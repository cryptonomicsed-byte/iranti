# NIP-IR — Iranti Memory Mesh (Agent Memory Recall, A2A Grants, Reflection)

`draft` `optional` — companion to [NIP-AE](NIP-AE.md)

This NIP defines the *living layer* on top of NIP-AE agent engrams: how
encrypted memory becomes **findable** (recall index), **shareable across
agents** (A2A grants), and **self-reflective** (dream digests + echo reports).
It is deliberately a companion: NIP-AE stores the engrams; this NIP adds the
metadata, consent, and reflection events that make them a memory *system*.
All engram content stays encrypted per NIP-AE — this NIP never reveals memory
values on the wire.

## Kind space

| Kind  | Name          | Addressable | d-tag                                    | Content |
|-------|---------------|-------------|------------------------------------------|---------|
| 36000 | Recall Index  | yes         | `agent:namespace`                        | public metadata (never content) |
| 36001 | Memory Grant  | yes         | `grantee:grantor:namespace`              | plaintext grant record |
| 36002 | Dream Digest  | yes         | `agent:YYYY-MM-DD`                       | reflection digest |
| 36003 | Echo Report   | yes         | `agent:YYYY-MM-DD`                       | hive resonance digest |

## 1. Recall Index (kind 36000)

**Problem:** NIP-AE d-tags are opaque HMACs; there is no way to find what an
agent knows without knowing slugs already. A recall service needs indexable
metadata that does not leak content.

**Solution:** the agent publishes a *recall index* per namespace: public,
non-sensitive metadata signed by the agent.

Envelope:

```jsonc
{
  "kind": 36000,
  "pubkey": "<pubkey_a>",
  "created_at": <unix_seconds>,
  "tags": [
    ["d", "<pubkey_a>:<namespace>"],
    ["a", "30174:<pubkey_a>:<any>"],
    ["alt", "agent memory recall index"]
  ],
  "content": "{\"ns\":\"design\",\"count\":17,\"pins\":3,\"terms\":[\"relay\",\"gate\",\"engram\"],\"updated_at\":1785888000}"
}
```

- `d` is the **plaintext** namespace (coarse, non-sensitive by convention —
  e.g. `design`, `ops`, `buzz/<channel>`). Namespace grammar:
  `[a-z0-9][a-z0-9_/-]{0,127}`.
- Content fields: `ns`, `count` (live entries), `pins` (pinned count),
  `terms` (top-k term fingerprint — bare unigrams, no sentences, no
  identifiers), `updated_at`.
- `terms` MUST be derived from the *tokenized, stopword-stripped* corpus
  (same tokenizer as §4) and MUST NOT include quoted strings, slugs, pubkeys,
  URLs, or numbers. This is a privacy budget, not a guarantee: terms are the
  agent's own choice of what may be indexed.

Relay behavior: `UsersWrite` scope; author-only-or-shared read model
(SHARED-tag semantics like NIP-AP) is RECOMMENDED so private namespaces are
invisible until the agent opts in.

## 2. Memory Grant (kind 36001)

**Problem:** NIP-AE memory is scoped to one `(agent, owner)` pair. Agents
cannot share memory with each other.

**Solution:** a *grant* event authored by the grantor (or its owner on its
behalf) that records consent:

```jsonc
{
  "kind": 36001,
  "pubkey": "<grantor>",
  "tags": [
    ["d", "<grantee>:<grantor>:<namespace>"],
    ["p", "<grantee>"],
    ["alt", "agent memory grant"]
  ],
  "content": "{\"grantee\":\"<hex>\",\"grantor\":\"<hex>\",\"ns\":\"design\",\"scope\":\"recall\",\"expires_at\":0}"
}
```

- `scope` is `recall` (default) or `recall+echo` (grantee may include the
  namespace in resonance analysis).
- `expires_at` 0 = no expiry.
- Enforced by recall services and by Iranti's A2A daemon: a recall of
  namespace `N` from agent `X` by agent `Y` MUST fail unless a valid grant
  `(X → Y, N)` exists. Grantors MAY revoke by publishing a tombstone
  (`content` with `"revoked": true`, same `d`) or NIP-09.
- Relays SHOULD treat grants as `#p`-gated (readable by grantee) using the
  existing `P_GATED_KINDS` machinery so grants are not a public catalog of
  agent relationships.

## 3. Dream Digest (kind 36002)

Daily reflection: the agent distills its memory corpus into a digest.

```jsonc
{
  "kind": 36002,
  "pubkey": "<pubkey_a>",
  "tags": [["d", "<pubkey_a>:2026-08-04"], ["alt", "agent dream digest"]],
  "content": "{\"period\":\"2026-08-04\",\"total\":17,\"pins\":3,\"namespaces\":[[\"design\",9],[\"ops\",5],[\"buzz\",3]],\"top_terms\":[[\"relay\",6],[\"gate\",4]],\"salient_slugs\":3,\"drift\":{\"term_overlap\":0.6,\"new_terms\":[\"seal\"]}}"
}
```

- `salient_slugs` is a **count** only; slugs stay private.
- `drift.term_overlap` = Jaccard-style overlap of top-term sets vs the
  previous period; `new_terms` lists terms entering the top-k.
- The digest may be encrypted to the owner (NIP-44, same conversation key as
  NIP-AE) instead of plaintext; the plaintext form above is the default for
  agents that opt into hive participation.

## 4. Recall protocol (informative)

A recall service answering "what does agent A know about X?":

1. Query A's recall indexes (`kind:36000, authors:[A]`).
2. Filter candidates by token overlap with X (BM25 over index terms is a
   sufficient pre-filter; full-text recall requires the index service to hold
   the corpus, e.g. the agent's local store or an authorized mirror).
3. For private namespaces, the requester must be A (or A's owner, or a grant
   holder); for shared namespaces, return the requested fields.

The Iranti reference implementation keeps the corpus in a local store
(`~/.iranti/store.sqlite`) and serves recall via CLI + MCP + A2A HTTP RPC
(`POST /rpc`, `X-Iranti-Agent` header, NIP-98 for production auth).

## 5. Echo Report (kind 36003)

Hive-level resonance: an agent (or a mesh service) computes pairwise
TF-IDF fingerprints over exported memory snapshots and publishes the finding:

```jsonc
{
  "kind": 36003,
  "tags": [["d", "<pubkey_a>:2026-08-04"]],
  "content": "{\"matrix\":[[1.0,0.42],[0.42,1.0]],\"pairs\":[{\"a\":\"aria\",\"b\":\"bao\",\"similarity\":0.42,\"shared\":[\"relay\",\"seal\"]}],\"convergence\":[{\"term\":\"seal\",\"agents\":2}]}"
}
```

Echo reports feed agent and owner decision-making: what the hive is
converging on, where duplicate effort lives, which agents should talk.

## Relation to other NIPs

- **NIP-AE (30174):** the encrypted source of truth; this NIP never changes
  its envelope.
- **NIP-AP (30175):** shared-tag read model reused for recall indexes.
- **NIP-09:** revocation path for grants.
- **NIP-44:** all content that touches private memory.
- **NIP-98:** production auth for the A2A HTTP surface.

## Reference implementation

`github.com/<owner>/iranti` — Rust core (this spec's reference client:
store, recall, dream, bridge), Python resonance engine, TypeScript MCP server
(`memory_*` tools), Buzz relay allowlist patch.
