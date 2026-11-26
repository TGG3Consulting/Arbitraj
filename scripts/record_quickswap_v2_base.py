"""Фабрика и Router02 из официального справочника QuickSwap Base. Проверяет код пар и обмены штатной формой."""
import re

import requests
from record_lista_ethereum import FOLDER, Capture, calldata, deployed, read, rpc, write
from record_curve_ng_deployments import code
from record_quickswap_base import DOC
from record_solidly_v3 import HOLDERS
from record_v2_chain_pools import minimum


def main():
    text = requests.get(DOC,timeout=20)
    text.raise_for_status()
    section = text.text.split("BASE V2",1)[1].split("BASE V4",1)[0]
    addresses = dict((k,a.lower()) for k,a in re.findall(r"<td>([^<]+)</td><td>(0x[0-9a-fA-F]{40})</td>",section))
    factory,router = addresses["V2 Factory address"],addresses["V2 router address"]
    e = read(FOLDER/"base.json")
    c = Capture(e)
    code(e,factory)
    obj,router_url = deployed(e,router)
    if obj["compilation"]["name"]!="UniswapV2Router02" or c.one(router,"factory()",returns=["address"])[0]!=factory:
        raise ValueError("роутер не подтверждён")
    contract = obj["compilation"]["fullyQualifiedName"].split(":")[0]
    router_source = obj["sources"][contract]["content"]
    if "swapExactTokensForTokensSupportingFeeOnTransferTokens(" not in router_source:
        raise ValueError("нужен ABI штатного обмена")
    count, = c.one(factory,"allPairsLength()",returns=["uint256"])
    samples = read(FOLDER/"catalog_recheck.json")["base"]["pages"]["quickswap-v2-base"]["samples"]
    directory,raw_code,markets = [], None, []
    header = rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    deadline = int(header["timestamp"],16)+300
    for row in samples:
        pool = row["address"].lower()
        tokens = [c.one(pool,k+"()",returns=["address"])[0] for k in ("token0","token1")]
        if (c.one(pool,"factory()",returns=["address"])[0]!=factory or
            c.one(factory,"getPair(address,address)",["address","address"],tokens,["address"])[0]!=pool):
            raise ValueError("пара не принадлежит фабрике")
        pairhash = code(e,pool)
        if raw_code is None:
            pair,pair_url = deployed(e,pool)
            path = pair["compilation"]["fullyQualifiedName"].split(":")[0]
            source = pair["sources"][path]["content"]
            lines = [l.strip() for l in source.splitlines() if "Adjusted =" in l or "balance0Adjusted.mul(balance1Adjusted)" in l]
            if len(lines)!=3 or any("mul(1000).sub(amount"+str(i)+"In.mul(3))" not in lines[i] for i in (0,1)):
                raise ValueError("не проверена комиссия обеих сторон")
            raw_code = pairhash
        if pairhash!=raw_code:
            raise ValueError("неоднородный байткод пары")
        directory.append(dict(pool=pool,tokens=tokens,runtime_hash=pairhash))
        if not all(t in HOLDERS["base"] for t in tokens):
            continue
        reserve = c.one(pool,"getReserves()",returns=["uint112","uint112","uint32"])[:2]
        cases = []
        for side,src in enumerate(tokens):
            dst = tokens[1-side]
            sender = HOLDERS["base"][src]
            if rpc(e,"eth_getCode",[sender,e["selector"]])!="0x":
                raise ValueError("нужен настоящий адрес плательщика без кода")
            dec, = c.one(src,"decimals()",returns=["uint8"])
            for dx in ((10**14,10**15) if dec==18 else (10**6,10**7)):
                want = minimum(dx,reserve[side],reserve[1-side],3000)
                if want==0: continue
                route = [src,dst]
                quoted, = c.one(router,"getAmountsOut(uint256,address[])",["uint256","address[]"],[dx,route],["uint256[]"])
                if tuple(quoted)!=(dx,want): raise ValueError("котировка расходится с формулой")
                swap = lambda n:calldata("swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                                         ["uint256","uint256","address[]","address","uint256"],[dx,n,route,sender,deadline])
                balance = calldata("balanceOf(address)",["address"],[sender])
                approve = lambda n:calldata("approve(address,uint256)",["address","uint256"],[router,n])
                calls = [("reset",src,approve(0)),("approve",src,approve(dx)),("in_before",src,balance),("out_before",dst,balance),
                         ("above",router,swap(want+1)),("exact",router,swap(want)),("in_after",src,balance),("out_after",dst,balance)]
                txs = [{"from":sender,"to":to,"data":data} for _,to,data in calls]
                stage,=rpc(e,"eth_simulateV1",[{"blockStateCalls":[{"calls":txs}],"validation":False},e["selector"]])
                if stage["parentHash"]!=e["selector"]["blockHash"]: raise ValueError("другая голова")
                results = {k:r for (k,_,_),r in zip(calls,stage["calls"],strict=True)}
                if any(r["status"]!=("0x0" if k=="above" else "0x1") for k,r in results.items()): raise ValueError("ошибка исполнения")
                n = lambda key:int(results[key]["returnData"],16)
                if n("in_before")-n("in_after")!=dx or n("out_after")-n("out_before")!=want: raise ValueError("ошибка балансов")
                cases.append(dict(side=side,dx=str(dx),out=str(want),sender=sender,calls=txs,results=results))
        markets.append(dict(pool=pool,tokens=tokens,reserves=[str(r) for r in reserve],cases=cases))
    if not any(len(m["cases"])==4 for m in markets):raise ValueError("нет полной матрицы")
    write(FOLDER/"quickswap_v2_base.json",dict(chain_id=e["chain_id"],endpoint=e["endpoint"],selector=e["selector"],documentation=DOC,
        factory=factory,router=router,official_count=count,router_source=router_url,pair_source=pair_url,fee_evidence=lines,
        canonical_pair_hash=raw_code,directory=directory,markets=markets,calls=c.calls,code={a:e["contracts"][a] for a in (factory,router)}))
    print(count,len(directory),[(m["pool"],len(m["cases"])) for m in markets])


if __name__=="__main__":
    main()
