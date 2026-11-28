"""Archive the same IPOR IBT/PT swap from pre-held shares and across its measured transfer lock."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT, STEP, calldata, rpc
from record_kewlswap_base import create_address
from record_omni_v2_base import ENDPOINTS
from record_woo_base import WETH

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
POOL = "0xbff897282390cb3e76fbb831ee2fd815f3934cde"
MANAGER = "0x6afa67c55fa542ae83b53ebe96e616c079c89077"
HOLDER = "0xcd6b980029e6e6e0733ac8ec3e02be9410d09799"
LOCK = "0x" + keccak(text="AccountIsLocked(uint256)")[:4].hex()
LOCK_EVENT = "0x" + keccak(text="RedemptionDelayForAccountUpdated(address,uint256)").hex()


def archive(name):
    with gzip.open(DATA / f"spectra_base_{name}.json.gz") as file:
        return json.load(file)


def quantity(result):
    if result["status"] != "0x1":
        raise ValueError(f"read reverted: {result.get('error')}")
    return int(result["returnData"], 16)


def replay(endpoint, snapshot, market, old, mode, funded):
    parent = snapshot["selector"]
    timestamp = snapshot["timestamp"]
    owner = old["owner"] if funded else HOLDER
    if rpc(endpoint, "eth_getCode", [owner, parent]) != "0x":
        raise ValueError("recorded payer is not an EOA")
    ibt, pt = market["coin0"], market["coin1"]
    underlying = market["underlying"]
    router = snapshot["router"]
    previous = old["legs"][0]
    qty = int(previous["qty"])
    nonce = int(rpc(endpoint, "eth_getTransactionCount", [owner, parent]), 16)
    executor = create_address(owner, nonce)
    destination = {"v5": executor, "pool": POOL, "router": router}[mode]

    def tx(to, sig, types, args):
        return {"from": owner, "to": to, "data": calldata(sig, types, args), "gas": "0x155cc0"}

    def balance(token, target=owner):
        return tx(token, "balanceOf(address)", ["address"], [target])

    def allowance(token, amount):
        return tx(token, "approve(address,uint256)", ["address","uint256"], [destination,amount])

    def quote(i, amount):
        return tx(POOL, "get_dy(int128,int128,uint256)", ["int128","int128","uint256"], [i,1-i,amount])

    def trade(i, amount, floor, before_out, deadline):
        source, target = (ibt,pt) if i == 0 else (pt,ibt)
        if mode == "pool":
            return tx(POOL, "exchange(int128,int128,uint256,uint256)", ["int128","int128","uint256","uint256"], [i,1-i,amount,floor])
        if mode == "router":
            inputs = [encode(["address","uint256"], [source,amount]),
                      encode(["address","uint256","uint256","uint256","uint256","address"], [POOL,i,1-i,amount,floor,owner]),
                      encode(["address","address","uint256"], [target,owner,before_out+floor])]
            return tx(router, "execute(bytes,bytes[],uint256)", ["bytes","bytes[]","uint256"], [bytes([0,30,15]),inputs,deadline])
        step = (4,POOL,source,target,10000,encode(["int128","int128"],[i,1-i]))
        return tx(executor, "execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"], [source,amount,target,floor,deadline,[step]])

    def simulate(first, second):
        blocks = [{"blockOverrides":{"time":hex(timestamp+offset)},"calls":[call for _,call in calls]}
                  for offset,calls in enumerate([first,second],1)]
        returned = rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":blocks,"validation":False},parent])
        if (len(returned) != 2 or returned[0]["parentHash"] != parent["blockHash"] or returned[1]["parentHash"] != returned[0]["hash"]
                or [int(b["number"],16) for b in returned] != [snapshot["block"]+1,snapshot["block"]+2]
                or [int(b["timestamp"],16) for b in returned] != [timestamp+1,timestamp+2]):
            raise ValueError("simulated headers do not form the requested two-block trajectory")
        result = {}
        for block,calls in zip(returned,[first,second],strict=True):
            for (name,call),reply in zip(calls,block["calls"],strict=True):
                result[name] = {"call":call,"result":reply}
        return result,[{k:b[k] for k in ("number","timestamp","hash","parentHash")} for b in returned]

    first = []
    if mode == "v5":
        if rpc(endpoint,"eth_getCode",[executor,parent]) != "0x":
            raise ValueError("predicted executor address occupied")
        bytecode = (ROOT/"contracts/out/ArbExecutor.bin").read_text().strip().removeprefix("0x")
        first.append(("constructor",{"from":owner,"data":"0x"+bytecode+encode(["address"],[WETH]).hex(),"gas":"0x780000"}))
    first += [("held",balance(ibt)),("lock_before",tx(MANAGER,"getAccountLockTime(address)",["address"],[owner]))]
    if funded:
        budget = int(old["budget"])
        first += [("underlying_before",balance(underlying)),("approve_funding",tx(underlying,"approve(address,uint256)",["address","uint256"],[ibt,budget])),
                  ("deposit",tx(ibt,"deposit(uint256,address)",["uint256","address"],[budget,owner])),("underlying_after",balance(underlying))]
    first += [("ibt_ready",balance(ibt)),("approve_ibt",allowance(ibt,qty)),("lock_after",tx(MANAGER,"getAccountLockTime(address)",["address"],[owner]))]
    second = [("pre_ibt",balance(ibt)),("pre_pt",balance(pt)),("quote",quote(0,qty))]
    preliminary,_ = simulate(first,second)
    val = lambda results,name: quantity(results[name]["result"])
    if mode == "v5":
        deployed=preliminary["constructor"]["result"]
        runtime="0x"+(ROOT/"contracts/out/ArbExecutor.bin-runtime").read_text().strip().removeprefix("0x")
        if deployed["status"]!="0x1" or deployed["returnData"].lower()!=runtime.lower():
            raise ValueError("constructor did not return the reviewed runtime")
    gross, pre_pt = val(preliminary,"quote"), val(preliminary,"pre_pt")
    if gross <= 0 or val(preliminary,"ibt_ready") < qty:
        raise ValueError("first leg quote or held shares insufficient")
    if funded:
        if val(preliminary,"underlying_before")-val(preliminary,"underlying_after")!=budget or val(preliminary,"ibt_ready")-val(preliminary,"held")!=qty:
            raise ValueError("real deposit did not acquire the recorded shares")
        unlock = val(preliminary,"lock_after")
        if not timestamp+1 < unlock <= timestamp+2:
            raise ValueError(f"unlock {unlock} lies outside the two simulated timestamps")
        first.append(("early_exchange",trade(0,qty,gross,pre_pt,timestamp+180)))
    elif val(preliminary,"lock_before") > timestamp+1:
        raise ValueError("original IBT balance locked")
    def checked_leg(i, amount, minimum, pre):
        source,target = (ibt,pt) if i==0 else (pt,ibt)
        prefix = "forward" if i==0 else "reverse"
        ls=[(prefix+"_strict",trade(i,amount,minimum+1,pre,timestamp+180))]
        if mode!="pool":
            ls.append((prefix+"_expired",trade(i,amount,minimum,pre,0)))
        ls += [(prefix,trade(i,amount,minimum,pre,timestamp+180)),(prefix+"_in",balance(source)),(prefix+"_out",balance(target))]
        return ls
    second += checked_leg(0,qty,gross,pre_pt)
    second += [("approve_pt",allowance(pt,gross)),("reverse_quote",quote(1,gross))]
    measured,headers = simulate(first,second)
    reverse = val(measured,"reverse_quote")
    if measured["forward"]["result"]["status"]=="0x1" and reverse>0:
        second += checked_leg(1,gross,reverse,val(measured,"forward_in"))
        if mode=="v5":
            second += [("executor_ibt",balance(ibt,executor)),("executor_pt",balance(pt,executor))]
        measured,headers=simulate(first,second)
    if funded:
        delayed=measured["early_exchange"]["result"]
        if delayed["status"]!="0x0" or (mode!="v5" and delayed.get("error",{}).get("data","")!=LOCK+encode(["uint256"],[unlock]).hex()):
            raise ValueError("early exchange did not reproduce original lock")
        events=[log for log in measured["deposit"]["result"]["logs"] if log["address"].lower()==MANAGER and log["topics"]==[LOCK_EVENT]]
        if len(events)!=1 or events[0]["data"]!="0x"+encode(["address","uint256"],[owner,unlock]).hex():
            raise ValueError("deposit did not emit payer's measured deadline")
    acceptance={}
    for step,source_before,target_before,amount,expected in [("forward","pre_ibt","pre_pt",qty,gross),("reverse","forward_out","forward_in",gross,reverse)]:
        if step not in measured:
            continue
        controls=[step+"_strict"]+([step+"_expired"] if mode!="pool" else [])
        debit=val(measured,source_before)-val(measured,step+"_in")
        net=val(measured,step+"_out")-val(measured,target_before)
        acceptance[step]={"input":str(amount),"quote":str(expected),"debit":str(debit),"net":str(net),
                          "accepted":measured[step]["result"]["status"]=="0x1" and debit==amount and net==expected and
                          all(measured[n]["result"]["status"]=="0x0" for n in controls)}
    if mode=="v5" and "executor_ibt" in measured:
        if val(measured,"executor_ibt") or val(measured,"executor_pt"):
            raise ValueError("executor retained shares")
        # The constructor/runtime are pinned by the Git commit. Store hashes to avoid duplicating 49k bytes per sample.
        for key,field in [("call","data"),("result","returnData")]:
            raw=bytes.fromhex(measured["constructor"][key].pop(field)[2:])
            measured["constructor"][key][field+"_keccak256"]="0x"+keccak(raw).hex()
    return dict(mode=mode,funding="deposit" if funded else "owned_ibt",owner=owner,raw_ibt=str(qty),headers=headers,
                acceptance=acceptance,records=measured)


def capture(endpoint):
    snapshot=archive("directory")
    if rpc(endpoint,"eth_getBlockByHash",[snapshot["selector"]["blockHash"],False])["stateRoot"]!=snapshot["state_root"]:
        raise ValueError("canonical state root differs")
    market=next(m for m in snapshot["markets"] if m["pool"]==POOL)
    original=next(m for m in archive("wallets")["markets"] if m["pool"]==POOL)
    cases=[replay(endpoint,snapshot,market,w,mode,funded) for w in original["wallets"] if w["mode"]=="pool"
           for funded in [False,True] for mode in ["pool","router","v5"]]
    return {"pool":POOL,"parent":snapshot["selector"],"state_root":snapshot["state_root"],"rpc":endpoint,"lock_manager":MANAGER,
            "scenarios":cases}

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    for endpoint in ENDPOINTS:
        try:
            result=capture(endpoint)
        except (ValueError,requests.RequestException) as error:
            print(endpoint,str(error)[:170],flush=True)
            if endpoint==ENDPOINTS[-1]:
                raise
        else:
            args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
            break

