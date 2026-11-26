"""Base Yum Yum Swap: полный реестр CREATE2, две штатные реализации исполнения и два независимых квотера."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_base_native_v3_directory import CanonicalReads, directory
from record_base_slipstream_executor import OWNER, ROOT, call, calldata, rpc, simulate_block
from record_oasisswap_v5 import scalar
from record_memebox_base import tx
from record_deltaswap_base import ENDPOINTS

FACTORY="0xf6c96ac4251905572c7083b1804825850b9bc9e6"
WETH="0x4200000000000000000000000000000000000006"
ROUTER="0xb7e6baaf58e5d32f49b23f45800a379a9d4764bc"
ROUTER02="0x21641cabb1e56dde5be6d0647e9b4de3bb93e81e"
Q1="0xa6f188a1c6b517b8ef6cbe381f8723a50e52fdb8"
Q2="0xac714a42bf2c9f8596ecf21bddabe8459dfb313e"
SIMPLE="(address,address,uint24,address,uint256,uint256,uint160)"
ORDER="(address,address,uint24,address,uint256,uint256,uint256,uint160)"
QUOTE="(address,address,uint256,uint24,uint160)"
HOLDER="0x179c94e8cb6e30f2f74e1dd467502aa92d21298e"
USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"

def view(to,sig,types=(),args=()):
    return {"to":to,"data":calldata(sig,types,args)}

def amount(row):
    return scalar(row)

def balance(a,owner):
    return tx(a,"balanceOf(address)",["address"],[owner])

def issued(router,a,b,qty,fee,floor,deadline,owner):
    if router==ROUTER:
        return tx(router,"exactInputSingle("+ORDER+")",[ORDER],[(a,b,fee,owner,deadline,qty,floor,0)])
    inner=calldata("exactInputSingle("+SIMPLE+")",[SIMPLE],[(a,b,fee,owner,qty,floor,0)])
    return tx(router,"multicall(uint256,bytes[])",["uint256","bytes[]"],[deadline,[bytes.fromhex(inner[2:])]])

def record(e,rows,read,url):
    tag=e["selector"]; header=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    if int(rpc(url,"eth_chainId",[]),16)!=8453 or header["stateRoot"]!=e["state_root"]:
        raise ValueError("unexpected Base state")
    deadline=int(header["timestamp"],16)+120
    roles={}
    for addr in [FACTORY,ROUTER,ROUTER02,Q1,Q2]:
        code=rpc(url,"eth_getCode",[addr,tag]); info={"bytes":len(code)//2-1,"hash":"0x"+keccak(bytes.fromhex(code[2:])).hex()}
        if addr!=FACTORY:
            calls=[("eth_call",[view(addr,"factory()"),tag]),("eth_call",[view(addr,"WETH9()"),tag])]
            raw=read.read(calls)
            if [decode(["address"],bytes.fromhex(r[2:]))[0] for r in raw]!=[FACTORY,WETH]:raise ValueError("periphery role differs")
        roles[addr]=info
    market=[]
    for p in rows:
        pool=p["pool"];a,b=p["tokens"]
        desc=[(pool,"factory()",()),(pool,"token0()",()),(pool,"token1()",()),(pool,"fee()",()),(pool,"tickSpacing()",()),
              (pool,"liquidity()",()),(pool,"slot0()",()),(a,"balanceOf(address)",(pool,)),(b,"balanceOf(address)",(pool,)),
              (a,"decimals()",()),(b,"decimals()",()),(FACTORY,"getPool(address,address,uint24)",(b,a,p["fee"]))]
        req=[("eth_call",[view(to,sig, (["address","address","uint24"] if len(args)==3 else ["address"] if args else []),args),tag]) for to,sig,args in desc]
        raw=read.read(req)
        words=[int(v,16) for v in raw]
        if [int(x,16) for x in (FACTORY,a,b)]+[p["fee"],p["spacing"]]!=words[:5] or words[-1]!=int(pool,16):raise ValueError("pool registry mismatch")
        slot=decode(["uint160","int24","uint16","uint16","uint16","uint8","bool"],bytes.fromhex(raw[6][2:]))
        code=rpc(url,"eth_getCode",[pool,tag])
        market.append(dict(**p,liquidity=str(words[5]),slot=list(slot),balances=list(map(str,words[7:9])),decimals=words[9:11],runtime="0x"+keccak(bytes.fromhex(code[2:])).hex(),cases=[]))
    for m in market:
        if m["liquidity"]=="0":continue
        token=next((t for t in (WETH,USDC) if t in m["tokens"]),None)
        if not token:continue
        other=next(b for b in m["tokens"] if b!=token)
        unit=10**m["decimals"][m["tokens"].index(token)]
        wallet=OWNER if token==WETH else HOLDER
        def run(calls):
            messages=[dict(t,**{"from":wallet}) for t in calls]
            return simulate_block(url,tag,{wallet:{"balance":hex(100*10**18)}},messages)
        for size in [max(1,unit//10**6),max(1,unit//10**3)]:
            funding=[tx(WETH,"deposit()",value=size)] if token==WETH else []
            for router in (ROUTER,ROUTER02):
                def leg(prefix,a,b,qty):
                    if qty==0:return None,None
                    fee=m["fee"]
                    q1=tx(Q1,"quoteExactInputSingle(address,address,uint24,uint256,uint160)",["address","address","uint24","uint256","uint160"],[a,b,fee,qty,0])
                    q2=tx(Q2,"quoteExactInputSingle("+QUOTE+")",[QUOTE],[(a,b,qty,fee,0)])
                    prep=prefix+[tx(a,"approve(address,uint256)",["address","uint256"],[router,qty])]
                    initial=run(prep+[q1,q2])
                    if any(v["status"]!="0x1" for v in initial):return {"reject":"funding or quote","results":initial[-2:]},None
                    price=amount(initial[-2]); out=int(initial[-1]["returnData"][2:66],16)
                    if price!=out:raise ValueError("different native quoters")
                    if out==0:return {"reject":"zero output"},None
                    balances=[balance(a,wallet),balance(b,wallet)]
                    failed=issued(router,a,b,qty,fee,out+1,deadline,wallet)
                    expired=issued(router,a,b,qty,fee,out,1,wallet)
                    done=issued(router,a,b,qty,fee,out,deadline,wallet)
                    calls=run(prep+balances+[failed,expired]+balances+[done]+balances)
                    start=len(prep); before=list(map(amount,calls[start:start+2]));middle=list(map(amount,calls[start+4:start+6]));after=list(map(amount,calls[-2:]))
                    if calls[start+2]["status"]!="0x0" or calls[start+3]["status"]!="0x0" or before!=middle:raise ValueError("price/deadline boundary failed")
                    if calls[-3]["status"]!="0x1":
                        return {"reject":"official swap reverted","quoted":str(out),"error":calls[-3].get("error")},None
                    if before[0]-after[0]!=qty or after[1]-before[1]!=out:raise ValueError("wallet payout mismatch")
                    return dict(amount=str(qty),received=str(out),gas=int(calls[-3]["gasUsed"],16),approval_gas=int(initial[len(prep)-1]["gasUsed"],16),
                                min_plus_one=calls[start+2].get("error"),expired=calls[start+3].get("error")),prep+[done]
                buy,prefix=leg(funding,token,other,size)
                sell,_=leg(prefix,other,token,int(buy["received"])) if prefix else (None,None)
                m["cases"].append(dict(owner=wallet,input=token,router=router,buy=buy,sell=sell))
    if not any(c["sell"] and "received" in c["sell"] for m in market for c in m["cases"]):raise ValueError("no funded wallet roundtrip")
    return dict(selector=tag,state_root=e["state_root"],block=e["block"],factory=FACTORY,endpoint=url,roles=roles,markets=market)

def capture():
    e=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base_native_v3.json").read_bytes());read=CanonicalReads(e)
    nonce,=read.read([("eth_getTransactionCount",[FACTORY,e["selector"]])]);total=int(nonce,16)-1
    pools=directory(read,e,FACTORY,FACTORY,1397802,total)
    # A single endpoint must supply the entire canonical wallet replay.
    errors=[]
    for url in ENDPOINTS:
        try:return record(e,pools,read,url)
        except (requests.RequestException,ValueError,RuntimeError,KeyError) as exc:errors.append((url,str(exc)[:170]))
    raise RuntimeError(f"No verified wallet replay: {errors}")

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);args=ap.parse_args()
    args.output.write_text(json.dumps(capture(),separators=(",",":"))+"\n",encoding="utf8")
