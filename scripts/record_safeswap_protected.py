"""Изолированная оплата SafeSwap proxy с ETH-комиссией отдельным авансом или из WETH.
Проверяет чистый результат, точное разрешение и возврат аванса на двух реальных рынках.
"""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import WETH
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH
from record_safeswap_trade import MULTICALL

MODULE="0x"+keccak(text="canonical SafeSwap module 2026-11-26")[-20:].hex()
ZERO="0x"+"00"*20
SNAP=ROOT/"tests/data/first_run_dex_2026-11-26/safeswap_ethereum_wallets.json.gz"
ORDER="(address,address,uint256,uint256,address,uint256)"
FILLED="0x"+keccak(text="Filled(address,address,address,address,uint256,uint256,uint256,bool)").hex()


def runtime(c, artifact, roles):
    constructor=artifact["creation"]+encode(["address"]*6,roles).hex()
    reply=c.session.post(c.endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_call",
                         "params":[{"from":OWNER,"data":"0x"+constructor},c.selector]},timeout=35)
    reply.raise_for_status()
    data=reply.json()["result"]
    template=bytes.fromhex(artifact["runtime"])
    normalized=bytearray.fromhex(data[2:])
    stored=set()
    for refs in artifact["immutableReferences"].values():
        values={bytes(normalized[r["start"]:r["start"]+r["length"]]) for r in refs}
        if len(values)!=1 or any(r["length"]!=32 for r in refs):raise ValueError("invalid immutable reference")
        stored.update(values)
        for r in refs:normalized[r["start"]:r["start"]+32]=bytes(32)
    if normalized!=template or stored!={encode(["address"],[a]) for a in roles}:raise ValueError("unbound immutable")
    return data


