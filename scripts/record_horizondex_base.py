"""Bind Base HorizonDEX Elastic V2 registry and periphery to funded wallet swaps and QuoterV2 on the same block."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_classic_directory import ENDPOINTS
from record_base_slipstream_executor import ROOT, EXEC, RUNTIME, STEP, calldata, rpc
from record_solidly_v3 import BASE_WETH, HOLDERS

FACTORY="0x07aced5690e09935b1c0e6e88b772d9440f64718"
POOL="0x66543ab7d0ac497ee5ff77e8c5965efcdb895f7f"
ROUTER="0x99aec509174cbf06f8f7e15ddeeb7bcc32363827"
QUOTER="0x94ddde405a00180891ed79dc1147f0d841c30d73"
READER="0x2e1cba4a9f6e69d27e65046fc93b873b05f79a39"
ORDER="(address,address,uint24,address,uint256,uint256,uint256,uint160)"
QUERY="(address,address,uint256,uint24,uint160)"

def capture(endpoint):
    e=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    e["endpoint"]=endpoint;tag=e["selector"];c=Capture(e)
    head=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if rpc(endpoint,"eth_chainId",[])!="0x2105" or int(head["number"],16)!=e["block"]:
        raise ValueError("wrong network or parent")
    deadline=int(head["timestamp"],16)+120
    addr=lambda to,method,types=(),args=():c.one(to,method,types,args,["address"])[0]
    a,b=[addr(POOL,f"token{i}()") for i in (0,1)]
    fee,=c.one(POOL,"swapFeeUnits()",returns=["uint24"])
    if a!=BASE_WETH or addr(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[a,b,fee])!=POOL:
        raise ValueError("factory registry differs")
    for role in (ROUTER,QUOTER,POOL):
        if addr(role,"factory()")!=FACTORY:raise ValueError("foreign periphery")
    hashes={}
    for role,address in dict(factory=FACTORY,pool=POOL,router=ROUTER,quoter=QUOTER,tick_reader=READER).items():
        code=rpc(endpoint,"eth_getCode",[address,tag])
        if code=="0x":raise ValueError("empty role "+role)
        hashes[role]={"address":address,"runtime_hash":"0x"+keccak(hexstr=code).hex()}
    info=c.one(POOL,"getPoolState()",returns=["uint160","int24","int24","bool"])
    liquid=c.one(POOL,"getLiquidityState()",returns=["uint128","uint128","uint128"])
    tick_distance,=c.one(POOL,"tickDistance()",returns=["int24"])
    ticks=[];start=-887272;chunk=32
    for _ in range(1024):
        page,=c.one(READER,"getTicksInRange(address,int24,uint32)",["address","int24","uint32"],[POOL,start,chunk],["int24[]"])
        # The Solidity reader leaves unused entries zeroed after MAX_TICK.
        if 887272 in page:
            end=page.index(887272)+1
            if any(page[end:]):raise ValueError("nonzero ticks after upper bound")
            page=page[:end]
        if not page or page[0]!=start or tuple(sorted(set(page)))!=page:raise ValueError("bad ticks page")
        ticks.extend(page if not ticks else page[1:])
        if page[-1]==887272:break
        if page[-1]<=start:raise ValueError("no advance on tick list")
        start=page[-1]
    else:raise ValueError("open ticks list")
    answers=posts(c,[(POOL,"ticks(int24)",["int24"],[t]) for t in ticks])
    points={str(t):list(decode(["uint128","int128","uint256","uint128"],bits)[:2]) for t,bits in zip(ticks,answers,strict=True)}
    sender=HOLDERS["base"][a]
    def tx(to,method,types=(),args=()):
        return {"from":sender,"to":to,"data":calldata(method,types,args),"gas":"0x300000"}
    word=lambda v:"0x"+format(v,"064x")
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    override={EXEC:{"code":"0x"+runtime,"stateDiff":{word(0):word(int(sender,16)),word(1):word(int(a,16))}}}
    def simulate(items,executor):
        frame={"calls":[v for _,v in items]}
        if executor:frame["stateOverrides"]=override
        stage,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},tag])
        if stage["parentHash"]!=tag["blockHash"]:raise ValueError("changed parent")
        return dict(zip([k for k,_ in items],stage["calls"],strict=True))
    def leg(prefix,side,x,y,amount,executor):
        def order(floor,end):
            if executor:
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                    ["address","uint256","address","uint256","uint256",STEP+"[]"],
                    [x,amount,y,floor,end,[(8,POOL,x,y,10000,b"")]])
            return tx(ROUTER,"swapExactInputSingle("+ORDER+")",[ORDER],[(x,y,fee,sender,end,amount,floor,0)])
        balance=lambda token:tx(token,"balanceOf(address)",["address"],[sender])
        inp=[("allow",tx(x,"approve(address,uint256)",["address","uint256"],[EXEC if executor else ROUTER,amount])),
             ("quote",tx(QUOTER,"quoteExactInputSingle("+QUERY+")",[QUERY],[(x,y,amount,fee,0)])),
             ("start_in",balance(x)),("start_out",balance(y))]
        prepare=prefix+[(side+k,v) for k,v in inp]
        before=simulate(prepare,executor)
        raw=before[side+"quote"]
        if raw["status"]!="0x1":raise ValueError(raw.get("error"))
        used,out,*_=decode(["uint256","uint256","uint160","uint32","uint256"],bytes.fromhex(raw["returnData"][2:]))
        if not out or used!=amount:raise ValueError("partial pool quote")
        effects=[("above",order(out+1,deadline)),("expired",order(out,0)),("exact",order(out,deadline)),("end_in",balance(x)),("end_out",balance(y))]
        effects += [("pool_state",tx(POOL,"getPoolState()")),("liquidity",tx(POOL,"getLiquidityState()"))]
        if executor:
            effects.extend((name,tx(token,"balanceOf(address)",["address"],[EXEC])) for name,token in [("left_in",x),("left_out",y)])
        steps=prepare+[(side+k,v) for k,v in effects];results=simulate(steps,executor)
        for label,item in results.items():
            if item["status"]!=("0x0" if label.endswith(("above","expired")) else "0x1"):raise ValueError((label,item.get("error")))
        n=lambda label:int(results[side+label]["returnData"],16)
        if (n("start_in")-n("end_in"),n("end_out")-n("start_out"),n("exact"))!=(used,out,out):
            raise ValueError("payer delta or recipient net payout differs")
        if executor and (n("left_in") or n("left_out")):raise ValueError("V5 retained a token")
        return steps,out,dict(amount=str(amount),received=str(out),calls=[v for _,v in steps],results=results)
    trials=[]
    for amount in [10**12,10**13]:
        case={}
        for executor in (False,True):
            steps,bought,buy=leg([],"buy",a,b,amount,executor)
            _,_,sell=leg(steps,"sell",b,a,bought,executor)
            if executor:case["v5"]={"buy":buy,"sell":sell}
            else:case.update(buy=buy,sell=sell)
        for side in ("buy","sell"):
            if case[side]["received"]!=case["v5"][side]["received"]:raise ValueError("native/V5 mismatch")
        trials.append(case)
    return dict(chain_id=8453,selector=tag,state_root=head["stateRoot"],roles=hashes,tokens=[a,b],fee=fee,spacing=tick_distance,
                state=info,liquidity=liquid,tick_reader_chunk=chunk,ticks=points,raw=c.calls,cases=trials,v5="0x"+keccak(hexstr=runtime).hex())

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);args=p.parse_args()
    for endpoint in ENDPOINTS:
        try:
            args.output.write_text(json.dumps(capture(endpoint),indent=2)+"\n",encoding="utf8",newline="\n")
            break
        except (OSError,ValueError,KeyError) as err:print(endpoint,str(err)[:300],flush=True)
    else:raise ValueError("no canonical settlement on HorizonDEX")
