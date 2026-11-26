"""Capture both sides of all CommunityV3 pools at the factory snapshot, using independent eth_call requests."""
import argparse
import json
from pathlib import Path

from community_v3_rpc import Readers
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata
from record_metric_chains import VIRTUAL
from v3_factory_storage import verify_directory

FIXTURES = ROOT / "tests/data/first_run_dex_2026-11-26"
GAS = 16_777_216


def collect(cache_file):
    evidence = json.loads((FIXTURES / "community_v3_ethereum.json").read_bytes())
    proof = json.loads((FIXTURES / "community_v3_storage.json").read_bytes())
    if proof["selector"] != evidence["selector"] or proof["block"] != evidence["block"]:
        raise ValueError("storage proof and factory data refer to different blocks")
    verify_directory({**proof, "pools": evidence["pools"],
                      "tiers": {tier: entry["spacing"] for tier, entry in evidence["tiers"].items()}},
                     evidence["registry"])
    selector = evidence["selector"]
    pools = evidence["pools"]
    tokens = sorted({t for p in pools for t in p["tokens"]})
    runtime = (ROOT / "contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x")
    runtime_hash = "0x" + keccak(bytes.fromhex(runtime)).hex()
    if runtime_hash != evidence["execution"]["virtual_quoter_keccak"]:
        raise ValueError("archived executor used a different virtual quoter")
    ref = evidence["execution"]["pools"][0]
    target = next(p for p in pools if p["pool"] == ref["pool"])
    sample = ref["cases"][0]
    size = int(sample["input"])
    data = calldata("quoteExactIn(address,bool,uint256)", ["address","bool","uint256"],
                    [target["pool"], sample["side"] == 0, size])
    probe = ("eth_call", [{"to": VIRTUAL, "data": data, "gas": hex(GAS)}, selector, {VIRTUAL: {"code": "0x" + runtime}}])
    expected = "0x" + size.to_bytes(32,"big").hex() + int(sample["bought"]).to_bytes(32,"big").hex()
    with Readers(cache_file, selector, evidence["block"], proof["state_root"], probe, expected) as readers:
        batch = readers.batch
        metadata = batch([("eth_call", [{"to": a, "data": calldata("decimals()")}, selector]) for a in tokens])
        decimals = {}
        missing = {}
        for address, reply in zip(tokens, metadata, strict=True):
            data = bytes.fromhex(reply.get("result", "0x")[2:])
            if len(data) == 32 and int.from_bytes(data, "big") <= 255:
                decimals[address] = int.from_bytes(data, "big")
            else:
                missing[address] = reply.get("error", {"returnData": reply.get("result")})
        work = []
        markets = []
        for pool in pools:
            market = {"pool": pool["pool"], "tokens": pool["tokens"], "quotes": []}
            markets.append(market)
            for side, asset in enumerate(pool["tokens"]):
                if asset not in decimals:
                    continue
                unit = 10 ** decimals[asset]
                for amount in (unit, 10 * unit):
                    row = {"side": side, "input": str(amount)}
                    market["quotes"].append(row)
                    if amount > 2**255 - 1:
                        row["skipped"] = "amount exceeds int256"
                        continue
                    data = calldata("quoteExactIn(address,bool,uint256)", ["address", "bool", "uint256"],
                                    [pool["pool"], side == 0, amount])
                    tx = {"to": VIRTUAL, "data": data, "gas": hex(GAS)}
                    work.append((row, ("eth_call", [tx, selector, {VIRTUAL: {"code": "0x" + runtime}}])))
        for (row, _), reply in zip(work, batch([req for _, req in work]), strict=True):
            if error := reply.get("error"):
                row["error"] = error
            else:
                raw = bytes.fromhex(reply["result"][2:])
                if len(raw) != 64:
                    raise ValueError("virtual quoter returned malformed data")
                used, out = decode(["uint256", "uint256"], raw)
                if used > int(row["input"]):
                    raise ValueError("quoter exceeded the requested input")
                row.update(used=str(used), out=str(out))
        providers = {node.url: node.requests for node in readers.nodes}
        cached = readers.cached
    return dict(chain=1, selector=selector, height=evidence["block"], state_root=proof["state_root"],
                verified_providers=providers, cached_reads=cached,
                factory=evidence["registry"], quoter=runtime_hash, gas=GAS, decimals=decimals,
                missing_decimals=missing, markets=markets)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    cache_file = args.output.with_suffix(".sqlite")
    args.output.write_text(json.dumps(collect(cache_file), indent=2) + "\n", encoding="utf-8", newline="\n")
