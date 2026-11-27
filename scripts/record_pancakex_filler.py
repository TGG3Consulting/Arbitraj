"""Публично исполненные эксклюзивные PCSX ордера в историческом блоке. Новый filler расходует собственную модельную кассу."""
import argparse
import gzip
import json
from datetime import datetime, timezone
from pathlib import Path

from eth_account import Account
from record_gladius_fills import Reader, FILL, ZERO, addr, call_data, canonical
from record_gladius_inventory import SWAPS, TOKENS, approve, funding, tx
from record_pancakex_history import PCS

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
M3 = "0xca11bde05977b3631167028862be2a173976ca11"


def clock(s):
    return int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp())


def record(net):
    base = json.loads(gzip.decompress((DATA / f"pancakex_{net}_history.json.gz").read_bytes()))
    src = next(r for r in base["history"] if r["order"]["type"] == "EXCLUSIVE_DUTCH_ORDER")
    o = src["order"]
    expiry = clock(o["deadline"])
    assert expiry < datetime.now(timezone.utc).timestamp() - 60
    who = Account.create().address.lower()
    reactor, maker = o["reactor"].lower(), o["offerer"].lower()
    assert who not in [maker, o["exclusiveFiller"].lower(), src["transaction"]["from"].lower()]
    reader = Reader(SWAPS[net])
    head = reader.rpc("eth_getBlockByHash", [src["header"]["parentHash"], False])
    assert head["hash"] == src["header"]["parentHash"]
    proceeds, spend = o["input"]["token"].lower(), o["outputs"][0]["token"].lower()
    payload = tuple(bytes.fromhex(o[k][2:]) for k in ["encodeOrder", "signature"])
    entry = "executeExclusiveDutch" if reactor == PCS else "execute"
    execute = call_data(entry + "((bytes,bytes))", ["(bytes,bytes)"], [payload])
    setup = [] if spend == ZERO else funding(who, spend, *TOKENS[net])
    def balance(asset, owner):
        fn, token = ("getEthBalance(address)", M3) if asset == ZERO else ("balanceOf(address)", asset)
        return tx(who, token, call_data(fn, ["address"], [owner]))
    balances = [balance(a, p) for a, p in [(spend, who), (proceeds, who), (spend, maker),
                                             (proceeds, maker), (spend, reactor)]]
    def attempt(stamp, cap):
        init = setup if spend == ZERO else setup + [approve(who, spend, reactor, cap)]
        action = tx(who, reactor, execute, cap if spend == ZERO else 0)
        calls = init + balances + [action] + balances + [action] + balances
        block = {"blockOverrides":{"time":hex(stamp)}, "stateOverrides":{who:{"balance":hex(10**19)}}, "calls":calls}
        result, = reader.rpc("eth_simulateV1", [{"validation":False, "blockStateCalls":[block]},canonical(head)])
        assert result["parentHash"] == head["hash"] and int(result["timestamp"],16) == stamp
        res = result["calls"]
        assert len(res) == len(calls) and all(v["status"]=="0x1" for v in res[:len(init)])
        n = len(init)
        def read(idx):
            a = res[idx:idx+len(balances)]
            assert all(row["status"]=="0x1" for row in a)
            return [int(row["returnData"],16) for row in a]
        before, after, last = read(n), read(n+6), read(n+12)
        use, twice = res[n+5], res[n+11]
        ok = use["status"]=="0x1"
        assert twice["status"]=="0x0" and after == last
        paid, received = before[0]-after[0], after[1]-before[1]
        if ok:
            fills = [l for l in use["logs"] if l["address"].lower()==reactor and l["topics"][:2]==[FILL,o["hash"]]]
            assert len(fills)==1 and addr(fills[0]["topics"][2])==who
            assert paid > 0 and received > 0 and paid <= cap
            assert (paid,received,before[4])==(after[2]-before[2],before[3]-after[3],after[4])
        else:
            assert before==after
        return {"timestamp":stamp,"cap":str(cap),"before":list(map(str,before)),"after":list(map(str,after)),
                "paid":str(paid),"received":str(received),"setup_gas":sum(int(z["gasUsed"],16) for z in res[:n]),
                "gas":int(use["gasUsed"],16),"ok":ok,"error":use.get("error"),"duplicate_error":twice.get("error"),
                "logs":use.get("logs",[])}
    cap = int(o["outputs"][0]["startAmount"]) * 103 // 100 + 1
    stamps = {"earliest_child":max(int(src["header"]["timestamp"],16),int(head["timestamp"],16)+1), "after_exclusivity":max(clock(o["decayStartTime"])+1,int(head["timestamp"],16)+1),
              "last_second":expiry,"expired":expiry+1}
    rows = {key:attempt(stamp,cap) for key,stamp in stamps.items()}
    for key,row in list(rows.items()):
        if row["ok"]:
            rows[key+"_cap_minus_one"]=attempt(row["timestamp"],int(row["paid"])-1)
            assert not rows[key+"_cap_minus_one"]["ok"]
    assert not rows["expired"]["ok"]
    return {"network":net,"historical_tx":src["transaction"]["hash"],"parent":canonical(head),"original_timestamp":int(src["header"]["timestamp"],16),"order_hash":o["hash"],
            "filler":who,"exclusive":o["exclusiveFiller"],"entry":entry,"paid_token":spend,"received_token":proceeds,
            "cash_seed_wei":str(10**19),"override_bps":o["exclusivityOverrideBps"],"cases":rows}


if __name__=="__main__":
    p=argparse.ArgumentParser()
    p.add_argument("network",choices=SWAPS)
    p.add_argument("output",type=Path)
    args=p.parse_args()
    args.output.write_text(json.dumps(record(args.network),separators=(",",":")),encoding="utf8")
