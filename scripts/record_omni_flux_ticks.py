"""Считывает всю полосу CL тиков, пройденную подтверждёнными ногами, для проверки локальной математики."""
import argparse
import gzip
import json
from pathlib import Path

from eth_utils import keccak
from eth_abi import decode
from record_base_launch_hooks import DATA
from record_omni_flux_registry import batch, data

TYPES=["int128","int128","uint160","uint128","int24","uint24","uint16"]
TOPIC="0x"+keccak(text="Swap(bytes32,address,"+",".join(TYPES)+")").hex()

def read(at,manager,sig,types,args):
    calls=[(manager,data(sig,types,params)) for params in args]
    results=[]
    for i in range(0,len(calls),80):
        chunk=batch(at,calls[i:i+80])
        if not all(ok for ok,_ in chunk):raise ValueError("не прочитан тик/битмап")
        results.extend(raw for _,raw in chunk)
    return results

def record():
    with gzip.open(DATA/"omni_flux_cl_wallets.json.gz") as f:wallet=json.load(f)
    with gzip.open(DATA/"omni_flux_base_directory.json.gz") as f:directory=json.load(f)
    at=wallet["selector"]
    if at!=directory["selector"]:raise ValueError("разные блоки")
    states={r["id"]:r for r in directory["markets"]};rows=[]
    for m in wallet["markets"]:
        initial=states[m["pool"]];tick=initial["slot0"][1];manager=m["key"][3]
        endpoints=[tick];replays=[]
        for c in m["cases"]:
            legs=[]
            for k in ("buy","sell"):
                leg=c.get(k) or {}
                if leg.get("stage")!="accepted":continue
                logs=[l for l in leg["results"][-3]["logs"] if l["address"].lower()==manager and l["topics"][:2]==[TOPIC,m["pool"]]]
                if len(logs)!=1:raise ValueError("нет единственного Swap")
                amt0,amt1,sqrt,liquidity,next_tick,fee,protocol=decode(TYPES,bytes.fromhex(logs[0]["data"][2:]))
                forward=amt0<0
                if (-(amt0 if forward else amt1),amt1 if forward else amt0)!=(int(leg["paid"]),int(leg["net"])):raise ValueError("событие не совпало с кошельком")
                endpoints.append(next_tick)
                legs.append(dict(forward=forward,paid=leg["paid"],net=leg["net"],sqrt=str(sqrt),tick=next_tick,liquidity=str(liquidity),fee=fee))
            if legs:replays.append(dict(mode=c["mode"],legs=legs))
        if not replays:continue
        spacing=(int(m["key"][-1],16)>>16)&0xffffff
        lo,hi=min(endpoints)//spacing//256-1,max(endpoints)//spacing//256+1
        id=bytes.fromhex(m["pool"][2:]);indices=[]
        words=read(at,manager,"getPoolBitmapInfo(bytes32,int16)",["bytes32","int16"],[[id,w] for w in range(lo,hi+1)])
        for i,raw in enumerate(words):
            word=int.from_bytes(raw,"big")
            indices.extend(((lo+i)*256+b)*spacing for b in range(256) if word&(1<<b))
        ticks=[]
        infos=read(at,manager,"getPoolTickInfo(bytes32,int24)",["bytes32","int24"],[[id,i] for i in indices])
        for t,raw in zip(indices,infos):
            gross,net=decode(["uint128","int128"],raw[:64]);ticks.append([t,str(gross),str(net)])
        rows.append(dict(id=m["pool"],slot0=list(map(str,initial["slot0"])),liquidity=initial["liquidity"],spacing=spacing,words=[lo,hi],ticks=ticks,replays=replays))
    return dict(selector=at,markets=rows)
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);a=p.parse_args()
    a.output.write_bytes(gzip.compress(json.dumps(record(),indent=2).encode(),mtime=0))
