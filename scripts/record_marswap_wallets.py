"""MarSwap: обе фабрики, варианты нативной и процентной оплаты на настоящем кошельке."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from record_ethereum_v2_factories import registry, digest, raw, WETH
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, EXEC, RUNTIME, rpc, calldata
from record_ethereum_x7 import ENDPOINTS
from record_solidly_v3 import HOLDERS
from record_v2_chain_pools import swap
from record_omni_v2_base import role as _base_role

DEPLOYMENTS={"base":"0xee42fe6d6be1ef43701ddabc417ad22d82c5ecc3","ethereum":"0x5e763172d59b3b580af29a1c9fa4ac1cee69c5dd"}
ROUTER="0x2f52793a3f5d27ce9f856081dcbed42611394243"
ZERO="0x"+"00"*20


def compare(c,head,pool,pair,terms):
    payer=HOLDERS["ethereum"][WETH];token=next(a for a in pair if a!=WETH)
    deadline=int(head["timestamp"],16)+180;total=terms[1]+terms[2];flat=terms[3]
    def tx(to,sig,types=(),args=(),value=0):
        return {"from":payer,"to":to,"data":calldata(sig,types,args),"gas":"0x500000","value":hex(value)}
    def run(calls,via):
        frame={"calls":calls}
        if via=="v5":
            word=lambda n:"0x"+format(n,"064x")
            frame["stateOverrides"]={EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),"stateDiff":{word(0):word(int(payer,16)),word(1):word(int(WETH,16))}}}
        block,=rpc(c.endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},c.selector])
        if block["parentHash"]!=c.selector["blockHash"]:raise ValueError("changed canonical state")
        return block["calls"]
    def leg(prefix,via,x,y,amount):
        target=EXEC if via=="v5" else ROUTER;fee=0 if via=="flat" else total
        appro=tx(x,"approve(address,uint256)",["address","uint256"],[target,amount]);bal=lambda t,a:tx(t,"balanceOf(address)",["address"],[a])
        reads=[bal(x,pool),bal(y,pool),bal(x,payer),bal(y,payer),tx(pool,"getReserves()")]
        pre=prefix+[appro]+reads
        front=run(pre,via);r=list(decode(["uint112","uint112","uint32"],raw(front[-1]["returnData"])))[:2]
        if pair.index(x):r.reverse()
        used=amount*(10000-fee);out=used*r[1]//(10000*r[0]+used)
        if out==0:raise ValueError("zero nominal output")
        def order(minimum,when):
            if via=="v5":return {**swap(pool,x,y,amount,minimum,when,total*100),"from":payer}
            return tx(ROUTER,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                      ["uint256","uint256","address[]","address","uint256"],[amount,minimum,[x,y],payer,when],flat if via=="flat" else 0)
        value=lambda a:decode(["uint256"],raw(a["returnData"]))[0]
        model=out
        probe=run(pre+[order(1,deadline),bal(x,payer),bal(y,payer)],via)
        if probe[-3]["status"]!="0x1":
            raise ValueError((via,x,amount,"unavailable at one raw unit",probe[-3].get("error",{}).get("message")))
        out=value(probe[-1])-value(front[-2])
        if out<=0 or value(front[-3])-value(probe[-2])!=amount:
            raise ValueError((via,x,amount,"wallet debit/credit unavailable"))
        calls=pre+[order(out+1,deadline),order(out,0),order(out,deadline),bal(x,payer),bal(y,payer)]
        result=run(calls,via);exact=result[-3]
        if exact["status"]!="0x1" or result[-5]["status"]!="0x0" or result[-4]["status"]!="0x0":
            raise ValueError((via,amount,exact.get("error"),"controls",result[-5]["status"],result[-4]["status"]))
        if value(result[-8])-value(result[-2])!=amount or value(result[-1])-value(result[-7])!=out:raise ValueError("not full wallet settlement")
        return calls,out,dict(input=str(amount),net=str(out),nominal=str(model),reserves=list(map(str,r)),pair_balances=[str(value(a)) for a in front[-5:-3]],msg_value=str(flat if via=="flat" else 0),calls=calls,results=result)
    cases=[]
    for via in ["percentage","v5","flat"]:
        for amount in [10**8,10**9]:
            try:
                calls,bought,buy=leg([],via,WETH,token,amount);_,_,sell=leg(calls,via,token,WETH,bought)
                cases.append(dict(mode=via,buy=buy,sell=sell))
            except (ValueError,KeyError,OverflowError) as exc:
                cases.append(dict(mode=via,input=str(amount),rejected=str(exc)[:260]))
    return dict(owner=payer,cases=cases)


def capture(chain,endpoint):
    network=DEPLOYMENTS[chain];snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26"/(chain+".json")).read_bytes());c=Capture(dict(snap,endpoint=endpoint))
    head=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False]);factory=rpc(endpoint,"eth_getCode",[network,c.selector])
    if head["hash"]!=c.selector["blockHash"] or factory=="0x":raise ValueError("wrong canonical factory")
    init,=c.one(network,"INIT_CODE_PAIR_HASH()",returns=["bytes32"]);count,=c.one(network,"allPairsLength()",returns=["uint256"])
    rows=registry(c,network,"0x"+init.hex()) if count else []
    terms=c.one(network,"lpInfo()",returns=["address","uint256","uint256","uint256"])
    info=dict(id="marswap-"+chain,selector=c.selector,height=snap["block"],root=head["stateRoot"],factory=network,factory_code_hash=digest(raw(factory)),
              init_hash="0x"+init.hex(),terms=list(terms),markets=rows,markets_count=count)
    if not count:return info
    # The router named by the MARS token belongs to another DEX; compare the actual historical pair callback caller.
    source,router=_base_role(c,ROUTER,chain_id=1)
    if c.one(ROUTER,"factory()",returns=["address"])[0]!=network or c.one(ROUTER,"WETH()",returns=["address"])[0]!=WETH:raise ValueError("wrong router")
    if "lpFee + marsFee" not in source["sources"]["MarsRouter.sol"]["content"]:raise ValueError("unexpected pricing source")
    info["router"]=router
    for r in rows:
        r["runtime_hash"]=digest(raw(rpc(endpoint,"eth_getCode",[r["pool"],c.selector])))
        if WETH in r["tokens"] and all(map(int,r["reserves"])):
            r["settlements"]=compare(c,head,r["pool"],r["tokens"],terms)
    cases=[case for r in rows for case in r.get("settlements",{}).get("cases",[])]
    if not cases or len(cases)!=sum("settlements" in r for r in rows)*6:
        raise ValueError("incomplete ordinary router or V5 observations")
    info["complete_round_trips"]=sum("buy" in case and "sell" in case for case in cases)
    return info

if __name__=="__main__":
    args=argparse.ArgumentParser(description=__doc__);args.add_argument("network",choices=DEPLOYMENTS);args.add_argument("output",type=Path);args=args.parse_args()
    for ep in (["https://mainnet.base.org","https://base-rpc.publicnode.com"] if args.network=="base" else ENDPOINTS):
        try:
            data=capture(args.network,ep);args.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8",newline="\n");break
        except (ValueError,KeyError,OSError) as exc:print(ep,str(exc)[:450],flush=True)
    else:raise ValueError("нет канонического снимка MarSwap")
