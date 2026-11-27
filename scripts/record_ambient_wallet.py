"""Funded EOA and existing V5, with the same market, order and capital as the original exchange."""
import gzip
import hashlib
import json
import sys
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
import requests

from record_ambient_ethereum import DEX, QUERY, WETH, ZERO, FUND, SWAP, approve, balance, tx
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block

UINT="uint256"
ORDER="(address,address,uint256)"
FUN=[ORDER,"address","address","uint128","uint128","bool","uint128",UINT]
DATA=ROOT/"tests/data/first_run_dex_2026-11-26/ambient_ethereum_payments.json.gz"


def run(url,compiled):
    original=json.loads(gzip.decompress(DATA.read_bytes()))
    tag=original["head"]
    header=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    assert header["hash"]==tag["blockHash"] and int(rpc(url,"eth_chainId",[]),16)==1
    end=int(header["timestamp"],16)+900
    assert int(rpc(url,"eth_getTransactionCount",[OWNER,tag]),16)==0
    module="0x"+keccak(bytes.fromhex("d694"+OWNER[2:]+"80"))[-20:].hex()
    assert rpc(url,"eth_getCode",[module,tag])=="0x" and rpc(url,"eth_getCode",[EXEC,tag])=="0x"
    word=lambda x:"0x"+int(x).to_bytes(32,"big").hex()
    env={OWNER:{"balance":hex(5*10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
         "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    creation=compiled["creation"]+encode(["address"]*3,[DEX,QUERY,WETH]).hex()
    deploy={"from":OWNER,"data":"0x"+creation,"gas":hex(5_000_000)}
    ack=simulate_block(url,tag,env,[deploy,tx(module,"dex()"),tx(module,"query()"),tx(module,"wrapped()")])
    assert all(r["status"]=="0x1" for r in ack) and [int(r["returnData"],16) for r in ack[1:]]==[int(a,16) for a in (DEX,QUERY,WETH)]
    runtime=bytes.fromhex(ack[0]["returnData"][2:])
    manifest=dict(kind="ambient_ethereum",chainId=1,sourceBlock=tag["blockHash"],exchange=DEX,query=QUERY,wrapped=WETH,
                  roles=original["roles"],data="0x"+creation,gas=int(ack[0]["gasUsed"],16),solc=compiled["solc"],
                  creationSha256=hashlib.sha256(bytes.fromhex(creation)).hexdigest(),runtimeSha256=hashlib.sha256(runtime).hexdigest(),
                  runtimeKeccak="0x"+keccak(runtime).hex())
    fills=[]
    for leg in original["payments"]:
        physical=[leg["source"],leg["target"]]
        market=(*sorted(physical),420)
        key=keccak(encode(["address","address",UINT],market))
        src,dst=[WETH if a==ZERO else a for a in physical]
        cap,want=map(int,(leg["spent"],leg["received"]))
        target=not leg["exact_input"]
        # Leave change from the maximum buy budget; the wallet pays only the original signed flow.
        budget=cap+10 if target else cap
        seed=10**17
        for mode in (["direct"] if target else ["direct","v5"]):
            spender=module if mode=="direct" else EXEC
            prefix=[deploy,tx(WETH,"deposit()",value=2*seed)]
            if src!=WETH:
                fill=(WETH,src,500,OWNER,end,seed,budget,0)
                prefix += [approve(WETH,FUND,seed),tx(FUND,"exactInputSingle("+SWAP+")",[SWAP],[fill])]
            prefix.append(approve(src,spender,budget))
            def swap(maximum,minimum,deadline,price=0):
                if mode=="direct":
                    return tx(module,"execute("+",".join(FUN)+")",FUN,[market,src,dst,maximum,minimum,target,price,deadline])
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address",UINT,"address",UINT,UINT,STEP+"[]"],
                          [src,maximum,dst,minimum,deadline,[(17,module,src,dst,10000,encode(["bytes32"],[key]))]])
            views=[balance(src,OWNER),balance(dst,OWNER),balance(src,module),balance(dst,module),
                   tx(src,"allowance(address,address)",["address","address"],[module,DEX])]
            strict=swap(cap-1,want,end) if target else swap(budget,want+1,end)
            checks=[strict,swap(budget,want,1)]
            if mode=="direct" and target:
                # A limit at the current price permits no full output despite a generous input cap.
                state=next(s for s in original["states"] if (s["base"],s["quote"],s["index"])==market)
                checks.append(swap(budget,want,end,state["price"]))
            calls=prefix+views+checks+views+[swap(budget,want,end)]+views
            res=simulate_block(url,tag,env,calls)
            i=len(prefix); j=i+len(views); k=j+len(checks); paid=k+len(views)
            failed=[(n,r.get("error")) for n,r in enumerate(res) if r["status"]!="0x1"]
            if [n for n,_ in failed]!=list(range(j,k)):
                raise ValueError(("payment sequence",mode,physical,target,failed,cap,want))
            def state(n):
                return [int(x["returnData"],16) for x in res[n:n+len(views)]]
            before,middle,after=state(i),state(k),state(paid+1)
            assert before==middle and [before[0]-after[0],after[1]-before[1]]==[cap,want] and before[2:]==after[2:]==[0,0,0]
            fills.append(dict(mode=mode,input=src,output=dst,market=market,pool_key="0x"+key.hex(),target=target,
                              budget=str(budget),spent=str(cap),received=str(want),gas=int(res[paid]["gasUsed"],16),
                              strict=failed[0][1],expired=failed[1][1],refusals=[e for _,e in failed],call=calls[paid],logs=res[paid]["logs"]))
    return dict(head=tag,manifest=manifest,owner=OWNER,module=module,payments=fills)


if __name__=="__main__":
    compiled=json.loads(Path(sys.argv[2]).read_bytes())
    for url in ["https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"]:
        try:
            report=run(url,compiled)
            break
        except (requests.RequestException,RuntimeError) as err:
            print(url,str(err)[:180],flush=True)
    else:
        raise RuntimeError("canonical Ambient wallet simulation unavailable")
    Path(sys.argv[1]).write_text(json.dumps(report,separators=(",",":")),encoding="utf8")
