"""Archive real Fermi receipts and isolate target-slot predictions from public builder frames.

Run with no arguments to replay saved public frames. --capture replaces them from the
documented public data endpoints. Only read methods are allowed. No wallet/key needed.
"""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import calldata, call, OWNER

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "tests/data/first_run_dex_2026-11-26/fermi_ethereum.json"
RPC = "https://eth.drpc.org"
BUILDERS = {"Titan": ("https://eu.data.titanbuilder.xyz/data", "titan_getPammVenues"),
            "BuilderNet": ("https://rpc.buildernet.org/pamm/rpc", "buildernet_getPammStateOverrides")}
DEPLOYMENTS = {
    "0xb1076fe3ab5e28005c7c323bac5ac06a680d452e": "0x596bf970265aef6ba59fdef5afd1f15a36d689021b7a8d6185c658e8f57002bd",
    "0x5979458912f80b96d30d4220af8e2e4925a33320": "0x08c1de6b867595f487f0441af9b4410a6d60d2496db170863d7ce6415d82fdd8",
    "0xf6d5b2fefec7b420d9990b95790adfd3d4c4f977": "0x2fd7c6ce3a0992b6c216037e92f9d9ebf3cc7602b4c84d4337ea985875827694",
}
ACTIVE = "0xf6d5b2fefec7b420d9990b95790adfd3d4c4f977"
ENGINE = "0x10e1a2b624e14ac59426199e520320313a98b0be"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
UNIV2 = "0x7a250d5630b4cf539739df2c5dacb4c659f2488d"
ARGS = ["address", "address", "int256"]
QUOTE = "quoteAmounts(address,address,int256)"
SWAP = "fermiSwapWithAllowances(address,address,int256,uint256,address)"
SWAP_EVENTS = ["FermiSwap(address,address,address,uint256,uint256)", "Swapped(address,address,address,uint256,uint256,address)"]


def digest(data):
    return "0x" + keccak(data).hex()


def request(url, method, params):
    if (url, method) not in BUILDERS.values() and not (url == RPC and method in {
        "eth_chainId", "eth_getBlockByNumber", "eth_getCode", "eth_getTransactionReceipt",
        "eth_call", "eth_simulateV1", "eth_getLogs"
    }):
        raise ValueError("read method required")
    response = requests.post(url, json=dict(jsonrpc="2.0", id=1, method=method, params=params), timeout=20)
    response.raise_for_status()
    result = response.json()
    if "error" in result:
        raise ValueError((method, result["error"]))
    return result["result"]


def rpc(method, params):
    return request(RPC, method, params)


def block(number):
    b = rpc("eth_getBlockByNumber", [hex(number), False])
    return {k: b[k] for k in ("number", "hash", "timestamp", "parentHash")}


def selector(b):
    return {"blockHash": b["hash"], "requireCanonical": True}


def header_frame(name, result):
    if name == "Titan":
        row = next(v for v in result["venues"] if v["maker"] == "Fermi" and v["quoteAddress"].lower() == ACTIVE)
        number, slot = int(row["blockNumber"]), int(row["slot"])
    else:
        row = result[ACTIVE]
        number, slot = int(result["blockNumber"], 16), int(result["slot"])
    # Only the documented engine storage is relevant. Never apply balance/nonce/code
    # or token/wallet storage updates from a builder to a quote simulation.
    diff = row["stateOverride"][ENGINE]["stateDiff"]
    if not diff or len(diff) > 128 or any(len(bytes.fromhex(h[2:])) != 32 for pair in diff.items() for h in pair):
        raise ValueError("invalid Fermi stateDiff")
    return dict(source=BUILDERS[name][0], method=BUILDERS[name][1], slot=slot, target=number, stateDiff=diff)


def historical(router, txid):
    r = rpc("eth_getTransactionReceipt", [txid])
    b = block(int(r["blockNumber"], 16))
    if r["status"] != "0x1" or b["hash"] != r["blockHash"]:
        raise ValueError("receipt is no longer canonical")
    tag = selector(b)
    getter = lambda sig: "0x" + rpc("eth_call", [{"to": router, "data": calldata(sig)}, tag])[-40:]
    vault, engine = getter("traderVault()"), getter("fermi()")
    topics = [digest(v.encode()) for v in SWAP_EVENTS]
    ev = next(x for x in r["logs"] if x["address"] == router and x["topics"][0] in topics)
    legacy = ev["topics"][0] == topics[0]
    indexed = ["0x" + x[-40:] for x in ev["topics"][1:]]
    value = bytes.fromhex(ev["data"][2:])
    amounts = decode(["uint256", "uint256"] if legacy else ["uint256", "uint256", "address"], value)
    recipient = indexed[0] if legacy else amounts[2]
    transfers = []
    for token, amount, incoming in [(indexed[1], amounts[0], True), (indexed[2], amounts[1], False)]:
        matches = [x for x in r["logs"] if x["address"] == token and
                   x["topics"][0] == digest(b"Transfer(address,address,uint256)") and
                   len(x["topics"]) == 3 and int(x["data"], 16) == amount and
                   "0x" + x["topics"][2 if incoming else 1][-40:] == vault and
                   (incoming or "0x" + x["topics"][2][-40:] == recipient)]
        if not matches:
            raise ValueError(("vault transfer mismatch", router, token))
        transfers.append(matches[0])
    runtime = bytes.fromhex(rpc("eth_getCode", [router, tag])[2:])
    return dict(block=b, router=router, runtime_keccak=digest(runtime), engine=engine, trader_vault=vault,
                transaction=txid, transaction_to=r["to"], event=ev, transfers=transfers)


def ownership_window():
    counts = dict.fromkeys(DEPLOYMENTS, 0)
    for end in range(26494988, 26494988-637, -91):
        logs = rpc("eth_getLogs", [dict(fromBlock=hex(end-90), toBlock=hex(end), address=list(DEPLOYMENTS),
                                        topics=[[digest(x.encode()) for x in SWAP_EVENTS]])])
        for x in logs:
            counts[x["address"]] += 1
    return dict(first=block(26494988-636), last=block(26494988), event_counts=counts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", action="store_true")
    args = parser.parse_args()
    if rpc("eth_chainId", []) != "0x1":
        raise ValueError("wrong chain")
    frames = ({n: header_frame(n, request(url, method, [])) for n, (url, method) in BUILDERS.items()}
              if args.capture else json.loads(OUT.read_text("utf-8"))["frames"])
    from record_fermi_swaps import simulation
    out = dict(chain_id=1, rpc=RPC, router=ACTIVE, engine=ENGINE, owner=OWNER, weth=WETH, usdt=USDT,
               primary="https://docs.titanbuilder.xyz/propamms", frames=frames,
               historical=[historical(k, v) for k, v in DEPLOYMENTS.items()], window=ownership_window(),
               simulations={k: simulation(v) for k, v in frames.items()})
    OUT.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
