"""Record Ethereum 0x RFQ maker legs, transaction participants and real token movements."""
import argparse
import gzip
import json
from collections import defaultdict
from pathlib import Path

from eth_utils import keccak
from fork_fee_rpc import Reader
from record_zeroex_base_payments import DATA, get_logs, settlement
from zeroex_rfq_format import REGISTRY


def role(reader, signature, at):
    payload = "0x" + (keccak(text=signature)[:4] + (2).to_bytes(32, "big")).hex()
    raw = reader.one("eth_call", [{"to": REGISTRY, "data": payload}, at])
    if len(raw) != 66 or int(raw, 16) <= 0 or int(raw, 16) >= 2**160:
        raise ValueError("Invalid Settler registry role")
    return "0x" + raw[-40:]


def capture():
    head = json.loads((DATA / "ethereum.json").read_bytes())
    reader = Reader("ethereum", head)
    h = head["block"]
    settlers = [role(reader, method, reader.tag) for method in ("ownerOf(uint256)", "prev(uint128)")]
    groups = defaultdict(list)
    start = h - 599
    for log in get_logs(reader, {"address": settlers}, start, h):
        if not log["topics"] and len(log["data"]) == 98:
            groups[log["transactionHash"]].append(log)
    hashes = sorted(groups)
    if not hashes:
        raise ValueError("No RFQ events in reference window")
    txs = reader.many(("eth_getTransactionByHash", [x]) for x in hashes)
    receipts = reader.many(("eth_getTransactionReceipt", [x]) for x in hashes)
    rows = []
    headers = {}
    for key, tx, receipt in zip(hashes, txs, receipts, strict=True):
        if tx["hash"] != key:
            raise ValueError("Wrong transaction")
        row = settlement(tx, receipt, groups[key])
        height = row["block"]
        if height not in headers:
            header = reader.one("eth_getBlockByNumber", [hex(height), False])
            if int(header["number"], 16) != height:
                raise ValueError("Wrong block number")
            headers[height] = header["hash"]
        if headers[height] != tx["blockHash"]:
            raise ValueError("Replaced transaction")
        row.update(payer=tx["from"], target=tx["to"], gasUsed=int(receipt["gasUsed"], 16))
        if row["legs"]:
            row["input"] = tx["input"]
            row["transfers"] = [dict(address=l["address"], topics=l["topics"], data=l["data"])
                                for l in receipt["logs"] if l["address"] in settlers or len(l["topics"]) == 3]
        rows.append(row)
    return {"chainId": head["chain_id"], "fromBlock": start, "toBlock": h, "selector": reader.tag,
            "settlers": settlers, "headers": headers, "transactions": rows}


if __name__ == "__main__":
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("output", type=Path)
    output = args.parse_args().output
    data = capture()
    output.write_bytes(gzip.compress(json.dumps(data, separators=(",", ":")).encode(), mtime=0))
    print(len(data["transactions"]), sum(len(r["legs"]) for r in data["transactions"]))
