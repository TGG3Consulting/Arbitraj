"""Сопоставляет публичные эпохи лимитного менеджера Treble с физическими Algebra-пулами и Gecko."""
import argparse
import gzip
import json
from pathlib import Path
import requests
from eth_abi import decode,encode
from eth_utils import keccak
from record_base_launch_bindings import read
from record_base_launch_hooks import DATA

MC="0xca11bde05977b3631167028862be2a173976ca11"
ZERO="0x"+"00"*20
def capture():
    with gzip.open(DATA/"base_algebra_treble-v4.json.gz") as f:trusted=json.load(f)
    at=trusted["selector"];manager="0x"+trusted["reads"]["limitOrderManager()"][-40:]
    factory=trusted["roles"]["factory"]["address"];obs=[]
    def data(sig,types=(),params=()):return keccak(text=sig)[:4]+encode(types,params)
    def fetch(target,raw):
        call={"to":target,"data":"0x"+raw.hex()}
        result=read("eth_call",[call,at]);obs.append(dict(call=call,result=result));return bytes.fromhex(result[2:])
    def group(q):
        output=[]
        for i in range(0,len(q),40):
            r=fetch(MC,data("aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[[(a,False,p) for a,p in q[i:i+40]]]))
            for ok,out in decode(["(bool,bytes)[]"],r)[0]:
                if not ok:raise ValueError("неполное чтение Treble")
                output.append(out)
        return output
    next_id,=decode(["uint232"],fetch(manager,data("epochNext()")))
    rows=[]
    for i,raw in enumerate(group([(manager,data("epochInfos(uint232)",["uint232"],[i])) for i in range(1,next_id)]),1):
        filled,lo,hi,liq,x,y,deployer,x_total,y_total=decode(["bool","int24","int24","uint128","address","address","address","uint128","uint128"],raw)
        rows.append(dict(id=i,filled=filled,tick_lower=lo,tick_upper=hi,liquidity=str(liq),x=x,y=y,deployer=deployer,amount0=str(x_total),amount1=str(y_total)))
    pairs=sorted({(m["x"],m["y"],m["deployer"]) for m in rows if m["x"]!=ZERO})
    calls=[(factory,data("poolByPair(address,address)",["address","address"],[x,y]) if d==ZERO else data("customPoolByPair(address,address,address)",["address"]*3,[d,x,y])) for x,y,d in pairs]
    markets=[dict(x=x,y=y,deployer=d,pool=decode(["address"],result)[0]) for (x,y,d),result in zip(pairs,group(calls),strict=True)]
    for i in range(0,len(markets),25):
        part=[m["pool"] for m in markets[i:i+25] if m["pool"]!=ZERO]
        if not part:continue
        u="https://api.geckoterminal.com/api/v2/networks/base/pools/multi/"+",".join(part)
        r=requests.get(u,timeout=18);r.raise_for_status()
        mapped={m["attributes"]["address"].lower():m["relationships"]["dex"]["data"]["id"] for m in r.json()["data"]}
        for m in markets[i:i+25]:m["indexer_id"]=mapped.get(m["pool"])
    return dict(selector=at,manager=manager,factory=factory,epoch_next=next_id,epochs=rows,markets=markets,observations=obs)
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("out",type=Path);a=p.parse_args()
    a.out.write_bytes(gzip.compress((json.dumps(capture(),indent=2)+"\n").encode(),mtime=0))
