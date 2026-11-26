"""Reproduce DeltaSwap's complete Base or Ethereum registry and the deployed FOT router's exact wallet limits."""
import argparse
import json
import math
from dataclasses import dataclass, replace
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

@dataclass(frozen=True)
class Market:
    chain: int
    factory: str
    pair: str
    router: str
    wrapped: str
    token: str

BASE=Market(8453,FACTORY,PAIR,ROUTER,WETH,USDC)
ETHEREUM=Market(1,"0x5fbe219e88f6c6f214ce6f5b1fcaa0294f31ae1b",
    "0x1321228815fcf38b75356760db800c7becc7731f","0xcf2b6bc8c0e0a1292db7f0ae89410670796350c8",
    "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2","0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
ETH_ENDPOINTS=("https://mainnet.gateway.tenderly.co","https://eth.drpc.org","https://ethereum-rpc.publicnode.com")
STATE_ROOTS[26494988]="0xbd73f03df514c4d550c21a6d567e25a4190f646acbb94b8e8c2462d7c9b17db7"

FOLDER=ROOT/"tests/data/first_run_dex_2026-11-26"
FIELDS=(("getReserves()",["uint112","uint112","uint32"]),("getLiquidityEMA()",["uint112","uint32"]),
        ("getTradeLiquidityEMAParams()",["uint112","uint112","uint32"]),("gammaPool()",["address"]))


def header(endpoint,block,chain):
    body={"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":[hex(block),False]}
    response=requests.post(endpoint,json=body,timeout=18)
    response.raise_for_status()
    value=response.json()["result"]
    assert int(value["number"],16)==block and int(rpc(endpoint,"eth_chainId",[]),16)==chain
    return value


def verified(endpoint,address,selector,chain,creation=False):
    url=f"https://sourcify.dev/server/v2/contract/{chain}/{address}"
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


def wallet_case(endpoint,tag,state,fees,qty,deadline,market):
    def invoke(target,sig,types=(),args=()):
        return call(target,calldata(sig,types,args))
    def quote(a,b,dx):
        return invoke(market.router,"getAmountsOut(uint256,address[])",["uint256","address[]"],[dx,[a,b]])
    def allow(token,dx):
        return invoke(token,"approve(address,uint256)",["address","uint256"],[market.router,dx])
    def swap(a,b,dx,out):
        return invoke(market.router,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
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
    before=[call(market.wrapped,calldata("deposit()"),qty),allow(market.wrapped,qty)]
    bought=qout(before,market.wrapped,market.token,qty)
    second=before+[swap(market.wrapped,market.token,qty,bought),allow(market.token,bought)]
    sold=qout(second,market.token,market.wrapped,bought)
    samples=[]
    for prefix,a,b,dx,out in [(before,market.wrapped,market.token,qty,bought),(second,market.token,market.wrapped,bought,sold)]:
        state_calls=[invoke(market.pair,fn) for fn,_ in FIELDS]
        results=read(prefix+state_calls+[balance(a),balance(b),swap(a,b,dx,out),balance(a),balance(b)])
        assert all(x["status"]=="0x1" for x in results)
        r,liq,trade,gamma=[decode(types,raw(x["returnData"])) for (_,types),x in zip(FIELDS,results[len(prefix):],strict=False)]
        assert gamma[0]!=market.router
        rin,rout=(r[0],r[1]) if a<b else (r[1],r[0])
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


def capture(endpoint,block,expected_hash,complete,market):
    head=header(endpoint,block,market.chain)
    assert head["hash"]==expected_hash and head["stateRoot"]==STATE_ROOTS[block],"archive header or state root changed"
    selector={"blockHash":head["hash"],"requireCanonical":True}
    cap=Capture(dict(endpoint=endpoint,selector=selector))
    proof={}
    for role,address in (("factory",market.factory),("router",market.router),("sample_pair",market.pair)):
        row,code,init=verified(endpoint,address,selector,market.chain,role=="sample_pair")
        proof[role]=row
        if role=="factory":factory_code=code
        if init is not None:
            offset=factory_code.find(init)
            assert offset>=0 and factory_code.find(init,offset+1)==-1
            proof["pair_init"]=digest(init)
            proof["codecopy"]=[offset,len(init)]
    assert cap.one(market.router,"factory()",returns=["address"])==(market.factory,)
    assert cap.one(market.router,"WETH()",returns=["address"])==(market.wrapped,)
    assert cap.one(market.factory,"getPair(address,address)",["address","address"],[market.wrapped,market.token],["address"])==(market.pair,)
    rows=registry(cap,market.factory,proof["pair_init"]) if complete else [dict(pool=market.pair)]
    values=iter(posts(cap,[(row["pool"],call_name,[],[]) for row in rows for call_name,_ in FIELDS]))
    for row in rows:
        r,l,t,g=[decode(types,next(values)) for _,types in FIELDS]
        if complete:assert list(map(str,r[:2]))==row["reserves"]
        row.update(liquidity_ema=list(map(str,l)),trade_ema=list(map(str,t)),gamma_pool=g[0])
    fee=cap.one(market.factory,"dsFeeInfo()",returns=["uint8","uint8"])
    gs,=cap.one(market.factory,"gsFee()",returns=["uint8"])
    state=dict(height=block,selector=selector,state_root=head["stateRoot"],proof=proof,ds_fee_info=fee,gs_fee=gs,pools=rows)
    reserves=cap.one(market.pair,"getReserves()",returns=FIELDS[0][1])
    wrapped_index=int(market.wrapped>market.token)
    reserve=reserves[wrapped_index]
    deadline=int(head["timestamp"],16)+120
    sizes=[reserve//10000,reserve//10]
    if fee[1]>0:
        # On this historical parent, the first leg is just below the fee boundary.
        # The reverse leg has both inputs in tradeLiquiditySum at the same EVM block.
        other=reserves[1-wrapped_index]
        liquidity=cap.one(market.pair,"getLiquidityEMA()",returns=FIELDS[1][1])[0]
        threshold_input=2*(liquidity*fee[1]//1000)*reserve//math.isqrt(reserve*other)
        sizes.append(threshold_input*9//10)
    state["wallet"]=[wallet_case(endpoint,selector,state,fee,amount,deadline,market) for amount in sizes]
    if fee[1]>0:
        assert {c["invariant_fee"] for group in state["wallet"] for c in group}=={0,fee[0]}
        assert [trade["invariant_fee"] for trade in state["wallet"][-1]]==[0,fee[0]]
    if market.chain==1 and complete:
        # The indexer lists only USDC/WETH. Inspect the two other markets holding most WETH by actual raw reserves.
        book=[(int(row["reserves"][row["tokens"].index(market.wrapped)]),row) for row in rows
              if market.wrapped in row["tokens"] and row["pool"]!=market.pair]
        state["other_wallets"]=[]
        for amount,row in sorted(book,key=lambda item:-item[0])[:2]:
            other=row["tokens"][1-row["tokens"].index(market.wrapped)]
            selected=replace(market,pair=row["pool"],token=other)
            result=dict(pool=row["pool"],token=other,wrapped_reserve=str(amount),samples=[])
            for qty in [amount//10000,amount//10]:
                try:
                    if qty<=0:raise ValueError("zero input")
                    result["samples"].append(dict(input=str(qty),trades=wallet_case(endpoint,selector,state,fee,qty,deadline,selected)))
                except (ValueError,AssertionError) as err:
                    result["samples"].append(dict(input=str(qty),error=str(err)[:170] or "wallet settlement assertion failed"))
            state["other_wallets"].append(result)
    return state


def canonical(block,expected_hash,complete,market=BASE):
    errors=[]
    for endpoint in (ETH_ENDPOINTS if market.chain==1 else ENDPOINTS):
        try:
            return capture(endpoint,block,expected_hash,complete,market)
        except (requests.RequestException,ValueError,KeyError,AssertionError,RuntimeError) as error:
            errors.append((endpoint,str(error)[:200]))
    raise RuntimeError(f"No endpoint supplied the required canonical evidence: {errors}")


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    parser.add_argument("--network",choices=("base","ethereum"),default="base")
    args=parser.parse_args()
    snap=json.loads((FOLDER/f"{args.network}.json").read_bytes())
    market=ETHEREUM if args.network=="ethereum" else BASE
    current=canonical(snap["block"],snap["selector"]["blockHash"],True,market)
    assert current["selector"]==snap["selector"]
    result=dict(current=current)
    if args.network=="base":result["historical"]=canonical(19_000_000,HISTORICAL_HASH,False)
    args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8",newline="\n")
