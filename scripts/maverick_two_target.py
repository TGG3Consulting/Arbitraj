"""Проверка outputSingleWithTickLimit по реальному расходу и выплате в одной канонической симуляции."""
from eth_abi import decode
from record_base_slipstream_executor import OWNER, calldata, simulate_block
from record_ethervista_wallets import tx


def targets(c, roles, pool, token_a, cases, deadline):
    router, wrapped=roles["router"]["address"],roles["router"]["wrapped"]
    env={OWNER:{"balance":hex(10**19)}}
    def request(prefix):
        return simulate_block(c.endpoint,c.selector,env,prefix)
    def action(src,out,cap,expiry):
        steps=[calldata("checkDeadline(uint256)",["uint256"],[expiry]),
               calldata("outputSingleWithTickLimit(address,address,bool,uint256,int32,uint256,uint256)",
                        ["address","address","bool","uint256","int32","uint256","uint256"],
                        [OWNER,pool,src==token_a,out,2**31-1 if src==token_a else -2**31,cap,out])]
        return tx(router,"multicall(bytes[])",["bytes[]"],[[bytes.fromhex(s[2:]) for s in steps]])
    results=[]
    for original in cases:
        if original["mode"]!="router":continue
        size=int(original["size"])
        prefix=[tx(wrapped,"deposit()",value=size*2)]
        legs={}
        for side in ["buy","sell"]:
            leg=original[side]
            src,dst,out=leg["input"],leg["output"],int(leg["exact"]["received"])
            # One atom of reverse wallet currency may be unavailable after a different rounded buy.
            # Target half the observed sell amount; keep the exact received funding in this same block.
            if side=="sell":out//=2
            cap=size*2 if side=="buy" else int(original["buy"]["exact"]["received"])
            approve=tx(src,"approve(address,uint256)",["address","uint256"],[router,cap])
            reads=[tx(t,"balanceOf(address)",["address"],[OWNER]) for t in [src,dst]]
            prep=[*prefix,approve]
            selected=action(src,out,cap,deadline)
            r=request([*prep,*reads,selected,*reads])
            if any(x["status"]!="0x1" for x in r):raise ValueError("exact target unavailable")
            before_in,before_out,_,after_in,after_out=r[-5:]
            value=lambda x:int(x["returnData"],16)
            spent=value(before_in)-value(after_in);net=value(after_out)-value(before_out)
            if not (0<spent<=cap and net==out):raise ValueError("actual exact-target wallet payment differs")
            narrow=action(src,out,spent-1,deadline);late=action(src,out,cap,0)
            exact=action(src,out,spent,deadline)
            strict=request([*prep,*reads,narrow,late,*reads,exact,*reads])[-9:]
            if ([i["status"] for i in strict]!=["0x1","0x1","0x0","0x0","0x1","0x1","0x1","0x1","0x1"] or
                value(strict[0])!=value(strict[4]) or value(strict[1])!=value(strict[5]) or
                value(strict[4])-value(strict[7])!=spent or value(strict[8])-value(strict[5])!=out):
                raise ValueError("wrong exact target cap or rollback")
            returned,=decode(["bytes[]"],bytes.fromhex(strict[6]["returnData"][2:]))
            a,b=decode(["uint256","uint256"],returned[-1])
            if (a,b)!=(spent,net):raise ValueError("router returned different debit or payout")
            legs[side]=dict(input=src,output=dst,requested=str(out),maximum=str(cap),paid=str(spent),received=str(net),
                            payer=OWNER,calldata=exact["data"],gas=strict[6]["gasUsed"],checks=strict)
            prefix=[*prep,exact]
        results.append(dict(size=str(size),legs=legs))
    return results
