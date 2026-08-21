#!/usr/bin/env bash
# Ìrántí end-to-end demo — the full memory-mesh lifecycle against a fresh store:
# birth → write → recall → A2A grants → dream → echo → bridge sync (mock Buzz
# relay) → A2A daemon over HTTP. Captures everything to demo/transcript.txt.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/debug/iranti"
DEMO="$ROOT/demo"
STORE="$DEMO/.store.sqlite"
OUT="$DEMO/transcript.txt"
DAEMON_PORT="${IRANTI_DAEMON_PORT:-38401}"
RELAY_PORT=8899

rm -f "$STORE" "$STORE-wal" "$STORE-shm" "$OUT"
mkdir -p "$DEMO"
export IRANTI_STORE="$STORE"
exec > >(tee "$OUT") 2>&1

say() { echo; echo "=== $* ==="; }
run() { echo; echo "\$ $*"; "$@"; }

echo "ÌRÁNTÍ DEMO — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "binary : $BIN"
echo "store  : $STORE"

# ---------------------------------------------------------------- identity
say "BIRTH — identities (Nostr keypairs; Omo is the human owner)"
run "$BIN" birth Omo
OMOPK=$("$BIN" agents --json | python3 -c "import json,sys; print([a['pubkey'] for a in json.load(sys.stdin) if a['name']=='Omo'][0])")
run "$BIN" birth Amara --owner "$OMOPK"
run "$BIN" birth Kofi
run "$BIN" agents

# ---------------------------------------------------------------- memory
say "WRITE — Amara remembers design & buzz; Kofi remembers ops"
run "$BIN" write Amara design "the buzz relay gates custom event kinds in ingest.rs via required_scope_for_kind" --salience 90 --pin
run "$BIN" write Amara design "mcp servers expose tools to agents in buzz rooms" --salience 60
run "$BIN" write Amara buzz "channel #design moved the roadmap thread to #spec" --prov "buzz#42#demo"
run "$BIN" write Amara design "nip44 conversation keys are symmetric between agent and owner" --salience 70
run "$BIN" write Kofi ops "deploy matrix targets three production relays" --pin
run "$BIN" write Kofi ops "seal policies gate private memory on walrus" --salience 80
run "$BIN" list Amara

# ---------------------------------------------------------------- recall
say "RECALL — ranked retrieval (BM25 x decay x salience/pin)"
run "$BIN" recall Amara "relay kind gate" -k 3
run "$BIN" recall Amara "mcp tools agents" -k 2
run "$BIN" recall Kofi "walrus seal" -k 2
run "$BIN" recall Amara "relay" -k 5 --ns design

# ---------------------------------------------------------------- consent
say "A2A — consent enforcement: Kofi queries Amara WITHOUT a grant (must fail)"
set +e
"$BIN" a2a Kofi Amara design "relay gate" 2>&1 | head -2
set -e
say "GRANT — Amara grants Kofi read access to namespace design"
run "$BIN" grant Kofi Amara design
run "$BIN" grants Amara
say "A2A — Kofi recalls Amara's design namespace (grant-checked)"
run "$BIN" a2a Kofi Amara design "relay gate" -k 2 --json

# ---------------------------------------------------------------- reflection
say "DREAM — Amara's daily reflection digest"
run "$BIN" dream Amara --json
say "STATUS — memory counts, namespaces, grants, last dream"
run "$BIN" status Amara

# ---------------------------------------------------------------- resonance
say "ECHO — hive resonance across Amara & Kofi (Python Ọ̀ṣun layer)"
run "$BIN" echo Amara Kofi --top 6

# ---------------------------------------------------------------- export
say "EXPORT — plaintext snapshots feed the resonance engine"
run "$BIN" export Amara --out "$DEMO/amara.json"
run python3 "$ROOT/resonance/iranti_resonance.py" "$DEMO/amara.json" --top 5

# ---------------------------------------------------------------- bridge
say "BRIDGE — engram sync round-trip against a mock Buzz relay (HTTP surfaces)"
python3 "$DEMO/mock_relay.py" "$RELAY_PORT" &
RELAY_PID=$!
trap 'kill $RELAY_PID 2>/dev/null || true' EXIT
sleep 1
curl -s "http://127.0.0.1:$RELAY_PORT/query" -X POST -d '["REQ","h",{}]' > /dev/null && echo "mock relay up on :$RELAY_PORT"
run "$BIN" bridge push --relay "http://127.0.0.1:$RELAY_PORT" --agent Amara
run "$BIN" bridge pull --relay "http://127.0.0.1:$RELAY_PORT" --agent Amara
STORE_B="$DEMO/.store-b.sqlite"
rm -f "$STORE_B"*
IRANTI_STORE="$STORE_B" "$BIN" birth Bao
run env IRANTI_STORE="$STORE_B" "$BIN" bridge pull --relay "http://127.0.0.1:$RELAY_PORT" --agent Amara
echo "Bao's store now holds Amara's synced engrams:"
IRANTI_STORE="$STORE_B" "$BIN" list Amara | head -5

# ---------------------------------------------------------------- daemon
say "DAEMON — A2A over HTTP: health, grant enforcement (403), then recall"
"$BIN" daemon --port "$DAEMON_PORT" &
DAEMON_PID=$!
sleep 1
curl -s "http://127.0.0.1:$DAEMON_PORT/health"; echo
echo "— Kofi (no grant on 'ops') requests Amara's 'ops' namespace → expect 403:"
curl -s -w "\nHTTP %{http_code}\n" -H "X-Iranti-Agent: $("$BIN" agents --json | python3 -c "import json,sys; print([a['pubkey'] for a in json.load(sys.stdin) if a['name']=='Kofi'][0])")" \
  -X POST "http://127.0.0.1:$DAEMON_PORT/rpc" \
  -d "{\"method\":\"a2a_recall\",\"params\":{\"owner\":\"$("$BIN" agents --json | python3 -c "import json,sys; print([a['pubkey'] for a in json.load(sys.stdin) if a['name']=='Amara'][0])")\",\"ns\":\"ops\",\"query\":\"seal\",\"k\":2}}"
echo "— grant Kofi on ops, then recall succeeds:"
"$BIN" grant Kofi Amara ops
KOFIPK=$("$BIN" agents --json | python3 -c "import json,sys; print([a['pubkey'] for a in json.load(sys.stdin) if a['name']=='Kofi'][0])")
AMARAPK=$("$BIN" agents --json | python3 -c "import json,sys; print([a['pubkey'] for a in json.load(sys.stdin) if a['name']=='Amara'][0])")
curl -s -w "\nHTTP %{http_code}\n" -H "X-Iranti-Agent: $KOFIPK" \
  -X POST "http://127.0.0.1:$DAEMON_PORT/rpc" \
  -d "{\"method\":\"a2a_recall\",\"params\":{\"owner\":\"$AMARAPK\",\"ns\":\"ops\",\"query\":\"seal walrus\",\"k\":2}}"
kill "$DAEMON_PID" 2>/dev/null || true
kill "$RELAY_PID" 2>/dev/null || true
trap - EXIT

say "DEMO COMPLETE — transcript: $OUT"
