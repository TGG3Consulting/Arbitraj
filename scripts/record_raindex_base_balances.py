"""Остатки трех реальных ERC20 у каждого размещения Raindex Base на том же EIP-1898 блоке."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import time

import requests
from eth_utils import keccak
from record_raindex_quotes import DATA
from raindex_wire import SELECTORS

SOURCE="https://raw.githubusercontent.com/rainlanguage/raindex/0352173773372ab0b8f3f779586bec446d4eca99/deployments/raindex.json"


def push4(bytecode):
    i=0
    found=set()
    while i<len(bytecode):
        op=bytecode[i]
        if op==0x63:
            found.add(bytecode[i+1:i+5].hex())
        i+=1+max(0,op-0x5f) if 0x60<=op<=0x7f else 1
    return found


def capture(url):
    stable=json.loads(gzip.decompress((DATA/"base_usdt_cycle.json.gz").read_bytes()))
    tag=stable["selector"]
    session=requests.Session()
    def batch(calls):
        requests_=[{"id":i,"jsonrpc":"2.0","method":m,"params":p} for i,(m,p) in enumerate(calls)]
        result=session.post(url,json=requests_,timeout=45)
        if result.status_code==429:
            wait=result.headers.get("Retry-After","15")
            time.sleep(min(60,max(1,int(wait))) if wait.isdigit() else 15)
            result=session.post(url,json=requests_,timeout=45)
        result.raise_for_status()
        items={r["id"]:r for r in result.json()}
        return [items[i]["result"] for i in range(len(calls))]
    chain,head=batch([("eth_chainId",[]),("eth_getBlockByNumber",[hex(stable["block"]),False])])
    if int(chain,16)!=8453 or head["hash"]!=tag["blockHash"]:
        raise ValueError("canonical Base")
    r=session.get(SOURCE,timeout=20);r.raise_for_status()
    deployments=[d for d in r.json() if d["network"]=="base"]
    if len(deployments)!=len({d["address"].lower() for d in deployments}):
        raise ValueError("duplicate deployment")
    assets={"USDC":stable["working"],"USDT":stable["cash"],"WETH":stable["assets"][2]}
    names=list(assets)
    out=[]
    for d in deployments:
        book=d["address"].lower()
        calls=[("eth_getCode",[book,tag])]
        for token in assets.values():
            calls.append(("eth_call",[{"to":token,"data":"0x70a08231"+book[2:].zfill(64)},tag]))
        code,*words=batch(calls)
        if any(len(w)!=66 for w in words):
            raise ValueError("balanceOf word")
        raw=bytes.fromhex(code[2:])
        sel=SELECTORS[d["interfaceVersion"]]
        out.append(dict(book=book,version=d["interfaceVersion"],start_block=d["startBlock"],
                        code_bytes=len(raw),code_hash="0x"+keccak(raw).hex(),
                        take_selector=sel,push4_take=sel in push4(raw),
                        balances={k:str(int(word,16)) for k,word in zip(names,words,strict=True)}))
    return dict(chain_id=8453,height=stable["block"],selector=tag,source=SOURCE,
                source_sha256=hashlib.sha256(r.content).hexdigest(),tokens=assets,books=out)


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("out",type=Path);args=parser.parse_args()
    last=None
    for url in ("https://base.drpc.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"):
        try:
            data=capture(url)
            args.out.write_text(json.dumps(data,separators=(",",":")))
            break
        except (requests.RequestException,ValueError,KeyError,TypeError) as e:
            last=e
    else:
        raise RuntimeError(f"archive unavailable: {last}")
