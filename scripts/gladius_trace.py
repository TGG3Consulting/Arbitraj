"""Архив трёх опубликованных внутренних вызовов Gladius в Base без подписи или исходного ордера."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26/gladius_base_trace.json.gz"
OUTER = "0xd0f2d8ac"
FILLER = "0x70d2473f911683fef696fb23ed25540e46d70734"
REACTOR = "0x3c53c04d633bec3fb0de3492607c239bf92d07f9"
PERMIT = "0x000000000022d473030f116ddee9f6b43ac78ba3"
CALLS = {(FILLER, REACTOR, "0x0d335884"), (REACTOR, FILLER, "0x585da628"), (REACTOR, PERMIT, "0x137c29fe")}


def digest(data):
    return "0x" + keccak(bytes.fromhex(data[2:])).hex()


def fragments(tx):
    assert tx["to"] == FILLER and tx["input"].startswith(OUTER)
    (order, witness), extra = decode(["(bytes,bytes)", "bytes"], bytes.fromhex(tx["input"][10:]))
    assert order and witness and extra
    return {"<order>":order.hex(), "<witness>":witness.hex(), "<callback>":extra.hex()}


def compact(raw):
    tx, header = raw["tx"], raw["header"]
    assert tx["blockHash"] == header["hash"] == raw["receipt"]["blockHash"]
    args = fragments(tx)
    frames = []
    for item in raw["trace"]:
        call = item["action"]
        if call.get("callType") != "call" or (call["from"], call["to"], call["input"][:10]) not in CALLS:
            continue
        assert not item.get("error") and int(call["value"], 16) == 0
        data = call["input"]
        for name, value in args.items():
            data = data.replace(value, name)
        assert "<witness>" in data or "<order>" in data
        frames.append({"from":call["from"], "to":call["to"], "data":data,
                       "digest":digest(call["input"]), "path":item["traceAddress"]})
    assert len(frames) == len(CALLS)
    return {"block":header["hash"], "tx_input_digest":digest(tx["input"]), "frames":frames}


def archived_calls(tx, row):
    args = fragments(tx)
    assert tx["blockHash"] == row["block"] and digest(tx["input"]) == row["tx_input_digest"]
    calls = []
    for frame in row["frames"]:
        data = frame["data"]
        for name, value in args.items():
            data = data.replace(name, value)
        assert digest(data) == frame["digest"]
        calls.append({"from":frame["from"], "to":frame["to"], "input":data, "value":"0x0"})
    assert len(calls) == len(CALLS) and {(a["from"], a["to"], a["input"][:10]) for a in calls} == CALLS
    return calls


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("captures", nargs="+", type=Path, help="JSON с header, tx, receipt, trace_transaction")
    parser.add_argument("--out", type=Path, default=DATA)
    args = parser.parse_args()
    rows = {}
    for src in args.captures:
        raw = json.loads(src.read_bytes())
        rows[raw["tx"]["hash"]] = compact(raw)
    args.out.write_bytes(gzip.compress(json.dumps({"rpc":"https://base.drpc.org", "method":"trace_transaction",
                                                   "transactions":rows}, separators=(",",":")).encode(), mtime=0))
