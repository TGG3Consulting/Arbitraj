"""Capture public builder snapshots for bopAMM before a slot, pinned to the observed canonical parent."""
import argparse
import concurrent.futures
import json
import time
from pathlib import Path

import requests
from record_bopamm_ethereum import AMM, PAMM, REGISTRY, WETH, USDC, OWNER
from record_base_slipstream_executor import calldata
from record_fermi_ethereum import BUILDERS

GENESIS = 1606824023
RPC = "https://eth.drpc.org"

def rpc(url,method,params,clock=None):
    response = requests.post(url,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=5)
    if clock is not None:
        clock.update(received_ns=time.time_ns(),http_date=response.headers.get("Date"))
    response.raise_for_status()
    payload = response.json()
    if "result" not in payload:
        raise ValueError(str(payload.get("error"))[:180])
    return payload["result"]

def number(x):
    return int(x,0) if isinstance(x,str) else int(x)

def capture(name,head):
    url,method = BUILDERS[name]
    timing = {}
    data = rpc(url,method,[],timing)
    observed = timing["received_ns"]
    if name == "Titan":
        rows = [r for r in data["venues"] if r["maker"] == "Bebop" and r["quoteAddress"].lower() in (AMM,PAMM)]
        row, = rows
        target,slot,key = number(row["blockNumber"]),number(row["slot"]),row["quoteAddress"].lower()
    else:
        key, = [k for k in data if k.lower() in (AMM,PAMM)]
        row = data[key]
        target,slot = number(data["blockNumber"]),number(data["slot"])
    registry, = [v for k,v in row["stateOverride"].items() if k.lower() == REGISTRY]
    diff = registry["stateDiff"]
    if not 0 < len(diff) <= 128 or not all(isinstance(k,str) and len(k)==66 and k.startswith("0x")
            and len(bytes.fromhex(k[2:]))==32 for pair in diff.items() for k in pair):
        raise ValueError("invalid registry storage patch")
    timestamp = GENESIS + slot*12
    parent = head.result()
    tag = {"blockHash":parent["hash"],"requireCanonical":True}
    remaining_ns = timestamp*10**9 - observed
    record = dict(target=target,slot=slot,observed_ns=observed,remaining_ns=remaining_ns,
                  parent=parent["hash"],parent_height=number(parent["number"]),key=key,
                  builder_http_date=timing["http_date"],
                  registry=REGISTRY,registry_slots=len(diff),other_patched_accounts=sum(
                      bool(v.get("stateDiff")) for k,v in row["stateOverride"].items() if k.lower()!=REGISTRY))
    if target != number(parent["number"])+1 or number(parent["timestamp"])>=timestamp or not 0 < remaining_ns <= 12*10**9:
        return record
    signature="quote(address,address,uint256)"
    def quote(a,b,n):
        tx={"from":OWNER,"to":AMM,"data":calldata(signature,["address","address","uint256"],[a,b,n])}
        try:
            return str(int(rpc(RPC,"eth_call",[tx,tag,{REGISTRY:{"stateDiff":diff}},{"number":hex(target),"time":hex(timestamp)}]),16))
        except (requests.RequestException,ValueError) as error:
            return {"refusal":str(error)[:160]}
    record["quotes"] = [quote(WETH,USDC,10**15),quote(USDC,WETH,10**6)]
    tip = rpc(RPC,"eth_getBlockByNumber",["latest",False])
    record["finished_ns"] = time.time_ns()
    record["same_live_parent"] = tip["hash"] == parent["hash"]
    record["stateDiff"] = diff
    return record

if __name__ == "__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("output",type=Path)
    parser.add_argument("--trials",type=int,default=10)
    args=parser.parse_args()
    out=[]
    with concurrent.futures.ThreadPoolExecutor(3) as pool:
        for i in range(args.trials):
            head=pool.submit(rpc,RPC,"eth_getBlockByNumber",["latest",False])
            jobs={name:pool.submit(capture,name,head) for name in BUILDERS}
            row={}
            for name,item in jobs.items():
                try:
                    row[name]=item.result()
                except (requests.RequestException,ValueError,KeyError) as exc:
                    row[name]={"refusal":str(exc)[:180]}
            out.append(row)
            if i+1 < args.trials:
                time.sleep(1.7)
    args.output.write_text(json.dumps(out,separators=(",",":")),encoding="utf8")
