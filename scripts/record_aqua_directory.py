"""От Shipped до Docked обеих сетей, без ключа индексатора или разрешений адреса."""
import argparse
import json
import time
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
from eth_abi import decode
from eth_abi.exceptions import DecodingError

from aqua_evidence import call, ORDER, REGISTRY, ROUTER, TOPICS, order_record, unpack
from record_base_slipstream_executor import ROOT

DEPLOYMENT = {"ethereum":(25567141,"0xe37e4dd7e73302a57cbf8ef6cff2424a787df9bf462d69f2de6d149dca43fb1a"),
              "base":(48839900,"0x6ce475fdeedd0c0df488f5b65bec1b38b714174bd9fbc399a87586f43498045e")}
NODES = {"ethereum":["https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"],
         "base":["https://base-rpc.publicnode.com","https://mainnet.base.org","https://base.public.blockpi.network/v1/rpc/public","https://base.gateway.tenderly.co"]}


def one(url,method,params):
    r=requests.post(url,json={"id":1,"jsonrpc":"2.0","method":method,"params":params},timeout=30)
    r.raise_for_status()
    data=r.json()
    if data.get("result") is None:
        raise ValueError(str(data.get("error"))[:300])
    return data["result"]


def snapshot(network):
    first,creation=DEPLOYMENT[network]
    head=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())[network]
    last=head["block"]
    nodes=NODES[network]
    def query(method,params):
        for attempt in range(3):
            for url in nodes:
                try:return one(url,method,params)
                except (requests.RequestException,ValueError) as exc:last_error=exc
            time.sleep(1+attempt)
        raise RuntimeError(method) from last_error
    header=query("eth_getBlockByNumber",[hex(last),False])
    birth=query("eth_getTransactionReceipt",[creation])
    if header["hash"]!=head["hash"] or int(birth["blockNumber"],16)!=first or birth["status"]!="0x1":
        raise ValueError("directory endpoints")
    wanted=[h for h,n in TOPICS.items() if n in ("Shipped","Docked")]
    def logs(blocks):
        lo,hi=blocks
        for url in nodes:
            if "blockpi" in url and (hi-lo>=5000 or lo<51000000):continue
            if "tenderly" in url and hi-lo>=1000:continue
            if url=="https://rpc.mevblocker.io" and hi-lo>=10000:continue
            try:
                return one(url,"eth_getLogs",[dict(address=REGISTRY,topics=[wanted],fromBlock=hex(lo),toBlock=hex(hi))])
            except (ValueError,requests.RequestException):pass
        step=5000 if hi-lo>=5000 else 1000
        if hi-lo>=step:
            return [v for i in range(lo,hi+1,step) for v in logs((i,min(hi,i+step-1)))]
        return query("eth_getLogs",[dict(address=REGISTRY,topics=[wanted],fromBlock=hex(lo),toBlock=hex(hi))])
    step=10000 if network=="ethereum" else 32768
    spans=[(i,min(last,i+step-1)) for i in range(first,last+1,step)]
    with ThreadPoolExecutor(max_workers=3) as workers:
        records=[log for interval in workers.map(logs,spans) for log in interval]
    counts=Counter()
    shipped=[]
    for log in sorted(records,key=lambda v:(int(v["blockNumber"],16),int(v["logIndex"],16))):
        name,values=unpack(log)
        maker,app,h=values[:3]
        counts[(app,name)]+=1
        if app==ROUTER and name=="Shipped":shipped.append((maker,h,log,values[3]))
    pushed=next(k for k,v in TOPICS.items() if v=="Pushed")
    def registration(item):
        maker,h,log,data=item
        try: rec=order_record(decode([ORDER],bytes.fromhex(data[2:]))[0])
        except (DecodingError,ValueError,OverflowError):rec=None
        row=dict(maker=maker,order=h,tx=log["transactionHash"],block=log["blockNumber"],blockHash=log["blockHash"])
        if not rec or rec[0]!=h or rec[1]["maker"]!=maker:return dict(**row,unsupported=True)
        receipt=query("eth_getTransactionReceipt",[row["tx"]])
        if receipt["blockHash"]!=row["blockHash"] or receipt["status"]!="0x1":
            raise ValueError("changed registration receipt")
        pos=int(log["logIndex"],16)
        assets=[]
        for event in sorted(receipt["logs"],key=lambda x:int(x["logIndex"],16)):
            if int(event["logIndex"],16)<=pos:continue
            if event["address"].lower()!=REGISTRY or event["topics"][:1]!=[pushed]:break
            _,v=unpack(event)
            if tuple(v[:3])!=(maker,ROUTER,h):break
            assets.append(v[3])
        return dict(**row,traits=rec[1]["traits"],data=rec[1]["data"],assets=assets)
    with ThreadPoolExecutor(max_workers=3) as workers:history=list(workers.map(registration,shipped))
    programs={}
    for row in history:
        if row.get("unsupported"):continue
        k=(row["maker"],row["order"])
        p=programs.setdefault(k,dict(maker=k[0],order=k[1],traits=row["traits"],data=row["data"],assets={}))
        for token in row["assets"]:p["assets"].setdefault(token,None)
    # Docked omits the affected tokens. safeBalances checks count of each token independently.
    selector={"blockHash":header["hash"],"requireCanonical":True}
    jobs=[(key,t) for key,p in programs.items() for t in p["assets"]]
    types=["address","address","bytes32","address"]
    multi="0xca11bde05977b3631167028862be2a173976ca11"
    for i in range(0,len(jobs),64):
        part=jobs[i:i+64]
        calls=[(REGISTRY,False,bytes.fromhex(call("rawBalances",types,[k[0],ROUTER,bytes.fromhex(k[1][2:]),t])[2:])) for k,t in part]
        args=call("aggregate3",["(address,bool,bytes)[]"],[calls])
        raw=query("eth_call",[{"to":multi,"data":args},selector])
        results,=decode(["(bool,bytes)[]"],bytes.fromhex(raw[2:]))
        for (key,token),(ok,res) in zip(part,results,strict=True):
            if not ok:raise ValueError("rawBalances failed")
            value,count=decode(["uint248","uint8"],res)
            programs[key]["assets"][token]={"virtual":str(value),"count":count}
    active=[p for p in programs.values() if any(0<v["count"]<255 for v in p["assets"].values())]
    if query("eth_getBlockByNumber",[hex(last),False])["hash"]!=header["hash"]:
        raise ValueError("directory reorg")
    return dict(chain=network,registry=REGISTRY,app=ROUTER,first=first,creation=creation,through=last,
                blockHash=header["hash"],stateRoot=header["stateRoot"],events=len(records),
                counts=[[*k,v] for k,v in counts.items()],registration_transactions=len(history),
                unsupported_registrations=sum(bool(p.get("unsupported")) for p in history),active=active)

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("network",choices=NODES);p.add_argument("output",type=Path)
    a=p.parse_args()
    a.output.write_text(json.dumps(snapshot(a.network),separators=(",",":")),encoding="utf8")
