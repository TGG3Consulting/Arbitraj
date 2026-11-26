"""Compare the Oasis pair's spendable reserves, fee caches and both native router entry points."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import OWNER, ROOT, call, calldata, rpc, simulate_block
from record_ethereum_v2_factories import registry

FACTORY="0xc8126578093366968199707b7f8edff258f473b6"
ROUTER="0xb588a34afa5be3f3af703159fb6586a2caf04ba8"
WETH="0x4200000000000000000000000000000000000006"
BASE_CASH=["0x833589fcd6edb6e08f4c7c32d4f71b54bda02913","0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca"]
FUND="0x4752ba5dbc23f44d87826276bf6fd6b1c372ad24"
def capture(c,head):
    def tx(to,sig,types=(),args=(),value=0):return call(to,calldata(sig,types,args),value)
    def run(calls):
        return simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(50*10**18)}},calls)
    def raw(r):return bytes.fromhex(r["returnData"][2:])
    def ok(r):return r["status"]=="0x1"
    def bal(t):return tx(t,"balanceOf(address)",["address"],[OWNER])
    def approve(t,n):return tx(t,"approve(address,uint256)",["address","uint256"],[ROUTER,n])
    src={}
    for addr in (FACTORY,ROUTER):
        url=f"https://sourcify.dev/server/v2/contract/8453/{addr}"
        response=requests.get(url,params={"fields":"compilation,abi,sources,runtimeBytecode.onchainBytecode"},timeout=30)
        response.raise_for_status();doc=response.json();code=rpc(c.endpoint,"eth_getCode",[addr,c.selector])
        if code.lower()!=doc["runtimeBytecode"]["onchainBytecode"].lower():raise ValueError("deployment code differs")
        src[addr]={"url":url,"name":doc["compilation"]["fullyQualifiedName"],"runtime":"0x"+keccak(bytes.fromhex(code[2:])).hex(),"bytes":len(code[2:])//2}
    if c.one(ROUTER,"factory()",returns=["address"])!=(FACTORY,) or c.one(ROUTER,"WETH()",returns=["address"])!=(WETH,):
        raise ValueError("router binding differs")
    init,=c.one(FACTORY,"pairCodeHash()",returns=["bytes32"]);markets=registry(c,FACTORY,"0x"+init.hex())
    reads=[(p["pool"],g,[],[]) for p in markets for g in ("fee()","feeCache0()","feeCache1()","oasisFeeProportion()")]
    responses=iter(posts(c,reads))
    for market in markets:
        fee,cache0,cache1,part=[decode(["uint256"],next(responses))[0] for _ in range(4)]
        market.update(fee=fee,fee_cache=[str(cache0),str(cache1)],protocol_share=part)
    try:
        rebate,=c.one(ROUTER,"getRebate(address)",["address"],[OWNER],["uint64"])
    except ValueError as error:
        rebate={"error":str(error)[:160]}
    deadline=int(head["timestamp"],16)+120
    observations=[]
    for asset in (WETH,*BASE_CASH):
        candidates=[m for m in markets if asset in m["tokens"] and min(map(int,m["reserves"]))>0]
        candidates.sort(key=lambda x:-int(x["reserves"][x["tokens"].index(asset)]))
        for m in candidates:
            ix=m['tokens'].index(asset); token=m['tokens'][1-ix];rin=int(m['reserves'][ix])
            sizes=sorted({min(rin//10000,10**15 if asset==WETH else 10**6),min(rin//500,10**16 if asset==WETH else 10**7)})
            if len(sizes)!=2 or sizes[0]==0:continue
            cases=[]
            for size in sizes:
                funding=tx(WETH,"deposit()",value=size) if asset==WETH else tx(FUND,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[size,[WETH,asset],OWNER,deadline],10**18)
                for method in ("swapExactTokensForTokens","swapExactTokensForTokensSupportingFeeOnTransferTokens"):
                    for use in (False,True):
                        def leg(prefix,a,b,n):
                            prep=prefix+[approve(a,n),bal(a),bal(b)]
                            quote=tx(ROUTER,"getAmountsOut(uint256,address[],bool,address)",["uint256","address[]","bool","address"],[n,[a,b],use,OWNER])
                            req=run(prep+[quote]);bad=next((i for i,r in enumerate(req) if not ok(r)),None)
                            if bad is not None:return {"failed":"setup" if bad<len(prep) else "quote","step":bad,"error":req[bad].get("error")},None
                            (amts,)=decode(["uint256[]"],raw(req[-1]))
                            if len(amts)!=2 or amts[0]!=n:raise ValueError("invalid quote")
                            def swap(minimum):
                                return tx(ROUTER,method+"(uint256,uint256,address[],address,uint256,bool)",["uint256","uint256","address[]","address","uint256","bool"],[n,minimum,[a,b],OWNER,deadline,use])
                            q=amts[1];order=swap(q);replies=run(prep+[swap(q+1),order,bal(a),bal(b)])
                            k=len(prep); strict,fill=replies[k:k+2]
                            condition={"request":str(n),"quoted":str(q),"strict_rejected":not ok(strict)}
                            if ok(strict):
                                # A successful negative control changes the overlay; evaluate the fill from the original state.
                                replies=run(prep+[order,bal(a),bal(b)]);fill=replies[k]
                            if not ok(fill):return {**condition,"failed":"swap","error":fill.get("error")},None
                            if not all(map(ok,replies[-2:])):raise ValueError("balances unavailable")
                            num=lambda r:decode(["uint256"],raw(r))[0]
                            spent=num(req[-3])-num(replies[-2]);received=num(replies[-1])-num(req[-2])
                            result=dict(condition,spent=str(spent),received=str(received),gas=int(fill["gasUsed"],16))
                            if spent!=n or received<=0:return {**result,"failed":"wallet"},None
                            return result,prep+[order]
                        buy,prefix=leg([funding],asset,token,size)
                        case=dict(size=str(size),method=method,use_rebate=use,buy=buy)
                        if prefix:case['sell'],_=leg(prefix,token,asset,int(buy["received"]))
                        cases.append(case)
            observations.append(dict(pool=m["pool"],settlement=asset,cases=cases))
            if all(any(q.get('sell',{}).get('received') and all(q[k]['received']==q[k]['quoted'] and q[k]['strict_rejected'] for k in ('buy','sell'))
                         for q in cases if q['size']==str(n)) for n in sizes):
                return dict(origin=src,init="0x"+init.hex(),payer=OWNER,rebate=rebate,pools=markets,samples=observations)
    return dict(origin=src,init="0x"+init.hex(),payer=OWNER,rebate=rebate,pools=markets,samples=observations)

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);args=parser.parse_args()
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    for endpoint in ("https://mainnet.base.org","https://base.gateway.tenderly.co"):
        try:
            c=Capture(dict(snap,endpoint=endpoint));head=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
            if head["hash"]!=c.selector["blockHash"] or int(head["number"],16)!=snap["block"] or rpc(endpoint,"eth_chainId",[])!="0x2105":raise ValueError("wrong Base archive")
            result=capture(c,head);result.update(selector=c.selector,state_root=head["stateRoot"],height=snap["block"])
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8");break
        except (requests.RequestException,ValueError,KeyError) as error: print(endpoint,str(error)[:200],flush=True)
    else:raise ValueError("no canonical archive supplied OasisSwap inputs")
