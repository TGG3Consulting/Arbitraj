"""Reproduce DeltaSwap's complete Base registry and the deployed FOT router's exact wallet limits."""
import argparse
import json
import math
from pathlib import Path

import requests
from eth_abi import decode

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT, OWNER, call, calldata, rpc, simulate_block
from record_base_v2_factories import WETH, USDC
from record_ethereum_v2_factories import digest, raw, registry

FACTORY="0x9a9a171c69cc811dc6b59bb2f9990e34a22fc971"
PAIR="0xe5ca910f564e7a3ee1ca7a4d9e0d83e0e5dbdd02"
ROUTER="0x1b7655aa64b7bd54077de56b64a0f92bcba05b85"
ENDPOINTS=("https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com")
HISTORICAL_HASH="0xa0d5bc196ab72a8bedcc36cd66c3395adf38feb7a86f340275308a22aa2b6240"
STATE_ROOTS={62262756:"0x037b0c48cd33236cb94ec9e9fc7438c120b138c2c31d39d0ebe42521ee65676b",19_000_000:"0x31c948b65417c12d9f11df676d39ce57d8c14439c20be917e948069e8b11ad07"}

FOLDER=ROOT/"tests/data/first_run_dex_2026-11-26"
FIELDS=(("getReserves()",["uint112","uint112","uint32"]),("getLiquidityEMA()",["uint112","uint32"]),
        ("getTradeLiquidityEMAParams()",["uint112","uint112","uint32"]),("gammaPool()",["address"]))


def header(endpoint,block):
    body={"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":[hex(block),False]}
    response=requests.post(endpoint,json=body,timeout=18)
    response.raise_for_status()
    value=response.json()["result"]
    assert int(value["number"],16)==block and int(rpc(endpoint,"eth_chainId",[]),16)==8453
    return value


def verified(endpoint,address,selector,creation=False):
    url="https://sourcify.dev/server/v2/contract/8453/"+address
    fields="compilation,deployment,runtimeBytecode.onchainBytecode"+(",creationBytecode.onchainBytecode" if creation else "")
    response=requests.get(url,params={"fields":fields},timeout=24)
    response.raise_for_status()
    j=response.json()
    code=raw(rpc(endpoint,"eth_getCode",[address,selector]))
    assert j["runtimeMatch"]=="exact_match" and code==raw(j["runtimeBytecode"]["onchainBytecode"])
    evidence=dict(address=address,url=url,contract=j["compilation"]["name"],runtime=digest(code),bytes=len(code),
                  deployment=j["deployment"]["transactionHash"])
    init=raw(j["creationBytecode"]["onchainBytecode"]) if creation else None
    return evidence,code,init


def wallet_case(endpoint,tag,state,fees,qty,deadline):
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
        return simulate_block(endpoint,tag,{OWNER:{"balance":hex(qty+10**18)}},calls)
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


def capture(endpoint,block,expected_hash,complete):
    head=header(endpoint,block)
    assert head["hash"]==expected_hash and head["stateRoot"]==STATE_ROOTS[block],"archive header or state root changed"
    selector={"blockHash":head["hash"],"requireCanonical":True}
    cap=Capture(dict(endpoint=endpoint,selector=selector))
    proof={}
    for role,address in (("factory",FACTORY),("router",ROUTER),("sample_pair",PAIR)):
        row,code,init=verified(endpoint,address,selector,role=="sample_pair")
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
    sizes=[reserve//10000,reserve//10]
    if fee[1]>0:
        # On this historical parent, the first leg is just below the fee boundary.
        # The reverse leg has both inputs in tradeLiquiditySum at the same EVM block.
        other=cap.one(PAIR,"getReserves()",returns=FIELDS[0][1])[1]
        liquidity=cap.one(PAIR,"getLiquidityEMA()",returns=FIELDS[1][1])[0]
        threshold_input=2*(liquidity*fee[1]//1000)*reserve//math.isqrt(reserve*other)
        sizes.append(threshold_input*9//10)
    state["wallet"]=[wallet_case(endpoint,selector,state,fee,amount,deadline) for amount in sizes]
    if fee[1]>0:
        assert {c["invariant_fee"] for group in state["wallet"] for c in group}=={0,fee[0]}
        assert [trade["invariant_fee"] for trade in state["wallet"][-1]]==[0,fee[0]]
    return state


def canonical(block,expected_hash,complete):
    errors=[]
    for endpoint in ENDPOINTS:
        try:
            return capture(endpoint,block,expected_hash,complete)
        except (requests.RequestException,ValueError,KeyError,AssertionError,RuntimeError) as error:
            errors.append((endpoint,str(error)[:200]))
    raise RuntimeError(f"No endpoint supplied the required canonical evidence: {errors}")


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    snap=json.loads((FOLDER/"base.json").read_bytes())
    current=canonical(snap["block"],snap["selector"]["blockHash"],True)
    assert current["selector"]==snap["selector"]
    historical=canonical(19_000_000,HISTORICAL_HASH,False)
    args.output.write_text(json.dumps(dict(current=current,historical=historical),indent=2)+"\n",encoding="utf-8",newline="\n")
