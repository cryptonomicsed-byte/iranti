// Ìrántí MCP server — the memory mesh as agent tools.
//
// Any MCP host (Buzz agents via buzz-dev-mcp / ACP, Hermes, Claude Desktop,
// Cursor, ...) discovers and invokes every Iranti capability as a tool. The
// server wraps the `iranti` CLI (Rust core) and the resonance engine (Python);
// all logic lives in the core, this layer is pure tool surface.
//
// Register in Buzz: add this server to the MCP server list so room agents can
// call memory_recall / memory_grant / memory_dream etc. (see docs/buzz-integration.md).

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const server = new McpServer({ name: "iranti-mcp", version: "0.1.0" });

// Locate the iranti CLI: IRANTI_BIN env wins, else repo target/debug/iranti.
function cliBin(): string {
  if (process.env.IRANTI_BIN) return process.env.IRANTI_BIN;
  return path.resolve(__dirname, "../../target/debug/iranti");
}

function runCli(args: string[]): any {
  // Only set IRANTI_STORE when the caller actually provided one -- forcing
  // it to "" when unset made clap's `[env: IRANTI_STORE=]` fallback see a
  // provided-but-empty value instead of "not set", which broke the CLI's
  // own default store path (~/.iranti/store.sqlite) for any subcommand
  // that requires --store, e.g. `a2a` ("a value is required for
  // '--store <STORE>' but none was supplied"). Found live via a real
  // Vantage-Voice integration test.
  const env = { ...process.env };
  if (process.env.IRANTI_STORE) {
    env.IRANTI_STORE = process.env.IRANTI_STORE;
  } else {
    delete env.IRANTI_STORE;
  }
  const stdout = execFileSync(cliBin(), args, {
    encoding: "utf-8",
    env,
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 64 * 1024 * 1024,
  });
  const trimmed = stdout.trim();
  if (!trimmed) return {};
  try {
    return JSON.parse(trimmed);
  } catch {
    return { text: trimmed };
  }
}

type TextResult = { content: { type: "text"; text: string }[] };

function tool<S extends z.ZodRawShape>(
  name: string,
  description: string,
  inputSchema: S,
  cb: (a: z.infer<z.ZodObject<S>>) => TextResult | Promise<TextResult>
) {
  const handler = async (args: unknown) => cb(args as z.infer<z.ZodObject<S>>);
  server.registerTool(name, { description, inputSchema }, handler as never);
}

const ok = (data: unknown): TextResult => ({
  content: [{ type: "text", text: typeof data === "string" ? data : JSON.stringify(data, null, 2) }],
});
const err = (e: unknown): TextResult => ({
  content: [{ type: "text", text: JSON.stringify({ error: String((e as Error).message ?? e) }, null, 2) }],
});

// ---- tools ----

tool("memory_birth", "Birth a new agent identity on the Iranti mesh (Nostr keypair, NIP-AE owner binding).", {
  name: z.string().describe("agent name"),
  owner: z.string().optional().describe("owner pubkey hex (NIP-AE p-tag)"),
}, ({ name, owner }) => {
  try {
    const args = ["birth", name, "--json"];
    if (owner) args.push("--owner", owner);
    return ok(runCli(args));
  } catch (e) { return err(e); }
});

tool("memory_write", "Write a memory entry into an agent's namespace (slug is generated).", {
  agent: z.string().describe("agent name or pubkey"),
  namespace: z.string().describe("coarse recall axis, e.g. design, ops, buzz"),
  text: z.string().describe("memory content"),
  pin: z.boolean().optional().describe("pinned memories resist decay"),
  salience: z.number().int().min(0).max(100).optional().describe("importance 0-100"),
  ttl_days: z.number().int().optional().describe("expire after N days"),
  prov: z.string().optional().describe("provenance pointer e.g. buzz#42#<event_id>"),
}, ({ agent, namespace, text, pin, salience, ttl_days, prov }) => {
  try {
    const args = ["write", agent, namespace, text, "--json"];
    if (pin) args.push("--pin");
    if (salience !== undefined) args.push("--salience", String(salience));
    if (ttl_days !== undefined) args.push("--ttl-days", String(ttl_days));
    if (prov) args.push("--prov", prov);
    return ok(runCli(args));
  } catch (e) { return err(e); }
});

