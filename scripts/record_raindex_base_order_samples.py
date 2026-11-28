"""Две отдельные публичные покупки за собственные средства в изолированном дочернем блоке Base."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from record_raindex_wallets import GOOD, OWNER, capture

from record_raindex_quotes import DATA

USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
WETH="0x4200000000000000000000000000000000000006"
WLTH="0x99b2b1a2adb02b38222adcd057783d7e5d1fcc7d"
FACTORY="0x5e7bb104d84c7cb9b682aac2f3d509f5f406809a"
ROUTER="0xbe6d8f0d05cc4be24d5167a3ef062215be6d18a5"
POOL="0x02d48df39d05596439b3aee17e2a2a9512dd0f07"

def replay(books,seed,run,bal,approve,tx,end,money,identities,tag):
    def word(response):
        if not GOOD(response):
            raise ValueError("control read")
        return int(response["returnData"],16)
    controls=run([tx(POOL,method,[],[]) for method in ("token0","token1","factory","tickSpacing")]
                 +[tx(ROUTER,"factory",[],[])])
    usdc,wlth,factory,spacing,router_factory=map(word,controls)
    if [usdc,wlth,factory,router_factory]!=[int(a,16) for a in (USDC,WLTH,FACTORY,FACTORY)]:
        raise ValueError("Slipstream identities")
    [pair]=run([tx(FACTORY,"getPool",["address","address","int24"],[USDC,WLTH,spacing])])
    if word(pair)!=int(POOL,16):
        raise ValueError("Slipstream registry")
    args=(USDC,WLTH,spacing,OWNER,end,1_000_000,1,0)
    bridge=[approve(USDC,ROUTER,1_000_000),tx(ROUTER,"exactInputSingle",
        ["(address,address,int24,address,uint256,uint256,uint256,uint160)"],[args])]
    bridges=run(seed+[bal(USDC),bal(WLTH)]+bridge+[bal(USDC),bal(WLTH)])
    if not all(map(GOOD,bridges)):
        raise ValueError("WLTH funding")
    before=list(map(word,bridges[len(seed):len(seed)+2]));after=list(map(word,bridges[-2:]))
    if before[0]-after[0]!=1_000_000 or after[1]<=before[1]:
        raise ValueError("WLTH owner deltas")
    outcomes=[]
    for book in books:
        for order in book["orders"]:
            for leg in order["quotes"]:
                asset,out=leg["pay"],leg["receive"]
                if not order["exists"] or asset not in (USDC,WETH,WLTH) or out not in (USDC,WETH,WLTH):
                    continue
                if leg.get("wallet_balance") is not None:
                    funded=min(int(leg["wallet_balance"],16),int(leg["book_allowance"],16))>0
                else:
                    raw=int(leg["vault"],16)
                    funded=bool(raw & ((1<<224)-1)) if book["version"]>=5 else raw>0
                if not funded:
                    continue
                dec_in=6 if asset==USDC else 18;dec_out=6 if out==USDC else 18
                prefix=seed+bridge if asset==WLTH else seed
                budget={WLTH:after[1]-before[1],WETH:10**15,USDC:1_000_000}[asset]
                for size in ((1_000,10_000) if out==USDC else (10**10,10**12)):
                    _,case=money(prefix,book,order,leg,budget,size,True,dec_in,dec_out)
                    outcomes.append(case)
    return {"selector":tag,"payer":OWNER,"modules":identities,"wlth_funding":{"source":
        "https://github.com/aerodrome-finance/slipstream","factory":FACTORY,"pool":POOL,
        "router":ROUTER,"spacing":spacing,"before":list(map(str,before)),"after":list(map(str,after))},
        "cases":outcomes}

def main():
    p=argparse.ArgumentParser();p.add_argument("compiled",type=Path);p.add_argument("output",type=Path);a=p.parse_args()
    snapshot=json.loads(gzip.decompress((DATA/"raindex_base_deployment_balances.json.gz").read_bytes()))
    compiled=json.loads(a.compiled.read_bytes())
    for url in ("https://base.gateway.tenderly.co","https://mainnet.base.org"):
        try:
            result={k:capture("base",url,artifact,selected_books=snapshot["order_samples"],additional=replay)
                    for k,artifact in (("direct",None),("protected",compiled))}
        except (KeyError,ValueError,requests.RequestException) as e:
            print(str(e)[:160],flush=True)
            if url.endswith("mainnet.base.org"):
                raise
        else:
            a.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
            break

if __name__=="__main__":
    main()
