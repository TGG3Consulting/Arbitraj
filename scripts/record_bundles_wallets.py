"""Compare Bundles Swap's inclusive WETH quote and separately funded BUN fee on the same market."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import ENDPOINTS, OWNER, balance, read_batch, token_flow, tx
from record_bundles_ethereum import capture
from record_ethereum_v2_factories import raw

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
ADAPTER = "0x"+keccak(text="BundlesSwap isolated payer adapter")[-20:].hex()
ORDER = "(address,address,address,address,uint256,uint256,uint256)"
PARAM = "(uint256,uint256,uint256,address[],uint16[],address,uint256)"


def evaluate(run, prefix, frame):
    rows = run(prefix+[frame])
    if any(r["status"]!="0x1" for r in rows[:-1]):
        raise ValueError("archive funding failed before the selected quote")
    return rows[-1]


def attempt(run, row, a, b, qty, path, holders, conf, protocol, end, wire):
    router,weth=conf["address"],conf["weth"]
    tier=row["fee"]
    spender={"router":router,"module":ADAPTER,"v5":EXEC}[wire]
    def fund(t,n):
        if t==weth:
            return tx(t,"deposit()",value=n)
        return {**tx(t,"transfer(address,uint256)",["address","uint256"],[OWNER,n]),"from":holders[t]["holder"]}
    def approve(t,n):
        return tx(t,"approve(address,uint256)",["address","uint256"],[spender,n])
    prefix=[fund(a,qty),approve(a,qty)]
    if path=="converted":
        if a==weth:
            name,other="estimateExactETHForTokens",b
        else:
            name,other="estimateExactTokensForETH",a
        quoted=evaluate(run,prefix,tx(router,name+"(address,uint16,uint256)",["address","uint16","uint256"],[other,tier,qty]))
    else:
        quoted=evaluate(run,prefix,tx(router,"getAmountsOut(uint256,address[],uint16[])",
                        ["uint256","address[]","uint16[]"],[qty,[a,b],[tier]]))
    if quoted["status"]!="0x1":
        return dict(stage="quote",error=quoted.get("error"))
    out = (int(quoted["returnData"],16) if path=="converted"
           else decode(["uint256[]"],raw(quoted["returnData"]))[0][-1])
    if not out:
        return dict(stage="quote",error="zero output")
    cap=0
    if path=="separate":
        if a==protocol:
            cap=qty*tier//(1000+tier)
        elif b==protocol:
            cap=out*tier//1000
        else:
            fee_args=[0,qty,out,0] if a==row["tokens"][0] else [out,0,0,qty]
            fee=evaluate(run,prefix,tx(row["pool"],"getFees(uint256,uint256,uint256,uint256)",["uint256"]*4,fee_args))
            if fee["status"]!="0x1":return dict(stage="oracle",error=fee.get("error"))
            cap=int(fee["returnData"],16)
            if cap:
                prefix+=[fund(protocol,cap),approve(protocol,cap)]
    def swap(limit,deadline):
        if wire=="module":
            budget=cap if path=="separate" and protocol not in (a,b) else 0
            return tx(ADAPTER,"execute("+ORDER+",uint256)",[ORDER,"uint256"],[(row["pool"],a,b,OWNER,qty,limit,deadline),budget])
        if wire=="v5":
            step=(17,ADAPTER,a,b,10000,encode(["address"],[row["pool"]]))
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                      ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,qty,b,limit,deadline,[step]])
        if path=="separate":
            return tx(router,"swapExactTokensForTokens("+PARAM+")",[PARAM],[(qty,limit,cap,[a,b],[tier],OWNER,deadline)])
        if a==weth:
            return tx(router,"swapExactWETHForTokens(uint256,address,uint16,address,uint256,uint256)",
                      ["uint256","address","uint16","address","uint256","uint256"],[qty,b,tier,OWNER,deadline,limit])
        return tx(router,"swapExactTokensForWETH(address,uint16,address,uint256,uint256,uint256)",
                  ["address","uint16","address","uint256","uint256","uint256"],[a,tier,OWNER,deadline,qty,limit])
    watches=list(dict.fromkeys([a,b,protocol]))
    balances=[balance(t,OWNER) for t in watches]
    rows=run(prefix+balances+[swap(out,end)]+balances)
    k=len(prefix)+len(watches)
    if any(r["status"]!="0x1" for i,r in enumerate(rows) if i!=k):
        raise ValueError(("could not fund physical balance",row["pool"],a,qty))
    done=rows[k]
    if done["status"]!="0x1":
        return dict(stage="wallet",quote=str(out),fee_cap=str(cap),error=done.get("error"))
    delta={t:int(rows[k+1+i]["returnData"],16)-int(rows[len(prefix)+i]["returnData"],16) for i,t in enumerate(watches)}
    if delta[a]!=-qty or delta[b]<out or (protocol not in [a,b] and delta[protocol] != (-cap if path=="separate" else 0)):
        raise ValueError(("unexpected payer delta",path,delta,qty,out,cap))
    for t,value in delta.items():
        if token_flow(done["logs"],t,OWNER)!=value:
            raise ValueError(("wallet ERC20 transfers differ from balance",t,value))
    strict=evaluate(run,prefix,swap(delta[b]+1,end))
    expired=evaluate(run,prefix,swap(out,1))
    if strict["status"]!="0x0" or expired["status"]!="0x0":
        raise ValueError(("unprotected bounds",row["pool"],path))
    return dict(stage="settled",quote=str(out),delta={k:str(v) for k,v in delta.items()},fee_cap=str(cap),
                gas=int(done["gasUsed"],16),logs=done["logs"],calldata=swap(out,end)["data"],
                strict=strict.get("error"),expired=expired.get("error"))


def wallets(endpoint,artifact=None):
    record=capture(endpoint)
    expected=json.loads(gzip.decompress((DATA/"bundles_ethereum_directory.json.gz").read_bytes()))
    if json.loads(json.dumps(record))!=expected:raise ValueError("directory or router no longer matches archive")
    holders=json.loads((DATA/"bundles_ethereum_holders.json").read_bytes())
    at,protocol,router=record["selector"],record["protocol_token"],record["router"]
    from record_base_slipstream_executor import rpc
    head=rpc(endpoint,"eth_getBlockByHash",[at["blockHash"],False])
    end=int(head["timestamp"],16)+300
    overrides={who:{"balance":hex(10**19)} for who in {OWNER,*(v["holder"] for v in holders.values())}}
    if artifact:
        if any(c!="0x" for c in read_batch(endpoint,[("eth_getCode",[a,at]) for a in (OWNER,ADAPTER,EXEC)])):
            raise ValueError("isolated address already deployed")
        create={"from":OWNER,"data":"0x"+artifact["creation"]+encode(["address"],[router["address"]]).hex()}
        code,gas=read_batch(endpoint,[("eth_call",[create,at]),("eth_estimateGas",[create,at])])
        template=raw("0x"+artifact["runtime"])
        concrete=bytearray(raw(code))
        refs=list(artifact["immutableReferences"].values())
        if len(refs)!=1 or not refs[0]:
            raise ValueError("unexpected immutable layout")
        for ref in refs[0]:
            start=ref["start"]
            if ref["length"]!=32 or concrete[start:start+32]!=encode(["address"],[router["address"]]):
                raise ValueError("unexpected concrete router")
            concrete[start:start+32]=template[start:start+32]
        if bytes(concrete)!=template:raise ValueError("unexpected deployed adapter instructions")
        word=lambda n:"0x"+format(n,"064x")
        overrides.update({ADAPTER:{"code":code},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                          "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(router["weth"],16))}}})
        record["module"]=dict(address=ADAPTER,router=router["address"],runtime_hash="0x"+keccak(raw(code)).hex(),deploy_gas=int(gas,16))
    def run(calls):
        return simulate_block(endpoint,at,overrides,calls)
    record["cases"]=[]
    for row in record["markets"]:
        for i,a in enumerate(row["tokens"]):
            b=row["tokens"][1-i]
            available=int(holders[a]["balance"]) if a!=router["weth"] else 10**17
            base=min(int(row["reserves"][i])//100_000,available//100)
            for qty in sorted({max(1,base),max(2,base*5)}):
                paths=["separate"] if protocol in row["tokens"] else ["separate","converted"]
                for path in paths:
                    wires=["router"] if not artifact else (["module","v5"] if path=="converted" or protocol in row["tokens"] else ["module"])
                    for wire in wires:
                        outcome=attempt(run,row,a,b,qty,path,holders,router,protocol,end,wire)
                        record["cases"].append(dict(pool=row["pool"],input=a,amount=str(qty),mode=path,wire=wire,**outcome))
    if not any(r["stage"]=="settled" for r in record["cases"]):
        raise ValueError("no confirmed wallet settlement")
    if artifact:
        from record_bundles_sequence import measure
        record["sequences"]=measure(run,record,ADAPTER,end)
    return record


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    parser.add_argument("--artifact",type=Path)
    args=parser.parse_args()
    for ep in ENDPOINTS:
        try:
            result=wallets(ep,json.loads(args.artifact.read_bytes()) if args.artifact else None)
            args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
            print(len(result["cases"]),sum(c["stage"]=="settled" for c in result["cases"]))
            break
        except (RuntimeError,ValueError,requests.RequestException,KeyError,TypeError) as error:
            print(ep,str(error)[:220])
    else:raise SystemExit("no confirmed Bundles Swap payouts")
