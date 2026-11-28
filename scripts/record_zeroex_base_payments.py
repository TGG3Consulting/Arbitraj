"""Сверяет квитанции двух Settler 0x Base с маркерами RFQ, денежными переводами и хешами блоков."""
import argparse
import gzip
import json
from collections import defaultdict
from pathlib import Path

from fork_fee_rpc import Reader, URLS
from record_base_zeroex_rfq import TRANSFER, identify

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"


def marker_identity(log):
    """Сопоставляет поля журнальной записи EVM. Необязательные метаданные зависят от узла."""
    if log.get("removed", False) is not False:
        raise ValueError("Removed log")
    return (log["blockHash"].lower(), log["transactionHash"].lower(), int(log["logIndex"], 16),
            log["address"].lower(), tuple(t.lower() for t in log["topics"]), log["data"].lower())


def settlement(tx, receipt, markers):
    if tx["hash"] != receipt["transactionHash"] or tx["blockHash"] != receipt["blockHash"]:
        raise ValueError("Inconsistent transaction/receipt")
    receipt_logs = {marker_identity(log) for log in receipt["logs"]}
    if int(tx["blockNumber"], 16) != int(receipt["blockNumber"], 16) or any(
            log["blockHash"] != receipt["blockHash"] or log["transactionHash"] != tx["hash"]
            or int(log["blockNumber"], 16) != int(receipt["blockNumber"], 16) for log in receipt["logs"]):
        raise ValueError("Inconsistent receipt log position")
    marks = [marker_identity(m) for m in markers]
    if (receipt["status"] != "0x1" or len(set(marks)) != len(marks)
            or not all(k in receipt_logs and k[:2] == (receipt["blockHash"], tx["hash"]) for k in marks)):
        raise ValueError("Missing settled RFQ marker")
    tokens = {l["address"] for l in receipt["logs"] if len(l["topics"]) == 3 and l["topics"][0] == TRANSFER}
    legs = []
    for address in sorted({m["address"] for m in markers}):
        proof = identify(tx, receipt, address)
        if proof:
            leg = {k: proof[k] for k in ("maker", "settler", "matchedCalldata")}
            for role in ("marker", "makerInput", "makerOutput"):
                leg[role] = {key: proof[role][key] for key in
                             ("blockHash", "blockNumber", "transactionHash", "logIndex", "address", "topics", "data")}
            legs.append(leg)
    return {"transaction": tx["hash"], "block": int(receipt["blockNumber"], 16),
            "blockHash": receipt["blockHash"], "tokens": sorted(tokens),
            "markers": [{"logIndex": i, "settler": addr, "data": payload} for _, _, i, addr, _, payload in sorted(marks)],
            "legs": legs}


def get_logs(reader, fields, start, end):
    try:
        return reader.one("eth_getLogs", [{**fields, "fromBlock": hex(start), "toBlock": hex(end)}])
    except ValueError:
        if end - start < 1000:
            raise
        mid = (start + end) // 2
        return get_logs(reader, fields, start, mid) + get_logs(reader, fields, mid + 1, end)


def capture(reader=None):
    head = json.loads((DATA / "base.json").read_bytes())
    roles = json.loads((DATA / "zeroex_base_registry_roles.json").read_bytes())["observations"][-1]
    if roles["selector"] != head["selector"]:
        raise ValueError("The role recording and market recording require the same block")
    addresses = ["0x" + roles[k]["response"]["result"][-40:] for k in ("current", "previous")]
    cash = json.loads(gzip.decompress((DATA / "base_usdt_cycle.json.gz").read_bytes()))["cash"]
    height = head["block"]
    URLS["base"].insert(0, "https://base.api.pocket.network")
    reader = reader or Reader("base", head)
    anchor = reader.one("eth_getBlockByNumber", [hex(height), False])
    if anchor["hash"] != head["selector"]["blockHash"] or int(anchor["number"], 16) != height:
        raise ValueError("Noncanonical reference block")
    by_tx = defaultdict(list)
    windows = []
    for start in range(height - 43199, height + 1, 3600):
        end = start + 3599
        logs = get_logs(reader, {"address": addresses}, start, end)
        marks = [l for l in logs if not l["topics"] and len(l["data"]) == 98]
        for marker in marks:
            by_tx[marker["transactionHash"]].append(marker)
        windows.append([start, end, len(marks)])
    hashes = sorted(by_tx)
    txs = reader.many(("eth_getTransactionByHash", [h]) for h in hashes)
    receipts = reader.many(("eth_getTransactionReceipt", [h]) for h in hashes)
    if any(t["hash"] != h for h, t in zip(hashes, txs, strict=True)):
        raise ValueError("Unexpected transaction returned by node")
    rows = [settlement(t, r, by_tx[h]) for h, t, r in zip(hashes, txs, receipts, strict=True)]
    blocks = sorted({(r["block"], r["blockHash"]) for r in rows})
    headers = reader.many(("eth_getBlockByNumber", [hex(n), False]) for n, _ in blocks)
    if any(block["hash"] != h or int(block["number"], 16) != n for (n, h), block in zip(blocks, headers, strict=True)):
        raise ValueError("The receipt belongs to a different block")
    makers = sorted({leg["maker"] for r in rows for leg in r["legs"]})
    if not makers:
        raise ValueError("No confirmed maker in this snapshot")
    topics = ["0x" + a[2:].zfill(64) for a in makers]
    transfers = []
    for day in range(30):
        end = height - 43200 * day
        start = end - 43199
        directions = ([TRANSFER, topics], [TRANSFER, None, topics])
        hashes = [sorted({l["transactionHash"] for l in get_logs(reader, {"address": cash, "topics": t}, start, end)})
                  for t in directions]
        transfers.append([start, end, *hashes])
    return {"chainId": 8453, "selector": reader.tag, "height": height, "settlers": addresses, "cash": cash,
            "canonicalBlocks": dict(blocks), "windows": windows, "transactions": rows, "makers": makers, "makerCashTransfers": transfers}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_bytes(gzip.compress(json.dumps(capture(), separators=(",", ":")).encode(), mtime=0))
