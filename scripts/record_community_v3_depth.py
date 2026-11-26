"""Capture both sides of all CommunityV3 pools at the factory snapshot, using independent eth_call requests."""
import argparse
import json
import time
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata
from record_metric_chains import VIRTUAL
from v3_factory_storage import verify_directory

FIXTURES = ROOT / "tests/data/first_run_dex_2026-11-26"
GAS = 16_777_216
RPC_BATCH_LIMIT = 3  # dRPC public endpoint rejects larger batches before executing them.


def request_batch(session, endpoint, calls):
    # Each response has its own gas budget and state, even for two amounts in the same pool.
    collected = []
    for start in range(0, len(calls), RPC_BATCH_LIMIT):
        group = calls[start:start + RPC_BATCH_LIMIT]
        payload = [dict(jsonrpc="2.0", id=i, method=method, params=params)
                   for i, (method, params) in enumerate(group)]
        for attempt in range(4):
            try:
                response = session.post(endpoint, json=payload, timeout=55)
                response.raise_for_status()
                replies = {r["id"]: r for r in response.json()}
                if len(replies) != len(group):
                    raise ValueError("incomplete batch")
                ordered = [replies[i] for i in range(len(group))]
                for reply in ordered:
                    if error := reply.get("error"):
                        msg = str(error.get("message", "")).lower()
                        if not any(x in msg for x in ("revert", "out of gas", "gas required exceeds")):
                            raise ValueError(f"RPC did not execute call: {msg[:140]}")
                collected.extend(ordered)
                break
            except (requests.RequestException, KeyError, ValueError):
                if attempt == 3:
                    raise
                time.sleep(2 ** attempt)
    return collected


def collect():
    evidence = json.loads((FIXTURES / "community_v3_ethereum.json").read_bytes())
    proof = json.loads((FIXTURES / "community_v3_storage.json").read_bytes())
    if proof["selector"] != evidence["selector"] or proof["block"] != evidence["block"]:
        raise ValueError("storage proof and factory data refer to different blocks")
    verify_directory({**proof, "pools": evidence["pools"],
                      "tiers": {tier: entry["spacing"] for tier, entry in evidence["tiers"].items()}},
                     evidence["registry"])
    selector = evidence["selector"]
    endpoint = json.loads((FIXTURES / "ethereum.json").read_bytes())["endpoint"]
    pools = evidence["pools"]
    tokens = sorted({t for p in pools for t in p["tokens"]})
    runtime = (ROOT / "contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x")
    runtime_hash = "0x" + keccak(bytes.fromhex(runtime)).hex()
    if runtime_hash != evidence["execution"]["virtual_quoter_keccak"]:
        raise ValueError("archived executor used a different virtual quoter")
    with requests.Session() as session:
        def batch(calls):
            return request_batch(session, endpoint, calls)
        header, chain = batch([("eth_getBlockByHash", [selector["blockHash"], False]), ("eth_chainId", [])])
        if (int(chain["result"], 16) != 1 or header["result"]["hash"] != selector["blockHash"]
                or int(header["result"]["number"], 16) != evidence["block"]
                or header["result"]["stateRoot"] != proof["state_root"]):
            raise ValueError("RPC did not return the requested Ethereum block")
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
    return dict(chain=1, selector=selector, height=evidence["block"], state_root=header["result"]["stateRoot"],
                factory=evidence["registry"], quoter=runtime_hash, gas=GAS, decimals=decimals,
                missing_decimals=missing, markets=markets)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(collect(), indent=2) + "\n", encoding="utf-8", newline="\n")
