"""Реестры SafeSwap на Ethereum и фактический расход плательщика через Router и шаг V2 по одному бюджету."""
import argparse
import json
import re
from pathlib import Path

import requests
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, OWNER, rpc
from record_diamondswap_base import direct_wallet
from record_ethereum_v2_factories import WETH, STABLES, FUNDING, digest, registry, raw
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH
from record_ethervista_wallets import tx

DEPLOYMENTS=[
    ("original","0x26023843814cff92b8d75311d64d1c032b8b29f2","0xcf7d4b75b7bccdb8b4f992fe05970680e2ee1a02"),
    ("v2","0x9cf2f35e3656d4c68474525d67d9459da3a000cd","0x1fdd76e18dd21046b7e7d54c8254bf08b239e4d9"),
    ("proxy-1","0x84d02e4ef772a3915def2d0c3f5d9452f1376795","0xeb70d821aab9171ac02f676c2e3c6fd2df16da09"),
    ("proxy-2","0x7f09d4be6bbf4b0ff0c97ca5c486a166198aeaee","0x0a654cb371fbc3c49ccb176984d64e86fc931eea"),
]

def roles(c,factory,router):
    if c.one(router,"factory()",returns=["address"])!=(factory,) or c.one(router,"WETH()",returns=["address"])!=(WETH,):
        raise ValueError("router binding differs")
    out={}
    for key,addr in (("factory",factory),("router",router)):
        code=rpc(c.endpoint,"eth_getCode",[addr,c.selector]);src="https://sourcify.dev/server/v2/contract/1/"+addr
        response=c.session.get(src,params={"fields":"compilation,runtimeBytecode.onchainBytecode,sources,deployment"},timeout=25)
        response.raise_for_status();contract=response.json()
        if not raw(code) or code.lower()!=contract["runtimeBytecode"]["onchainBytecode"].lower() or contract.get("runtimeMatch") not in ("match","exact_match"):
            raise ValueError("different "+key)
        out[key]=dict(address=addr,runtime=digest(raw(code)),name=contract["compilation"]["name"],source=src,creation=contract["deployment"]["transactionHash"],deployer=contract["deployment"]["deployer"])
        if contract["compilation"]["name"]=="SafeswapFactory":
            text="".join("".join(re.sub(r"/\*.*?\*/|//[^\n]*","",v["content"],flags=re.S).split()) for v in contract["sources"].values())
            if not all(x in text for x in ("balance0.mul(1000).sub(amount0In.mul(2))","balance1.mul(1000).sub(amount1In.mul(2))",
                                         "_takeFee(_token0,amount0Out", "_safeTransfer(_token0,to,amount0Out)","_safeTransfer(_token1,to,amount1Out)")):
                raise ValueError("different pair invariant or payout")
        if contract["compilation"]["name"]=="SafeswapRouter":
            text="".join("".join(re.sub(r"/\*.*?\*/|//[^\n]*","",v["content"],flags=re.S).split()) for v in contract["sources"].values())
            ratios=[(n,d) for n,d in ((9975,10000),(998,1000)) if f"amountInWithFee=amountIn.mul({n})" in text and f"denominator=reserveIn.mul({d}).add(amountInWithFee)" in text]
            if len(ratios)!=1 or "balanceOf(to).sub(balanceBefore)>=amountOutMin" not in text:raise ValueError("different router fee or wallet floor")
            numerator,denominator=ratios[0]
            out[key]["fee_pips"]=1_000_000*(denominator-numerator)//denominator
    return out

def capture(endpoint,wallet):
    snapshot=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c=Capture(dict(snapshot,endpoint=endpoint));h=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if h["hash"]!=c.selector["blockHash"] or h["stateRoot"]!=ROOT_HASH or rpc(endpoint,"eth_chainId",[])!="0x1":raise ValueError("wrong header")
    result=[]
    for name,f,router in DEPLOYMENTS:
        binding=roles(c,f,router)
        init=c.one(f,"INIT_CODE_PAIR_HASH()",returns=["bytes32"])[0]
        markets=registry(c,f,"0x"+init.hex())
        tokens=[(token,"balanceOf(address)",["address"],[m["pool"]]) for m in markets for token in m["tokens"]]
        from record_base_classic_directory import optional
        balances=iter(optional(c,tokens))
        for m in markets:
            m["balances"]=[str(int.from_bytes(v,"big")) if v else None for v in (next(balances),next(balances))]
            m["runtime"]=digest(raw(rpc(endpoint,"eth_getCode",[m["pool"],c.selector])))
        row=dict(id=name,roles=binding,init="0x"+init.hex(),markets=markets)
        result.append(row)
        if name not in ("original","v2"):continue
        if binding["factory"]["deployer"].lower()!=binding["router"]["deployer"].lower():raise ValueError("unrelated factory and router deployments")
        if len({m["runtime"] for m in markets})!=1:raise ValueError("multiple pair runtime versions")
        row["pair_pips"]=2000;row["router_pips"]=binding["router"]["fee_pips"]
        if not wallet:continue
        for m in markets:
            money=next((x for x in [WETH,*STABLES] if x in m["tokens"]),None)
            if money is None:continue
            native=money==WETH
            cap=int(m["reserves"][m["tokens"].index(money)])
            values=sorted(set(min(n,max(1,cap//div)) for n,div in [(10**14 if native else 100_000,10000),(10**16 if native else 1_000_000,500)]))
            m["rounds"]=[];m["unavailable"]=[]
            stamp=int(h["timestamp"],16)
            def fund(amount):return tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[amount,[WETH,money],OWNER,stamp+120],10**16)
            for size in values:
                for mode in ("v5","router"):
                    try:
                        rounds=direct_wallet(c,m,[size],stamp,2000,router,wrapped=WETH,modes=(mode,),cash=money,funding=None if native else fund,audit=True,keep_partial=True)
                        m["rounds"].extend(rounds)
                    except (KeyError,ValueError) as error:m["unavailable"].append(dict(mode=mode,size=str(size),cause=str(error)[:220]))
        if not all(any(t["mode"]==mode and "sell" in t for m in markets for t in m.get("rounds",[]))for mode in ("v5","router")):raise ValueError("missing execution mode for "+name)
    return dict(selector=c.selector,state_root=h["stateRoot"],chain=1,height=int(h["number"],16),deployments=result)

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output",type=Path);cli.add_argument("--wallet",action="store_true");args=cli.parse_args()
    failures=[]
    for url in ENDPOINTS:
        try:
            result=capture(url,args.wallet);args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8");break
        except (ValueError,RuntimeError,requests.RequestException,KeyError) as err:failures.append((url,str(err)[:250]))
    else:raise RuntimeError(failures)
