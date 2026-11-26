"""Compare protected module execution against public bonding or native Elastic with one ordinary funded signer."""
import argparse
import json
from pathlib import Path
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC,calldata,rpc
from record_virtuals_base import BASE,ENDPOINTS,capture
from record_horizondex_base import FACTORY,POOL,QUOTER,ROUTER,ORDER,QUERY,ROOT
from record_solidly_v3 import BASE_WETH,HOLDERS

WALLET="0x5bea9a35022b9d5a80214501c3a472cadee8b6df"
USDBC="0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca"

def settlement(endpoint,id,artifact):
    src=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    tag=src["selector"];head=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    deadline=int(head["timestamp"],16)+180
    elastic=id=="horizon"
    evidence=None if elastic else capture(id,endpoint)
    if elastic:
        owner=HOLDERS["base"][BASE_WETH];asset,token=BASE_WETH,USDBC;pool,router,entry=POOL,ROUTER,ROUTER
        sizes=[10**12,10**13]
    else:
        owner=WALLET;asset=BASE;sample=evidence["samples"][0];token,pool=sample["token"],sample["pool"]
        router=evidence["roles"]["router"]["address"];entry=next(r["entry"] for r in sample["status"] if r["trading"] and r["pair"]==pool and not r["graduated"])
        sizes=[10**16,10**17]
    def tx(to,signature,types=(),values=()):
        return {"from":owner,"to":to,"data":calldata(signature,types,values),"gas":"0x500000"}
    bal=lambda t,a:tx(t,"balanceOf(address)",["address"],[a])
    allowance=lambda t,a,b:tx(t,"allowance(address,address)",["address","address"],[a,b])
    def run(items,module):
        frame={"calls":[call for _,call in items]}
        if module:frame["stateOverrides"]={EXEC:{"code":"0x"+artifact["runtime"].removeprefix("0x")}}
        r,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},tag])
        if r["parentHash"]!=tag["blockHash"]:raise ValueError("changed canonical parent")
        return dict(zip([k for k,_ in items],r["calls"],strict=True))
    def words(raw,types):
        if raw["status"]!="0x1":raise ValueError(str(raw.get("error"))[:140])
        return decode(types,bytes.fromhex(raw["returnData"][2:]))
    def leg(prefix,side,amount,module):
        buy=side=="buy";x,y=(asset,token) if buy else (token,asset)
        spender=EXEC if module else router
        def order(minimum,expires):
            if module:return tx(EXEC,"execute(address,address,address,uint256,uint256,uint256)",["address"]*3+["uint256"]*3,[pool,x,y,amount,minimum,expires])
            if elastic:return tx(router,"swapExactInputSingle("+ORDER+")",[ORDER],[(x,y,40,owner,expires,amount,minimum,0)])
            return tx(entry,side+"(uint256,address,uint256,uint256)",["uint256","address","uint256","uint256"],[amount,token,minimum,expires])
        reads=[("before_in",bal(x,owner)),("before_out",bal(y,owner)),("backing",bal(y,pool))]
        if elastic:reads.append(("quote",tx(QUOTER,"quoteExactInputSingle("+QUERY+")",[QUERY],[(x,y,amount,40,0)])))
        else:
            reads.extend([("reserves",tx(pool,"getReserves()")),("invariant",tx(pool,"kLast()"))])
            if id=="virtuals-unicorn-base" and buy:reads.append(("anti",tx(router,"hasAntiSniperTax(address)",["address"],[pool])))
        prepare=prefix+[(side+k,v) for k,v in reads]+[(side+"allow",tx(x,"approve(address,uint256)",["address","uint256"],[spender,amount]))]
        pre=run(prepare,module);num=lambda k:words(pre[side+k],["uint256"])[0]
        if elastic:
            used,gross,*_=words(pre[side+"quote"],["uint256","uint256","uint160","uint32","uint256"])
            if used!=amount:raise ValueError("partial Elastic quote")
            net=gross
        else:
            if side+"anti" in pre and num("anti"):raise ValueError("sample has active launch tax")
            a,b=words(pre[side+"reserves"],["uint256","uint256"]);k=num("invariant")
            bt,st=evidence["fees"]
            gross=a-k//(b+amount-amount*bt//100) if buy else b-k//(a+amount)
            net=gross if buy else gross-gross*st//100
        floor=net if module else gross
        effects=[("strict",order(floor+1,deadline)),("expired",order(floor,0)),("exact",order(floor,deadline)),
                 ("after_in",bal(x,owner)),("after_out",bal(y,owner)),("wallet_allow",allowance(x,owner,spender))]
        if module:effects.extend([("held_in",bal(x,EXEC)),("held_out",bal(y,EXEC)),("module_allow",allowance(x,EXEC,router))])
        calls=prepare+[(side+k,v) for k,v in effects];result=run(calls,module)
        for label,r in result.items():
            if r["status"]!=("0x0" if label.endswith(("strict","expired")) else "0x1"):raise ValueError((id,side,label,r.get("error")))
        n=lambda k:words(result[side+k],["uint256"])[0]
        if (n("before_in")-n("after_in"),n("after_out")-n("before_out"))!=(amount,net):raise ValueError("owner settlement differs from source math")
        if module and (n("exact")!=net or any(n(k) for k in ["held_in","held_out","module_allow","wallet_allow"])):raise ValueError("module residue, allowance or return differs")
        return calls,net,dict(amount=str(amount),gross=str(gross),net=str(net),calls=[tx for _,tx in calls],results=result)
    cases=[]
    for amount in sizes:
        case={}
        for key,module in [("native",False),("module",True)]:
            sequence,output,buy=leg([],"buy",amount,module);_,_,sell=leg(sequence,"sell",output,module)
            case[key]={"buy":buy,"sell":sell}
        cases.append(case)
    return dict(id=id,selector=tag,owner=owner,market=pool,router=router,entry=entry,tokens=[asset,token],registry=evidence,
                runtime="0x"+keccak(hexstr=artifact["runtime"]).hex(),cases=cases)
if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("id",choices=["virtuals-base","virtuals-unicorn-base","horizon"]);parser.add_argument("artifact",type=Path);parser.add_argument("output",type=Path);args=parser.parse_args()
    for ep in ENDPOINTS:
        try:
            record=settlement(ep,args.id,json.loads(args.artifact.read_bytes()));args.output.write_text(json.dumps(record,indent=2)+"\n",encoding="utf8",newline="\n");break
        except (ValueError,OSError,KeyError,StopIteration) as exc:print(ep,str(exc)[:250],flush=True)
    else:raise ValueError("no complete protected wallet evidence for "+args.id)
