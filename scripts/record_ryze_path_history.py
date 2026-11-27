"""Ищет реальные одноступенчатые и двухступенчатые заявки Ryze по indexed активам; сохраняет только прошедшие каноническую проверку."""
import argparse
import gzip
import json
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import requests
import websocket
from eth_abi import decode
from eth_utils import keccak
from record_ryze_tokens import DATA,TOKENS

_thread=threading.local()
def rpc(method,params):
    if method not in ("eth_getLogs","eth_getTransactionByHash","eth_getTransactionReceipt","eth_getBlockByNumber","eth_call"):raise ValueError(method)
    for attempt in range(3):
        if not getattr(_thread,"ws",None):_thread.ws=websocket.create_connection("wss://base-rpc.publicnode.com",timeout=18,suppress_origin=True)
        try:
            _thread.ws.send(json.dumps(dict(id=1,jsonrpc="2.0",method=method,params=params)));reply=json.loads(_thread.ws.recv())
            if "result" not in reply:raise RuntimeError(str(reply.get("error"))[:200])
            return reply["result"]
        except (websocket.WebSocketException,ConnectionError,TimeoutError,RuntimeError):
            _thread.ws.close();_thread.ws=None
            if attempt==2:raise
def sig(s):return "0x"+keccak(text=s).hex()
def topic(a):return "0x"+a[2:].zfill(64)
def encoding():
    page=requests.get("https://documentation.ryze.pro/abi/amm/MultiHopRouter.json",timeout=12)
    page.raise_for_status()
    def tt(x):return '('+','.join(map(tt,x['components']))+')'+x['type'][5:] if x['type'].startswith('tuple') else x['type']
    types=[tt(x) for x in next(f for f in page.json() if f.get('name')=='executeSwapIntents')['inputs']]
    selector=sig("executeSwapIntents("+",".join(types)+")")[:10]
    evt=sig("IntentSwapExecuted(address,address,address,uint256,uint256,address,uint256,uint256)")
    return types,selector,evt

def run(first,last):
    d=json.loads((DATA/"base_recovered_markets.json").read_bytes())["ryze"];router=d["router"]["address"]
    types,selector,evt=encoding()
    found=[];coverage={};wanted=[("WETH","cbBTC"),("cbBTC","WETH"),("USDC","cbBTC")]
    for source,dest in wanted:
        inp= TOKENS[source];out=TOKENS[dest];pages=list(range(first,last+1,45_000));cases=[]
        def page(start):
            end=min(last,start+44_999)
            return rpc("eth_getLogs",[dict(address=router,fromBlock=hex(start),toBlock=hex(end),topics=[evt,None,topic(inp),topic(out)])])
        with ThreadPoolExecutor(max_workers=5) as e:rows=[r for block in e.map(page,pages) for r in block]
        for row in reversed(rows):
            tx=rpc("eth_getTransactionByHash",[row["transactionHash"]])
            if tx['to']!=router or tx['input'][:10]!=selector:continue
            batch,_,prices=decode(types,bytes.fromhex(tx['input'][10:]))
            if len(batch)!=1:continue
            (user,asset,received,amount,minimum,path,recipient,deadline,nonce),_=batch[0]
            if asset!=inp or received!=out or len(path)!=(1 if source=="USDC" else 2):continue
            if any(h[0] not in {m["pool"]["address"] for m in d["markets"]} for h in path):continue
            block=rpc("eth_getBlockByNumber",[tx["blockNumber"],False]);receipt=rpc("eth_getTransactionReceipt",[tx["hash"]])
            if block["hash"]!=row["blockHash"] or tx["blockHash"]!=block["hash"] or receipt["blockHash"]!=block["hash"] or receipt["status"]!="0x1":raise ValueError("reorg")
            spent,got,to,seq,fee=decode(["uint256","uint256","address","uint256","uint256"],bytes.fromhex(row["data"][2:]))
            if (spent,to,seq)!=(amount,recipient,nonce) or got<minimum:raise ValueError("wrong receipt")
            parent=dict(blockHash=block["parentHash"],requireCanonical=True)
            fee_call=dict(to=router,data=sig("intentFee()")[:10])
            usd=int(rpc("eth_call",[fee_call,parent]),16)
            signed_price=next(price[1] for price in prices if price[0]==inp)
            decimals={"WETH":18,"cbBTC":8,"USDC":6}[source]
            cases.append(dict(parent=parent,fee_call=fee_call,usd_fee=str(usd),signed_price=str(signed_price),decimals=decimals,transactionHash=tx["hash"],transaction={k:tx[k] for k in ("to","from","input","value","blockHash","blockNumber")},receipt=receipt,
                        hops=len(path),amount=str(spent),net=str(got),fee=str(fee)))
            if len(cases)==2:break
        coverage[f"{source}/{dest}"]=dict(blocks=[first,last],logs=len(rows),selected=len(cases));found.extend(cases)
    return dict(source="eth_getLogs, executeSwapIntents calldata и каноническая квитанция",coverage=coverage,cases=found)
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("first",type=int);p.add_argument("last",type=int);p.add_argument("out",type=Path);a=p.parse_args()
    result=run(a.first,a.last);a.out.write_bytes(gzip.compress(json.dumps(result,separators=(",",":")).encode(),mtime=0));print(result["coverage"])