def capture(endpoint, artifact):
    d=json.loads(gzip.decompress(SNAP.read_bytes()))
    c=Capture(dict(endpoint=endpoint,selector=d["selector"]))
    head=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if (rpc(endpoint,"eth_chainId",[])!="0x1" or head["hash"]!=c.selector["blockHash"] or head["stateRoot"]!=ROOT_HASH or
            any(rpc(endpoint,"eth_getCode",[a,c.selector])!="0x" for a in (OWNER,MODULE,EXEC))):raise ValueError("wrong chain, state or occupied account")
    deadline=int(head["timestamp"],16)+300
    word=lambda n:"0x"+format(n,"064x")
    ether=lambda who:tx(MULTICALL,"getEthBalance(address)",["address"],[who])
    result=[]
    for dep in d["deployments"][2:]:
        router=dep["roles"]["router"]["address"]
        factory=dep["roles"]["factory"]["address"]
        public=dep.get("access",{}).get("trade",{}).get("address",ZERO)
        code=runtime(c,artifact,[OWNER,EXEC,factory,router,public,WETH])
        env={OWNER:{"balance":hex(10**20)},MODULE:{"code":code},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
             "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
        def run(seq):
            return simulate_block(endpoint,c.selector,env,seq)
        def fees(pre,a,b,n):
            if public==ZERO:return 0
            read=tx(public,"getFees(address[],uint256,address)",["address[]","uint256","address"],[[a,b],n,MODULE])
            answers=run([*pre,read])
            if any(x["status"]!="0x1" for x in answers):raise ValueError("module fee or preparation reverted")
            total,fee,fa,fb=decode(["uint256"]*4,bytes.fromhex(answers[-1]["returnData"][2:]))
            if total!=fee or fa or fb:raise ValueError("unverified token fee")
            return fee
        def leg(pre,mode,a,b,n):
            separate=mode=="separate"
            target=EXEC if mode=="v5" else MODULE
            fee=fees(pre,a,b,n) if separate else 0
            approval=tx(a,"approve(address,uint256)",["address","uint256"],[target,n])
            seq=[*pre,approval]
            def command(minimum,end):
                if separate:return tx(MODULE,"execute("+ORDER+")",[ORDER],[(a,b,n,minimum,OWNER,end)],fee)
                if mode=="module":return tx(MODULE,"swapFrom(address,address,uint256,uint256,address,uint256)",
                                           ["address","address","uint256","uint256","address","uint256"],[a,b,n,minimum,OWNER,end])
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,n,b,minimum,end,[(29,MODULE,a,b,10000,b"")]])
            checks=[balance(a,OWNER),balance(b,OWNER),ether(OWNER),balance(a,MODULE),balance(b,MODULE),ether(MODULE),
                    balance(a,EXEC),balance(b,EXEC),tx(a,"allowance(address,address)",["address","address"],[MODULE,router]),
                    tx(MODULE,"feeReserve()")]
            if public!=ZERO:checks.extend([balance(a,public),balance(b,public),ether(public)])
            split=len(checks)
            def trial(minimum,end):
                payload=command(minimum,end)
                rows=run(seq+checks+[payload]+checks)
                start=len(seq); win=rows[start+split]
                other=rows[:start+split]+rows[start+split+1:]
                if any(x["status"]!="0x1" for x in other):raise ValueError("funding, approval or balance check failed")
                before=[int(x["returnData"],16) for x in rows[start:start+split]]
                after=[int(x["returnData"],16) for x in rows[-split:]]
                if win["status"]!="0x1":
                    if before!=after or win.get("logs"):raise ValueError("revert altered balances")
                    return (None,win.get("error"))
                got=after[1]-before[1]
                if before[0]-after[0]!=n or before[2]-after[2]!=fee or before[3:]!=after[3:] or before[8] or got<=0:
                    raise ValueError("payment, residual or fee mismatch")
                ev=[x for x in win["logs"] if x["address"].lower()==MODULE and x["topics"][0].lower()==FILLED]
                if len(ev)!=1:raise ValueError("missing settlement event")
                output,spent,credit,actual_fee,separate_flag=decode(["address","uint256","uint256","uint256","bool"],bytes.fromhex(ev[0]["data"][2:]))
                if (output,spent,credit,separate_flag)!=(b,n,got,separate) or (separate and actual_fee!=fee):raise ValueError("wrong settlement")
                return (dict(spent=str(n),received=str(got),fee=str(actual_fee),external_fee=str(fee),before=list(map(str,before)),after=list(map(str,after)),
                             transaction=payload,gas=int(win["gasUsed"],16),approval_gas=int(rows[start-1]["gasUsed"],16),logs=win["logs"]),None)
            loose,why=trial(1,deadline)
            if loose is None:raise ValueError(("protected loose",mode,a,n,str(why)[:145]))
            got=int(loose["received"])
            exact,error=trial(got,deadline)
            above,ei=trial(got+1,deadline)
            expired,et=trial(got,1)
            if not exact or error or exact["received"]!=str(got) or above or expired or not ei or not et:
                raise ValueError("exact minimum or expiry not enforced")
            return dict(exact=exact,above=ei,expired=et),[*seq,command(got,deadline)]
        for m in dep["markets"]:
            if WETH not in m["tokens"]:continue
            observed=[r for r in m.get("rounds",[])+m.get("paid_public",[]) if r.get("mode","router")=="router" and
                      all(r.get(s,{}).get("exact",{}).get("status")=="0x1" for s in ("buy","sell"))]
            if not observed:continue
            token=next(a for a in m["tokens"] if a!=WETH)
            for r in observed:
                size=int(r["buy"]["exact"]["spent"])
                for mode in ("separate","module","v5"):
                    # The wrapped modes fund the economically same buy: principal plus its ETH fee.
                    fee=fees([],WETH,token,size);budget=size if mode=="separate" else size+fee
                    pre=[tx(WETH,"deposit()",value=budget)]
                    if public!=ZERO and mode!="separate":pre+=[tx(MODULE,"fundFees()",value=10**16)]
                    buy,prefix=leg(pre,mode,WETH,token,budget)
                    sell,_=leg(prefix,mode,token,WETH,int(buy["exact"]["received"]))
                    result.append(dict(profile=dep["id"],mode=mode,pool=m["pool"],token=token,payer=OWNER,module=MODULE,
                                       executor=EXEC,factory=factory,router=router,public=public,runtime="0x"+keccak(hexstr=code).hex(),
                                       buy=buy,sell=sell))
    if {r["profile"] for r in result}!={"proxy-1","proxy-2"}:raise ValueError("missing protected market")
    return dict(selector=c.selector,height=d["height"],state_root=ROOT_HASH,runtime_template="0x"+keccak(hexstr=artifact["runtime"]).hex(),
                cases=result)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("artifact",type=Path);parser.add_argument("output",type=Path)
    args=parser.parse_args();errors=[]
    for endpoint in ENDPOINTS:
        try:
            data=capture(endpoint,json.loads(args.artifact.read_bytes()))
            args.output.write_text(json.dumps(data,separators=(",",":")),encoding="utf8");break
        except (requests.RequestException,ValueError,KeyError,RuntimeError) as error:errors.append((endpoint,str(error)[:350]))
    else:raise RuntimeError(errors)