tool("memory_recall", "Recall memories: BM25 relevance x recency decay x salience/pin ranking.", {
  agent: z.string().describe("agent name or pubkey"),
  query: z.string().describe("what to remember"),
  k: z.number().int().min(1).max(50).optional().describe("top-k hits"),
  ns: z.string().optional().describe("restrict to namespace"),
}, ({ agent, query, k, ns }) => {
  try {
    const args = ["recall", agent, query, "--json"];
    if (k) args.push("-k", String(k));
    if (ns) args.push("--ns", ns);
    return ok(runCli(args));
  } catch (e) { return err(e); }
});

tool("memory_list", "List an agent's memories (optionally by namespace).", {
  agent: z.string().describe("agent name or pubkey"),
  ns: z.string().optional(),
}, ({ agent, ns }) => {
  try {
    const args = ["list", agent, "--json"];
    if (ns) args.push("--ns", ns);
    return ok(runCli(args));
  } catch (e) { return err(e); }
});

tool("memory_forget", "Forget a memory by slug (local tombstone).", {
  agent: z.string(),
  slug: z.string().describe("mem/... slug"),
}, ({ agent, slug }) => {
  try {
    return ok(runCli(["forget", agent, slug]));
  } catch (e) { return err(e); }
});

tool("memory_grant", "Grant an agent read access to one of your namespaces (A2A consent).", {
  grantee: z.string().describe("receiving agent"),
  granter: z.string().describe("granting agent"),
  ns: z.string().describe("namespace to share"),
}, ({ grantee, granter, ns }) => {
  try {
    return ok(runCli(["grant", grantee, granter, ns]));
  } catch (e) { return err(e); }
});

tool("memory_revoke", "Revoke an A2A namespace grant.", {
  grantee: z.string(),
  granter: z.string(),
  ns: z.string(),
}, ({ grantee, granter, ns }) => {
  try {
    return ok(runCli(["revoke", grantee, granter, ns]));
  } catch (e) { return err(e); }
});

tool("memory_a2a_recall", "Cross-agent recall: query another agent's namespace (grant-checked).", {
  requester: z.string().describe("the agent asking"),
  owner: z.string().describe("the agent whose memory is asked"),
  ns: z.string(),
  query: z.string(),
  k: z.number().int().min(1).max(50).optional(),
}, ({ requester, owner, ns, query, k }) => {
  try {
    const args = ["a2a", requester, owner, ns, query, "--json"];
    if (k) args.push("-k", String(k));
    return ok(runCli(args));
  } catch (e) { return err(e); }
});

tool("memory_dream", "Dream cycle: distill an agent's memory into a daily digest (reflection).", {
  agent: z.string(),
}, ({ agent }) => {
  try {
    return ok(runCli(["dream", agent, "--json"]));
  } catch (e) { return err(e); }
});

tool("memory_echo", "Hive resonance: cross-agent echo detection over memory fingerprints.", {
  agents: z.array(z.string()).optional().describe("agents to compare (default: all)"),
  top: z.number().int().optional(),
}, ({ agents, top }) => {
  try {
    const args = ["echo"];
    if (agents && agents.length) args.push(...agents);
    if (top) args.push("--top", String(top));
    return ok(runCli(args));
  } catch (e) { return err(e); }
});

tool("memory_status", "Status: memory counts, namespaces, grants, last dream.", {
  agent: z.string(),
}, ({ agent }) => {
  try {
    return ok(runCli(["status", agent, "--json"]));
  } catch (e) { return err(e); }
});

tool("memory_export", "Export an agent's plaintext snapshot (feeds the resonance engine).", {
  agent: z.string(),
  out: z.string().optional().describe("output path (default: stdout)"),
}, ({ agent, out }) => {
  try {
    const args = ["export", agent];
    if (out) args.push("--out", out);
    return ok(runCli(args));
  } catch (e) { return err(e); }
});

tool("memory_agents", "List agents on this mesh.", {}, () => {
  try {
    return ok(runCli(["agents", "--json"]));
  } catch (e) { return err(e); }
});

// ---- boot ----

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error("iranti-mcp ready: 13 tools registered (memory_*)");
}

main().catch((e) => {
  console.error("iranti-mcp fatal:", e);
  process.exit(1);
});
