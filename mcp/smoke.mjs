// Deterministic MCP smoke test: boots dist/index.js on stdio, performs the
// MCP handshake, lists tools, and asserts the expected tool surface.
// Usage: node smoke.mjs  (expect "MCP SMOKE PASS")

import { spawn } from "node:child_process";

const child = spawn("node", ["dist/index.js"], { stdio: ["pipe", "pipe", "pipe"] });
let out = "";
child.stdout.on("data", (d) => (out += d.toString()));
child.stderr.on("data", () => {});

const send = (m) => child.stdin.write(JSON.stringify(m) + "\n");
const EXPECTED = [
  "memory_birth", "memory_write", "memory_recall", "memory_list", "memory_forget",
  "memory_grant", "memory_revoke", "memory_a2a_recall", "memory_dream", "memory_echo",
  "memory_status", "memory_export", "memory_agents",
];

setTimeout(() => send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "smoke", version: "0" } } }), 400);
setTimeout(() => send({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }), 900);
setTimeout(() => {
  const names = [...new Set([...out.matchAll(/"name":"([a-z0-9_]+)"/g)].map((m) => m[1]))];
  const missing = EXPECTED.filter((t) => !names.includes(t));
  if (!out.includes('"tools"') || missing.length > 0) {
    console.error("MCP SMOKE FAIL — missing tools:", missing, "\nraw:", out.slice(0, 2000));
    child.kill();
    process.exit(1);
  }
  console.log(`MCP SMOKE PASS — ${names.length} tools registered (all ${EXPECTED.length} expected present)`);
  child.kill();
  process.exit(0);
}, 1600);
