"""Находит инструкции PUSH адресов в runtime из артефактов GH с архивной приёмкой Base PMM."""
import argparse
import json
from pathlib import Path
from eth_utils import keccak

DATA = Path("tests/data/first_run_dex_2026-11-26")
PROFILES = Path("config/first_run_dex.json")


def offsets(code):
    result = {}
    pc = 0
    while pc < len(code):
        length = code[pc] - 0x5f if 0x60 <= code[pc] <= 0x7f else 0
        if length:
            word = code[pc+1:pc+1+length]
            if len(word) <= 32:
                result.setdefault(int.from_bytes(word, "big"), pc)
        pc += 1 + length
    return result


def capture(artifacts):
    profiles = json.loads(PROFILES.read_text(encoding="utf8"))
    versions = {}
    for artifact in artifacts:
        deployed = json.loads(artifact.read_text(encoding="utf8"))["runtime"].removeprefix("0x")
        code = bytes.fromhex(deployed)
        h = "0x" + keccak(code).hex()
        versions[h] = dict(runtime=deployed, artifact=artifact.name, offsets=offsets(code))
    rows = {}
    for profile in profiles:
        if profile["id"] not in ("elfomofi", "baibai", "lunarbase"):
            continue
        settings = profile["settings"]
        targets = [settings["dex.pmm_pool"]]
        if "dex.vault" in settings:
            targets.append(settings["dex.vault"])
        bindings = []
        for h in settings["dex.module_runtime_hashes"]:
            pcs = [versions[h]["offsets"][int(target, 16)] for target in targets]
            bindings.append(" ".join([h, *map(str, pcs)]))
        rows[profile["id"]] = bindings
    for v in versions.values():
        del v["offsets"]
    return dict(versions=versions, bindings=rows)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=Path, nargs="+")
    args = parser.parse_args()
    out = capture(args.artifacts)
    (DATA/"pmm_module_bindings.json").write_text(json.dumps(out, indent=2)+"\n",encoding="utf8")
