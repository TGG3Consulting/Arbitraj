"""Публичные routes и route/build Kyber Base и фактическая атрибуция платежа из своего кошелька."""
import argparse
import json
from pathlib import Path
import requests
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import call
from record_e3_base import OWNER, UNI, USDC, WETH, tx, ok, result
from record_kaliber_access import AAPL, VAULTS, Reader
from record_kaliber_base import EVENT, TRANSFER

API="https://aggregator-api.kyberswap.com/base/api/v1"
ROUTER="0x6131b5fae19ea4f9d964eac0408e4408b66337b5"
EXECUTOR="0x8f10b468b06c6fd214b65f87778827f7d113f996"
PAIR="0x9640cd02fa4537a1fc5f9573438e87ef690b3c67f04e7bc06f3f262b072e086b"


def build(http,src,dst,amount,deadline):
    q=dict(tokenIn=src,tokenOut=dst,amountIn=str(amount),excludeRFQSources="true")
    response=http.get(API+"/routes",params=q,timeout=20)
    out=dict(src=src,dst=dst,amount=amount,deadline=deadline,routes_status=response.status_code)
    if not response.ok:return out,None
    j=response.json()
    if j.get("code")!=0:return dict(out,routes_code=j.get("code")),None
    summary=j["data"]["routeSummary"]
    assert summary["tokenIn"].lower()==src and summary["tokenOut"].lower()==dst and int(summary["amountIn"])==amount
    out.update(legs=[dict(pool=x["pool"],exchange=x["exchange"]) for part in summary["route"] for x in part],
               expected=int(summary["amountOut"]),quoted_at=summary.get("timestamp"),api_fee=summary.get("extraFee"))
    b=http.post(API+"/route/build",json=dict(routeSummary=summary,sender=OWNER,recipient=OWNER,slippageTolerance=50,
                                            deadline=deadline,enableGasEstimation=False),timeout=20)
    out["build_status"]=b.status_code
    if not b.ok:return out,None
    j=b.json()
    if j.get("code")!=0:return dict(out,build_code=j.get("code")),None
    data=j["data"]
    assert j["data"]["routerAddress"].lower()==summary["routerAddress"].lower()==ROUTER and int(data["amountIn"])==amount
    assert int(data.get("transactionValue",0) or 0)==0
    payload=data["data"];out.update(selector=payload[:10],digest="0x"+keccak(bytes.fromhex(payload[2:])).hex(),build_out=int(data["amountOut"]))
    return out,call(ROUTER,payload,gas=7000000)


def settle(logs,src,dst,spent,received):
    fills=[]; edges={}
    for l in logs:
        t=l.get("topics",[])
        if len(t)==3 and t[0]==TRANSFER and l["address"] in (src,dst):
            edge=(l["address"],"0x"+t[1][-40:],"0x"+t[2][-40:])
            edges[edge]=edges.get(edge,0)+int(l["data"],16)
        if len(t)==3 and t[0]==EVENT and l["address"] in VAULTS:
            tin,tout,used,net,age=decode(["address","address","uint256","uint256","uint256"],bytes.fromhex(l["data"][2:]))
            fills.append(dict(cash=l["address"],trader="0x"+t[2][-40:],pair=t[1],src=tin,dst=tout,paid=used,net=net,age=age))
    def wallet(token):
        return sum(v*((b==OWNER)-(a==OWNER)) for (t,a,b),v in edges.items() if t==token)
    assert -wallet(src)==spent and wallet(dst)==received
    if len(fills)!=1:return fills,False
    f=fills[0]
    pure=(f["trader"]==EXECUTOR and f["pair"]==PAIR and (f["src"],f["dst"])==(src,dst)
          and (f["paid"],f["net"])==(spent,received) and spent>0 and received>0
          and edges.get((src,EXECUTOR,f["cash"]))==spent and edges.get((dst,f["cash"],EXECUTOR))==received)
    return fills,pure


def capture():
    r=Reader("base")
    h=r.rpc("eth_getBlockByNumber",["latest",False])
    tag=dict(blockHash=h["hash"],requireCanonical=True);height=int(h["number"],16);stamp=int(h["timestamp"],16)+2
    observed={v:r.rpc("eth_getCode",[v,tag]) for v in VAULTS}
    assert all("0x"+keccak(bytes.fromhex(code[2:])).hex()==r.code_hash(v) for v,code in observed.items())
    http=requests.Session();rows=[]
    def measure(src,dst,amount,prefix):
        row,swap=build(http,src,dst,amount,stamp+180)
        rows.append(row)
        if not swap:return None
        approve=tx(src,"approve(address,uint256)",["address","uint256"],[ROUTER,amount])
        read=lambda t:tx(t,"balanceOf(address)",["address"],[OWNER])
        seq=prefix+[approve,read(src),read(dst),swap,read(src),read(dst)]
        frame=dict(blockOverrides={"time":hex(stamp)},stateOverrides={OWNER:{"balance":hex(2*10**18)}},calls=seq)
        b,=r.rpc("eth_simulateV1",[dict(blockStateCalls=[frame],validation=False),tag])
        assert b["parentHash"]==h["hash"]
        calls=b["calls"];assert all(ok(x) for x in calls[:len(prefix)])
        approved,sa,da,tr,sb,db=calls[len(prefix):]
        assert all(ok(x) for x in (approved,sa,da,sb,db))
        spent=int.from_bytes(result(sa))-int.from_bytes(result(sb));net=int.from_bytes(result(db))-int.from_bytes(result(da))
        row.update(success=ok(tr),spent=spent,received=net,gas=int(tr["gasUsed"],16),approve_gas=int(approved["gasUsed"],16),
                   error=tr.get("error"),fills=[],pure_kaliber=False)
        if not ok(tr):
            assert spent==net==0
            return None
        assert 0<spent<=amount and net>0
        row["fills"],row["pure_kaliber"]=settle(tr["logs"],src,dst,spent,net)
        row["net_meets_quote_floor"]=net>=row["build_out"]*9950//10000
        return prefix+[approve,swap],net
    for amount in (10**6,500250000):
        seed=tx(UNI,"exactOutputSingle((address,address,uint24,address,uint256,uint256,uint160))",["(address,address,uint24,address,uint256,uint256,uint160)"],[(WETH,USDC,500,OWNER,amount,10**18,0)],10**18)
        bought=measure(USDC,AAPL,amount,[seed])
        if bought:
            prefix,available=bought
            measure(AAPL,USDC,available,prefix)
    return dict(source=API,chain=8453,tag=tag,height=height,timestamp=stamp,payer=OWNER,router=ROUTER,results=rows)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path)
    ap.parse_args().output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

