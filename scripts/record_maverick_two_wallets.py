"""Сверяет Quoter, штатный мультивызов и Maverick V2 callback V5 по чистой оплате кошелька."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import OWNER, EXEC, STEP, calldata, rpc
from record_diamondswap_base import direct_wallet
from record_ethervista_wallets import tx
from record_maverick_two_directory import DATA
from maverick_two_target import targets


def capture(network,endpoint,source):
    c=Capture({"endpoint":endpoint,"selector":source["selector"]})
    header=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if header["stateRoot"]!=source["state_root"] or int(rpc(endpoint,"eth_chainId",[]),16)!=source["chain_id"]:
        raise ValueError("wrong canonical market")
    roles=source["roles"]
    router,wrapped=roles["router"]["address"],roles["router"]["wrapped"]
    quoter=roles["quoter"]["address"]
    def swaps(p):
        return sum(v["count"] for v in source["events"].get(p,{}).values())
    rows=[(p,m) for p,m in source["markets"].items() if wrapped in m["tokens"] and not m["permissioned_swap"] and not m["locked"] and min(map(int,m["reserves"]))>0]
    rows.sort(key=lambda x: (swaps(x[0]),int(x[1]["reserves"][x[1]["tokens"].index(wrapped)])),reverse=True)
    samples=[];assets=set()
    for pool,m in rows:
        a,b=m["tokens"];opposite=b if a==wrapped else a
        if opposite in assets:continue
        def build(x,y,amount,floor,deadline,mode):
            if mode=="v5":
                step=(7,pool,x,y,10000,b"")
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],[x,amount,y,floor,deadline,[step]])
            orders=[calldata("checkDeadline(uint256)",["uint256"],[deadline]),
                    calldata("exactInputSingle(address,address,bool,uint256,uint256)",["address","address","bool","uint256","uint256"],
                             [OWNER,pool,x==a,amount,floor])]
            return tx(router,"multicall(bytes[])",["bytes[]"],[[bytes.fromhex(s[2:]) for s in orders]])
        def quote(x,y,amount):
            return tx(quoter,"calculateSwap(address,uint128,bool,bool,int32)",["address","uint128","bool","bool","int32"],
                      [pool,amount,x==a,False,(2**31-1) if x==a else -2**31])
        try:
            cases=direct_wallet(c,dict(m,pool=pool),[10**15,10**16],int(header["timestamp"],16),0,router,
                               wrapped=wrapped,audit=True,build_action=build,quote_call=quote)
            exact_out=targets(c,roles,pool,a,cases,int(header["timestamp"],16)+120)
            for trial in cases:
                for name in ["buy","sell"]:
                    leg=trial[name]
                    used,out,_gas=decode(["uint256","uint256","uint256"],bytes.fromhex(leg["official_quote"]["response"][2:]))
                    if [used,out]!=[int(leg["exact"]["spent"]),int(leg["exact"]["received"])]:
                        raise ValueError(f"{name} quoter, wallet and core disagree")
        except (RuntimeError,ValueError,KeyError) as exc:
            samples.append(dict(pool=pool,reason=str(exc)[:250]));continue
        samples.append(dict(pool=pool,cases=cases,exact_out=exact_out));assets.add(opposite)
        if len(assets)==2:break
    if len(assets)!=2:raise ValueError([(r['pool'],r.get('reason')) for r in samples[:5]])
    return dict(chain_id=source["chain_id"],selector=c.selector,state_root=source["state_root"],roles=roles,samples=samples)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("network",choices=["base","ethereum"]);p.add_argument("output",type=Path)
    args=p.parse_args()
    with gzip.open(DATA/f"maverick_v2_{args.network}_directory.json.gz","rt") as f:source=json.load(f)
    endpoints=(["https://base.gateway.tenderly.co","https://mainnet.base.org","https://base-rpc.publicnode.com"] if args.network=="base"
               else ["https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"])
    errors=[]
    for endpoint in endpoints:
        try:
            result=capture(args.network,endpoint,source)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8");break
        except (RuntimeError,ValueError,KeyError,requests.RequestException) as exc:
            errors.append((endpoint,str(exc)[:400]))
    else:raise ValueError(errors)
