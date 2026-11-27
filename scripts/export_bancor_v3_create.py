"""Bind the unsigned Bancor CREATE to the runtime used by the paid archive."""
import argparse
import gzip
import json
from hashlib import sha256
from itertools import permutations
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak


def export(compiled, paid, original, chain):
    roles = original["roles"]
    if original["selector"] != paid["selector"] or paid["selector"] != chain["selector"]:
        raise ValueError("different archive blocks")
    cases = {row["mode"] for row in paid["cases"] if int(row.get("spent", 0)) and int(row.get("received", 0))}
    if cases != {"wallet", "native_wallet", "v5"}:
        raise ValueError("missing paid cash mode")
    wrapped = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
    native = "0x" + "ee" * 20
    names = ("BancorNetwork", "BancorNetworkInfo", "MasterVault")
    args = [roles[name]["address"] for name in names] + [wrapped, native]
    references = list(compiled["immutableReferences"].values())
    if len(references) != len(args) or len(set(args)) != len(args):
        raise ValueError("invalid compiler immutables or roles")
    for values in permutations(args):
        runtime = bytearray.fromhex(compiled["runtime"])
        for offsets, addr in zip(references, values, strict=True):
            for field in offsets:
                start = field["start"]
                if field["length"] != 32 or start < 0 or start + 32 > len(runtime) or any(runtime[start:start+32]):
                    raise ValueError("invalid immutable reference")
                runtime[start:start+32] = encode(["address"], [addr])
        if "0x" + keccak(bytes(runtime)).hex() == paid["module_hash"]:
            break
    else:
        raise ValueError("compiler runtime differs from paid archive")
    creation = bytes.fromhex(compiled["creation"]) + encode(["address"] * len(args), args)
    return dict(kind="bancor_v3", chainId=1, roles=roles, arguments=args, wrappedHash=chain["contracts"][wrapped]["keccak256"],
                sourceBlock=paid["selector"]["blockHash"], runtimeKeccak=paid["module_hash"],
                runtimeSha256=sha256(runtime).hexdigest(), creationSha256=sha256(creation).hexdigest(),
                data="0x" + creation.hex(), solc=compiled["solc"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiled", "paid", "output"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    folder = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
    with gzip.open(folder / "bancor_v3_ethereum.json.gz", "rt") as archive:
        manifest = export(json.loads(args.compiled.read_bytes()), json.loads(args.paid.read_bytes()),
                          json.load(archive), json.loads((folder / "ethereum.json").read_bytes()))
    args.output.write_bytes((json.dumps(manifest, separators=(",", ":")) + "\n").encode())
