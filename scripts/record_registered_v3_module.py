"""Реальный байткод V5 и один модуль с точным именем callback. Проверяет платёж, allowance и срок у обоих входов."""
import argparse
import json
from pathlib import Path
from eth_abi import encode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, RUNTIME, STEP, rpc, simulate_block
from record_fraxswap_swaps import bound_runtime
from record_oasisswap_v5 import allowance, balance, scalar, word
from record_memebox_base import tx
from record_deltaswap_base import ENDPOINTS
from requests import RequestException

WETH="0x4200000000000000000000000000000000000006"
ADAPTER="0x"+keccak(text="isolated registered V3 module")[-20:].hex()

def record(market,artifact,url,*,bunny=False,candy=False):
    tag=market["selector"];authority=market["router"] if bunny else market["factory"]
    head=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    if head["stateRoot"]!=market["state_root"] or int(rpc(url,"eth_chainId",[]),16)!=8453:raise ValueError("unexpected parent")
    if any(rpc(url,"eth_getCode",[a,tag])!="0x" for a in (EXEC,ADAPTER)):raise ValueError("isolated module address occupied")
    runtime=bound_runtime(artifact,authority,url,tag)
    code=RUNTIME.read_text().strip().removeprefix("0x"); deadline=int(head["timestamp"],16)+120
    completed=[]
    for pool in market["markets"]:
        # Both native ABI paths establish the same quote; one funded buy/reverse is sufficient as the independent reference.
        observed=set()
        for ref in pool["cases"]:
            if not ref["sell"] or "received" not in ref["sell"]:continue
            owner=ref["owner"];a=ref["input"];b=next(t for t in pool["tokens"] if t!=a);size=int(ref["buy"]["amount"])
            key=(owner,a,size)
            if key in observed:continue
            observed.add(key)
            env={owner:{"balance":hex(100*10**18)},ADAPTER:{"code":"0x"+runtime},
                EXEC:{"code":"0x"+code,"stateDiff":{word(0):word(int(owner,16)),word(1):word(int(WETH,16))}}}
            def run(calls):
                return simulate_block(url,tag,env,[dict(c,**{"from":owner}) for c in calls])
            for mode in ("direct","v5"):
                target=EXEC if mode=="v5" else ADAPTER
                def leg(prefix,token,out,reference):
                    qty=int(reference["amount"]);net=int(reference["received"])
                    def order(floor,end):
                        if (bunny or candy) and mode=="v5":
                            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                      ["address","uint256","address","uint256","uint256",STEP+"[]"],
                                      [token,qty,out,floor,end,[(29,ADAPTER,token,out,10000,b"")]])
                        if mode=="direct":
                            return tx(ADAPTER,"execute(address,address,address,uint256,uint256,uint256)",
                                      ["address","address","address","uint256","uint256","uint256"],[pool["pool"],token,out,qty,floor,end])
                        encoded=encode(["address"],[pool["pool"]])
                        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[token,qty,out,floor,end,[(17,ADAPTER,token,out,10000,encoded)]])
                    prep=prefix+[tx(token,"approve(address,uint256)",["address","uint256"],[target,qty])]
                    checks=[balance(token,owner),balance(out,owner),allowance(token,owner,target),
                            allowance(token,EXEC,ADAPTER),balance(token,EXEC),balance(out,EXEC),balance(token,ADAPTER),balance(out,ADAPTER)]
                    success=order(net,deadline)
                    calls=run(prep+checks+[order(net+1,deadline),order(net,1)]+checks+[success]+checks)
                    start=len(prep);n=len(checks);before=list(map(scalar,calls[start:start+n]))
                    strict,expired=calls[start+n:start+n+2]; middle=list(map(scalar,calls[start+n+2:start+2*n+2]));done=calls[-n-1];after=list(map(scalar,calls[-n:]))
                    if any(c["status"]!="0x1" for c in calls[:start]) or strict["status"]!="0x0" or expired["status"]!="0x0" or middle!=before:
                        raise ValueError("deadline/minimum failed to contain state changes")
                    if done["status"]!="0x1":
                        raise ValueError(f"{mode} {pool['pool']} {token} {qty}: exact wallet call reverted {done.get('error')}")
                    if scalar(done)!=net or after[1]-before[1]!=net or before[0]-after[0]!=qty or any(before[3:]+after[2:]):
                        raise ValueError("wallet input/output, module balance or residual allowance differs")
                    if candy:
                        view=tx(ADAPTER,"getAmountOut(address,address,uint256)",["address","address","uint256"],[token,out,qty])
                        if scalar(run(prep+[view])[-1])!=net:
                            raise ValueError("CandySwap module quote differs from wallet receipt")
                        if token==WETH:
                            # The isolated payer makes its own earlier deposit. It must not be counted
                            # as payment for its subsequent order, including when V5 calls the module.
                            gift=max(1,qty//10)
                            own_deposit=[tx(WETH,"deposit()",value=gift),tx(WETH,"transfer(address,uint256)",["address","uint256"],[pool["pool"],gift])]
                            proof=run(prep+own_deposit+[view,balance(out,owner),success,balance(out,owner)])
                            if any(x["status"]!="0x1" for x in proof) or scalar(proof[-4])!=net or scalar(proof[-1])-scalar(proof[-3])!=net:
                                raise ValueError("prior payer deposit changed the subsequent order payout")
                    return dict(amount=str(qty),received=str(net),gas=int(done["gasUsed"],16),approval_gas=int(calls[start-1]["gasUsed"],16),
                                strict=strict["error"],expired=expired["error"],before=list(map(str,before)),after=list(map(str,after))),prep+[success]
                fund=ref.get("funding", [tx(WETH,"deposit()",value=size)] if a==WETH else [])
                buy,fund=leg(fund,a,b,ref["buy"]);sell,_=leg(fund,b,a,ref["sell"])
                completed.append(dict(pool=pool["pool"],mode=mode,owner=owner,input=a,buy=buy,sell=sell))
    if not completed:raise ValueError("no funded direct/V5 comparison")
    return dict(**({"router":authority} if bunny else {"factory":authority}),selector=tag,endpoint=url,module_hash="0x"+keccak(bytes.fromhex(runtime)).hex(),
                executor_hash="0x"+keccak(bytes.fromhex(code)).hex(),cases=completed)

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    for k in ("market","module","output"):ap.add_argument(k,type=Path)
    ap.add_argument("--bunny",action="store_true")
    ap.add_argument("--candy",action="store_true")
    a=ap.parse_args();m=json.loads(a.market.read_bytes());b=json.loads(a.module.read_bytes());errors=[]
    if a.bunny:
        from record_bunnyswap_base import FRIEND, OWNER
        m["markets"]=[dict(pool=m["pair"],tokens=[WETH,FRIEND],
                           cases=[dict(owner=OWNER,input=WETH,buy=c["buy"],sell=c["sell"]) for c in m["cases"]])]
    if a.candy:
        from record_base_slipstream_executor import OWNER
        markets={row["pool"]:row["tokens"] for row in m["pools"]}
        def observed(row):
            return {"amount":row["spent"],"received":row["paid"]}
        m["markets"]=[dict(pool=row["pool"],tokens=markets[row["pool"]],cases=[
            dict(owner=OWNER,input=WETH,buy=observed(c["buy"]),sell=observed(c["sell"])) for c in row["cases"]
        ]) for row in m["net_payouts"]]
    for url in dict.fromkeys([m["endpoint"],*ENDPOINTS]):
        try:j=record(m,b,url,bunny=a.bunny,candy=a.candy)
        except (RequestException,ValueError,RuntimeError,KeyError) as err:errors.append((url,str(err)[:180]));continue
        a.output.write_text(json.dumps(j,separators=(",",":"))+"\n",encoding="utf8");break
    else:raise RuntimeError(f"Registered V3 archive failed: {errors}")
