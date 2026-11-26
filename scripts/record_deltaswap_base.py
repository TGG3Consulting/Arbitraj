"""Reproduce DeltaSwap's complete Base registry and the deployed FOT router's exact wallet limits."""
import argparse
import json
import math
from pathlib import Path

import requests
from eth_abi import decode

from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, OWNER, call, calldata, rpc, simulate_block
from record_base_v2_factories import WETH, USDC
from record_community_v3_ethereum import posts
from record_ethereum_v2_factories import digest, raw, registry

FACTORY="0x9a9a171c69cc811dc6b59bb2f9990e34a22fc971"
PAIR="0xe5ca910f564e7a3ee1ca7a4d9e0d83e0e5dbdd02"
ROUTER="0x1b7655aa64b7bd54077de56b64a0f92bcba05b85"
ENDPOINT="https://mainnet.base.org"
FOLDER=ROOT/"tests/data/first_run_dex_2026-11-26"
FIELDS=(("getReserves()",["uint112","uint112","uint32"]),("getLiquidityEMA()",["uint112","uint32"]),
        ("getTradeLiquidityEMAParams()",["uint112","uint112","uint32"]),("gammaPool()",["address"]))


def header(block):
    body={"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":[hex(block),False]}
    response=requests.post(ENDPOINT,json=body,timeout=18)
    response.raise_for_status()
    value=response.json()["result"]
    assert int(value["number"],16)==block and int(rpc(ENDPOINT,"eth_chainId",[]),16)==8453
    return value


def verified(address,selector,creation=False):
    url="https://sourcify.dev/server/v2/contract/8453/"+address
    fields="compilation,deployment,runtimeBytecode.onchainBytecode"+(",creationBytecode.onchainBytecode" if creation else "")
    response=requests.get(url,params={"fields":fields},timeout=24)
    response.raise_for_status()
    j=response.json()
    code=raw(rpc(ENDPOINT,"eth_getCode",[address,selector]))
    assert j["runtimeMatch"]=="exact_match" and code==raw(j["runtimeBytecode"]["onchainBytecode"])
    evidence=dict(address=address,url=url,contract=j["compilation"]["name"],runtime=digest(code),bytes=len(code),
                  deployment=j["deployment"]["transactionHash"])
    init=raw(j["creationBytecode"]["onchainBytecode"]) if creation else None
    return evidence,code,init


