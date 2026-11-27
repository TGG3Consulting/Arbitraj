"""Attribute the addresses in DefiLlama #799 to payment roles from canonical, historical receipts."""
import argparse
import gzip
import json
from datetime import datetime, timezone
from pathlib import Path

import requests
import rlp
from eth_utils import keccak

from fork_fee_rpc import Reader
from record_gladius_fills import FILL, REACTOR
from record_mooniswap import CanonicalCalls

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
ETH_FILLER = "0xf08db8d79312ce610aeed9463ede1a6bb8ae4235"
URL = "https://gladius.rubicon.finance/dutch-auction/orders"


def raw(value):
    return bytes.fromhex(value.removeprefix("0x"))


def receipt_hash(rec):
    logs = [[raw(v["address"]), [raw(t) for t in v["topics"]], raw(v["data"])] for v in rec["logs"]]
    payload = rlp.encode([int(rec["status"], 16), int(rec["cumulativeGasUsed"], 16), raw(rec["logsBloom"]), logs])
    kind = int(rec["type"], 16)
    return "0x" + keccak((bytes([kind]) if kind else b"") + payload).hex()


def open_orders(chain):
    stamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    try:
        with requests.get(URL, params={"chainId": chain, "orderStatus": "open"}, timeout=12, stream=True) as response:
            result = dict(at=stamp, status=response.status_code)
            if response.ok:
                body = bytearray()
                for chunk in response.iter_content(8192):
                    body.extend(chunk)
                    if len(body) > 512_000:
                        return dict(**result, observation="response exceeds snapshot size")
                orders = json.loads(body)["orders"]
                result["with_encoded_order_and_signature"] = sum(
                    isinstance(o, dict) and bool(o.get("encodedOrder")) and bool(o.get("signature"))
                    for o in orders
                )
            return result
    except (requests.RequestException, ValueError, KeyError, TypeError) as error:
        return dict(at=stamp, error=type(error).__name__)


def capture(network):
    history = json.loads(gzip.decompress((DATA/f"gladius_{network}_history.json.gz").read_bytes()))
    reader = Reader(network, json.loads((DATA/f"{network}.json").read_bytes()))
    if reader.tag != history["selector"]:
        raise ValueError("history does not share registry head")
    rows = history["samples"]
    receipts = reader.many([("eth_getTransactionReceipt", [q["hash"]]) for q in rows])
    blocks = reader.many([("eth_getBlockByNumber", [hex(q["height"]), False]) for q in rows])
    records = []
    for q, rec, block in zip(rows, receipts, blocks, strict=True):
        if rec["status"] != "0x1" or block["hash"] != q["block"] or rec["blockHash"] != q["block"] or rec["transactionHash"] != q["hash"]:
            raise ValueError("detached payment")
        index = int(rec["transactionIndex"], 16)
        if index != q["index"] or block["transactions"][index] != q["hash"]:
            raise ValueError("different included transaction")
        joins = [v for v in rec["logs"] if len(v["topics"]) == 4 and v["topics"][:2] == [FILL, q["order_hash"]]]
        if len(joins) != 1:
            raise ValueError("ambiguous Fill")
        log, = joins
        filler, maker = ["0x" + t[-40:].lower() for t in log["topics"][2:]]
        if (log["address"].lower(), filler, maker, int(log["data"], 16)) != (REACTOR, q["filler"], q["maker"], int(q["nonce"])):
            raise ValueError("wrong payment role")
        records.append(dict(tx=q["hash"], block=q["block"], receiptsRoot=block["receiptsRoot"], index=index,
                            receiptKeccak=receipt_hash(rec), emitter=REACTOR, filler=filler, maker=maker,
                            order=q["order_hash"], nonce=q["nonce"], expiry=q["deadline"], exclusive=q["exclusive"]))
    result = dict(chain=history["chain"], selector=history["selector"], records=records,
                  current_public_get=open_orders(history["chain"]))
    if network == "ethereum":
        code = reader.one("eth_getCode", [ETH_FILLER, reader.tag])
        reactor, = CanonicalCalls(reader).one(ETH_FILLER, "reactor()", returns=["address"])
        if reactor != REACTOR or not any(q["filler"] == ETH_FILLER for q in records):
            raise ValueError("filler binding")
        result["source_second_address"] = dict(address=ETH_FILLER, reactor=reactor, bytes=len(raw(code)),
                                               runtime="0x"+keccak(raw(code)).hex())
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps({n: capture(n) for n in ("base", "ethereum")}, indent=2)+"\n", encoding="utf-8")

