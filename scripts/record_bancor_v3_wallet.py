"""Replay protected Bancor cash modes with a wallet funded outside Bancor. No signed or broadcast requests."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak
from record_bancor_v3 import NATIVE, MULTI
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, OWNER, EXEC, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import tx,balance
from record_ethereum_v2_factories import WETH,FUNDING
from record_safeswap_protected import runtime

MODULE="0x"+keccak(text="Bancor protected isolated Ethereum exchange").hex()[-40:]
ETH="0x"+"00"*20
ORDER="(address,address,uint256,uint256,uint256,uint256,bool)"


def capture(url,artifact):
    folder=ROOT/"tests/data/first_run_dex_2026-11-26"
    market=json.load(gzip.open(folder/"bancor_v3_ethereum.json.gz","rt"))
    c=Capture(dict(selector=market["selector"],endpoint=url))
    r=market["roles"];net=r["BancorNetwork"]["address"];info=r["BancorNetworkInfo"]["address"];vault=r["MasterVault"]["address"];bnt=market["bnt"]
    code=runtime(c,artifact,[net,info,vault,WETH,NATIVE])
    at=c.selector;h=rpc(url,"eth_getBlockByHash",[at["blockHash"],False]);end=int(h["timestamp"],16)+300
    word=lambda a:"0x"+format(int(a,16),"064x")
    overrides={OWNER:{"balance":hex(100*10**18)},MODULE:{"code":code},
               EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),"stateDiff":{"0x"+"00"*32:word(OWNER),word("1"):word(WETH)}}}
    if any(rpc(url,"eth_getCode",[addr,at])!="0x" for addr in overrides):raise ValueError("isolation")
    if c.one(net,"isWhitelisted(address)",["address"],[MODULE],["bool"])[0]:
        raise ValueError("unexpected fee exemption for isolated module")
    def bal(asset,owner):
        return tx(MULTI,"getEthBalance(address)",["address"],[owner]) if asset==ETH else balance(asset,owner)
    cases=[]
    link=next(t for t,p in market["pools"].items() if p["enabled"] and t!=NATIVE)
    for src,dst in [(NATIVE,bnt),(bnt,NATIVE),(NATIVE,link),(link,NATIVE),(bnt,link),(link,bnt)]:
        for exact in (True,False):
            size=10**15
            func=("tradeOutputBySourceAmount" if exact else "tradeInputByTargetAmount")+"(address,address,uint256)"
            q=c.one(info,func,["address","address","uint256"],[src,dst,size],["uint256"])[0]
            spent,received=(size,q) if exact else (q,size)
            for mode in ["wallet","native_wallet","v5"]:
                if mode=="native_wallet" and NATIVE not in (src,dst) or mode=="v5" and not exact:continue
                wrap=ETH if mode=="native_wallet" else WETH
                a=wrap if src==NATIVE else src;b=wrap if dst==NATIVE else dst
                form=int(src==NATIVE)+2*int(dst==NATIVE)
                cap=spent if exact else spent+max(13,spent//10)
                prefix=[]
                if a==WETH:
                    prefix.append(tx(WETH,"deposit()",value=cap))
                elif a!=ETH:
                    prefix.append(tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                                     ["uint256","address[]","address","uint256"],[cap,[WETH,a],OWNER,end],value=10**18))
                if a!=ETH:
                    prefix.append(tx(a,"approve(address,uint256)",["address","uint256"],[EXEC if mode=="v5" else MODULE,cap]))
                def order(limit,floor,deadline,flags):
                    if mode=="v5":
                        step=(17,MODULE,a,b,10000,encode(["uint256"],[flags]))
                        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,limit,b,floor,deadline,[step]])
                    return tx(MODULE,"execute("+ORDER+")",[ORDER],[(a,b,flags,limit,floor,deadline,not exact)],value=limit if a==ETH else 0)
                balances=[bal(t,p) for p in (OWNER,MODULE,EXEC) for t in (a,b)]
                calls=prefix+balances+[order(cap if exact else spent-1,received+1 if exact else received,end,form),
                      order(cap,received,0,form),order(cap,received,end,form|4),order(cap,received,end,form)]+balances
                frame=simulate_block(url,at,overrides,calls)
                rec=dict(mode=mode,source=src,target=dst,asset_in=a,asset_out=b,form=form,exact_input=exact,maximum=str(cap),minimum=str(received))
                if any(v["status"]!="0x1" for v in frame[:len(prefix)]):
                    raise ValueError(("funding",a,mode))
                before,controls,trade,after=frame[-16:-10],frame[-10:-7],frame[-7],frame[-6:]
                if any(v["status"]!="0x0" for v in controls) or trade["status"]!="0x1":
                    raise ValueError(("cash controls",src,dst,exact,mode,[v["status"] for v in controls],trade.get("error")))
                before,after=([int(v["returnData"],16) for v in x] for x in (before,after))
                if (before[0]-after[0],after[1]-before[1])!=(spent,received) or before[2:]!=after[2:]:
                    raise ValueError(("physical cash",src,dst,exact,mode))
                rec.update(spent=str(spent),received=str(received),gas=int(trade["gasUsed"],16),logs=trade["logs"]);cases.append(rec)
    return dict(selector=at,module=MODULE,module_hash="0x"+keccak(hexstr=code).hex(),cases=cases)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("artifact",type=Path);p.add_argument("out",type=Path);args=p.parse_args()
    errors=[]
    for url in ["https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"]:
        try:
            data=capture(url,json.loads(args.artifact.read_bytes()))
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as e:errors.append((url,str(e)[:240]))
    else:raise RuntimeError(errors)
    args.out.write_text(json.dumps(data,separators=(",",":")),encoding="utf8")
