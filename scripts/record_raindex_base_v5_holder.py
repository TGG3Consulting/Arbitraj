"""Archive-only Base V5 ABI control using two historical public holders and their unmodified ERC20 balances."""
import argparse
import gzip
import json
from decimal import Decimal, localcontext
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from raindex_wire import decimal, take
from record_raindex_base_v5 import books_for_replay
from record_raindex_wallets import ROOT, LIMIT, tx

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
METHODS = {"eth_chainId", "eth_getCode", "eth_getBlockByHash", "eth_call", "eth_simulateV1"}


def capture(endpoint, compiled, holders):
    tag = holders["anchor"]
    def rpc(method, params):
        if method not in METHODS:
            raise ValueError("archive method")
        reply = requests.post(endpoint, json={"jsonrpc":"2.0","id":1,"method":method,"params":params}, timeout=18)
        reply.raise_for_status()
        payload = reply.json()
        if "result" not in payload:
            raise ValueError(str(payload.get("error"))[:200])
        return payload["result"]

    if int(rpc("eth_chainId",[]),16) != 8453:
        raise ValueError("chain")
    head = rpc("eth_getBlockByHash", [tag["blockHash"],False])
    if head["hash"] != tag["blockHash"] or int(head["number"],16) != holders["height"]:
        raise ValueError("archive anchor")
    stamp = int(head["timestamp"],16) + 2
    [book] = books_for_replay()
    address = book["book"]
    module = "0x"+keccak(text="Raindex protected isolated wallet "+address)[-20:].hex()
    if rpc("eth_getCode",[module,tag]) != "0x":
        raise ValueError("audit module address occupied")
    args = encode(["address","bytes32","bytes4"],[address,bytes.fromhex(book["runtime"][2:]),bytes.fromhex("d0f7a862")])
    runtime = rpc("eth_call",[{"data":"0x"+compiled["creation"].removeprefix("0x")+args.hex()},tag])
    profile = json.loads(gzip.decompress((DATA/"raindex_base_v5_protected_wallets.json.gz").read_bytes()))
    expected = profile["modules"][address]["runtime"]
    if "0x"+keccak(bytes.fromhex(runtime[2:])).hex() != expected:
        raise ValueError("pinned constructor")
    def run(holder, calls):
        calls = [dict(call, **{"from":holder}) for call in calls]
        [child] = rpc("eth_simulateV1",[{"blockStateCalls":[{
            "blockOverrides":{"time":hex(stamp)},
            "stateOverrides":{module:{"code":runtime}},
            "calls":calls
        }]},tag])
        if child["parentHash"] != tag["blockHash"] or len(child["calls"]) != len(calls):
            raise ValueError("archive child")
        return child["calls"]
    def value(row):
        if row["status"] != "0x1":
            raise ValueError("balance or funding observation reverted")
        return int(row["returnData"],16)
    def balance(token, wh):
        return tx(token,"balanceOf",["address"],[wh])
    def allowance(token):
        return tx(token,"allowance",["address","address"],[module,address])
    cases = []
    for source in holders["cases"]:
        token, holder = source["asset"], source["holder"]
        if rpc("eth_getCode",[holder,tag]) != "0x":
            raise ValueError("historical holder code")
        dec, available = map(value, run(holder, [tx(token,"decimals",[],[]),balance(token,holder)]))
        if available != int(source["asset_balance_raw"]) or available <= 0:
            raise ValueError("historical holder changed")
        cap = min(available,max(1,10**dec//100))
        checks = [balance(token,holder),balance(USDC,holder),balance(token,module),balance(USDC,module),allowance(token)]
        candidates = [(o,l) for o in book["live"] for l in o["legs"] if l["pay"]==token and l["receive"]==USDC]
        for order, leg in candidates:
            with localcontext() as ctx:
                ctx.prec=100
                quantity=int(min(decimal(leg["cap"]),Decimal(cap).scaleb(-dec)/decimal(leg["ratio"])*Decimal("0.9"))*10**6)
            if quantity < 1:
                cases.append({"asset":token,"order":order["hash"],"stage":"below_unit"})
                continue
            wire = take(5,order["order"],leg["i"],leg["j"],quantity,quantity,6,True)
            def payment(minimum,maximum,credit):
                approve=tx(token,"approve",["address","uint256"],[module,maximum])
                fill=tx(module,"fill",[LIMIT,"bytes"],[(token,USDC,minimum,maximum,credit,stamp+300),bytes.fromhex(wire[2:])])
                rows=run(holder,checks+[approve,fill]+checks)
                before=list(map(value,rows[:5])); after=list(map(value,rows[-5:]))
                if rows[5]["status"]!="0x1":raise ValueError("historical allowance")
                return before,after,rows[6]
            before,after,fill=payment(1,cap,quantity)
            row={"asset":token,"order":order["hash"],"i":leg["i"],"j":leg["j"],"holder":holder,"cap":str(cap),"quantity":str(quantity)}
            if fill["status"]!="0x1":
                if before!=after:raise ValueError("rollback")
                cases.append(dict(row,stage="revert",error=fill.get("error")))
                continue
            paid,received=before[0]-after[0],after[1]-before[1]
            if not (0<paid<=cap and received>=quantity and before[2:]==after[2:]):
                raise ValueError("observed settlement")
            b2,a2,f2=payment(paid,paid,received)
            if f2["status"]!="0x1" or (b2,a2)!=(before,after):
                raise ValueError("tight limits")
            b3,a3,f3=payment(paid,paid,received+1)
            if f3["status"]=="0x1" or b3!=a3:raise ValueError("net credit protection")
            cases.append(dict(row,stage="paid",paid=str(paid),net=str(received),before=list(map(str,before)),after=list(map(str,after)),
                              logs=fill["logs"],receipt_reference=source["origin"],tight_reproduced=True,excess_minimum_rolled_back=True))
            break
    return {"height":holders["height"],"selector":tag,"chain":8453,"book":address,"module":module,"runtime":expected,
            "source":"Public prior holders in isolated child of the recorded block. The project payer has zero of these assets.",
            "endpoint":endpoint,"cases":cases}


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiled",type=Path)
    parser.add_argument("output",type=Path)
    opts=parser.parse_args()
    owners=json.loads((DATA/"raindex_base_v5_holder_inputs.json").read_bytes())
    artifact=json.loads(opts.compiled.read_bytes())
    error=None
    for endpoint in ("https://base.gateway.tenderly.co","https://mainnet.base.org"):
        try:
            result=capture(endpoint,artifact,owners)
        except (requests.RequestException,ValueError,KeyError) as exc:
            error=exc
        else:
            opts.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf-8")
            break
    else:
        raise SystemExit(str(error)[:240])
