"""Обычный держатель LB через штатный роутер и V5 на канонической высоте. Только симуляции."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from record_base_slipstream_executor import EXEC, RUNTIME, STEP, calldata, rpc
from record_sectorone_directory import BASE_ENDPOINTS, ETH_ENDPOINTS
from record_solidly_v3 import HOLDERS, BASE_WETH, ETH_WETH


def settle(d, endpoint, row, first, payer, sizes):
    legacy = d["version"] == "2.0"
    selector, router, pool = d["selector"], d["router"], row["pool"]
    head = rpc(endpoint,"eth_getBlockByHash",[selector["blockHash"],False])
    deadline = int(head["timestamp"],16)+180
    weth = BASE_WETH if d["chain"]=="base" else ETH_WETH
    for address in (EXEC,payer):
        if rpc(endpoint,"eth_getCode",[address,selector])!="0x": raise ValueError("контрольный адрес имеет код")

    def tx(to,sig,types,args):
        return {"from":payer,"to":to,"data":calldata(sig,types,args),"gas":"0x900000"}

    def run(calls,mode):
        frame={"calls":calls}
        if mode=="v5":
            word=lambda n:"0x"+format(n,"064x")
            frame["stateOverrides"]={EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                                           "stateDiff":{word(0):word(int(payer,16)),word(1):word(int(weth,16))}}}
        block,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},selector])
        if block["parentHash"]!=selector["blockHash"]:raise ValueError("смена родительского блока")
        return block["calls"]

    def leg(prefix,mode,x,y,amount):
        for_y = x==row["tokens"][0]
        spender=EXEC if mode=="v5" else router
        approval=tx(x,"approve(address,uint256)",["address","uint256"],[spender,amount])
        balances=[tx(t,"balanceOf(address)",["address"],[payer]) for t in (x,y)]
        qt="uint256" if legacy else "uint128"
        quote=tx(router,f"getSwapOut(address,{qt},bool)",["address",qt,"bool"],[pool,amount,for_y])
        def order(minimum,expiry):
            if mode=="v5":
                return tx(EXEC,f"execute(address,uint256,address,uint256,uint256,{STEP}[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],
                          [x,amount,y,minimum,expiry,[(24 if legacy else 18,pool,x,y,10000,b"")]])
            path_types=["uint256[]","address[]"] if legacy else ["(uint256[],uint8[],address[])"]
            path=[[row["step"]],[x,y]] if legacy else [([row["step"]],[2],[x,y])]
            types=["uint256","uint256",*path_types,"address","uint256"]
            return tx(router,"swapExactTokensForTokensSupportingFeeOnTransferTokens("+",".join(types)+")",types,
                      [amount,minimum,*path,payer,expiry])
        pre=prefix+[approval,*balances,quote]
        front=run(pre,mode)
        if any(v["status"]!="0x1" for v in front):
            raise ValueError("отказ исходной котировки или подготовительного состояния")
        raw=bytes.fromhex(front[-1]["returnData"][2:])
        if legacy:
            out,fee=decode(["uint256","uint256"],raw)
        else:
            left,out,fee=decode(["uint128"]*3,raw)
            if left:raise ValueError("частичный вход")
        if not out:raise ValueError("выход округлился в ноль")
        calls=pre+[order(out+1,deadline),order(out,0),order(out,deadline),*balances]
        results=run(calls,mode)
        if [r["status"] for r in results[-5:]]!=["0x0","0x0","0x1","0x1","0x1"]:
            raise ValueError("строгий минимум, срок или выплата расходятся с исходным расчётом")
        value=lambda r:int(r["returnData"],16)
        if value(front[-3])-value(results[-2])!=amount or value(results[-1])-value(front[-2])!=out:
            raise ValueError("доход или расход расходится с фактическим балансом")
        return prefix+[approval,order(out,deadline)],out,dict(input=str(amount),net=str(out),fee=str(fee),
                                                              gas=int(results[-3]["gasUsed"],16),calls=calls,results=results)
    other=next(t for t in row["tokens"] if t!=first)
    cases=[]
    for mode in ("router","v5"):
        for amt in sizes:
            seq,bought,buy=leg([],mode,first,other,amt)
            case = dict(mode=mode,buy=buy)
            try:
                _,_,case["sell"]=leg(seq,mode,other,first,bought)
            except ValueError as exc:
                case["reverse_rejected"]=str(exc)[:220]
            cases.append(case)
    return dict(first_token=first,owner=payer,pool=pool,step=row["step"],cases=cases)


def capture(d,endpoint):
    targets=[]
    holders=HOLDERS[d["chain"]]
    for m in d["markets"]:
        if m.get("hooks","0x"+"00"*32)!="0x"+"00"*32 or m.get("implementation",d["implementation"])!=d["implementation"]:
            continue
        for i,first in enumerate(m["tokens"]):
            reserves=list(map(int,m["reserves"]))
            if first not in holders or not reserves[1-i]:continue
            if first in (BASE_WETH,ETH_WETH):
                max_size=10**14
            else:
                max_size=max(100, reserves[i]//100000)
            size=min(max_size,reserves[i]//1000) if reserves[i] else (10**10 if first in (BASE_WETH,ETH_WETH) else 100)
            if size>=8:targets.append((sum(t in holders for t in m["tokens"]),reserves[1-i].bit_length(),size,m,first))
    failures=[]
    one_side=None
    for _,_,qty,m,first in sorted(targets,key=lambda x:(-x[0],-x[1],-x[2])):
        try:
            pay=settle(d,endpoint,m,first,holders[first],[qty//4,qty])
        except (KeyError,ValueError) as exc:
            failures.append(dict(pool=m["pool"],first=first,error=str(exc)[:220]))
            continue
        if all("sell" in c for c in pay["cases"]):
            return dict(directory=d,settlement=pay,rejected_examples=failures)
        if one_side is None:
            one_side=pay
    if one_side is not None:
        return dict(directory=d,settlement=one_side,rejected_examples=failures)
    if not targets:
        return dict(directory=d,settlement=None,unresolved="нет подтверждённого входного актива кошелька при положительном выходном резерве")
    raise ValueError(str(failures[:4])[:800])


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("directory",type=Path);ap.add_argument("output",type=Path);args=ap.parse_args()
    directory=json.loads(args.directory.read_bytes())
    for ep in (BASE_ENDPOINTS if directory["chain"]=="base" else ETH_ENDPOINTS):
        try:
            replay=capture(directory,ep)
            args.output.write_text(json.dumps(replay,indent=2)+"\n",encoding="utf8")
            break
        except (OSError,KeyError,ValueError) as exc:
            print(ep,str(exc)[:600],flush=True)
    else:raise ValueError("контрольная выплата SectorOne недоступна")
