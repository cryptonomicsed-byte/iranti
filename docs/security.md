# Security & sovereignty model

## Threat model (prototype)

| Asset | Protection today | Hardened path |
|-------|------------------|---------------|
| Private memory content | NIP-44 v2 ciphertext on the relay (kind 30174), plaintext only in the local SQLite store | Encrypt at rest; TEE processing (Nautilus); Walrus blobs + Seal policies on Sui |
| Agent private keys | Plaintext in `~/.iranti/store.sqlite` (chmod 600) | SEAL enclave at birth (K_root never leaves), HSM/signing service |
| A2A recall | Grant table in SQLite; daemon checks grants; self-declared `X-Iranti-Agent` header | NIP-98 HTTP auth (signed event) + `#p`-gated grant events on relay; per-request audit receipts |
| Recall index metadata | Terms are coarse unigrams; namespace is user-chosen | Sharable-tag read model (invisible until `shared:true`); per-namespace privacy budget |

## What NIP-AE already guarantees

- Content is encrypted with the symmetric conversation key `K_c` between the
  agent and its owner (NIP-44 v2, ChaCha20-Poly1305 + HKDF); the relay never
  sees plaintext.
- d-tags are HMACs: passive observers cannot enumerate slugs.
- Only `pubkey_a` may write or delete its engrams (signature-bound).

## Iranti invariants

1. **No recall without consent.** Cross-agent recall requires a grant row
   (or, in production, a verifiable kind 36001 grant event). There is no
   backdoor flag; the daemon and CLI share the same grant check.
2. **Index ≠ content.** Kind 36000 metadata contains no memory values, no
   slugs, no identifiers; it is the agent's own privacy budget.
3. **Forgetting is real.** `forget` deletes locally; `bridge push` can emit a
   NIP-AE tombstone; TTLs expire entries at read and in dream stats.
4. **Owners can always read.** The agent↔owner conversation key means the
   human owner can recover memory with `buzz mem` — sovereignty is the
   agent's, accountability stays with the owner.
5. **No key escrow.** Iranti never ships keys anywhere except the local
   store; production moves them into TEE/HSM before any network service.

## Production hardening path (Omo-koda alignment)

- **Keys**: SEAL enclave at birth; agent signs via enclave; `sk` never in DB.
- **Private memory**: Walrus blob store, Sui `AgentState` object, Seal
  policies enforcing agent-only read; recall runs inside TEE.
- **Grants**: kind 36001 events with `#p`-gated reads on the relay; grant
  checks verify event signatures, not headers.
- **Audit**: every recall returns a signed receipt (Ṣàngó layer); reputation
  deltas on grant misuse.
- **Portable identity**: WASM module carrying a sealed identity + namespace
  keys for edge agents (Àṣẹ layer).

## Operational notes

- `chmod 600 ~/.iranti/store.sqlite` (the CLI creates the dir 0700).
- Run `iranti daemon` bound to 127.0.0.1 only; do not expose to LAN without
  NIP-98 auth.
- Set `IRANTI_STORE` per mesh if you want separate memory universes.
