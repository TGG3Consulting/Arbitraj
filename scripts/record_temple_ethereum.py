"""Сверяет TempleStableAMMRouter, фактическую валюту выплаты и границу treasury на Ethereum."""
import argparse
import json
from pathlib import Path
import requests
from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import OWNER, ROOT, rpc, simulate_block
from record_ethervista_wallets import tx, integer
from record_ethereum_v2_factories import WETH, FUNDING, digest
from record_ethereum_x7 import verified, ENDPOINTS, ROOT_HASH

PAIR="0x6021444f1706f15465bee85463bcc7d7cc17fc03"
ROUTER="0x98257c876ace5009e7b97843f8c71b3ae795c71e"
TEMPLE="0x470ebf5f030ed85fc1ed4c2d36b9dd02e77cf1b7"
FRAX="0x853d955acef822db058eb8505911ed77f175b99e"
DAI="0x6b175474e89094c44da98b954eedeac495271d0f"


def capture(c):
    head=rpc(c.endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if int(rpc(c.endpoint,"eth_chainId",[]),16)!=1 or head["stateRoot"]!=ROOT_HASH or head["hash"]!=c.selector["blockHash"]:
        raise ValueError("другой блок")
    roles={}
    for name,address in (("TempleUniswapV2Pair",PAIR),("TempleStableAMMRouter",ROUTER)):
        url,record,code=verified(c,address,name)
        roles[name]=dict(address=address,source=url,hash=digest(code))
    def get(address,sig,types=(),args=(),ret="address"):
        return c.one(address,sig,list(types),list(args),ret.split(","))
    if (get(PAIR,"router()")[0],get(PAIR,"token0()")[0],get(PAIR,"token1()")[0],get(ROUTER,"templeToken()")[0],get(ROUTER,"defendStable()")[0],
        get(ROUTER,"tokenPair(address)",["address"],[FRAX])[0])!=(ROUTER,TEMPLE,FRAX,TEMPLE,DAI,PAIR):
        raise ValueError("связь пула, роутера или валюты изменилась")
    treasury=get(ROUTER,"templeTreasury()")[0]
    assets=[TEMPLE,FRAX,DAI]
    balances=[tx(asset,"balanceOf(address)",["address"],[OWNER]) for asset in assets]
    quote_sell=lambda size:get(ROUTER,"swapExactTempleForStableQuote(address,uint256)",["address","uint256"],[PAIR,size],"bool,uint256")
    def rebalance(trials):
        return simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(10**19)}},trials)
    deadline=int(head["timestamp"],16)+120
    def one(prefix,buy,amount):
        source,dest=(FRAX,TEMPLE) if buy else (TEMPLE,FRAX)
        before=len(prefix)+1
        approve=tx(source,"approve(address,uint256)",["address","uint256"],[ROUTER,amount])
        signature="swapExactStableForTemple" if buy else "swapExactTempleForStable"
        def trial(limit,expires):
            action=tx(ROUTER,signature+"(uint256,uint256,address,address,uint256)",["uint256","uint256","address","address","uint256"],
                      [amount,limit,FRAX,OWNER,expires])
            rows=rebalance([*prefix,approve,*balances,action,*balances])
            if any(r["status"]!="0x1" for r in rows[:before]):
                raise ValueError("нет финансирования обычного кошелька")
            delta=[integer(y)-integer(x) for x,y in zip(rows[before:before+3],rows[-3:],strict=True)]
            return rows[before+3],delta,action,rows[before-1]
        loose,d,_,_=trial(1,deadline)
        if loose["status"]!="0x1" or d[assets.index(source)]!=-amount or d[2]!=0 or d[assets.index(dest)]<=0:
            raise ValueError(("нет ожидаемой выплаты",loose.get("error"),d))
        target=d[assets.index(dest)]
        ok,same,action,approval=trial(target,deadline)
        above,unchanged,_,_=trial(target+1,deadline)
        expired,untouched,_,_=trial(1,int(head["timestamp"],16)-1)
        if ok["status"]!="0x1" or same!=d or above["status"]!="0x0" or expired["status"]!="0x0" or any(unchanged+untouched):
            raise ValueError("предел/срок не откатил операцию")
        return dict(input=source,output=dest,amount=str(amount),minimum=str(target),delta=list(map(str,d)),gas=ok["gasUsed"],
                    approve_gas=approval["gasUsed"],return_data=ok["returnData"],logs=ok["logs"],too_high=above["error"],expired=expired["error"]),[*prefix,approve,action]
    cases=[]
    for amount in (10**18,10**19):
        funding=tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],
                   [amount,[WETH,FRAX],OWNER,deadline],value=10**18)
        buy,prefix=one([funding],True,amount)
        sell,_=one(prefix,False,int(buy["minimum"]))
        cases.append(dict(buy=buy,sell=sell))
    alternative=[]
    for size in (10**15,10**18,10**20,10**21,10**22):
        flag,qty=quote_sell(size)
        available=get(DAI,"balanceOf(address)",["address"],[ROUTER],"uint256")[0] if flag else None
        alternative.append(dict(size=str(size),uses_treasury=flag,output=DAI if flag else FRAX,quoted=str(qty),
                                inventory=str(available) if flag else None))
    if not any(v["uses_treasury"] and int(v["quoted"])>int(v["inventory"]) for v in alternative):
        raise ValueError("не воспроизвелась граница выплаты treasury")
    return dict(chain_id=1,selector=c.selector,roles=roles,pair=PAIR,router=ROUTER,treasury=treasury,
                stable=FRAX,defend_stable=DAI,assets=assets,wallet=OWNER,cases=cases,alternative=alternative)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    snapshot=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_text())
    errors=[]
    for endpoint in ENDPOINTS:
        try:
            result=capture(Capture(dict(snapshot,endpoint=endpoint)))
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as error:
            errors.append((endpoint,str(error)[:280]))
    else:
        raise ValueError(errors)
