"""Fund a fresh wallet from wrapped ETH. Use original Well calls to acquire all other inputs."""
import argparse
import gzip
import json
from collections import deque
from pathlib import Path

from eth_utils import keccak
from fork_fee_rpc import Reader
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx
from record_concave_ethereum import approval
from record_llama_base_cl import CanonicalCalls

WRAP={"base":"0x4200000000000000000000000000000000000006","ethereum":"0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"}
DATA=ROOT/"tests/data/first_run_dex_2026-11-26"
SWAP="swapFrom(address,address,uint256,uint256,address,uint256)"
ARGS=["address","address","uint256","uint256","address","uint256"]

def capture(network):
    filename="pinto_base_wells.json.gz" if network=="base" else "basin_ethereum_wells.json.gz"
    registry=json.loads(gzip.decompress((DATA/filename).read_bytes()))
    reader=Reader(network,json.loads((DATA/(network+".json")).read_bytes()))
    assert reader.tag==registry["selector"]
    wrapped=WRAP[network]
    for address in (OWNER,EXEC):
        if reader.one("eth_getCode",[address,reader.tag])!="0x":raise ValueError("occupied simulation address")
    word=lambda n:"0x"+format(n,"064x")
    runtime="0x"+RUNTIME.read_text().strip().removeprefix("0x")
    overrides={OWNER:{"balance":hex(10**18)},EXEC:{"code":runtime,
                "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(wrapped,16))}}}
    deadline=int(reader.header["timestamp"],16)+300
    def run(calls):
        return simulate_block(None,reader.tag,overrides,calls,read_rpc=lambda _,m,a:reader.one(m,a))
    def good(row):return row["status"]=="0x1"
    def original(well,a,b,n,floor,end):
        return tx(well,SWAP,ARGS,[a,b,n,floor,OWNER,end])
    def executor(well,a,b,n,floor,end):
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                ["address","uint256","address","uint256","uint256",STEP+"[]"],
                [a,n,b,floor,end,[(29,well,a,b,10000,b"")]])
    seed=10**15
    paths={wrapped:([tx(wrapped,"deposit()",value=seed)],seed)}
    queue=deque([wrapped])
    while queue:
        a=queue.popleft()
        prefix,owned=paths[a]
        for well,market in registry["wells"].items():
            ts=market["tokens"]
            coins=[t["token"] for t in ts]
            if a not in coins:continue
            i=coins.index(a);b=coins[1-i]
            if b in paths:continue
            n=min(owned,ts[i]["reserve"]//100)
            if n<=0:continue
            route=prefix+[approval(a,well,n),original(well,a,b,n,1,deadline)]
            rows=run([*route,balance(b,OWNER)])
            if all(map(good,rows)) and (out:=int(rows[-1]["returnData"],16))>0:
                paths[b]=(route,out)
                queue.append(b)
    def trial(well,a,b,n,prefix,wrapped_call,floor,end):
        spend=EXEC if wrapped_call else well
        swap=(executor if wrapped_call else original)(well,a,b,n,floor,end)
        reads=[balance(a,OWNER),balance(b,OWNER),balance(a,well),balance(b,well)]
        calls=[*prefix,approval(a,spend,n),tx(well,"getSwapOut(address,address,uint256)",["address","address","uint256"],[a,b,n]),*reads,swap,*reads]
        pos=len(calls)-5
        rows=run(calls)
        if not all(good(r) for idx,r in enumerate(rows) if idx not in (pos,pos-5)):
            raise ValueError("failed funding/approval/balance before tested swap "+well)
        q=rows[pos-5]
        val=rows[pos]
        before=[int(r["returnData"],16) for r in rows[pos-4:pos]]
        after=[int(r["returnData"],16) for r in rows[pos+1:]]
        deltas=[y-x for x,y in zip(before,after,strict=True)]
        if not good(val):
            if any(deltas):raise ValueError("wallet changed on revert")
            return {"status":"reverted","error":val.get("error",{}),"delta":deltas}
        observed=[-token_flow(val["logs"],a,OWNER),token_flow(val["logs"],b,OWNER)]
        if observed!=[-deltas[0],deltas[1]] or observed[0]!=n or observed[1]<floor:
            raise ValueError("wallet transfers disagree with exact input or output floor")
        return {"status":"paid","quote":int(q["returnData"],16) if good(q) else None,"delta":deltas,
                "gas":int(val["gasUsed"],16),"logs":val["logs"]}
    cases=[]
    for well,market in registry["wells"].items():
        for i,t in enumerate(market["tokens"]):
            a=t["token"];b=market["tokens"][1-i]["token"]
            if a not in paths:
                cases.append({"well":well,"input":a,"status":"no input reachable through funded wells"})
                continue
            prefix,owned=paths[a]
            maximum=min(owned,t["reserve"]//100)
            if not maximum:
                cases.append({"well":well,"input":a,"status":"zero at one percent of reserves"})
                continue
            for n in sorted({max(1,maximum//20),maximum}):
                baseline=trial(well,a,b,n,prefix,False,1,deadline)
                guarded=trial(well,a,b,n,prefix,True,1,deadline)
                row={"well":well,"input":a,"output":b,"amount":n,"original":baseline,"v5":guarded}
                if baseline["status"]=="paid" and guarded["status"]=="paid":
                    if baseline["delta"]!=guarded["delta"]: raise ValueError("cash difference between routes")
                    min_out=guarded["delta"][1]+1
                    for key,after_min,after_time in [("minPlusOne",min_out,deadline),("expired",1,1)]:
                        control=trial(well,a,b,n,prefix,True,after_min,after_time)
                        if control["status"]!="reverted":raise ValueError("invalid minimum or deadline accepted")
                        row[key]=control
                cases.append(row)
    result={"chain":registry["chain"],"block":registry["block"],"parent":reader.tag,
            "wallet":OWNER,"executor":EXEC,"v5Runtime":"0x"+keccak(hexstr=runtime).hex(),"cases":cases}
    if network=="base":
        stable=registry["wells"]["0x3e1133ac082716ddc3114bbefeed8b1731ea9cb1"]["roles"]["function"]["address"]
        lut="0xba510a1eb7262853cdf910db2f6e4d1ee9457196"
        txid="0x3c5e92070761b540f3365e6d0ccc97c3664cb3b3453a8cf3921a8aebe30d4f5c"
        # Verified constructor from Sourcify. Independently confirm its argument on chain.
        birth=reader.one("eth_getTransactionByHash",[txid])
        receipt=reader.one("eth_getTransactionReceipt",[txid])
        assert reader.one("eth_getBlockByNumber",[receipt["blockNumber"],False])["hash"]==receipt["blockHash"]==birth["blockHash"]
        # The factory uses CREATE2, so check the known constructor argument in the deployment call too.
        if lut[2:] not in birth["input"].lower():raise ValueError("LUT constructor argument absent")
        cap=CanonicalCalls(reader)
        result["stable"]={"function":stable,"lut":lut,"a":cap.one(lut,"getAParameter()",returns=["uint256"])[0],
                          "creation":txid,"source":"https://sourcify.dev/server/v2/contract/8453/"+stable+"?fields=creationBytecode.transformationValues,deployment"}
    return result
if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("network",choices=list(WRAP));p.add_argument("output",type=Path);args=p.parse_args()
    args.output.write_bytes(json.dumps(capture(args.network),separators=(",",":")).encode())
