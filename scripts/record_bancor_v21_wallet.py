"""Compare the canonical v2.1 conversion with its deadline-bound wallet and atomic V5 path."""
import argparse
import gzip
from hashlib import sha256
import json
from pathlib import Path
import requests
from eth_abi import encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_balancer_v1 import rpc
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import balance, tx
from record_bancor_v21 import FUNDING, MULTI, NATIVE, UINT, WETH, ZERO
from record_safeswap_protected import runtime

MODULE="0x"+keccak(text="Bancor legacy isolated wallet v2.1")[-20:].hex()
ORDER="(address,address,uint256,uint256,uint256,address[])"


def capture(endpoint,compiled,archive):
    at=archive["selector"]
    c=Capture(dict(endpoint=endpoint,selector=at))
    header=rpc(endpoint,"eth_getBlockByNumber",[hex(archive["block"]),False])
    if header["hash"]!=at["blockHash"] or rpc(endpoint,"eth_chainId",[])!="0x1":
        raise ValueError("historical state changed")
    roles=archive["roles"]
    registry=roles["BancorConverterRegistry"]["address"]
    network=roles["BancorNetwork"]["address"]
    code=runtime(c,compiled,[registry,network,WETH])
    deadline=int(header["timestamp"],16)+600
    word=lambda n:"0x"+format(n,"064x")
    overrides={OWNER:{"balance":hex(100*10**18)},MODULE:{"code":code},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
               "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    if any(rpc(endpoint,"eth_getCode",[addr,at])!="0x" for addr in overrides):
        raise ValueError("archive wallet collision")
    def cash(token,address):
        return tx(MULTI,"getEthBalance(address)",["address"],[address]) if token==ZERO else balance(token,address)
    controls=[]
    for row in archive["controls"]:
        if "spent" not in row:continue
        path=row["path"]; native=NATIVE in (path[0],path[-1])
        amount=int(row["spent"]);minimum=int(row["received"])
        for mode in (["wallet","native_wallet","v5"] if native else ["wallet","v5"]):
            def physical(t):return (ZERO if mode=="native_wallet" else WETH) if t==NATIVE else t
            src=physical(path[0]);dst=physical(path[-1]);payer=EXEC if mode=="v5" else MODULE
            prep=[]
            if src==WETH:prep.append(tx(WETH,"deposit()",value=amount))
            elif src!=ZERO:
                prep.append(tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",[UINT,"address[]","address",UINT],
                               [amount,[WETH,src],OWNER,deadline],value=10**18))
            if src!=ZERO:prep.append(tx(src,"approve(address,uint256)",["address",UINT],[payer,amount]))
            def order(steps,limit,end):
                if mode!="v5":
                    return tx(MODULE,"execute("+ORDER+")",[ORDER],[(src,dst,amount,limit,end,steps)],value=amount if src==ZERO else 0)
                legs=[]
                for i in range(1,len(steps),2):
                    anchor=int(steps[i],16) | (int(steps[i-1]==NATIVE)<<160) | (int(steps[i+1]==NATIVE)<<161)
                    legs.append((17,MODULE,physical(steps[i-1]),physical(steps[i+1]),10000,encode([UINT],[anchor])))
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address",UINT,"address",UINT,UINT,STEP+"[]"],[src,amount,dst,limit,end,legs])
            balances=[cash(t,person) for person in (OWNER,EXEC,MODULE) for t in (src,dst)]
            wrong=[*path];wrong[1]=OWNER
            tests=[order(path,minimum+1,deadline),order(path,minimum,0),order(wrong,minimum,deadline)]
            real=order(path,minimum,deadline)
            result=simulate_block(endpoint,at,overrides,prep+balances+tests+balances+[real]+balances,rpc)
            n=len(prep); failed=[i for i,r in enumerate(result) if r["status"]!="0x1"]
            if failed!=[n+6,n+7,n+8]:
                raise ValueError(("unexpected settlement",mode,path,amount,[(i,result[i].get("error")) for i in failed]))
            def values(group):return [int(r["returnData"],16) for r in group]
            before=values(result[n:n+6]);middle=values(result[n+9:n+15]);after=values(result[n+16:n+22])
            if before!=middle or before[0]-after[0]!=amount or after[1]-before[1]!=minimum or before[2:]!=after[2:]:
                raise ValueError(("cash or refund mismatch",mode,path,amount))
            fill=result[n+15]
            controls.append(dict(mode=mode,source=src,target=dst,path=path,spent=str(amount),received=str(minimum),gas=int(fill["gasUsed"],16),
                tx=real,logs=fill["logs"],refusals=[result[i]["error"] for i in failed],before=list(map(str,before)),after=list(map(str,after))))
    if not any(r["mode"]=="v5" and len(r["path"])>3 for r in controls):raise ValueError("no atomic composed payment")
    creation=bytes.fromhex(compiled["creation"])+encode(["address"]*3,[registry,network,WETH])
    wrapped_code=bytes.fromhex(rpc(endpoint,"eth_getCode",[WETH,at])[2:])
    if not wrapped_code:raise ValueError("missing wrapped native runtime")
    manifest=dict(kind="bancor_legacy",wrappedHash="0x"+keccak(wrapped_code).hex(),chainId=1,factory=registry,network=network,wrapped=WETH,
                  factoryHash=roles["BancorConverterRegistry"]["keccak256"],networkHash=roles["BancorNetwork"]["keccak256"],
                  sourceBlock=at["blockHash"],creationSha256=sha256(creation).hexdigest(),runtimeKeccak="0x"+keccak(hexstr=code).hex(),
                  runtimeSha256=sha256(bytes.fromhex(code[2:])).hexdigest(),data="0x"+creation.hex(),solc=compiled["solc"])
    return dict(selector=at,module=MODULE,manifest=manifest,controls=controls)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("compiled",type=Path);p.add_argument("output",type=Path)
    args=p.parse_args()
    with gzip.open(ROOT/"tests/data/first_run_dex_2026-11-26/bancor_v21_ethereum.json.gz","rt") as stream:archive=json.load(stream)
    errors=[]
    for endpoint in ("https://rpc.mevblocker.io","https://eth.drpc.org"):
        try:
            result=capture(endpoint,json.loads(args.compiled.read_bytes()),archive)
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as exc:errors.append((endpoint,str(exc)[:450]))
    else:raise ValueError(errors)
    args.output.write_bytes(json.dumps(result,separators=(",",":")).encode())
