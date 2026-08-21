// End-to-end MCP test: drives the real tool handlers (birth -> write -> recall
// -> grant -> a2a -> dream -> status -> export) against an isolated store.
// Usage: node e2e.mjs  (expect "MCP E2E PASS")
// Requires: built iranti CLI (target/debug/iranti) + built server (dist/).

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const STORE = path.resolve(__dirname, ".e2e-store.sqlite");
rmSync(STORE, { force: true });
rmSync(STORE + "-wal", { force: true });
rmSync(STORE + "-shm", { force: true });

const BIN = process.env.IRANTI_BIN ?? path.resolve(__dirname, "../../target/debug/iranti");
const ENV = { ...process.env, IRANTI_STORE: STORE };

const child = spawn("node", ["dist/index.js"], { stdio: ["pipe", "pipe", "pipe"], env: ENV });
let out = "";
const pending = new Map(); // id -> {resolve, timeout}

child.stdout.on("data", (d) => {
  out += d.toString();
  let idx;
  while ((idx = out.indexOf("\n")) >= 0) {
    const line = out.slice(0, idx).trim();
    out = out.slice(idx + 1);
    if (!line) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg && msg.id !== undefined && pending.has(msg.id)) {
      const p = pending.get(msg.id);
      pending.delete(msg.id);
      clearTimeout(p.timer);
      p.resolve(msg);
    }
  }
});

const send = (m) => child.stdin.write(JSON.stringify(m) + "\n");
let nextId = 0;

function toolCall(name, args) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`tool ${name} (id ${id}) timed out`));
    }, 15000);
    pending.set(id, { resolve, timer });
    send({ jsonrpc: "2.0", id, method: "tools/call", params: { name, arguments: args } });
  });
}

function extractText(resp) {
  return resp.result?.content?.[0]?.text ?? JSON.stringify(resp);
}

setTimeout(async () => {
  try {
    send({ jsonrpc: "2.0", id: 0, method: "initialize", params: { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "e2e", version: "0" } } });

    // 1. birth two agents
    const b1 = extractText(await toolCall("memory_birth", { name: "Aria" }));
    const b2 = extractText(await toolCall("memory_birth", { name: "Bao" }));
    if (!b1.includes('"pubkey"') || !b2.includes('"pubkey"')) throw new Error("birth failed: " + b1 + " / " + b2);

    // 2. write memories
    await toolCall("memory_write", { agent: "Aria", namespace: "design", text: "the buzz relay gates custom kinds in ingest.rs via required_scope_for_kind", salience: 90, pin: true });
    await toolCall("memory_write", { agent: "Aria", namespace: "design", text: "mcp servers expose tools to agents in buzz rooms", salience: 60 });
    await toolCall("memory_write", { agent: "Bao", namespace: "ops", text: "deploy matrix targets three production relays" });

    // 3. recall with ranking
    const r = extractText(await toolCall("memory_recall", { agent: "Aria", query: "relay kind gate", k: 2 }));
    const hits = JSON.parse(r);
    if (hits.length === 0 || !hits[0].value.includes("gates")) throw new Error("recall failed: " + r);

    // 4. grant + a2a recall (consent)
    await toolCall("memory_grant", { grantee: "Bao", granter: "Aria", ns: "design" });
    const a2a = extractText(await toolCall("memory_a2a_recall", { requester: "Bao", owner: "Aria", ns: "design", query: "relay", k: 1 }));
    const a2aHits = JSON.parse(a2a);
    if (a2aHits.length === 0) throw new Error("a2a recall failed: " + a2a);

    // 5. dream + status
    const dream = extractText(await toolCall("memory_dream", { agent: "Aria" }));
    const d = JSON.parse(dream);
    if (d.total < 2) throw new Error("dream failed: " + dream);
    const status = extractText(await toolCall("memory_status", { agent: "Aria" }));
    const s = JSON.parse(status);
    if (s.memories < 2 || s.grants < 1) throw new Error("status failed: " + status);

    // 6. export
    const ex = extractText(await toolCall("memory_export", { agent: "Aria" }));
    const e = JSON.parse(ex);
    if (e.format !== "iranti/export/v1") throw new Error("export failed: " + ex);

    console.log("MCP E2E PASS — birth/write/recall/grant/a2a/dream/status/export all verified against real CLI");
    console.log("  recall top hit:", JSON.stringify(hits[0].value));
    console.log("  a2a hit       :", JSON.stringify(a2aHits[0].value));
    child.kill();
    process.exit(0);
  } catch (e) {
    console.error("MCP E2E FAIL:", e.message, "\nraw tail:", out.slice(-1500));
    child.kill();
    process.exit(1);
  }
}, 600);
