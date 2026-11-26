"""Enumerate Base V3 pools from deployer nonces, factory logs and canonical getPool checks."""
import argparse
import json
import time
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata

SOURCES = ROOT/"tests/data/first_run_dex_2026-11-26"
DEPLOYMENT = {"9mm-v3-base":15754625, "throne-v3-base":2146977, "omni-exchange-v3-base":32873743}
CREATED = "0x"+keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"
ENDPOINTS = ("https://base.gateway.tenderly.co","https://mainnet.base.org")
LOG_RANGE = 800


class CanonicalReads:
    def __init__(self, evidence):
        self.session=requests.Session()
        self.nodes=[]
        self.turn=0
        for url in ENDPOINTS:
            try:
                chain, head=self.at(url,[("eth_chainId",[]),("eth_getBlockByNumber",[hex(evidence["block"]),False])])
                if int(chain,16)==8453 and (head["hash"],head["stateRoot"])==(evidence["selector"]["blockHash"],evidence["state_root"]):
                    self.nodes.append(url)
            except (ValueError,KeyError,requests.RequestException):
                continue
        if not self.nodes:
            raise ValueError("No public archive returned the canonical Base state")

    def at(self,url,calls):
        payload=[dict(jsonrpc="2.0",method=m,params=p,id=i) for i,(m,p) in enumerate(calls)]
        response=self.session.post(url,json=payload,timeout=35)
        response.raise_for_status()
        replies={e["id"]:e for e in response.json()}
        if len(replies)!=len(calls) or any("result" not in replies[i] for i in range(len(calls))):
            raise ValueError("incomplete or rejected public archive batch")
        return [replies[i]["result"] for i in range(len(calls))]

    def read(self,calls,limit=20):
        results=[]
        for start in range(0,len(calls),limit):
            group=calls[start:start+limit]
            for attempt in range(8):
                url=self.nodes[self.turn%len(self.nodes)]; self.turn+=1
                try:
                    chunk=self.at(url,group)
                    break
                except (requests.RequestException,ValueError,KeyError):
                    if attempt==7:raise
                    time.sleep(min(12,1+attempt))
            results.extend(chunk)
            time.sleep(.12)
        return results


def directory(read,e,deployer,factory,deployment,total):
    # Constructor leaves the deployer at nonce 1; each successful CREATE2 emits one event in the factory.
    ranges=[(deployment,e["block"],1,total+1)]
    leaves=[]
    while ranges:
        bisect=[]
        for left,right,before,after in ranges:
            if after==before:continue
            if right-left+1<=LOG_RANGE:leaves.append((left,right,before,after));continue
            bisect.append((left,right,before,after,(left+right)//2))
        mids=read.read([("eth_getTransactionCount",[deployer,hex(mid)]) for *_,mid in bisect])
        ranges=[]
        for (left,right,before,after,mid),raw in zip(bisect,mids,strict=True):
            n=int(raw,16)
            if not before<=n<=after:raise ValueError("nonmonotonic deployment nonce")
            ranges.extend([(left,mid,before,n),(mid+1,right,n,after)])
    windows=read.read([("eth_getLogs",[{"fromBlock":hex(left),"toBlock":hex(right),
                                        "address":factory,"topics":[CREATED]}]) for left,right,_,_ in leaves],limit=4)
    pools=[]
    for (_,_,before,after),events in zip(leaves,windows,strict=True):
        if len(events)!=after-before:raise ValueError("deployment nonce differs from window receipt count")
        for event in events:
            t0,t1=["0x"+t[-40:] for t in event["topics"][1:3]]
            fee=int(event["topics"][3],16)
            spacing,pool=decode(["int24","address"],bytes.fromhex(event["data"][2:]))
            pools.append(dict(pool=pool,tokens=[t0,t1],fee=fee,spacing=spacing,tx=event["transactionHash"],
                              block=int(event["blockNumber"],16),hash=event["blockHash"],index=int(event["logIndex"],16)))
    if len({p["pool"] for p in pools})!=len(pools) or len(pools)!=total:
        raise ValueError("incomplete factory")
    checks=[]
    for p in pools:
        call=calldata("getPool(address,address,uint24)",["address","address","uint24"],[*p["tokens"],p["fee"]])
        checks.append((factory,False,bytes.fromhex(call[2:])))
    for start in range(0,len(checks),60):
        group=checks[start:start+60]
        payload=calldata("aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[group])
        raw,=read.read([("eth_call",[{"to":MULTICALL,"data":payload},e["selector"]])])
        results,=decode(["(bool,bytes)[]"],bytes.fromhex(raw[2:]))
        for p,(ok,value) in zip(pools[start:start+60],results,strict=True):
            if not ok or decode(["address"],value)[0]!=p["pool"]:raise ValueError("wrong pool address")
    return pools


def record():
    e=json.loads((SOURCES/"base_native_v3.json").read_bytes())
    read=CanonicalReads(e)
    markets={}
    for name,known in e["markets"].items():
        deployer, factory=known["router"]["deployer"],known["factory"]
        actual=read.read([("eth_getTransactionCount",[deployer,e["selector"]]),
                          ("eth_call",[{"to":deployer,"data":calldata("factoryAddress()")},e["selector"]])])
        total=int(actual[0],16)-1
        if decode(["address"],bytes.fromhex(actual[1][2:]))[0]!=factory:
            raise ValueError("deployer points at another factory")
        pools=directory(read,e,deployer,factory,DEPLOYMENT[name],total)
        markets[name]=dict(factory=factory,deployer=deployer,nonce=total+1,
                           used_tiers=sorted({p["fee"] for p in pools}),pools=sorted(pools,key=lambda p:(p["block"],p["index"])))
    return dict(selector=e["selector"],height=e["block"],state_root=e["state_root"],markets=markets)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    args.output.write_text(json.dumps(record(),indent=2)+"\n",encoding="utf8",newline="\n")
