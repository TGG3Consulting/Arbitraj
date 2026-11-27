"""Measure single-pair Antfarm swaps on Ethereum with an independently funded owner."""
import argparse
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
from eth_abi import decode

from record_antfarm_ethereum import ATF, ROUTER
from record_base_slipstream_executor import OWNER, rpc, simulate_block
from record_bitswap_ethereum import balance, tx
from record_carbon_ethereum import ENDPOINTS, FUNDING, STABLES, WETH

DAI = "0x6b175474e89094c44da98b954eedeac495271d0f"
PARAM = "(uint256,uint256,uint256,address[],uint16[],address,uint256)"
MAX = 2**256-1


def approval(token, limit):
    return tx(token, "approve(address,uint256)", ["address","uint256"], [ROUTER,limit])


def order(x, y, qty, fee, minimum, ceiling, end, supporting=False):
    name = "swapExactTokensForTokens" + ("SupportingFeeOnTransferTokens" if supporting else "")
    return tx(ROUTER, name+"("+PARAM+")", [PARAM], [(qty,minimum,ceiling,[x,y],[fee],OWNER,end)])


def number(result):
    return int(result["returnData"],16)


def evidence(result):
    return {k:result[k] for k in ("status","gasUsed","returnData","error") if k in result}


def observe(url, d, row):
    idx = row["tokens"]
    x = next((t for t in (WETH,*STABLES,DAI,ATF) if t in idx), None)
    if not x:
        return dict(pool=row["pair"],reason="no independent funding asset in registry pair")
    y = idx[1-idx.index(x)]
    fee = row["fee"]
    end = d["block_time"]+300
    seed_tier = 250 if set(idx)=={ATF,WETH} and fee==100 else 100
    prep = [tx(WETH,"deposit()",value=10**16),approval(WETH,10**16),
            order(WETH,ATF,10**15,seed_tier,1,MAX,end),approval(ATF,MAX)]
    maximum = 10**14 if x==WETH else 10**6 if x in STABLES else 10**18
    if x not in (WETH,ATF):
        prep += [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                    ["uint256","address[]","address","uint256"],[maximum,[WETH,x],OWNER,end],value=10**16),
                 approval(x,maximum)]
    prep.append(approval(y,MAX))
    def run(calls):
        return simulate_block(url,d["selector"],{OWNER:{"balance":hex(10**18)}},calls)

    def leg(prefix, a, b, budget, supporting):
        q = ((budget+1)*1000-1)//(1000+fee) if a==ATF and not supporting else budget
        gross = q*(1000+fee)//1000 if a==ATF and not supporting else q
        if not q:
            return [],dict(rejected="input below router rounding")
        assets = list(dict.fromkeys([a,b,ATF]))
        balances = [balance(t,OWNER) for t in assets]
        header = prefix+balances
        loose = order(a,b,q,fee,1,MAX,end,supporting)
        paid = run(header+[loose]+balances)
        if any(r["status"]!="0x1" for r in paid[:len(header)]):
            raise ValueError(("funding or snapshot failed",row["pair"],[(i,evidence(r)) for i,r in enumerate(paid[:len(header)]) if r["status"]!="0x1"]))
        start = [number(v) for v in paid[len(prefix):len(header)]]
        finish = [number(v) for v in paid[-len(assets):]]
        change = dict(zip(assets,[v-u for u,v in zip(start,finish,strict=True)],strict=True))
        action = paid[len(header)]
        out = change[b]
        result = dict(source=a,target=b,budget=str(budget),parameter=str(q),supporting=supporting,transaction=loose,
                      measured={t:str(v) for t,v in change.items()},receipt=evidence(action))
        if action["status"]!="0x1":
            if any(change.values()):raise ValueError("failed Antfarm swap moved balances")
            return [],result
        if out<=0 or -change[a]!=gross:
            raise ValueError(("unexpected owner receipt",row["pair"],change,gross))
        nominal = out if supporting else decode(["uint256[]"],bytes.fromhex(action["returnData"][2:]))[0][-1]
        if a==ATF:
            # Supporting mode takes its fee ceiling from physical pair input, which may include dividends.
            pool_bal = run(prefix+[balance(a,row["pair"]),tx(row["pair"],"getReserves()")]) if supporting else None
            effective = gross if not supporting else gross + number(pool_bal[-2])-decode(["uint112","uint112","uint32"],bytes.fromhex(pool_bal[-1]["returnData"][2:]))[0]
            ceiling = effective*fee//(1000+fee)
        elif b==ATF:
            if supporting:
                pool_bal=run(prefix+[balance(a,row["pair"]),tx(row["pair"],"getReserves()")])
                reserves=decode(["uint112","uint112","uint32"],bytes.fromhex(pool_bal[-1]["returnData"][2:]))
                ceiling=(gross+number(pool_bal[-2])-reserves[idx.index(a)])*fee//1000
            else:
                ceiling=nominal*fee//(1000-fee)
        else:
            ceiling=-change[ATF]
        if ceiling<=0:raise ValueError("no bounded fee for successful swap")
        protected=order(a,b,q,fee,out,ceiling,end,supporting)
        fail=[order(a,b,q,fee,nominal+1,ceiling,end,supporting),order(a,b,q,fee,out,ceiling,0,supporting),
              order(a,b,q,fee,out,ceiling-1,end,supporting)]
        seq=header+fail+balances+[protected]+balances
        results=run(seq)
        if [r["status"] for r in results[len(header):len(header)+3]]!=["0x0"]*3:
            raise ValueError(("minimum, deadline or fee ceiling",row["pair"],ceiling))
        mid=len(header)+len(fail)
        if [number(v) for v in results[mid:mid+len(assets)]]!=start or results[-len(assets)-1]["status"]!="0x1" or [number(v) for v in results[-len(assets):]]!=finish:
            raise ValueError("control changed owner balances or protected swap differed")
        if a!=ATF and b!=ATF:
            denied=run(prefix+[approval(ATF,0),protected])
            if denied[-1]["status"]!="0x0":raise ValueError("separate ATF approval not required")
            result["missing_fee_approval"]=evidence(denied[-1])
        result.update(output=str(out),nominal=str(nominal),fee_ceiling=str(ceiling),transaction=protected,
                      controls=[evidence(r) for r in results[len(header):len(header)+3]],
                      receipt=evidence(results[-len(assets)-1]))
        return prefix+[protected],result

    reserve=int(row["reserves"][idx.index(x)])
    cap=max(1,min(maximum,reserve//100))
    examples=[]
    for qty in sorted({max(1,cap//4),cap}):
        for supporting in (False,True):
            after,first=leg(prep,x,y,qty,supporting)
            item=dict(forward=first)
            if after:
                _,item["reverse"]=leg(after,y,x,int(first["output"]),supporting)
            examples.append(item)
    return dict(pool=row["pair"],funding_tier=seed_tier,examples=examples)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("directory",type=Path);ap.add_argument("output",type=Path)
    args=ap.parse_args()
    directory=json.loads(args.directory.read_bytes())
    rows=[r for r in directory["pairs"] if int(r["supply"])>1000]
    for kind in ("atf","external_fee"):
        sample=next((r for r in directory["pairs"] if r["pair_class"]==kind and int(r["supply"])<=1000
                     and any(t in (WETH,*STABLES,DAI,ATF) for t in r["tokens"])),None)
        if sample:rows.append(sample)
    failures=[]
    for url in ENDPOINTS:
        try:
            if rpc(url,"eth_getCode",[OWNER,directory["selector"]])!="0x":raise ValueError("simulation payer has code")
            with ThreadPoolExecutor(max_workers=3) as pool:
                outcomes=list(pool.map(lambda r:observe(url,directory,r),rows))
            if not any("output" in e["forward"] for r in outcomes for e in r.get("examples",[])):raise ValueError("no paid market")
            args.output.write_text(json.dumps(dict(directory=directory,outcomes=outcomes),separators=(",",":"))+"\n",encoding="utf8")
            break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as exc:
            failures.append(str(exc)[:450])
    else:raise RuntimeError(failures)