def wallet_case(tag,state,fees,qty,deadline):
    def invoke(target,sig,types=(),args=()):
        return call(target,calldata(sig,types,args))
    def quote(a,b,dx):
        return invoke(ROUTER,"getAmountsOut(uint256,address[])",["uint256","address[]"],[dx,[a,b]])
    def allow(token,dx):
        return invoke(token,"approve(address,uint256)",["address","uint256"],[ROUTER,dx])
    def swap(a,b,dx,out):
        return invoke(ROUTER,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                      ["uint256","uint256","address[]","address","uint256"],[dx,out,[a,b],OWNER,deadline])
    def balance(a):
        return invoke(a,"balanceOf(address)",["address"],[OWNER])
    def read(calls):
        return simulate_block(ENDPOINT,tag,{OWNER:{"balance":hex(qty+10**18)}},calls)
    def word(item):
        return int(item["returnData"],16)
    def qout(before,a,b,dx):
        rows=read(before+[quote(a,b,dx)])
        assert all(int(x["status"],16)==1 for x in rows),str(rows[-1])[:140]
        inputs,outputs=decode(["uint256[]"],raw(rows[-1]["returnData"]))[0]
        assert inputs==dx and outputs>0
        return outputs
    before=[call(WETH,calldata("deposit()"),qty),allow(WETH,qty)]
    bought=qout(before,WETH,USDC,qty)
    second=before+[swap(WETH,USDC,qty,bought),allow(USDC,bought)]
    sold=qout(second,USDC,WETH,bought)
    samples=[]
    for i,(prefix,a,b,dx,out) in enumerate([(before,WETH,USDC,qty,bought),(second,USDC,WETH,bought,sold)]):
        state_calls=[invoke(PAIR,fn) for fn,_ in FIELDS]
        results=read(prefix+state_calls+[balance(a),balance(b),swap(a,b,dx,out),balance(a),balance(b)])
        assert all(x["status"]=="0x1" for x in results)
        r,liq,trade,gamma=[decode(types,raw(x["returnData"])) for (_,types),x in zip(FIELDS,results[len(prefix):],strict=False)]
        assert gamma[0]!=ROUTER
        rin,rout=(r[0],r[1]) if i==0 else (r[1],r[0])
        half=dx//2
        t=math.isqrt(half*(half*rout//rin))
        block=state["height"]+1
        diff=block-trade[2]
        accumulated=t if diff else t+trade[1]
        last=0 if diff>50 else trade[0]
        ema=accumulated if last==0 else accumulated*20//100+last*80//100
        fee=fees[0] if max(t,ema)>=liq[0]*fees[1]//1000 else 0
        net=dx*(1000-fee)
        assert t>0 and net*rout//(rin*1000+net)==out
        assert word(results[-5])-word(results[-2])==dx and word(results[-1])-word(results[-4])==out
        strict=read(prefix+[swap(a,b,dx,out+1)])[-1]
        reason=strict.get("error",{}).get("data") or strict["returnData"]
        assert strict["status"]=="0x0" and reason.startswith("0x08c379a0")
        why=decode(["string"],bytes.fromhex(reason[10:]))[0]
        assert why.endswith("INSUFFICIENT_OUTPUT_AMOUNT"),why
        samples.append(dict(input=a,output=b,amount=str(dx),out=str(out),amount_spent=str(dx),amount_received=str(out),
                            invariant_fee=fee,trade_liquidity=str(t),reserves=list(map(str,r[:2])),
                            liquidity_ema=list(map(str,liq)),trade_ema=list(map(str,trade)),gamma_pool=gamma[0],
                            next_revert=why,gas=int(results[-3]["gasUsed"],16)))
    return samples


def capture(block,complete):
    head=header(block)
    selector={"blockHash":head["hash"],"requireCanonical":True}
    cap=Capture(dict(endpoint=ENDPOINT,selector=selector))
    proof={}
    for role,address in (("factory",FACTORY),("router",ROUTER),("sample_pair",PAIR)):
        row,code,init=verified(address,selector,role=="sample_pair")
        proof[role]=row
        if role=="factory":factory_code=code
        if init is not None:
            offset=factory_code.find(init)
            assert offset>=0 and factory_code.find(init,offset+1)==-1
            proof["pair_init"]=digest(init)
            proof["codecopy"]=[offset,len(init)]
    assert cap.one(ROUTER,"factory()",returns=["address"])==(FACTORY,)
    assert cap.one(ROUTER,"WETH()",returns=["address"])==(WETH,)
    assert cap.one(FACTORY,"getPair(address,address)",["address","address"],[WETH,USDC],["address"])==(PAIR,)
    rows=registry(cap,FACTORY,proof["pair_init"]) if complete else [dict(pool=PAIR)]
    values=iter(posts(cap,[(row["pool"],call_name,[],[]) for row in rows for call_name,_ in FIELDS]))
    for row in rows:
        r,l,t,g=[decode(types,next(values)) for _,types in FIELDS]
        if complete:assert list(map(str,r[:2]))==row["reserves"]
        row.update(liquidity_ema=list(map(str,l)),trade_ema=list(map(str,t)),gamma_pool=g[0])
    fee=cap.one(FACTORY,"dsFeeInfo()",returns=["uint8","uint8"])
    gs,=cap.one(FACTORY,"gsFee()",returns=["uint8"])
    state=dict(height=block,selector=selector,state_root=head["stateRoot"],proof=proof,ds_fee_info=fee,gs_fee=gs,pools=rows)
    reserve=cap.one(PAIR,"getReserves()",returns=FIELDS[0][1])[0]
    deadline=int(head["timestamp"],16)+120
    state["wallet"]=[wallet_case(selector,state,fee,amount,deadline) for amount in [reserve//10000,reserve//10]]
    if fee[1]>0:assert {c["invariant_fee"] for group in state["wallet"] for c in group}=={0,fee[0]}
    return state


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    snap=json.loads((FOLDER/"base.json").read_bytes())
    current=capture(snap["block"],True)
    assert current["selector"]==snap["selector"]
    historical=capture(19_000_000,False)
    args.output.write_text(json.dumps(dict(current=current,historical=historical),indent=2)+"\n",encoding="utf-8",newline="\n")
