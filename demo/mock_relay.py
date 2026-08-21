#!/usr/bin/env python3
"""Tiny mock Buzz relay implementing the exact HTTP surfaces Iranti's bridge
uses (POST /events, POST /query, POST /count per block/buzz AGENTS.md).
In-memory store; enough to prove the engram push/pull round-trip live.

Usage: python3 mock_relay.py [port]
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

EVENTS = []  # list of event dicts


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length)
        try:
            data = json.loads(raw)
        except json.JSONDecodeError:
            self._send(400, ["ERROR", "bad json"])
            return

        if self.path == "/events":
            if not isinstance(data, dict) or "id" not in data:
                self._send(400, ["ERROR", "expected event object"])
                return
            EVENTS.append(data)
            self._send(200, ["OK", data["id"], True, "mock relay stored"])
        elif self.path == "/query":
            # ["REQ", <subid>, <filter>]
            if not (isinstance(data, list) and len(data) >= 3):
                self._send(400, ["ERROR", "expected [REQ, id, filter]"])
                return
            sub, flt = data[1], data[2]
            out = []
            for ev in EVENTS:
                if not _match(ev, flt):
                    continue
                out.append(["EVENT", sub, ev])
            out.append(["EOSE", sub])
            self._send(200, out)
        elif self.path == "/count":
            sub, flt = data[1], data[2]
            n = sum(1 for ev in EVENTS if _match(ev, flt))
            self._send(200, [["COUNT", sub, n]])
        else:
            self._send(404, ["ERROR", "not found"])

    def log_message(self, *a):
        pass


def _match(ev, flt):
    if "kinds" in flt and ev.get("kind") not in flt["kinds"]:
        return False
    if "authors" in flt and ev.get("pubkey") not in flt["authors"]:
        return False
    if "#p" in flt:
        tags = {t[0]: t[1] for t in ev.get("tags", []) if len(t) > 1}
        if tags.get("p") not in flt["#p"]:
            return False
    if "#h" in flt:
        tags = {t[0]: t[1] for t in ev.get("tags", []) if len(t) > 1}
        if tags.get("h") not in flt["#h"]:
            return False
    if "limit" in flt:
        return False  # handled by caller; keep simple
    return True


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8899
    srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(f"mock buzz relay on http://127.0.0.1:{port}", flush=True)
    srv.serve_forever()
