"""Сверяет iZiSwap Base и VirtualQuoter бота с собственным квотером iZiSwap на одном блоке."""
import argparse
import hashlib
import re
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture, ZERO
from record_curve_ng_deployments import code

REV="652b26927370cf963be361fd36d87c762db8c848"
DOC=f"https://raw.githubusercontent.com/izumiFinance/izumi-docs/{REV}/docs/source/iZiSwap/deployed_contracts/mainnet.rst"


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshots",type=Path)
    args=parser.parse_args()
    doc=requests.get(DOC,timeout=15)
    doc.raise_for_status()
    table=doc.text.split("Base (chainId: 8453)")[1].split(".. list-table::",2)[1]
    roles=dict(re.findall(r"^\s*- (.+?)\s*\n\s*- (0x[\da-fA-F]{40})",table,flags=re.M))
    folder=ROOT/"tests/data/first_run_dex_2026-11-26"
    e=read(folder/"base.json")
    c=Capture(e)
    for contract in roles.values():
        code(e,contract)
    factory=roles["factory"]
    quoter=roles["quoter (without limit)"]
    fx=read(args.snapshots/"base-integration-2026-11-26/sources/sample-pools.json")["iziswap-base"]
    vq=ROOT/"contracts/out/VirtualQuoter.bin-runtime"
    text=vq.read_text("utf8").strip()
    at="0x"+keccak(text="arb.virtual_quoter")[-20:].hex()
    tiers={}
    for f in (100,400,2000,10000):
        spacing,=c.one(factory,"fee2pointDelta(uint24)",["uint24"],[f],["int24"])
        if spacing:
            tiers[str(f)]=spacing
    pools=[]
    for sample in fx["samples"]:
        pool=sample["address"].lower()
        x,=c.one(pool,"tokenX()",returns=["address"])
        y,=c.one(pool,"tokenY()",returns=["address"])
        fee,=c.one(pool,"fee()",returns=["uint24"])
        owner,=c.one(pool,"factory()",returns=["address"])
        reported,=c.one(factory,"pool(address,address,uint24)",["address","address","uint24"],[x,y,fee],["address"])
        if owner!=factory.lower() or reported!=pool or not x<y:
            continue
        if str(fee) not in tiers:
            spacing,=c.one(factory,"fee2pointDelta(uint24)",["uint24"],[fee],["int24"])
            if spacing<=0:
                raise ValueError("пул реестра имеет неподтверждённый шаг точек")
            tiers[str(fee)]=spacing
        quotes=[]
        for a,b in ((x,y),(y,x)):
            balance,=c.one(a,"balanceOf(address)",["address"],[pool],["uint256"])
            for k in (100000,1000):
                amount=balance//k
                if not amount or amount>=1<<128:
                    continue
                path=bytes.fromhex(a[2:])+fee.to_bytes(3,"big")+bytes.fromhex(b[2:])
                native,=c.batch([(quoter,"swapAmount(uint128,bytes)",["uint128","bytes"],[amount,path])])
                if not native:
                    continue
                out,points=decode(["uint256","int24[]"],native)
                sig=keccak(text="quoteIzi(address,bool,uint256)")[:4]
                calldata="0x"+(sig+encode(["address","bool","uint256"],[pool,a==x,amount])).hex()
                req=dict(jsonrpc="2.0",id=1,method="eth_call",params=[{"from":ZERO,"to":at,"data":calldata},
                         e["selector"],{at:{"code":"0x"+text}}])
                resp=requests.post(e["endpoint"],json=req,timeout=20)
                resp.raise_for_status()
                reply=resp.json()
                if reply.get("result"):
                    used,got=decode(["uint256","uint256"],bytes.fromhex(reply["result"][2:]))
                else:
                    used,got=0,0
                quotes.append(dict(token_in=a,amount_in=str(amount),native_out=str(out),point_after=points,
                              virtual_used=str(used),virtual_out=str(got),data=calldata,response=reply,
                              matched=used==amount and got==out and out>0))
        pools.append(dict(pool=pool,x=x,y=y,fee=fee,quotes=quotes))
    passed=[p for p in pools if len(p["quotes"])==4 and all(q["matched"] for q in p["quotes"])]
    write(folder/"iziswap_base.json",dict(chain_id=e["chain_id"],selector=e["selector"],factory=factory,
          endpoint=e["endpoint"],source=DOC,roles=roles,tiers=tiers,virtual_address=at,
          virtual_runtime_keccak="0x"+keccak(bytes.fromhex(text)).hex(),pools=pools,calls=c.calls))
    if not passed:
        raise ValueError("нет пула с полной двусторонней проверкой")
    write(folder/"base.json",e)
    conf=ROOT/"config/first_run_dex.json"
    records=[r for r in read(conf) if not(r["network"]=="base" and r["id"]=="iziswap-base")]
    records.append(dict(chain_id=8453,network="base",id="iziswap-base",source=DOC,settings={
                   "dex.kind":"iziswap","dex.factory":factory,"dex.quoter_virtual":True,"dex.v3_fee_tiers":list(tiers)}))
    write(conf,records)
    print(len(pools),len(passed))


if __name__=="__main__":
    main()
