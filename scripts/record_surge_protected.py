"""Проверяет WETH и составной USDT маршрут по двум последовательным блокам Surge с реальным фондированием."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import FUNDING, WETH
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH
from record_surge_ethereum import MC, TOKEN

MODULE = "0x" + keccak(text="canonical own Surge module 2026-11-26")[-20:].hex()
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
FIXTURE = ROOT / "tests/data/first_run_dex_2026-11-26/surge_ethereum_wallets.json.gz"


def capture(endpoint, artifact):
    native = json.loads(gzip.decompress(FIXTURE.read_bytes()))
    tag = native["selector"]
    head = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"], False])
    if rpc(endpoint, "eth_chainId", []) != "0x1" or head["hash"] != tag["blockHash"] or head["stateRoot"] != ROOT_HASH:
        raise ValueError("different Ethereum state")
    c = Capture(dict(endpoint=endpoint, selector=tag))
    if any(rpc(endpoint, "eth_getCode", [a, tag]) != "0x" for a in (OWNER, EXEC, MODULE)):
        raise ValueError("funding or code override address is occupied")
    if "0x"+keccak(hexstr=rpc(endpoint,"eth_getCode",[TOKEN,tag])).hex()!=native["runtime"]:
        raise ValueError("different market code")
    tokens = [TOKEN, WETH, OWNER, EXEC]
    init = "0x" + artifact["creation"] + encode(["address"]*4, tokens).hex()
    # Выполняем только конструктор в eth_call без создания контракта в сети.
    response=c.session.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_call",
                            "params":[{"from":OWNER,"data":init},tag]},timeout=30)
    response.raise_for_status()
    deployed=response.json()["result"]
    template = bytes.fromhex(artifact["runtime"])
    code = bytearray.fromhex(deployed[2:])
    roles = set()
    for positions in artifact["immutableReferences"].values():
        seen = {bytes(code[p["start"]:p["start"]+p["length"]]) for p in positions}
        if len(seen) != 1 or any(p["length"] != 32 for p in positions): raise ValueError("immutable mismatch")
        roles.update(seen)
        for p in positions:
            pos = p["start"]
            code[pos:pos+32] = bytes(32)
    if roles != {encode(["address"],[a]) for a in tokens} or code != template: raise ValueError("unbound runtime")
    word = lambda v: "0x"+format(v,"064x")
    env = {OWNER:{"balance":hex(10**20)}, MODULE:{"code":deployed}, EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
           "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    deadline = int(head["timestamp"],16)+300
    uni = next(v["settings"] for v in json.loads((ROOT/"config/first_run_dex.json").read_bytes())
               if v["network"]=="ethereum" and v["settings"]["dex.kind"]=="uniswap_v2")
    factory=uni["dex.factory"].lower()
    pair, = c.one(factory,"getPair(address,address)",["address","address"],[WETH,USDT],["address"])
    if not int(pair,16) or c.one(pair,"factory()",returns=["address"])!=(factory,):
        raise ValueError("USDT conversion market is not registered")
    exchange = lambda a,b: (0,pair,a,b,10000,encode(["uint32"],[3000]))
    protected = lambda a,b: (29,MODULE,a,b,10000,b"")
    def order(mode,cash,buy,n,minimum,end):
        a,b = (cash,TOKEN) if buy else (TOKEN,cash)
        if mode in ("module","native_module"):
            method="swapNative" if mode=="native_module" else "swapFrom"
            return tx(MODULE,method+"(address,address,uint256,uint256,address,uint256)",
                      ["address","address","uint256","uint256","address","uint256"],[a,b,n,minimum,OWNER,end],
                      n if mode=="native_module" and buy else 0)
        steps = [protected(WETH,TOKEN)] if buy else [protected(TOKEN,WETH)]
        if cash==USDT:
            steps = [exchange(USDT,WETH),*steps] if buy else [*steps,exchange(WETH,USDT)]
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,n,b,minimum,end,steps])
    def batch(first,second):
        args = [{"stateOverrides":env,"calls":first},{"calls":second}]
        frames = rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":args},tag])
        if (len(frames)!=2 or frames[0]["parentHash"]!=tag["blockHash"] or frames[1]["parentHash"]!=frames[0]["hash"]
            or any(int(f["number"],16)!=native["height"]+i+1 or len(f["calls"])!=len(args[i]["calls"]) for i,f in enumerate(frames))):
            raise ValueError("invalid parent or partial results")
        return [f["calls"] for f in frames]
    cases=[]
    for mode,cash,sizes in [("native_module",WETH,(10**15,10**16)),("module",WETH,(10**15,10**16)),("v5",WETH,(10**15,10**16)),("v5",USDT,(10**6,10**7))]:
        native_cash=mode=="native_module"
        spender=EXEC if mode=="v5" else MODULE
        def cash_balance(who):
            return tx(MC,"getEthBalance(address)",["address"],[who]) if native_cash else balance(cash,who)
        approve=lambda a,n:tx(a,"approve(address,uint256)",["address","uint256"],[spender,n])
        holdings=[read for a in (OWNER,MODULE,EXEC) for read in (cash_balance(a),balance(TOKEN,a))]
        holdings += [tx(MC,"getEthBalance(address)",["address"],[MODULE])]
        if cash!=WETH or native_cash:holdings += [balance(WETH,a) for a in (MODULE,EXEC)]
        def sample(calls):
            if any(r["status"]!="0x1" or len(r["returnData"])!=66 for r in calls):raise ValueError("balance read failed")
            return [int(r["returnData"],16) for r in calls]
        for n in sizes:
            fund=tx(WETH,"deposit()",value=n) if cash==WETH else tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                 ["uint256","address[]","address","uint256"],[n,[WETH,USDT],OWNER,deadline],10**18)
            prep=[] if native_cash else [fund,approve(cash,n)]
            def discover(a_prefix,op,probe):
                first,second=batch([*a_prefix,op,probe],[tx(MC,"getEthBalance(address)",["address"],[OWNER])])
                if any(r["status"]!="0x1" for r in first+second):raise ValueError(("buy preparation",[(r.get("error")) for r in first if r["status"]!="0x1"]))
                return int(first[-1]["returnData"],16)
            bought=discover(prep,order(mode,cash,True,n,1,deadline),balance(TOKEN,OWNER))
            buy=order(mode,cash,True,n,bought,deadline)
            sale_prep=[approve(TOKEN,bought)]
            trial=order(mode,cash,False,bought,1,deadline)
            a,b=batch(prep+[buy],sale_prep+[cash_balance(OWNER),trial,cash_balance(OWNER)])
            if any(r["status"]!="0x1" for r in a+b):raise ValueError(("sell preparation",mode,cash,n,b[2].get("error")))
            net=int(b[-1]["returnData"],16)-int(b[1]["returnData"],16)
            if not net or not bought:raise ValueError("empty return")
            def section(buying,size,paid):
                return [*holdings,order(mode,cash,buying,size,paid+1,deadline),order(mode,cash,buying,size,paid,1),
                        *holdings,order(mode,cash,buying,size,paid,deadline),*holdings]
            a,b=batch(prep+section(True,n,bought)+sale_prep+[trial,*holdings],sale_prep+section(False,bought,net))
            def assess(rows,buying,size,paid):
                k=len(holdings)
                before=sample(rows[:k]);rejected=rows[k:k+2];zero=sample(rows[k+2:2*k+2])
                win=rows[2*k+2];after=sample(rows[2*k+3:3*k+3])
                if before!=zero or any(r["status"]!="0x0" for r in rejected) or win["status"]!="0x1":raise ValueError("floor or deadline mismatch")
                spent=(before[0]-after[0]) if buying else (before[1]-after[1])
                got=(after[1]-before[1]) if buying else (after[0]-before[0])
                if (spent,got)!=(size,paid) or before[2:]!=after[2:]:raise ValueError("wallet or module residue differs")
                return dict(spent=str(spent),received=str(got),before=list(map(str,before)),after=list(map(str,after)),gas=int(win["gasUsed"],16),
                            calldata=order(mode,cash,buying,size,paid,deadline),logs=win["logs"],failures=[r.get("error") for r in rejected])
            ac=assess(a[len(prep):],True,n,bought);bc=assess(b[len(sale_prep):],False,bought,net)
            if a[-len(holdings)-1]["status"]!="0x0" or sample(a[-len(holdings):])!=list(map(int,ac["after"])) or ac["after"]!=bc["before"]:
                raise ValueError("same block sale accepted or continuation differs")
            cases.append(dict(mode=mode,cash=cash,native_cash=native_cash,buy=ac,sell=bc,approval_gas=[0 if native_cash else int(a[1]["gasUsed"],16),int(b[0]["gasUsed"],16)],
                              same_block=a[-len(holdings)-1]["error"]))
    return dict(chain=1,selector=tag,height=native["height"],state_root=ROOT_HASH,payer=OWNER,market=TOKEN,wrapped=WETH,executor=EXEC,
                runtime_template="0x"+keccak(template).hex(),runtime="0x"+keccak(hexstr=deployed).hex(),immutable_references=artifact["immutableReferences"],
                usdt_pair=pair,modeled_module=MODULE,cases=cases)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("artifact",type=Path);ap.add_argument("output",type=Path);args=ap.parse_args()
    errors=[]
    for endpoint in ENDPOINTS:
        try:
            data=capture(endpoint,json.loads(args.artifact.read_bytes()))
            args.output.write_text(json.dumps(data,separators=(",",":")),encoding="utf8");break
        except (requests.RequestException,ValueError,RuntimeError,KeyError) as e:errors.append((endpoint,str(e)[:300]))
    else:raise RuntimeError(errors)
