"""Сравнение собственных выплат Goblins Router02, V5 и защищённым модулем на Base."""
import argparse
import json
from pathlib import Path

from fork_fee_rpc import Reader
from record_llama_base_cl import CanonicalCalls, ROOT, registry, wallet


def capture(artifact):
    snapshot = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    reader = Reader("base", snapshot)
    cap = CanonicalCalls(reader)
    book = registry(reader, cap, "goblins", snapshot)
    native = wallet(reader, cap, book.copy())
    protected = wallet(reader, cap, book.copy(), artifact)
    native["cases"].extend(protected["cases"])
    native["unavailable"].extend(protected["unavailable"])
    native["module_hash"] = protected["module_hash"]
    return native


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("artifact", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(capture(json.loads(args.artifact.read_bytes())), separators=(",", ":")) + "\n", encoding="utf8")
