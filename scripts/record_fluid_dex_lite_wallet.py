"""Replay original Fluid Lite, a funded wallet module, and its V5 step with the same inputs."""

import gzip
import hashlib
import json
import sys
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, RUNTIME, STEP
from record_fluid_dex_lite import CORE, OWNER, ROOT, WETH, approve, balance, independently_fund, rpc, simulate_block, tx

DIR = ROOT / "tests/data/first_run_dex_2026-11-26"
DIRECT = ["bytes32","address","address","uint256","uint256","bool","uint256"]


def word(n):
    return "0x"+int(n).to_bytes(32,"big").hex()


def run(url, compiled, original):
    head = original["head"]
    header = rpc(url,"eth_getBlockByHash",[head["blockHash"],False])
    assert int(rpc(url,"eth_chainId",[]),16)==1 and header["hash"]==head["blockHash"]
    end = int(header["timestamp"],16)+600
    assert int(rpc(url,"eth_getTransactionCount",[OWNER,head]),16)==0
    module = "0x"+keccak(bytes.fromhex("d694"+OWNER[2:]+"80"))[-20:].hex()
    assert rpc(url,"eth_getCode",[module,head]) == rpc(url,"eth_getCode",[EXEC,head]) == "0x"
    env = {OWNER: {"balance":hex(2*10**18)}, EXEC: {"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
        "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    creation = compiled["creation"]+encode(["address","address"],[CORE,WETH]).hex()
    deploy = {"from":OWNER,"data":"0x"+creation,"gas":hex(5_000_000)}
    def sim(calls):
        return simulate_block(url,head,env,calls)
    ack = sim([deploy,tx(module,"dex()"),tx(module,"wrapped()"),tx(module,"venueHash()")])
    assert all(r["status"]=="0x1" for r in ack)
    assert [int(r["returnData"],16) for r in ack[1:]] == [int(CORE,16),int(WETH,16),int(original["roles"][CORE],16)]
    runtime = bytes.fromhex(ack[0]["returnData"][2:])
    manifest = dict(kind="fluid_dex_lite",chainId=1,sourceBlock=head["blockHash"],exchange=CORE,wrapped=WETH,
                    roles=original["roles"],data="0x"+creation,gas=int(ack[0]["gasUsed"],16),solc=compiled["solc"],
                    creationSha256=hashlib.sha256(bytes.fromhex(creation)).hexdigest(),
                    runtimeSha256=hashlib.sha256(runtime).hexdigest(),runtimeKeccak="0x"+keccak(runtime).hex())
    replay = []
    for index,pool in enumerate(original["pools"]):
        market = bytes.fromhex(pool["pool_id"][2:])+index.to_bytes(24,"big")
        for leg in pool["payments"]:
            if "spent" not in leg:
                continue
            src,dst = leg["source"],leg["target"]
            spent,received = int(leg["spent"]),int(leg["received"])
            exact_out = not leg["exact_input"]
            cap = spent+13 if exact_out else spent
            for route in (["direct"] if exact_out else ["direct","v5"]):
                payer = module if route=="direct" else EXEC
                # Replace the reference allowance to the original with one allowance to the actual spender.
                prep = [deploy]+independently_fund(src,end)[:-1]+[approve(src,payer,cap)]
                def order(budget,minimum,deadline,key=market):
                    if route=="direct":
                        return tx(module,"execute("+",".join(DIRECT)+")",DIRECT,[key,src,dst,budget,minimum,exact_out,deadline])
                    fields = ["address","uint256","address","uint256","uint256",STEP+"[]"]
                    return tx(EXEC,"execute("+",".join(fields)+")",fields,
                              [src,budget,dst,minimum,deadline,[(17,module,src,dst,10000,encode(["bytes32"],[key]))]])
                views = [balance(src,OWNER),balance(dst,OWNER),balance(src,module),balance(dst,module),
                         tx(src,"allowance(address,address)",["address","address"],[module,CORE])]
                controls = [order(spent-1,received,end) if exact_out else order(cap,received+1,end),
                            order(cap,received,1),order(cap,received,end,bytes([market[0]^1])+market[1:]),
                            tx(module,"dexCallback(address,uint256,bytes)",["address","uint256","bytes"],[src,1,b""])]
                calls = prep+views+controls+views+[order(cap,received,end)]+views
                res = sim(calls)
                n=len(prep); middle=n+len(views)+len(controls); paid=middle+len(views)
                fails=[(i,x.get("error")) for i,x in enumerate(res) if x["status"]!="0x1"]
                if [i for i,_ in fails]!=list(range(n+len(views),middle)):
                    raise ValueError(("cash/control mismatch",pool["pool_id"],route,exact_out,src,[(i,str(v)[:140]) for i,v in fails]))
                def state(i):
                    return [int(r["returnData"],16) for r in res[i:i+len(views)]]
                before,mid,after=state(n),state(middle),state(paid+1)
                assert before==mid and (mid[0]-after[0],after[1]-mid[1])==(spent,received) and before[2:]==after[2:]==[0,0,0]
                replay.append(dict(mode=route,market="0x"+market.hex(),source=src,target=dst,exact_input=leg["exact_input"],
                                   spent=str(spent),received=str(received),budget=str(cap),strict=fails[0][1],controls=[v for _,v in fails],
                                   tx=calls[paid],logs=res[paid]["logs"],gas=int(res[paid]["gasUsed"],16)))
    assert replay, "active Fluid DEX Lite block supplied no payable comparisons"
    return dict(manifest=manifest,owner=OWNER,module=module,head=head,payments=replay)


if __name__=="__main__":
    compiled=json.loads(Path(sys.argv[2]).read_bytes())
    original=json.loads(gzip.decompress((DIR/"fluid_dex_lite_ethereum.json.gz").read_bytes()))["active"]
    for url in ("https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"):
        try:
            report=run(url,compiled,original)
            break
        except (requests.RequestException, RuntimeError) as exc:
            print(url,str(exc)[:160],flush=True)
    else:
        raise RuntimeError("Fluid DEX Lite wallet archive unavailable")
    Path(sys.argv[1]).write_text(json.dumps(report,separators=(",",":")),encoding="utf8")
