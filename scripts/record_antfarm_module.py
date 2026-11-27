"""Replay the same primary fill with ATF held by the payer and ATF bought from the input budget."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import OWNER, rpc, simulate_block
from record_antfarm_wallets import ATF, ROUTER, ENDPOINTS, MAX, WETH, approval, order, balance, tx, evidence, number

MODULE="0x"+keccak(text="Antfarm Ethereum protected owner settlement")[-20:].hex()
ABI="(address,address,uint16,uint16,bool,uint256,uint256,uint256,uint256,uint256,uint256,uint256)"
TOPIC="0x"+keccak(text="Settled(address,address,address,uint256,uint256,uint256,uint256)").hex()


def record(url,data,runtime):
    d=data["directory"]
    tag=d["selector"]
    for a in (OWNER,MODULE):
        if rpc(url,"eth_getCode",[a,tag])!="0x":raise ValueError("simulation address already in use")
    overrides={OWNER:{"balance":hex(10**18)},MODULE:{"code":"0x"+runtime}}
    end=d["block_time"]+300
    def approved(token,limit):
        return tx(token,"approve(address,uint256)",["address","uint256"],[MODULE,limit])
    funding=next(r for r in d["pairs"] if r["tokens"]==[ATF,WETH] and r["fee"]==100)
    atf,weth=map(int,funding["reserves"])
    pools={r["pair"]:r for r in d["pairs"]}
    cases=[]
    for row in data["outcomes"]:
        primary=pools[row["pool"]]
        if primary["pair_class"]!="external_fee":continue
        for example in row.get("examples",[]):
            e=example["forward"]
            if e.get("source")!=WETH or e.get("supporting") or not e.get("output"):continue
            q=int(e["parameter"]); out=int(e["output"]); fee=int(e["fee_ceiling"])
            y=e["target"]
            # Router getAmountsIn: un-discount exact ATF then floor division plus one raw input.
            raw=fee*1000//900
            if raw>=atf:continue
            cost=weth*raw//(atf-raw)+1
            for tier in (0,100):
                maximum=q+cost if tier else q
                pre=[tx(WETH,"deposit()",value=maximum+10**15)]
                if not tier:
                    pre.extend([approval(WETH,10**15),order(WETH,ATF,10**15,100,fee,MAX,end),approved(ATF,fee)])
                pre.append(approved(WETH,maximum))
                balances=[balance(t,w) for w in (OWNER,MODULE) for t in (WETH,y,ATF)]
                def place(minimum,ceiling,deadline,max_budget=maximum,max_funding=(cost if tier else 0)):
                    return tx(MODULE,"execute("+ABI+")",[ABI],[(WETH,y,primary["fee"],tier,False,q,max_budget,
                              minimum,ceiling,fee,max_funding,deadline)])
                guards=[place(out+1,fee,end),place(out,fee-1,end),place(out,fee,0)]
                if tier:guards.append(place(out,fee,end,max_funding=cost-1))
                good=place(out,fee,end)
                calls=pre+balances+guards+balances+[good]+balances
                res=simulate_block(url,tag,overrides,calls)
                n=len(pre); m=len(balances); k=len(guards)
                if any(v["status"]!="0x1" for v in res[:n+m]) or any(v["status"]!="0x0" for v in res[n+m:n+m+k]):
                    raise ValueError(("funding or bounds",row["pool"],tier,[evidence(v) for v in res if v["status"]!="0x1"][:5]))
                before=list(map(number,res[n:n+m])); after_fail=list(map(number,res[n+m+k:n+2*m+k]))
                actual=res[n+2*m+k]; after=list(map(number,res[-m:]))
                if actual["status"]!="0x1" or before!=after_fail or before[3:]!=after[3:]:
                    raise ValueError(("settlement",row["pool"],tier,evidence(actual)))
                delta=[before[0]-after[0],after[1]-before[1],before[2]-after[2]]
                logs=[l for l in actual["logs"] if l["address"].lower()==MODULE and l["topics"][0].lower()==TOPIC]
                if delta!=[maximum,out,0 if tier else fee] or len(logs)!=1:
                    raise ValueError(("wallet or event",row["pool"],tier,delta))
                ev=logs[0]
                if [t[-40:].lower() for t in ev["topics"][1:]]!=[z[2:] for z in (OWNER,WETH,y)] or list(decode(
                    ["uint256"]*4,bytes.fromhex(ev["data"][2:])))!=[*delta,0]:
                    raise ValueError("settlement event differs from wallet")
                cases.append(dict(pool=row["pool"],mode="buy_fee" if tier else "held_fee",primary_input=str(q),
                                  funding_cost=str(cost),net=list(map(str,delta)),transaction=good,gas=int(actual["gasUsed"],16),
                                  controls=list(map(evidence,res[n+m:n+m+k])),event=ev))
    if len({c["pool"] for c in cases})<2 or {c["mode"] for c in cases}!={"held_fee","buy_fee"}:
        raise ValueError("insufficient protected fee coverage")
    return dict(selector=tag,state_root=d["state_root"],module=MODULE,runtime_hash="0x"+keccak(hexstr=runtime).hex(),cases=cases)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    for field in ("market","artifact","output"):ap.add_argument(field,type=Path)
    a=ap.parse_args();data=json.loads(a.market.read_bytes());art=json.loads(a.artifact.read_bytes())
    errors=[]
    for ep in ENDPOINTS:
        try:report=record(ep,data,art["runtime"])
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as exc:
            errors.append(str(exc)[:400]);continue
        a.output.write_text(json.dumps(report,separators=(",",":"))+"\n",encoding="utf8");break
    else:raise RuntimeError(errors)
