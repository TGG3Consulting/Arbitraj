"""Resolve the protocol and the network before reading any address in shared Llama adapters."""
import argparse
import hashlib
import json
from pathlib import Path
import re

import requests


REV = "be9967c3f0d3ccd06c871bab0d16beb3304de719"
FILES = ("factory/uniV2.ts", "factory/uniV3.ts")
GITHUB = "https://raw.githubusercontent.com/DefiLlama/dimension-adapters/"


def section(lines, key, module):
    start = next(i for i, line in enumerate(lines) if line.startswith("const " + key + ":"))
    end = next(i for i in range(start + 1, len(lines)) if lines[i].rstrip() in ("}", "};"))
    markers = {f'  {quote}{module}{quote}: {{' for quote in (chr(34), chr(39))}
    entries = [i for i in range(start + 1, end) if lines[i].rstrip() in markers]
    if not entries:
        return []
    if len(entries) != 1:
        raise ValueError(("duplicate source key", module))
    line = entries[0]
    stop = next(i for i in range(line + 1, end) if lines[i].rstrip() in ("  },", "  }"))
    return list(enumerate(lines[line:stop + 1], line + 1))


def scope(lines, source, module, network):
    tag = "[CHAIN." + network.upper() + "]"
    for category in ("configs", "subgraphConfigs", "feesConfigs") if source == FILES[0] else ("configs", "feesConfigs"):
        block = section(lines, category, module)
        if not block:
            continue
        matches = [(n, line.strip()) for n, line in block if line.lstrip().startswith(tag + ":")]
        if len(matches) > 1:
            raise ValueError(("multiple network entries", network, module))
        if not matches:
            return {"kind": "absent", "lines": [block[0][0], block[-1][0]]}
        n, line = matches[0]
        out = {"kind": "subgraph" if category == "subgraphConfigs" else "other", "lines": [n, n]}
        if category in ("configs", "feesConfigs"):
            field = re.match(re.escape(tag) + r""":\s*\{\s*factory:\s*['"](0x[0-9a-fA-F]{40})['"]""", line)
            if field:
                out.update(kind="fee_factory" if category == "feesConfigs" else "factory", factory=field[1].lower())
        return out
    raise ValueError(("protocol not in source registries", module))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshots", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    sources = {}
    evidence = {"revision": REV, "source_sha256": {}, "networks": {}}
    for path in FILES:
        reply = requests.get(GITHUB + REV + "/" + path, timeout=25)
        reply.raise_for_status()
        evidence["source_sha256"][path] = hashlib.sha256(reply.content).hexdigest()
        sources[path] = reply.content.decode().splitlines()
    for network in ("base", "ethereum"):
        directory = args.snapshots / (network + "-integration-2026-11-26") / "sources"
        adapters = json.loads((directory / "llama-adapters.json").read_bytes())["rows"]
        found = {}
        for row in adapters:
            path = row.get("path")
            if path not in FILES:
                continue
            assert row["link"].endswith("/" + REV + "/" + path)
            found[row["id"]] = dict(module=row["module"], path=path, **scope(sources[path], path, row["module"], network))
        evidence["networks"][network] = found
    args.output.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf8")


if __name__ == "__main__":
    main()
