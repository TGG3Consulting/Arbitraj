"""Канонические корневые суммы и balanceOf до/после двух уже оплаченных вызовов ShibaSwap Router."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode

TXS=["0xe2a5f9da17cd4ed905bda01ef08db72ab0fb6e667a62f06f878e8ed69477a784",
     "0x32286bffaf7371fccbddbf31f30efbef112aa7c428a0bd586699be75cc154044"]


def capture(endpoint):
    def call(method,params):
        r=requests.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=16)
        r.raise_for_status()
        return r.json()["result"]
    rows=[]
    for hash_ in TXS:
        tx=call("eth_getTransactionByHash",[hash_])
        receipt=call("eth_getTransactionReceipt",[hash_])
        block=tx["blockHash"]
        i=int(tx["transactionIndex"],16)
        if receipt["status"]!="0x1" or receipt["blockHash"]!=block or call("eth_getBlockByNumber",[tx["blockNumber"],False])["hash"]!=block:
            raise ValueError("noncanonical receipt")
        src,dst,_,who,_,qty,_,_=decode(["address","address","uint24","address","uint256","uint256","uint256","uint160"],bytes.fromhex(tx["input"][10:]))
        root=call("trace_get",[hash_,[]])
        geth=call("debug_traceTransaction",[hash_,{"tracer":"callTracer","tracerConfig":{"onlyTopCall":True}}])
        if root["transactionHash"]!=hash_ or root["blockHash"]!=block or root["result"]["output"]!=geth["output"] or geth["input"]!=tx["input"]:
            raise ValueError("root and signed call differ")
        points={}
        for key,asset,owner in [("source",src,tx["from"]),("target",dst,who)]:
            data="0x70a08231"+owner[2:].lower().rjust(64,"0")
            frames=[]
            for ix in (i,i+1):
                frames.append(call("debug_traceCall",[{"from":tx["from"],"to":asset,"data":data},{"blockHash":block,"requireCanonical":True},
                                   {"txIndex":hex(ix),"tracer":"callTracer","tracerConfig":{"onlyTopCall":True}}]))
            points[key]=frames
        debit=int(points["source"][0]["output"],16)-int(points["source"][1]["output"],16)
        net=int(points["target"][1]["output"],16)-int(points["target"][0]["output"],16)
        if net!=int(geth["output"],16) or (int(tx["value"],16)+debit)!=qty:
            raise ValueError("historical owner amounts differ")
        rows.append(dict(hash=hash_,block=block,index=i,payer=tx["from"],input=src,output=dst,recipient=who,qty=str(qty),native=tx["value"],
                         debit=str(debit),net=str(net),root=root,geth=geth,balances=points))
    return rows


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path)
    p.add_argument("--endpoint",default="https://eth.drpc.org")
    a=p.parse_args()
    a.output.write_text(json.dumps(capture(a.endpoint),indent=1)+"\n",encoding="utf8")
