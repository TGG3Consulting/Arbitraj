"""Проверка официального Router и шага Algebra V5 на реальных default/custom-пулах Cypher Ethereum."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, STEP, rpc
from record_base_classic_directory import optional
from record_bitswap_ethereum import tx
from record_cypher_degen_v2 import DATA
from record_cypher_v4 import FACTORY, QUOTER, ROUTER
from record_diamondswap_base import direct_wallet
from record_ethereum_v2_factories import WETH, STABLES, FUNDING
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

PARAMS="(address,address,address,address,uint256,uint256,uint256,uint160)"

def funding_path(c, directory, target_pool, token, end, amount):
    # Funding is a genuine purchase of exactly the tested reserve using only deposited WETH.
    # Its allowance is limited to a separate budget; unused WETH stays with this payer.
    if token==WETH:return [tx(WETH,"deposit()",value=amount)]
    cap=10**17
    if token in STABLES:
        return [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[amount,[WETH,token],OWNER,end],cap)]
    paths=[m for m in directory if m["pool"]!=target_pool and {WETH,token}==set(m["tokens"])]
    if not paths:return None
    operations=[(QUOTER,"quoteExactOutputSingle(address,address,address,uint256,uint160)",["address","address","address","uint256","uint160"],[WETH,token,m["deployer"],amount,0]) for m in paths]
    quoted=optional(c,operations);eligible=[]
    for m,raw in zip(paths,quoted,strict=True):
        if raw and len(raw)>=32 and 0<int.from_bytes(raw[:32],"big")<=cap:eligible.append((int.from_bytes(raw[:32],"big"),m["deployer"]))
    if not eligible:return None
    _,dep=min(eligible)
    return [tx(WETH,"deposit()",value=cap),tx(WETH,"approve(address,uint256)",["address","uint256"],[ROUTER,cap]),
            tx(ROUTER,"exactOutputSingle("+PARAMS+")",[PARAMS],[(WETH,token,dep,OWNER,end,amount,cap,0)])]

def capture(ep, shard, uncovered=False):
    directory=json.loads(gzip.decompress((DATA/"cypher_v4_ethereum_directory.json.gz").read_bytes()))
    tag=directory["selector"];head=rpc(ep,"eth_getBlockByHash",[tag["blockHash"],False])
    if rpc(ep,"eth_chainId",[])!="0x1" or head["hash"]!=tag["blockHash"] or head["stateRoot"]!=ROOT_HASH or int(head["number"],16)!=directory["height"]:
        raise ValueError("incorrect canonical Ethereum archive")
    if any(rpc(ep,"eth_getCode",[a,tag])!="0x" for a in (OWNER,EXEC)):raise ValueError("occupied test wallet")
    c=Capture(dict(endpoint=ep,selector=tag));result=[]
    proved=set()
    if uncovered:
        earlier=json.loads(gzip.decompress((DATA/f"cypher_v4_ethereum_wallets_{shard}.json.gz").read_bytes()))
        if earlier["selector"]!=tag:raise ValueError("different reference registry")
        proved={m["pool"] for m in earlier["markets"] if any("sell" in r for r in m["cases"])}
    for idx,m in enumerate(directory["markets"]):
        if idx%4!=shard or m["pool"] in proved:continue
        row=dict(pool=m["pool"],deployer=m["deployer"],liquidity=m["liquidity"],cases=[])
        result.append(row)
        if not uncovered and not int(m["liquidity"]):row["skipped"]="zero active liquidity";continue
        cash=next((a for a in (WETH,*STABLES) if a in m["tokens"]),None)
        if cash is None and uncovered:
            depth={t:max((int(other["liquidity"]) for other in directory["markets"] if other["pool"]!=m["pool"] and set(other["tokens"])=={t,WETH}),default=-1) for t in m["tokens"]}
            cash=max(depth,key=depth.get)
        if cash is None:row["skipped"]="no independent WETH/settlement funding";continue
        n,=optional(c,[(cash,"balanceOf(address)",["address"],[m["pool"]])])
        if n is None or (not uncovered and not int.from_bytes(n,"big")):row["skipped"]="no physical cash";continue
        physical=int.from_bytes(n,"big");dec=c.one(cash,"decimals()",returns=["uint8"])[0]
        unit=10**15 if cash==WETH else 10**dec
        deadline=int(head["timestamp"],16)+120
        def prefund(amount):
            return tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[amount,[WETH,cash],OWNER,deadline],10**18)
        def action(a,b,size,limit,end,mode):
            if mode=="router":return tx(ROUTER,"exactInputSingle("+PARAMS+")",[PARAMS],[(a,b,m["deployer"],OWNER,end,size,limit,0)])
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[a,size,b,limit,end,[(2,m["pool"],a,b,10000,b"")]])
        def quote(a,b,size):
            return tx(QUOTER,"quoteExactInputSingle(address,address,address,uint256,uint160)",["address","address","address","uint256","uint160"],[a,b,m["deployer"],size,0])
        sizes=sorted({max(1,min(unit,physical//1000)),max(2,min(unit*10,physical//100))}) if physical else [max(1,unit//100),unit]
        for size in sizes:
            for mode in ("router","v5"):
                connected=funding_path(c,directory["markets"],m["pool"],cash,deadline,size) if uncovered else None
                if uncovered and connected is None:
                    row["cases"].append(dict(mode=mode,size=str(size),refused="no independent funded input at this size"));continue
                try:
                    one,=direct_wallet(c,m,[size],int(head["timestamp"],16),0,ROUTER,wrapped=WETH,modes=(mode,),
                                      audit=True,keep_partial=True,cash=cash,funding=(lambda _:connected) if uncovered else None if cash==WETH else prefund,
                                      build_action=action,quote_call=quote)
                    row["cases"].append(one)
                except ValueError as err:
                    if not any(str(err).startswith(reason) for reason in ("could not settle ","recipient floor or wallet debit differs","failed funding or allowance","failed quote funding or allowance")):raise
                    row["cases"].append(dict(mode=mode,size=str(size),refused=str(err)[:320]))
        if uncovered and not any("sell" in case for case in row["cases"]):
            token=next(t for t in m["tokens"] if t!=cash)
            decimal=c.one(token,"decimals()",returns=["uint8"])[0]
            quantity=max(1,10**decimal//10000)
            for size in [quantity,quantity*10]:
                fund=funding_path(c,directory["markets"],m["pool"],token,deadline,size)
                if not fund:
                    row["cases"].append(dict(mode="independent-sale",size=str(size),refused="no independent funding for reverse sell"));continue
                for mode in ("router","v5"):
                    try:
                        one,=direct_wallet(c,m,[size],int(head["timestamp"],16),0,ROUTER,wrapped=WETH,modes=(mode,),
                                  cash=cash,funding=prefund,fund_sale=lambda _:fund,audit=True,build_action=action,quote_call=quote)
                        row["cases"].append(dict(one,independently_funded_sale=True))
                    except ValueError as err:
                        if not any(str(err).startswith(reason) for reason in ("could not settle ","recipient floor or wallet debit differs","failed funding or allowance","failed quote funding or allowance")):raise
                        row["cases"].append(dict(mode=mode,size=str(size),independently_funded_sale=True,refused=str(err)[:320]))
    if not uncovered and not any("buy" in r for m in result for r in m["cases"]):raise ValueError("no paid wallet outcome in shard")
    return dict(height=directory["height"],selector=tag,state_root=ROOT_HASH,factory=FACTORY,router=ROUTER,quoter=QUOTER,shard=shard,markets=result)

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("shard",type=int,choices=range(4));ap.add_argument("output",type=Path)
    ap.add_argument("--uncovered",action="store_true");args=ap.parse_args();errors=[]
    for ep in ENDPOINTS:
        try:
            d=capture(ep,args.shard,args.uncovered)
            args.output.write_text(json.dumps(d,separators=(",",":")),encoding="utf8");break
        except (requests.RequestException,KeyError,RuntimeError,ValueError) as e:errors.append((ep,str(e)[:350]))
    else:raise ValueError(errors)
