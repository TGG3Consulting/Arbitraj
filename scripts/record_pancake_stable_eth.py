"""Подтверждает PancakeSwap StableSwap Ethereum по API проекта, журналу создания и реестру."""
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_curve_ng_deployments import code

API="https://configs.pancakeswap.com/api/data/cached/stableswaps?chainId=1"
PAIR_SOURCE="31235deb5e2e60a47391484937ce61fdb88d785f8a8b2d4752b03b6f594f3e76"
TOPIC="0x"+keccak(text="NewStableSwapPair(address,address,address,address,address)").hex()


def main():
    folder=ROOT/"tests/data/first_run_dex_2026-11-26"
    e=read(folder/"ethereum.json")
    c=Capture(e)
    r=requests.get(API,timeout=15)
    r.raise_for_status()
    markets=[]
    factories=set()
    for entry in r.json():
        pool=entry["stableSwapAddress"].lower()
        verified_url=f"https://sourcify.dev/server/v2/contract/1/{pool}?fields=sources,compilation,deployment,runtimeBytecode.onchainBytecode"
        source=requests.get(verified_url,timeout=20).json()
        content=source["sources"]["contracts/PancakeStableSwapTwoPool.sol"]["content"]
        digest="0x"+keccak(bytes.fromhex(source["runtimeBytecode"]["onchainBytecode"][2:])).hex()
        if source["runtimeMatch"]!="exact_match" or code(e,pool)!=digest or keccak(text=content).hex()!=PAIR_SOURCE:
            raise ValueError(f"вариант пула расходится с решением 471: {pool}")
        tx=source["deployment"]["transactionHash"]
        receipt=requests.post(e["endpoint"],json=dict(id=1,jsonrpc="2.0",method="eth_getTransactionReceipt",params=[tx]),timeout=20).json()["result"]
        events=[log for log in receipt["logs"] if log["topics"][0]==TOPIC and log["topics"][1][-40:]==pool[2:]]
        if len(events)!=1:
            raise ValueError("нет однозначного NewStableSwapPair")
        f=events[0]["address"]
        factories.add(f)
        x,y=[entry[key]["address"].lower() for key in ("token","quoteToken")]
        actual,t0,t1,lp=c.one(f,"getPairInfo(address,address)",["address","address"],[x,y],["address"]*4)
        if actual!=pool or {t0,t1}!={x,y} or lp!=entry["lpAddress"].lower():
            raise ValueError("реестр расходится с API и событием")
        killed,=c.one(pool,"is_killed()",returns=["bool"])
        coins=[c.one(pool,"coins(uint256)",["uint256"],[i],["address"])[0] for i in range(2)]
        if set(coins)!={x,y}:
            raise ValueError("монеты пула расходятся с API")
        dec=[c.one(a,"decimals()",returns=["uint8"])[0] for a in coins]
        balances=[c.one(pool,"balances(uint256)",["uint256"],[i],["uint256"])[0] for i in range(2)]
        amp,=c.one(pool,"A()",returns=["uint256"])
        fee,=c.one(pool,"fee()",returns=["uint256"])
        q=[]
        for i in range(2):
            for divisor in (10000,100):
                dx=balances[i]//divisor
                raw,=c.batch([(pool,"get_dy(uint256,uint256,uint256)",["uint256"]*3,[i,1-i,dx])])
                if raw and int.from_bytes(raw):
                    q.append(dict(i=i,j=1-i,dx=str(dx),dy=str(int.from_bytes(raw))))
        markets.append(dict(pool=pool,killed=killed,coins=coins,decimals=dec,balances=list(map(str,balances)),a=str(amp),fee=str(fee),
                            quotes=q,creation_tx=tx,creation_event=events[0],verified_source=verified_url,runtime=digest))
    if len(factories)!=1:
        raise ValueError("выборка требует разных фабрик")
    factory,=factories
    code(e,factory)
    total,=c.one(factory,"pairLength()",returns=["uint256"])
    listed=[c.one(factory,"swapPairContract(uint256)",["uint256"],[i],["address"])[0] for i in range(total)]
    if set(listed)!={r["pool"] for r in markets}:
        raise ValueError("в официальном списке отсутствует пул фабрики")
    write(folder/"pancake_stable_ethereum.json",dict(chain_id=1,selector=e["selector"],endpoint=c.endpoint,source=API,
          factory=factory,source_keccak=PAIR_SOURCE,markets=markets,calls=c.calls))
    if not any(not p["killed"] and len(p["quotes"])==4 for p in markets):
        raise ValueError("нет активного пула с двусторонними котировками")
    write(folder/"ethereum.json",e)
    path=ROOT/"config/first_run_dex.json"
    dex="pancakeswap-stableswap-ethereum"
    rows=[r for r in read(path) if (r["network"],r["id"])!=("ethereum",dex)]
    rows.append(dict(network="ethereum",chain_id=1,id=dex,source=API,settings={"dex.kind":"pancake_stableswap","dex.factory":factory}))
    write(path,rows)
    print(total,[(p["killed"],len(p["quotes"])) for p in markets])


if __name__=="__main__":
    main()
