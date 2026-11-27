"""Публичное архивное чтение и дочерние блоки со своим плательщиком, без подписей и сетевой отправки."""
import argparse
import gzip
import json
import time
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor

import requests
from eth_abi import encode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT, OWNER, EXEC, RUNTIME, STEP, call, calldata, rpc, simulate_block
from record_omni_v2_base import ENDPOINTS

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
WETH = "0x4200000000000000000000000000000000000006"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
MODULE = "0x" + keccak(text="Arbitraj Stabull protected wallet on isolated child")[-20:].hex()
WORD = lambda n: "0x" + format(n, "064x")
ASSETS = ["address","address","uint256","uint256","uint256"]
SIG = "(" + ",".join(ASSETS) + ")"
ORDER = ["address",*ASSETS]
SWAP = "(" + ",".join(ORDER) + ")"
OK = lambda op: op["status"] == "0x1"


def request(url, method, params):
    if method not in ("eth_call","eth_estimateGas"):
        raise ValueError(method)
    res = requests.post(url,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=25)
    res.raise_for_status()
    reply=res.json()
    if "result" not in reply:
        raise ValueError(reply.get("error"))
    return reply["result"]


def capture(url, artifact, network):
    chain,wrapped,cash,funding_profile=(8453,WETH,USDC,"uniswap-v2-base") if network=="base" else (
        1,"0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2","0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","uniswap_v2")
    d=json.loads(gzip.decompress((DATA / f"stabull_{network}_directory.json.gz").read_bytes()))
    tag=d["anchor"]; factory=d["factory"]; router=factory["router"]
    head=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False]); stamp=int(head["timestamp"],16)+2; expiry=stamp+240
    if head["hash"]!=tag["blockHash"] or int(rpc(url,"eth_chainId",[]),16)!=chain or d["chain"]!=chain:
        raise ValueError("canonical chain")
    if any(rpc(url,"eth_getCode",[a,tag])!="0x" for a in (OWNER,EXEC,MODULE)):
        raise ValueError("occupied isolated address")
    overrides={OWNER:{"balance":hex(2*10**18)}}
    identity=None
    if artifact:
        creation={"from":OWNER,"data":"0x"+artifact["creation"]+encode(["address"],[factory["address"]]).hex()}
        # Та же неподписанная транзакция своего тестового плательщика: runtime и газ CREATE.
        constructor=dict(creation,gas=hex(6_000_000),value="0x0")
        [created]=simulate_block(url,tag,overrides,[constructor])
        if not OK(created):raise ValueError(("constructor",created.get("error")))
        deployed=request(url,"eth_call",[creation,tag]);raw=bytes.fromhex(deployed[2:]);masked=bytearray(raw);blank=bytes.fromhex(artifact["runtime"])
        [refs]=artifact["immutableReferences"].values()
        for field in refs:
            start,size=field["start"],field["length"]
            if size!=32 or raw[start:start+size]!=encode(["address"],[factory["address"]]):raise ValueError("factory immutable")
            masked[start:start+size]=blank[start:start+size]
        if masked!=blank:raise ValueError("artifact body")
        gas=int(created["gasUsed"],16)
        identity={"runtime":"0x"+keccak(raw).hex(),"factory":factory["address"],"deployment_gas":gas,"solc":artifact["solc"]}
        overrides[MODULE]={"code":deployed}
        overrides[EXEC]={"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),"stateDiff":{WORD(0):WORD(int(OWNER,16)),WORD(1):WORD(int(wrapped,16))}}
    def run(calls):
        if network=="ethereum":time.sleep(1.5)
        block={"blockOverrides":{"time":hex(stamp)},"stateOverrides":overrides,"calls":calls}
        [child]=rpc(url,"eth_simulateV1",[{"blockStateCalls":[block]},tag])
        if child["parentHash"]!=tag["blockHash"] or len(child["calls"])!=len(calls):raise ValueError("incomplete child")
        return child["calls"]
    def tx(to,fn,types,args):
        return call(to,calldata(fn,types,args),gas=4_000_000)
    def approve(asset,spender,v):
        return tx(asset,"approve(address,uint256)",["address","uint256"],[spender,v])
    def balances(a,b,where):
        return [tx(t,"balanceOf(address)",["address"],[owner]) for t,owner in [(a,OWNER),(b,OWNER),(a,MODULE),(b,MODULE),(a,EXEC),(b,EXEC)]]+[
                tx(a,"allowance(address,address)",["address","address"],[MODULE,where])]
    funding=next(p["settings"]["dex.router"] for p in json.loads((ROOT/"config/first_run_dex.json").read_bytes()) if p["id"]==funding_profile and p["network"]==network)
    preparation=[call(funding,calldata("swapExactETHForTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[12_000_000,[wrapped,cash],OWNER,expiry]),10**17)]
    if not all(map(OK,run(preparation))):raise ValueError("own independent USDC funding")
    def probe(pool,mode,ip,op,amount,prefix,want):
        qsig="viewTargetSwap" if want else "viewOriginSwap"
        query=tx(pool,qsig+"(address,address,uint256)",["address","address","uint256"],[ip,op,amount])
        qr=run(prefix+[query])[-1]
        if not OK(qr) or not int(qr["returnData"],16):return None,{"stage":"quote unavailable","error":qr.get("error"),"raw":qr["returnData"]}
        q=int(qr["returnData"],16)
        cap=q+max(q//100,3) if want else amount
        payee={"pool":pool,"router":router,"module":MODULE,"v5":EXEC}[mode]
        auth=approve(ip,payee,cap)
        def order(limit,deadline):
            if mode=="pool":return tx(pool,("targetSwap" if want else "originSwap")+SIG,ASSETS,[ip,op,limit if want else amount,amount if want else limit,deadline])
            if mode=="router":return tx(router,"originSwap"+SWAP,ORDER,[cash,ip,op,amount,limit,deadline])
            if mode=="module":return tx(MODULE,("executeTarget" if want else "execute")+SWAP,ORDER,[pool,ip,op,limit if want else amount,amount if want else limit,deadline])
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],
                      [ip,amount,op,limit,deadline,[(17,MODULE,ip,op,10000,encode(["address"],[pool]))]])
        reads=balances(ip,op,pool);setup=prefix+[auth]
        def play(limit,deadline):
            trade=order(limit,deadline)
            r=run(setup+reads+[trade]+reads)
            if not all(map(OK,r[:len(setup)+len(reads)])) or not all(map(OK,r[-len(reads):])):raise ValueError("unavailable setup/read")
            before=[int(row["returnData"],16) for row in r[len(setup):len(setup)+len(reads)]]
            after=[int(row["returnData"],16) for row in r[-len(reads):]]
            return trade,r[-len(reads)-1],before,after,int(r[len(prefix)]["gasUsed"],16)
        _,first,b,a,_=play(cap if want else 1,expiry)
        base={"mode":mode,"input":ip,"output":op,"specified":str(amount),"quoted":str(q),"exact_output":want}
        if not OK(first):
            if a!=b:raise ValueError("failed payment moved funds")
            return None,dict(**base,stage="swap reverted",error=first.get("error"))
        paid=b[0]-a[0];net=a[1]-b[1]
        if (want and (paid>cap or net<amount)) or (not want and paid!=amount) or net<=0 or a[2:]!=b[2:]:
            return None,dict(**base,stage="wallet differs",paid=str(paid),net=str(net))
        tight=paid if want else net
        # Отказы и платёж видят одинаковое состояние пула. Неуспешные top-level вызовы откатываются.
        deadline=stamp if mode=="v5" else expiry
        fail_tx=order(tight-1 if want else tight+1,expiry)
        late_tx=order(tight,stamp-1 if mode=="v5" else stamp)
        payment=order(tight,deadline)
        calls=setup+reads+[fail_tx,late_tx]+reads+[payment]+reads
        outcomes=run(calls)
        pos=len(setup)+len(reads)
        failed,late=outcomes[pos:pos+2]
        out=outcomes[pos+2+len(reads)]
        for offset,expected in [(len(setup),b),(pos+2,b),(pos+3+len(reads),a if OK(out) else b)]:
            checks=outcomes[offset:offset+len(reads)]
            if not all(map(OK,checks)) or [int(row["returnData"],16) for row in checks]!=expected:
                raise ValueError((pool,mode,"cash boundary mismatch"))
        if OK(failed) or OK(late) or not all(map(OK,outcomes[:len(setup)])):
            raise ValueError((pool,mode,"boundary rollback"))
        if not OK(out):return None,dict(**base,stage="strict boundary unavailable",paid=str(paid),net=str(net))
        if mode=="v5":base["deadline_equality_gas"]=int(out["gasUsed"],16)
        before,after,auth_gas=b,a,int(outcomes[len(prefix)]["gasUsed"],16)
        return setup+[payment],dict(**base,stage="paid",paid=str(paid),net=str(net),gas=int(out["gasUsed"],16),logs=out["logs"],calldata=payment,
                                    wallet_and_module_before=list(map(str,before)),wallet_and_module_after=list(map(str,after)),
                                    approval_gas=auth_gas,min_or_cap_error=failed.get("error"),deadline_error=late.get("error"))
    modes=["module","v5"] if artifact else ["pool","router"]
    def cycle(row):
        pool=row["address"];token=row["token0"]
        if row["token1"]!=cash:raise ValueError(("unknown cash currency",pool))
        cases=[]
        for size in [1000,1_000_000]:
            for mode in modes:
                prefix,buy=probe(pool,mode,cash,token,size,preparation,False)
                c={"size_usdc":size,"mode":mode,"buy":buy};cases.append(c)
                if prefix:
                    amount=int(buy["net"]);_,sell=probe(pool,mode,token,cash,amount,prefix,False);c["sell"]=sell
                    if mode in ("pool","module"):
                        _,target=probe(pool,mode,cash,token,amount,preparation,True);c["buy_target"]=target
                        if sell.get("stage")=="paid":
                            _,target=probe(pool,mode,token,cash,max(1,int(sell["net"])//2),prefix,True);c["sell_target"]=target
        return {"pool":pool,"token":token,"cases":cases}
    with ThreadPoolExecutor(max_workers=1 if network=="ethereum" else 2) as ex:records=list(ex.map(cycle,d["pools"]))
    return {"chain":chain,"selector":tag,"state_root":head["stateRoot"],"simulated_at":stamp,"payer":OWNER,"module":identity,
            "funding":preparation,"records":records}


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);parser.add_argument("--module",type=Path)
    parser.add_argument("--network",choices=["base","ethereum"],default="base")
    opt=parser.parse_args()
    errors=[]
    endpoints=ENDPOINTS if opt.network=="base" else ["https://mainnet.gateway.tenderly.co"]
    for endpoint in endpoints:
        try:
            result=capture(endpoint,json.loads(opt.module.read_bytes()) if opt.module else None,opt.network)
            opt.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
            break
        except (requests.RequestException,ValueError,KeyError,RuntimeError) as e:
            errors.append(str(e)[:300])
    else:raise RuntimeError(errors)
