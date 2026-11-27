"""DAGrow Strategic Swap Pool. Pin upgrade and oracle guard, compare payer-specific quotes to wallet settlement."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode

from record_balancer_markets import Capture
from record_base_slipstream_executor import simulate_block
from record_bitswap_ethereum import BLOCK, ENDPOINTS, FUNDING, OWNER, WETH, read_batch, tx
from record_ethereum_v2_factories import digest, raw
from record_ethereum_x7 import ROOT_HASH

POOL = "0x9fc2038b773bde9e44fdaba0e5093647f53c5105"
DAGR = "0xfd06f2ea2a769a7698e3a5ef4a42a41a2cc0d4b9"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
# Public liquidity holder used only in an isolated archive block. No signature is requested.
MEMBER = "0xea3de3fc768959af7df63a9af3e91a26d2b4604c"
SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
SOURCE = "https://dagrow.app/whitepaper/"


def sent(who, asset, method, types=(), values=(), value=0):
    return {**tx(asset,method,types,values,value), "from": who}


def capture(endpoint, payments):
    at = {"blockHash":BLOCK,"requireCanonical":True}
    chain,head = read_batch(endpoint,[("eth_chainId",[]),("eth_getBlockByHash",[BLOCK,False])])
    if chain!="0x1" or head["hash"]!=BLOCK or head["stateRoot"]!=ROOT_HASH:
        raise ValueError("unexpected chain or state root")
    c=Capture(dict(endpoint=endpoint,selector=at))
    def single(a,name,types=(),args=(),ret="address"):
        return c.one(a,name,types,args,[ret])[0]
    guard=single(POOL,"poolGuard()")
    tokens=[single(POOL,name) for name in ("token0()","token1()")]
    if {DAGR,USDT}!=set(tokens) or single(POOL,"stablecoin()")!=USDT or single(POOL,"dagrToken()")!=DAGR:
        raise ValueError("SSP token identity differs")
    roles={}
    for name,address in [("pool",POOL),("guard",guard)]:
        word, code=read_batch(endpoint,[("eth_getStorageAt",[address,SLOT,at]),("eth_getCode",[address,at])])
        impl="0x"+word[-40:]
        bytecode,=read_batch(endpoint,[("eth_getCode",[impl,at])])
        if not raw(code) or not raw(bytecode):
            raise ValueError(("unbound upgrade",name,impl))
        roles[name]=dict(address=address,implementation=impl,proxy_hash=digest(raw(code)),implementation_hash=digest(raw(bytecode)))
    fees={k:single(guard,k+"()",ret="uint256") for k in ("ecoBps","exoBps")}
    paused=single(guard,"isSellPaused()",ret="bool")
    deadline=int(head["timestamp"],16)+180
    reserves=c.one(POOL,"getReserves()",returns=["uint112","uint112","uint32"])
    result=dict(selector=at,state_root=ROOT_HASH,height=int(head["number"],16),source=SOURCE,roles=roles,tokens=tokens,
                reserves=list(map(str,reserves[:2])),stable_decimals=single(POOL,"stablecoinDecimals()",ret="uint8"),
                fees_bps=fees,sell_paused=paused,cases=[])
    for user in (OWNER,MEMBER):
        for input_token,output_token,unit in [(USDT,DAGR,10**6),(DAGR,USDT,10**18)]:
            path=[input_token,output_token]
            tier=single(guard,"verifySwap(address,bool)",["address","bool"],[user,input_token==DAGR],"uint8")
            for amount in (unit,5*unit):
                quoted=single(POOL,"getAmountsOut(uint256,address[],address)",["uint256","address[]","address"],[amount,path,user],"uint256[]")
                if len(quoted)!=2 or quoted[0]!=amount or not quoted[1]:
                    raise ValueError(("incorrect quote",path,user,amount))
                reverse=single(POOL,"getAmountsIn(uint256,address[],address)",["uint256","address[]","address"],[quoted[1],path,user],"uint256[]")
                record=dict(payer=user,tier=tier,input=input_token,output=output_token,amount=str(amount),quote=str(quoted[1]),inverse=str(reverse[0]))
                result["cases"].append(record)
                if payments:
                    record["settlement"]=settle(endpoint,at,deadline,user,path,amount,quoted[1],reverse[0])
    return result


def settle(endpoint,at,deadline,user,path,amount,quote,inverse):
    # Fund without changing the tested pool, for each direction and size on the original state.
    a,b=path
    funds=max(amount,inverse)
    prefix=[]
    if a==USDT:
        prefix.append(sent(user,FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                           ["uint256","address[]","address","uint256"],[funds,[WETH,USDT],user,deadline],10**17))
    elif user!=MEMBER:
        prefix.append(sent(MEMBER,DAGR,"transfer(address,uint256)",["address","uint256"],[user,funds]))
    prefix.extend([sent(user,a,"approve(address,uint256)",["address","uint256"],[POOL,n]) for n in (0,funds)])
    balances=[sent(user,t,"balanceOf(address)",["address"],[user]) for t in (a,b)]
    overrides={who:{"balance":hex(10**19)} for who in (user,MEMBER)}
    def run(exact_in, limit, end, expected):
        method="swapExactTokensForTokens" if exact_in else "swapTokensForExactTokens"
        params=[amount if exact_in else quote,limit,path,user,end]
        op=sent(user,POOL,method+"(uint256,uint256,address[],address,uint256)",
                ["uint256","uint256","address[]","address","uint256"],params)
        replies=simulate_block(endpoint,at,overrides,prefix+balances+[op]+balances)
        k=len(prefix)+2
        if any(r["status"]!="0x1" for i,r in enumerate(replies) if i!=k):
            raise ValueError(("funding or balance check failed",path,user))
        swap=replies[k]
        if (swap["status"]=="0x1")!=expected:
            raise ValueError(("unexpected swap status",method,params[:2],str(swap.get("error",""))[:220]))
        if not expected:
            return {key:swap.get(key) for key in ("returnData","error")}
        before=[int(r["returnData"],16) for r in replies[k-2:k]]
        after=[int(r["returnData"],16) for r in replies[k+1:k+3]]
        debit,credit=before[0]-after[0],after[1]-before[1]
        return dict(debit=str(debit),credit=str(credit),gas=int(swap["gasUsed"],16),
                    returned=list(map(str,decode(["uint256[]"],raw(swap["returnData"]))[0])))
    actual=run(True,quote,deadline,True)
    if (int(actual["debit"]),int(actual["credit"]))!=(amount,quote):
        raise ValueError(("physical output differs",path,amount,quote,actual))
    return dict(exact_in=actual,strict=run(True,quote+1,deadline,False),expired=run(True,quote,1,False),
                exact_out=run(False,inverse,deadline,True))


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode",choices=["directory","payments"])
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    for ep in ENDPOINTS:
        try:
            result=capture(ep,args.mode=="payments")
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            print(len(result["cases"]),result["fees_bps"],result["sell_paused"])
            break
        except (requests.RequestException,RuntimeError,ValueError,TypeError,KeyError) as error:
            print(ep,str(error)[:300])
    else:
        raise SystemExit("no confirmed DAGrow snapshot")
