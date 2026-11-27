"""Compare Fluid's original swapHop and two V5 steps through different keys held by one singleton."""
import gzip
import json
import sys
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_fluid_dex_lite import CORE, DEX_KEY, OWNER, RESOLVER, ROOT, USDC, USDE, USDT, WETH, approve, balance, independently_fund, method, rpc, simulate_block, tx
from record_base_slipstream_executor import EXEC, RUNTIME, STEP
from record_fluid_dex_lite_wallet import word


def run(url, original, wallet):
    head=original["head"]
    block=rpc(url,"eth_getBlockByHash",[head["blockHash"],False])
    assert head==wallet["head"] and block["hash"]==head["blockHash"] and int(rpc(url,"eth_chainId",[]),16)==1
    end=int(block["timestamp"],16)+600
    module=wallet["module"]
    keys={}
    for i,pool in enumerate(original["pools"]):
        cash=(pool["token0"],pool["token1"],bytes.fromhex(pool["salt"][2:]))
        keys[frozenset(cash[:2])]=(cash,keccak(encode([DEX_KEY],[cash]))[:8]+i.to_bytes(24,"big"))
    create={"from":OWNER,"data":wallet["manifest"]["data"],"gas":hex(5_000_000)}
    env={OWNER:{"balance":hex(2*10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
          "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    results=[]
    for path in ([USDC,USDT,USDE], [USDE,USDT,USDC],
                 [USDC,USDT,USDC,USDT,USDE]):
        source,target=path[0],path[-1]
        amount=10**(18 if source==USDE else 6)
        entries=[keys[frozenset(pair)] for pair in zip(path,path[1:])]
        market=[x[0] for x in entries]
        terms=["address[]",DEX_KEY+"[]","int256"]
        q=tx(RESOLVER,method("estimateSwapHop",terms),terms,[path,market,amount])
        aq,=simulate_block(url,head,env,[q])
        assert aq["status"]=="0x1"
        expected=int(aq["returnData"],16)
        assert expected>0
        for form in ["original","v5","protected"]:
            spender={"original":CORE,"v5":EXEC,"protected":module}[form]
            prep=[create]+independently_fund(source,end)[:-1]+[approve(source,spender,amount)]
            def swap(floor):
                if form=="original":
                    fields=terms+["uint256[]","(address,bool,bytes,bytes)"]
                    return tx(CORE,method("swapHop",fields),fields,[path,market,amount,[0]*(len(market)-1)+[floor],(OWNER,False,b"",b"")])
                if form=="protected":
                    fields=["bytes32[]","address[]","uint256","uint256","uint256"]
                    return tx(module,method("executeHop",fields),fields,[[p[1] for p in entries],path,amount,floor,end])
                fields=["address","uint256","address","uint256","uint256",STEP+"[]"]
                steps=[(17,module,path[i],path[i+1],10000,encode(["bytes32"],[p[1]])) for i,p in enumerate(entries)]
                return tx(EXEC,method("execute",fields),fields,[source,amount,target,floor,end,steps])
            views=[balance(source,OWNER),balance(target,OWNER),balance(USDT,OWNER),balance(USDT,module),balance(USDT,EXEC)]
            calls=prep+views+[swap(expected+1)]+views+[swap(expected)]+views
            rec=simulate_block(url,head,env,calls);i=len(prep);v=len(views);paid=i+2*v+1
            fail=[n for n,r in enumerate(rec) if r["status"]!="0x1"]
            assert fail==[i+v],(form,source,[(n,rec[n].get("error")) for n in fail])
            def state(n):
                return [int(r["returnData"],16) for r in rec[n:n+v]]
            start,middle,finish=state(i),state(i+v+1),state(paid+1)
            assert start==middle and (start[0]-finish[0],finish[1]-start[1])==(amount,expected)
            assert start[2:]==finish[2:]
            ordered=[int(log["data"][2:66],16)&(2**64-1) for log in rec[paid]["logs"]
                     if log["address"].lower()==CORE and log["topics"]==["0x"+keccak(text="LogSwap(uint256,uint256)").hex()]]
            assert ordered==[int.from_bytes(p[1][:8],"big") for p in entries]
            if form=="protected":
                receipt=next(log for log in rec[paid]["logs"] if log["address"].lower()==module and log["topics"][0]=="0x"+keccak(text="HopFilled(bytes32,address,address,address,address,uint256,uint256)").hex())
                assert receipt["topics"][1]=="0x"+keccak(encode(["bytes32[]","address[]"],[[p[1] for p in entries],path])).hex()
                assert [int(t,16) for t in receipt["topics"][2:]]==[int(OWNER,16)]*2
            results.append(dict(form=form,path=path,markets=["0x"+p[1].hex() for p in entries],source=source,target=target,spent=str(amount),received=str(expected),gas=int(rec[paid]["gasUsed"],16),
                                tx=calls[paid],strict=rec[i+v]["error"],logs=rec[paid]["logs"]))
    return dict(head=head,payments=results)


if __name__=="__main__":
    p=ROOT/"tests/data/first_run_dex_2026-11-26"
    def load(name):
        return json.loads(gzip.decompress((p/name).read_bytes()))
    original=load("fluid_dex_lite_ethereum.json.gz")["active"]
    wallet=json.loads(Path(sys.argv[2]).read_bytes()) if len(sys.argv)>2 else load("fluid_dex_lite_wallet.json.gz")
    for url in ("https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"):
        try:
            report=run(url,original,wallet)
            break
        except (requests.RequestException,RuntimeError) as exc:
            print(url,str(exc)[:160],flush=True)
    else:
        raise RuntimeError("Fluid DEX Lite route archive unavailable")
    Path(sys.argv[1]).write_text(json.dumps(report,separators=(",",":")),encoding="utf8")
