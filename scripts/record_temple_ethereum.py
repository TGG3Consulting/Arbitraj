"""Сверяет TempleStableAMMRouter, фактическую валюту выплаты и границу treasury на Ethereum."""
import argparse
import json
from pathlib import Path
import requests
from eth_abi import encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import OWNER, ROOT, EXEC, RUNTIME, STEP, rpc, simulate_block
from record_ethervista_wallets import tx, integer
from record_ethereum_v2_factories import WETH, FUNDING, digest
from record_ethereum_x7 import verified, ENDPOINTS, ROOT_HASH

PAIR="0x6021444f1706f15465bee85463bcc7d7cc17fc03"
ROUTER="0x98257c876ace5009e7b97843f8c71b3ae795c71e"
TEMPLE="0x470ebf5f030ed85fc1ed4c2d36b9dd02e77cf1b7"
FRAX="0x853d955acef822db058eb8505911ed77f175b99e"
DAI="0x6b175474e89094c44da98b954eedeac495271d0f"
MODULE="0x"+keccak(text="Arbitraj Temple protected module Ethereum")[-20:].hex()


def module_at(c, artifact):
    blank=bytes.fromhex(artifact["runtime"])
    args=encode(["address","address"],[PAIR,ROUTER])
    creation={"from":OWNER,"data":"0x"+artifact["creation"]+args.hex()}
    request=c.session.post(c.endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":[creation,c.selector]},timeout=30)
    request.raise_for_status()
    result=bytes.fromhex(request.json()["result"][2:])
    check=bytearray(result)
    roles=set()
    for group in artifact["immutableReferences"].values():
        words=set()
        for ref in group:
            i,n=ref["start"],ref["length"]
            if n!=32:raise ValueError("immutable width")
            words.add(result[i:i+n]);check[i:i+n]=blank[i:i+n]
        if len(words)!=1:raise ValueError("inconsistent immutable")
        roles.update(words)
    if (roles!={encode(["address"],[PAIR]),encode(["address"],[ROUTER])} or bytes(check)!=blank
            or any(rpc(c.endpoint,"eth_getCode",[a,c.selector])!="0x" for a in (MODULE,EXEC))):
        raise ValueError("different deployed runtime or occupied address")
    return result


def capture(c,artifact=None):
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
    overrides={OWNER:{"balance":hex(10**19)}}
    module_hash=None
    if artifact:
        module=module_at(c,artifact)
        module_hash=digest(module)
        word=lambda x: "0x"+format(x,"064x")
        overrides[MODULE]={"code":"0x"+module.hex()}
        overrides[EXEC]={"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                         "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}
    def rebalance(trials):
        return simulate_block(c.endpoint,c.selector,overrides,trials)
    deadline=int(head["timestamp"],16)+120
    def one(prefix,buy,amount,mode):
        source,dest=(FRAX,TEMPLE) if buy else (TEMPLE,FRAX)
        before=len(prefix)+1
        spender={"router":ROUTER,"protected":MODULE,"v5":EXEC}[mode]
        approve=tx(source,"approve(address,uint256)",["address","uint256"],[spender,amount])
        signature="swapExactStableForTemple" if buy else "swapExactTempleForStable"
        def trial(limit,expires,out=dest):
            if mode=="router":
                action=tx(ROUTER,signature+"(uint256,uint256,address,address,uint256)",["uint256","uint256","address","address","uint256"],
                          [amount,limit,FRAX,OWNER,expires])
            elif mode=="protected":
                action=tx(MODULE,"swapFrom(address,address,uint256,uint256,address,uint256)",["address","address","uint256","uint256","address","uint256"],
                          [source,out,amount,limit,OWNER,expires])
            else:
                action=tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],[source,amount,out,limit,expires,[(29,MODULE,source,out,10000,b"")]])
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
        if mode!="router":
            wrong,other,_,_=trial(1,deadline,DAI)
            if wrong["status"]!="0x0" or any(other):raise ValueError("wrong payout currency")
        return dict(input=source,output=dest,amount=str(amount),minimum=str(target),delta=list(map(str,d)),gas=ok["gasUsed"],
                    approve_gas=approval["gasUsed"],return_data=ok["returnData"],logs=ok["logs"],too_high=above["error"],expired=expired["error"]),[*prefix,approve,action]
    cases=[]
    for mode in (("router","protected","v5") if artifact else ("router",)):
        for amount in (10**18,10**19):
            funding=tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],
                       [amount,[WETH,FRAX],OWNER,deadline],value=10**18)
            buy,prefix=one([funding],True,amount,mode)
            sell,_=one(prefix,False,int(buy["minimum"]),mode)
            cases.append(dict(mode=mode,buy=buy,sell=sell))
    if any([v[k]["delta"] for k in ("buy","sell")] != [cases[i%2][k]["delta"] for k in ("buy","sell")] for i,v in enumerate(cases)):
        raise ValueError("router and protected wallet outcomes differ")
    rt,rs,_=get(PAIR,"getReserves()",ret="uint112,uint112,uint32")
    iv,base=get(treasury,"intrinsicValueRatio()",ret="uint256,uint256")
    crossing=(rs*995*base-rt*1000*iv)//(995*iv)
    alternative=[]
    for size in sorted({1,10**15,10**18,10**20,10**21,10**22,*range(max(1,crossing-5),crossing+8)}):
        flag,qty=quote_sell(size)
        available=get(DAI,"balanceOf(address)",["address"],[ROUTER],"uint256")[0] if flag else None
        alternative.append(dict(size=str(size),uses_treasury=flag,output=DAI if flag else FRAX,quoted=str(qty),
                                inventory=str(available) if flag else None))
    if not any(v["uses_treasury"] and int(v["quoted"])>int(v["inventory"]) for v in alternative):
        raise ValueError("не воспроизвелась граница выплаты treasury")
    return dict(chain_id=1,selector=c.selector,state_root=head["stateRoot"],roles=roles,pair=PAIR,router=ROUTER,treasury=treasury,
                module_hash=module_hash,reserves=[str(rt),str(rs)],ratio=[str(iv),str(base)],router_defense_balance=str(get(DAI,"balanceOf(address)",["address"],[ROUTER],"uint256")[0]),
                stable=FRAX,defend_stable=DAI,assets=assets,wallet=OWNER,cases=cases,alternative=alternative)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output",type=Path)
    ap.add_argument("--artifact",type=Path)
    args=ap.parse_args()
    snapshot=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_text())
    errors=[]
    for endpoint in ENDPOINTS:
        try:
            result=capture(Capture(dict(snapshot,endpoint=endpoint)),json.loads(args.artifact.read_bytes()) if args.artifact else None)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as error:
            errors.append((endpoint,str(error)[:280]))
    else:
        raise ValueError(errors)
