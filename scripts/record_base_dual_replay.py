"""Сопоставляет getAmountOut, вход, чистый выход и min+1 на обычном V5 Base для двух форм кривой."""
import argparse
import json
from pathlib import Path
from eth_utils import keccak
from record_base_slipstream_executor import OWNER, EXEC, STEP, RUNTIME, call, calldata, rpc
from record_base_v2_factories import WETH, USDC, ROUTER

MARKETS = {
    "citadelswap": ["0x9dcff9c872bbf746633ce6d1c7a511a92bf75d39","0x3fe49194d3cf9a7005203468073a32d764fb86b6","0x7ae7434fe356ca9e60e973bd59974078fe8949bc"],
    "kim-v2-base":["0x4ca28220e0caea31bed133b3f22e1317dfeb79ab"],
    "leetswap-base":["0xde5a9e102ae116d3c392298b8d54f4e57db9b138","0x121fee573b1cee7a7b553c56c7c3446e57606014","0x83097b68027239c89576825bbb808ac405bf1c1e",
                     "0x74b0f4aef2ff55e5f0e229525530170ed930725c","0xef849707a01fff93a7b4b398307b8cd839f6a57b",
                     "0xc6ac0110084f6ca058bfaa741aa68be5189a8ca4","0x94dac4a3ce998143aa119c05460731da80ad90cf",
                     "0x75c7928b43bfec6f960456af8a6d6bd47ac77930","0xe96df8f5ef1a8790415068c798765b07d57643bd"],
}


def replay(name, book, endpoint, market, amount):
    pool=market["pool"]
    inp=WETH if WETH in market["tokens"] else USDC
    out=next(t for t in market["tokens"] if t!=inp)
    ts=book["timestamp"]+2
    deadline=ts+120
    word=lambda n:"0x"+format(n,"064x")
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    env={OWNER:{"balance":hex(10**20)},EXEC:{"code":"0x"+runtime,
         "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    if inp==WETH:
        fund=call(WETH,calldata("deposit()"),amount)
    else:
        fund=call(ROUTER,calldata("swapETHForExactTokens(uint256,address[],address,uint256)",
                  ["uint256","address[]","address","uint256"],[amount,[WETH,inp],OWNER,deadline]),10**18)
    def sim(calls):
        b={"blockOverrides":{"time":hex(ts)},"stateOverrides":env,"calls":calls}
        response,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[b]},book["selector"]])
        if response["parentHash"]!=book["selector"]["blockHash"] or int(response["timestamp"],16)!=ts:
            raise ValueError("unexpected simulation parent")
        return response["calls"]
    def swap(tin,tout,amount,floor):
        return call(EXEC,calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                    ["address","uint256","address","uint256","uint256",STEP+"[]"],
                    [tin,amount,tout,floor,deadline,[(3,pool,tin,tout,10000,b"")]]))
    def balance(t,owner):return call(t,calldata("balanceOf(address)",["address"],[owner]))
    def leg(before,tin,tout,amt):
        approve=call(tin,calldata("approve(address,uint256)",["address","uint256"],[EXEC,amt]))
        native=call(pool,calldata("getAmountOut(uint256,address)",["uint256","address"],[amt,tin]))
        native["from"]=EXEC
        if name=="leetswap-base":
            recipient=call(pool,calldata("getAmountOut(uint256,address,address)",["uint256","address","address"],[amt,tin,OWNER]))
        else:recipient=native
        inspect=[call(pool,calldata("getReserves()"))]
        if name!="leetswap-base":
            inspect.extend(call(pool,calldata(s)) for s in ("precisionMultiplier0()","precisionMultiplier1()"))
        if name=="leetswap-base":
            inspect.extend(call(book["factory"],calldata("tradingFees(address,address)",["address","address"],[pool,payer]))
                           for payer in (EXEC,OWNER))
        pre=[*before,*inspect,approve,native,recipient,balance(tin,OWNER),balance(tout,OWNER)]
        post=[balance(tin,OWNER),balance(tout,OWNER),balance(tin,EXEC),balance(tout,EXEC)]
        def trial(minimum):
            calls=sim([*pre,swap(tin,tout,amt,minimum),*post])
            trade=calls[len(pre)]
            if any(int(c["status"],16)!=1 for c in [*calls[:len(pre)],*calls[len(pre)+1:]]):
                raise ValueError("preparation or balance query reverted")
            money=lambda x:int(x["returnData"],16)
            if any(money(x)!=0 for x in calls[-2:]):raise ValueError("executor token residue")
            raw=calls[len(before)]["returnData"].removeprefix("0x")
            reserves=[str(int(raw[i:i+64],16)) for i in range(0,len(raw),64)]
            units=[str(money(c)) for c in calls[len(before)+1:len(before)+len(inspect)]]
            evidence={"fees_by_recipient":units} if name=="leetswap-base" else {"units":units}
            return dict(reserves=reserves,**evidence,status=trade["status"],gas=trade["gasUsed"],error=trade.get("error"),
                        spent=str(money(calls[len(pre)-2])-money(calls[-4])),
                        received=str(money(calls[-3])-money(calls[len(pre)-1])),
                        caller_quote=str(money(calls[len(pre)-4])),recipient_quote=str(money(calls[len(pre)-3])))
        probe=trial(1)
        if probe["status"]!="0x1": raise ValueError("V5 reverted "+str(probe["error"])[:130])
        actual=int(probe["received"])
        exact=trial(actual);high=trial(actual+1)
        if (exact["status"],exact["spent"],exact["received"],high["status"],high["spent"],high["received"])!=("0x1",str(amt),str(actual),"0x0","0","0"):
            raise ValueError(f"{tin} amount {amt}: exact {exact['spent']}/{exact['received']}, min+1 {high['status']} {high['spent']}/{high['received']}")
        return dict(input=tin,output=tout,amount=str(amt),exact=exact,plus_one=high),[*before,approve,swap(tin,tout,amt,actual)]
    buy,prefix=leg([fund],inp,out,amount)
    sell,_=leg(prefix,out,inp,int(buy["exact"]["received"]))
    return dict(buy=buy,sell=sell)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("name",choices=MARKETS);ap.add_argument("directory",type=Path);ap.add_argument("output",type=Path)
    ap.add_argument("--endpoint",default="https://mainnet.base.org")
    args=ap.parse_args()
    book=json.loads(args.directory.read_bytes())
    if int(rpc(args.endpoint,"eth_chainId",[]),16)!=8453:raise ValueError("incorrect network")
    results=[]
    for p in MARKETS[args.name]:
        m=next(m for m in book["markets"] if m["pool"]==p)
        token=WETH if WETH in m["tokens"] else USDC
        r=int(m["reserves"][m["tokens"].index(token)])
        caps=[10**15,10**16] if token==WETH else [10**6,10**7]
        fractions=[100,20] if args.name=="leetswap-base" else [10_000,500]
        for qty in sorted(set(min(l,r//d) for l,d in zip(caps,fractions))):
            if not qty:continue
            row=dict(market=m,requested=str(qty))
            try:row["result"]=replay(args.name,book,args.endpoint,m,qty)
            except ValueError as e:row["rejection"]=str(e)[:180]
            results.append(row)
    result=dict(protocol=args.name,chain=8453,selector=book["selector"],endpoint=args.endpoint,executor_runtime="0x"+keccak(bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))).hex(),cases=results)
    args.output.write_text(json.dumps(result,indent=2)+chr(10),encoding="utf8")
