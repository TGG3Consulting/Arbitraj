"""Compare the Sashimi router ledger with actual caller payments, including refunds on failure."""
from eth_abi import decode
from record_aero_basic_base import HOLDERS
from record_base_slipstream_executor import calldata, rpc
from record_sashimi_ethereum import ROUTER,WETH

def wallets(c,head,d):
    by_token=HOLDERS["ethereum"]
    end=int(head["timestamp"],16)+120
    def run(calls):
        b,=rpc(c.endpoint,"eth_simulateV1",[dict(blockStateCalls=[dict(calls=calls)],validation=False),c.selector])
        if b["parentHash"]!=c.selector["blockHash"] or len(b["calls"])!=len(calls):
            raise ValueError("wrong block or short response")
        return b["calls"]
    def leg(owner,x,y,amount,prefix):
        def tx(to,sig,types,values):
            return {"from":owner,"to":to,"data":calldata(sig,types,values),"gas":"0xa00000"}
        def bal(token):
            return tx(token,"balanceOf(address)",["address"],[owner])
        def swap(limit,deadline):
            return tx(ROUTER,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                      ["uint256","uint256","address[]","address","uint256"],[amount,limit,[x,y],owner,deadline])
        permission=tx(x,"allowance(address,address)",["address","address"],[owner,ROUTER])
        q=tx(ROUTER,"getAmountsOut(uint256,address[])",["uint256","address[]"],[amount,[x,y]])
        data=run(prefix+[permission,q])
        if any(z["status"]!="0x1" for z in data):
            return None,dict(stage="allowance_or_quote",result=data[-2:])
        nominal=decode(["uint256[]"],bytes.fromhex(data[-1]["returnData"][2:]))[0][-1]
        allowance=int(data[-2]["returnData"],16)
        def approve(n):
            return tx(x,"approve(address,uint256)",["address","uint256"],[ROUTER,n])
        prep=list(prefix)
        if allowance<amount:
            if allowance:prep.append(approve(0))
            prep.append(approve(amount))
        witness=run(prep+[bal(x),bal(y),swap(1,end),bal(x),bal(y)])
        if any(z["status"]!="0x1" for z in witness):
            idx=next(i for i,z in enumerate(witness) if z["status"]!="0x1")
            return None,dict(stage="approval" if idx<len(prep) else "settlement",nominal=str(nominal),index=idx,
                             revert=witness[idx].get("returnData"),error=witness[idx].get("error"))
        net=int(witness[-1]["returnData"],16)-int(witness[-4]["returnData"],16)
        debit=int(witness[-5]["returnData"],16)-int(witness[-2]["returnData"],16)
        if debit!=amount or net<1:
            raise ValueError("unexpected payer transfer")
        paid=swap(net,end)
        tail=[bal(x),bal(y),swap(net+1,end),swap(net,1),bal(x),bal(y),paid,bal(x),bal(y)]
        r=run(prep+tail)[-len(tail):]
        if ([z["status"] for z in r]!=["0x1","0x1","0x0","0x0","0x1","0x1","0x1","0x1","0x1"]
                or any(r[i]["returnData"]!=r[4+i]["returnData"] for i in (0,1))
                or int(r[0]["returnData"],16)-int(r[7]["returnData"],16)!=amount
                or int(r[8]["returnData"],16)-int(r[1]["returnData"],16)!=net):
            raise ValueError("router failed a payment or rollback invariant")
        return prep+[paid],dict(stage="paid",debit=str(amount),credit=str(net),nominal=str(nominal),gas=int(r[6]["gasUsed"],16),tail=tail,results=r)
    out=[]
    for m in d["pools"]:
        for i,token in enumerate(m["tokens"]):
            if token not in by_token or not all(map(int,m["reserves"])):continue
            owner=by_token[token]
            if rpc(c.endpoint,"eth_getCode",[owner,c.selector])!="0x":
                raise ValueError("payer is not an independent EOA")
            other=m["tokens"][1-i]
            reserve=int(m["reserves"][i])
            scale,=c.one(token,"decimals()",returns=["uint8"])
            for amount in sorted({min(reserve//5000,10**scale//(1000 if token==WETH else 1)),min(reserve//100,10**scale*(1 if token==WETH else 100))}-{0}):
                prefix,buy=leg(owner,token,other,amount,[])
                sell=leg(owner,other,token,int(buy["credit"]),prefix)[1] if prefix else None
                out.append(dict(pool=m["pool"],payer=owner,input=token,amount=str(amount),buy=buy,sell=sell))
    if not out:raise ValueError("no funded market was tested")
    return out
