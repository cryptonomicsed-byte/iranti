#!/usr/bin/env python3
"""Ìrántí Resonance Engine (Ọ̀ṣun layer) — hive intelligence over agent memory.

Consumes plaintext export snapshots produced by `iranti export` (format
iranti/export/v1) and computes cross-agent "echo" fingerprints:

  - per-agent TF-IDF term fingerprints over memory values
  - pairwise cosine similarity between agents (the echo matrix)
  - shared high-weight terms per pair (what the hive is converging on)
  - per-agent top terms and namespace diversity

The engine is deliberately dependency-free (stdlib only) and deterministic:
same exports in, same report out. A production deployment can swap the TF-IDF
vectorizer for embedding models behind the same JSON contract.

Usage:
    python3 iranti_resonance.py <export-dir> [--top N] [--min-sim 0.0]
        reads every *.json export in <export-dir> (or explicit file list)
        prints a JSON report to stdout
"""

import json
import math
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

STOPWORDS = frozenset(
    """the and for are but not you all can had her was one our out day get has him his how man
    new now old see two way who boy did its let put say she too use that with have this will your
    from they know want been good much some time very when come here just like long make many more
    only over such take than them well were what which would into about could other these there
    their where think thing things also back even first found give great help keep last left might
    must need never next once own still tell told under until while again away done down each else
    ever going idea life live mean most name open part right same said says should show small start
    state those three through took tried trying used whole yes""".split()
)

TOKEN_RE = re.compile(r"[a-z0-9]+")


def tokenize(text: str) -> list:
    return [w for w in TOKEN_RE.findall(text.lower()) if len(w) >= 3 and w not in STOPWORDS]


def tfidf_fingerprint(agent: dict) -> dict:
    """Document = concatenated memory values. Returns {term: weight}."""
    docs = agent.get("memories", [])
    tf = Counter()
    for m in docs:
        tf.update(tokenize(m.get("value", "")))
    n = len(docs)
    idf = {}
    for m in docs:
        for term in set(tokenize(m.get("value", ""))):
            idf[term] = idf.get(term, 0) + 1
    n = max(n, 1)
    weights = {}
    for term, count in tf.items():
        df = idf.get(term, 1)
        w = (1 + math.log(count)) * math.log(1 + n / df)
        if w > 0:
            weights[term] = w
    return weights


def cosine(a: dict, b: dict) -> float:
    if not a or not b:
        return 0.0
    common = set(a) & set(b)
    if not common:
        return 0.0
    dot = sum(a[t] * b[t] for t in common)
    na = math.sqrt(sum(v * v for v in a.values()))
    nb = math.sqrt(sum(v * v for v in b.values()))
    if na == 0 or nb == 0:
        return 0.0
    return dot / (na * nb)


def namespace_diversity(agent: dict) -> dict:
    counts = Counter(m.get("ns", "general") for m in agent.get("memories", []))
    return {"namespaces": dict(counts), "distinct": len(counts)}


def build_report(agents: list, top: int, min_sim: float) -> dict:
    fps = {}
    for a in agents:
        name = a.get("name", a.get("pubkey", "?")[:8])
        fps[name] = tfidf_fingerprint(a)

    matrix = []
    for na in agents:
        row = []
        for nb in agents:
            row.append(round(cosine(fps[na.get("name", "?")], fps[nb.get("name", "?")]), 4))
        matrix.append(row)

    pairs = []
    names = [a.get("name", "?") for a in agents]
    for i in range(len(agents)):
        for j in range(i + 1, len(agents)):
            sim = matrix[i][j]
            if sim < min_sim:
                continue
            fa, fb = fps[names[i]], fps[names[j]]
            shared = sorted(set(fa) & set(fb), key=lambda t: fa[t] + fb[t], reverse=True)[:top]
            pairs.append({
                "a": names[i],
                "b": names[j],
                "similarity": sim,
                "shared_top_terms": shared,
            })
    pairs.sort(key=lambda p: p["similarity"], reverse=True)

    fingerprints = {}
    for a in agents:
        name = a.get("name", "?")
        top_terms = sorted(fps[name].items(), key=lambda kv: kv[1], reverse=True)[:top]
        fingerprints[name] = {
            "top_terms": [t for t, _ in top_terms],
            "terms": len(fps[name]),
            "diversity": namespace_diversity(a),
        }

    # Hive convergence: terms shared by the most agents.
    term_agents = defaultdict(set)
    for a in agents:
        name = a.get("name", "?")
        for t in fps[name]:
            term_agents[t].add(name)
    convergence = sorted(
        ((t, len(owners)) for t, owners in term_agents.items() if len(owners) >= 2),
        key=lambda kv: (-kv[1], kv[0]),
    )[:top]

    return {
        "format": "iranti/resonance/v1",
        "agents": [a.get("name", "?") for a in agents],
        "matrix": matrix,
        "pairs": pairs,
        "fingerprints": fingerprints,
        "hive_convergence": [{"term": t, "agents": n} for t, n in convergence],
    }


def main(argv) -> int:
    if len(argv) < 1:
        print("usage: iranti_resonance.py <export-dir|file1.json ...> [--top N] [--min-sim X]", file=sys.stderr)
        return 2
    top = 10
    min_sim = 0.0
    args = list(argv)
    if "--top" in args:
        i = args.index("--top")
        top = int(args[i + 1])
        del args[i : i + 2]
    if "--min-sim" in args:
        i = args.index("--min-sim")
        min_sim = float(args[i + 1])
        del args[i : i + 2]

    files = []
    for a in args:
        p = Path(a)
        if p.is_dir():
            files.extend(sorted(p.glob("*.json")))
        elif p.is_file():
            files.append(p)
    if not files:
        print("no export files found", file=sys.stderr)
        return 2

    agents = []
    for f in files:
        try:
            data = json.loads(f.read_text())
        except json.JSONDecodeError as e:
            print(f"skip {f}: bad json ({e})", file=sys.stderr)
            continue
        if data.get("format") != "iranti/export/v1":
            print(f"skip {f}: not an iranti export", file=sys.stderr)
            continue
        agents.append(data)

    if len(agents) < 1:
        print("no valid iranti exports", file=sys.stderr)
        return 2

    report = build_report(agents, top, min_sim)
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
