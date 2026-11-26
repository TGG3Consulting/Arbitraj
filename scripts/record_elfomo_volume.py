"""Считает обе стороны сделок ElfomoFi по сумме одной ноги USDC за 24 часа до контрольного блока."""
import hashlib
import json
from collections import defaultdict
from datetime import datetime, timezone

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_deployments import ROOT, read, write

FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
TRADE = "0x" + keccak(text="ElfomoTrade(uint256,uint256,address,address,address,address,uint256,uint256)").hex()


def capture():
    base = read(FOLDER / "base.json")
    elfomo = read(FOLDER / "elfomofi_base.json")
    session = requests.Session()

    def rpc(method, params):
        if method not in ("eth_getBlockByNumber", "eth_getLogs"):
            raise ValueError("Метод за пределами чтения")
        r = session.post(base["endpoint"], json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, timeout=45)
        r.raise_for_status()
        return r.json()["result"]

    def header(num):
        h = rpc("eth_getBlockByNumber", [hex(num), False])
        return dict(number=num, timestamp=int(h["timestamp"], 16), hash=h["hash"], base_fee_wei=int(h["baseFeePerGas"], 16))

    end = header(base["block"])
    if end["hash"] != base["selector"]["blockHash"]:
        raise ValueError("Изменился канонический блок")
    time_lo = end["timestamp"] - 86400
    lo, hi = end["number"] - 1_000_000, end["number"]
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if header(mid)["timestamp"] <= time_lo:
            lo = mid
        else:
            hi = mid
    previous, start = header(lo), header(hi)
    if not previous["timestamp"] <= time_lo < start["timestamp"]:
        raise ValueError("Неверная граница окна")
    rows, seen = defaultdict(lambda: dict(count=0, usdc_raw=0)), set()
    pages = []
    for first in range(hi, end["number"] + 1, 1500):
        last = min(end["number"], first + 1499)
        query = dict(address=elfomo["router"], topics=[TRADE], fromBlock=hex(first), toBlock=hex(last))
        logs = rpc("eth_getLogs", [query])
        digest = hashlib.sha256(json.dumps(logs, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        pages.append(dict(first=first, last=last, count=len(logs), sha256=digest))
        for log in logs:
            block = int(log["blockNumber"], 16)
            key = (log["transactionHash"], int(log["logIndex"], 16))
            if key in seen or not first <= block <= last or log.get("removed") or log["address"] != elfomo["router"] or log["topics"][0] != TRADE:
                raise ValueError("Повторное, исключённое или чужое событие")
            seen.add(key)
            _, _, a, b, dx, dy = decode(["address"] * 4 + ["uint256"] * 2, bytes.fromhex(log["data"][2:]))
            row = rows[(a, b)]
            row["count"] += 1
            row["example_tx"] = key[0]
            if a == USDC or b == USDC:
                row["usdc_raw"] += dx if a == USDC else dy
            else:
                row["usdc_raw"] = None
    if header(end["number"])["hash"] != end["hash"]:
        raise ValueError("На отрезке сменился канонический блок")
    return dict(chain_id=8453, router=elfomo["router"], event=TRADE, endpoint=base["endpoint"],
                previous=previous, start=start, end=end, seconds=86400, count=len(seen), pages=pages,
                end_utc=datetime.fromtimestamp(end["timestamp"], timezone.utc).isoformat(),
                markets=[dict(token_in=a, token_out=b, **row) for (a, b), row in sorted(rows.items())])


if __name__ == "__main__":
    evidence = capture()
    write(FOLDER / "elfomo_volume_base.json", evidence)
    print(evidence["count"], len(evidence["markets"]), len(evidence["pages"]))
