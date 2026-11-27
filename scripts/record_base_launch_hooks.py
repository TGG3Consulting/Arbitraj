"""Сопоставляет Base PoolId c каноническим Initialize, проверяет keccak ключа и runtime хука."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from functools import lru_cache
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT, rpc
from record_omni_v2_base import ENDPOINTS

GROUPS=("bankr","bankr-index-base","o1-launchpad")
KEY="(address,address,uint24,int24,address)"
INIT="0x"+keccak(text="Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)").hex()
DATA=ROOT/"tests/data/first_run_dex_2026-11-26"


def inputs(folder):
    original=json.loads((folder.parent.parent/"base-integration-2026-11-26/sources/sample-pools.json").read_bytes())
    other=json.loads((folder/"dexscreener-original.json").read_bytes())
    result=[]
    for group in GROUPS:
        rows={}
        for item in json.loads((folder/(group+"-gecko.json")).read_bytes())["rows"]:
            a=item["attributes"]
            rows[a["address"].lower()]=dict(name=a["name"],time=int(datetime.fromisoformat(a["pool_created_at"]).timestamp()),source="GeckoTerminal")
        for item in original[group]["samples"]:
            key=item["address"].lower()
            d=next((r for r in other if r["pairAddress"].lower()==key),{})
            rows.setdefault(key,dict(name=item["name"],time=d.get("pairCreatedAt",0)//1000,source="GeckoTerminal original / DexScreener"))
            rows[key]["original"]=True
        result.extend(dict(**r,group=group,id=key) for key,r in rows.items())
    return result


def rpc_logs(url,manager,pid,lo,hi):
    payload=dict(jsonrpc="2.0",id=1,method="eth_getLogs",params=[dict(address=manager,fromBlock=hex(lo),toBlock=hex(hi),topics=[INIT,pid])])
    r=requests.post(url,json=payload,timeout=20)
    r.raise_for_status()
    d=r.json()
    if "result" not in d:raise ValueError(str(d.get("error"))[:180])
    return d["result"]


def record(folder,out):
    snap=json.loads((DATA/"base.json").read_bytes())
    profiles=json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    cfg=next(p["settings"] for p in profiles if p["chain_id"]==8453 and p["settings"]["dex.kind"]=="uniswap_v4")
    mgr=cfg["dex.pool_manager"].lower()
    at=snap["selector"];height=snap["block"];ep=ENDPOINTS[0]
    header=rpc(ep,"eth_getBlockByHash",[at["blockHash"],False]);end=int(header["timestamp"],16)
    if int(header["number"],16)!=height:raise ValueError("неверная высота")
    @lru_cache(maxsize=4096)
    def stamp(n):
        request=dict(jsonrpc="2.0",id=1,method="eth_getBlockByNumber",params=[hex(n),False])
        b=requests.post(ep,json=request,timeout=15);b.raise_for_status()
        return int(b.json()["result"]["timestamp"],16)
    def estimated(when):
        lo,hi=0,height
        start,finish=stamp(lo),end
        for _ in range(45):
            if hi-lo<=64:return (lo+hi)//2
            n=min(hi-1,max(lo+1,lo+(when-start)*(hi-lo)//max(1,finish-start)))
            t=stamp(n)
            if abs(t-when)<15:return n
            if t<when:lo,start=n,t
            else:hi,finish=n,t
            # Блоки Base на снимке 200 ms. Исторический шаг зависит от эпохи.
            later=min(n+250,height)
            if later>n:
                delta=stamp(later)-t
                if delta>0:
                    guess=max(lo,min(hi,n+(when-t)*(later-n)//delta))
                    when_guess=stamp(guess)
                    if abs(when-when_guess)<20:return guess
                    if when_guess<when:lo,start=guess,when_guess
                    else:hi,finish=guess,when_guess
        return (lo+hi)//2
    def confirm(r):
        if not r["time"]:return dict(**r,status="нет метки времени исходного рынка")
        if r["time"]>end:return dict(**r,status="дата индексатора позже контрольного блока")
        estimate=estimated(r["time"])
        # Фактическое время блоков определяет окно. Авторитетная идентичность приходит из лога.
        err=None
        for endpoint in ENDPOINTS:
            try:
                found=rpc_logs(endpoint,mgr,r["id"],max(0,estimate-490),min(height,estimate+490))
                if not found:continue
                if len(found)!=1:raise ValueError("неоднозначный Initialize")
                event=found[0]
                if event["address"].lower()!=mgr or event["topics"][:2]!=[INIT,r["id"]] or event.get("removed",False):raise ValueError("не совпала идентичность события")
                x,y=["0x"+word[-40:] for word in event["topics"][2:]]
                fee,spacing,hook,_,_=decode(["uint24","int24","address","uint160","int24"],bytes.fromhex(event["data"][2:]))
                key=[x,y,fee,spacing,hook]
                if "0x"+keccak(encode([KEY],[key])).hex()!=r["id"]:raise ValueError("не совпал хеш ключа")
                n=int(event["blockNumber"],16)
                if n>height or not n:raise ValueError("событие вне снимка")
                deployed=rpc(endpoint,"eth_getCode",[hook,at]);raw=bytes.fromhex(deployed[2:])
                return dict(**r,status="Initialize",key=key,log=event,hook_hash="0x"+keccak(raw).hex(),hook_bytes=len(raw))
            except (ValueError,requests.RequestException,KeyError) as ex:err=str(ex)[:160]
        return dict(**r,status="нет подтверждённого события в окне",error=err)
    with ThreadPoolExecutor(4) as pool: rows=list(pool.map(confirm,inputs(folder)))
    data=dict(height=height,selector=at,state_root=header["stateRoot"],manager=mgr,rows=rows)
    out.write_text(json.dumps(data,ensure_ascii=False,indent=2)+"\n",encoding="utf8")


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources",type=Path);parser.add_argument("output",type=Path)
    args=parser.parse_args();record(args.sources,args.output)
