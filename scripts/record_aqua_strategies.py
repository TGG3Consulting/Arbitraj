"""Каноническая касса 1inch Aqua за сутки и открытая котировка от отдельного адреса."""
import argparse
import json
from collections import Counter, defaultdict
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from aqua_evidence import EARLY, INPUTS, ORDER, REGISTRY, ROUTER, call, from_transaction, order_record, unpack
from record_base_slipstream_executor import OWNER, ROOT

URLS = {"ethereum": "https://rpc.mevblocker.io", "base": "https://base-rpc.publicnode.com"}
TRANSFER = "0x" + keccak(text="Transfer(address,address,uint256)").hex()


def capture(network, endpoint):
    source = json.loads((ROOT / "tests/data/base_ethereum_networks.json").read_bytes())[network]
    height = source["block"]
    at = {"blockHash": source["hash"], "requireCanonical": True}
    sess = requests.Session()

    def rpc(method, params, optional=False):
        reply = sess.post(endpoint, json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, timeout=45)
        reply.raise_for_status()
        obj = reply.json()
        if "result" not in obj:
            err = obj.get("error", {})
            if optional and (err.get("code") == 3 or "revert" in str(err).lower()):
                return {"error": err}
            raise RuntimeError((method, str(err)[:400]))
        return obj["result"]

    def view(address, name, types, args, sender=None, optional=False):
        tx = {"to": address, "data": call(name, types, args)}
        if sender:
            tx["from"] = sender
        return rpc("eth_call", [tx, at], optional)

    def block(height):
        return rpc("eth_getBlockByNumber", [hex(height), False])

    header = block(height)
    assert header["hash"] == source["hash"] and int(rpc("eth_chainId", []), 16) == (1 if network == "ethereum" else 8453)
    assert view(ROUTER, "AQUA", [], [])[-40:] == REGISTRY[2:]
    minimum = int(header["timestamp"], 16) - 86400
    span = 8192
    while int(block(max(0, height - span))["timestamp"], 16) >= minimum:
        span *= 2
    lo, hi = max(0, height - span), height
    while lo < hi:
        mid = (lo + hi) // 2
        if int(block(mid)["timestamp"], 16) < minimum:
            lo = mid + 1
        else:
            hi = mid
    assert int(block(lo - 1)["timestamp"], 16) < minimum <= int(block(lo)["timestamp"], 16)
    logs = []
    for start in range(lo, height + 1, 32768):
        logs.extend(rpc("eth_getLogs", [{"address": [EARLY, REGISTRY, ROUTER],
            "fromBlock": hex(start), "toBlock": hex(min(height, start + 32767))}]))
    fills, orders, counts = [], {}, Counter()
    by_tx = defaultdict(list)
    for log in sorted(logs, key=lambda x: (int(x["blockNumber"], 16), int(x["logIndex"], 16))):
        name, vals = unpack(log)
        emitter = log["address"].lower()
        counts[(emitter, name or log["topics"][0])] += 1
        if name == "Shipped" and emitter == REGISTRY and vals[1] == ROUTER:
            order = order_record(decode([ORDER], bytes.fromhex(vals[3][2:]))[0])
            assert order and order[0] == vals[2] and order[1]["maker"] == vals[0]
            orders[order[0]] = order[1]
        if name == "Swapped" and emitter == ROUTER:
            row = dict(zip(("hash", "maker", "taker", "tokenIn", "tokenOut", "amountIn", "amountOut"), vals, strict=True))
            row.update(tx=log["transactionHash"], index=int(log["logIndex"], 16), blockHash=log["blockHash"])
            fills.append(row)
            by_tx[row["tx"]].append(row)
    for txid, group in by_tx.items():
        tx = rpc("eth_getTransactionByHash", [txid])
        receipt = rpc("eth_getTransactionReceipt", [txid])
        assert tx["blockHash"] == receipt["blockHash"] == group[0]["blockHash"] and int(receipt["status"], 16) == 1
        orders.update(from_transaction(tx["input"], {row["hash"] for row in group}))
        previous = -1
        for fill in group:
            transfers = []
            for log in receipt["logs"]:
                if not previous < int(log["logIndex"], 16) < fill["index"] or len(log["topics"]) != 3 or log["topics"][0] != TRANSFER:
                    continue
                asset = log["address"].lower()
                sender, recipient = ["0x" + t[-40:] for t in log["topics"][1:]]
                if asset in (fill["tokenIn"], fill["tokenOut"]) and fill["maker"] in (sender, recipient):
                    transfers.append([asset, sender, recipient, int(log["data"], 16)])
            fill.update(transfers=transfers, origin=tx["from"].lower())
            previous = fill["index"]
    observed = {}
    for fill in fills:
        key = (fill["hash"], fill["tokenIn"], fill["tokenOut"])
        if key in observed:
            continue
        h, src, dst = key
        maker = fill["maker"]
        state = dict(order=h, maker=maker, tokenIn=src, tokenOut=dst)
        for label, asset in (("input", src), ("output", dst)):
            raw = view(REGISTRY, "rawBalances", ["address", "address", "bytes32", "address"], [maker, ROUTER, bytes.fromhex(h[2:]), asset])
            virtual, count = decode(["uint248", "uint8"], bytes.fromhex(raw[2:]))
            state[label] = dict(virtual=virtual, tokensCount=count,
                wallet=int(view(asset, "balanceOf", ["address"], [maker]), 16),
                allowance=int(view(asset, "allowance", ["address", "address"], [maker, REGISTRY]), 16))
        if h in orders:
            record = orders[h]
            assert record["maker"] == maker
            order = (maker, int(record["traits"]), bytes.fromhex(record["data"][2:]))
            qty = fill["amountIn"]
            # No taker callback or third-party credential. Use the known test wallet for both msg.sender and origin.
            state["qty"] = qty
            state["quote"] = view(ROUTER, "quote", INPUTS, [order, src, dst, qty, (0x61).to_bytes(22, "big")], OWNER, True)
        observed[key] = state
    assert block(height)["hash"] == header["hash"]
    return dict(network=network, fromBlock=lo, through=height, selector=at, startTimestamp=minimum,
                timestamp=int(header["timestamp"], 16), registry=REGISTRY, router=ROUTER, earlier=EARLY, probe=OWNER,
                counts=[[*k, v] for k, v in counts.items()], orders=orders, directions=list(observed.values()), fills=fills)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("network", choices=URLS)
    ap.add_argument("output", type=Path)
    ap.add_argument("--rpc")
    args = ap.parse_args()
    args.output.write_text(json.dumps(capture(args.network, args.rpc or URLS[args.network]), separators=(",", ":")), encoding="utf8")
