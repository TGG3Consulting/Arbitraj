"""Export an Ethereum V1 wallet CREATE only when its deployed runtime reproduces the paid archive."""
import argparse
import json
from hashlib import sha256
from itertools import permutations
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak


def export(compiled, paid, bindings):
    if bindings["selector"] != paid["selector"] or not all(any(
        r["mode"] == mode and int(r.get("received", "0")) > 0 for r in paid["cases"]
    ) for mode in ("wallet", "wallet_pair", "v5", "v5_pair")):
        raise ValueError("missing canonical payment for a wallet form")
    wrapped = bindings["wrapped"]
    roles = {**bindings["roles"], "wrapped": bindings["markets"][wrapped]["token_code"]}
    for name in ("factory", "template"):
        if roles[name]["address"] != paid[name]["address"] or roles[name]["keccak256"] != paid[name]["runtime"]:
            raise ValueError(("inconsistent original", name))
    hashes = {v["exchange_code"]["keccak256"] for v in bindings["markets"].values()}
    if len(hashes) != 1:
        raise ValueError("inconsistent forwarder")
    args = [roles["factory"]["address"], wrapped]
    refs = list(compiled["immutableReferences"].values())
    if len(refs) != 2:
        raise ValueError("unexpected module immutables")
    results = []
    for values in permutations(args):
        runtime = bytearray.fromhex(compiled["runtime"])
        for offsets, address in zip(refs, values, strict=True):
            for field in offsets:
                start, size = field["start"], field["length"]
                if size != 32 or start + size > len(runtime) or any(runtime[start:start + size]):
                    raise ValueError("invalid immutable template")
                runtime[start:start + size] = encode(["address"], [address])
        if "0x" + keccak(bytes(runtime)).hex() == paid["module_hash"]:
            results.append(bytes(runtime))
    if len(results) != 1:
        raise ValueError("compiler runtime and deployed wallet differ")
    creation = bytes.fromhex(compiled["creation"]) + encode(["address", "address"], args)
    return dict(kind="uniswap_v1", chainId=1, network="Ethereum", roles=roles, cloneHash=hashes.pop(),
                templateGetter="0x" + keccak(text="exchangeTemplate()")[:4].hex(),
                sourceBlock=paid["selector"]["blockHash"], runtimeKeccak=paid["module_hash"],
                runtimeSha256=sha256(results[0]).hexdigest(), creationSha256=sha256(creation).hexdigest(),
                data="0x"+creation.hex(), solc=compiled["solc"])


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("compiled", type=Path)
    cli.add_argument("paid", type=Path)
    cli.add_argument("output", type=Path)
    args = cli.parse_args()
    binding = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26/uniswap_v1_ethereum_prices.json"
    manifest = export(*(json.loads(p.read_bytes()) for p in (args.compiled, args.paid, binding)))
    args.output.write_bytes((json.dumps(manifest,separators=(",",":")) + "\n").encode())
