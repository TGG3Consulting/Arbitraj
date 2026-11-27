"""Measure Motoswap direct, guarded and mixed V5-compatible settlement on exactly the original state."""
import argparse
import gzip
import json
from hashlib import sha256
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
from fork_fee_rpc import Reader
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx
from record_motoswap import ARGS, FACTORY, FEE, FUNDING, STABLES, SWAP, WETH, approve

MODULE="0x"+keccak(text="MotoswapWallet archive simulation").hex()[-40:]
DIRECT="execute(address,address,address,uint256,uint256,uint256)"


def capture(artifact):
    folder=ROOT/"tests/data/first_run_dex_2026-11-26"
    original=json.loads(gzip.decompress((folder/"motoswap_ethereum.json.gz").read_bytes()))
    reader=Reader("ethereum",json.loads((folder/"ethereum.json").read_bytes()))
    if reader.tag!=original["selector"] or reader.header["stateRoot"]!=original["root"]:
        raise ValueError("different parent")
    creation="0x"+artifact["creation"]+encode(["address","address"],[FACTORY,FEE]).hex()
    runtime=reader.one("eth_call",[{"from":OWNER,"data":creation},reader.tag])
    normalized=bytearray.fromhex(runtime[2:])
    words=set()
    for refs in artifact["immutableReferences"].values():
        for ref in refs:
            pos=ref["start"]
            if ref["length"]!=32:
                raise ValueError("immutable encoding")
            words.add(bytes(normalized[pos:pos+32]))
            normalized[pos:pos+32]=bytes(32)
    if normalized.hex()!=artifact["runtime"] or words!={encode(["address"],[a]) for a in (FACTORY,FEE)}:
        raise ValueError("runtime/roles")
    word=lambda a: "0x"+format(int(a,16),"064x")
    states={OWNER:{"balance":hex(4*10**18)},MODULE:{"code":runtime},
            EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                  "stateDiff":{word("0"):word(OWNER),word("1"):word(WETH)}}}
    if any(reader.one("eth_getCode",[a,reader.tag])!="0x" for a in states):
        raise ValueError("occupied simulation role")
    deadline=int(reader.header["timestamp"],16)+300
    book={v["pool"]:v["tokens"] for v in original["directory"]["pairs"]}
    payments=[]
    for i,row in enumerate(original["trials"]):
        leg=row["paid"]
        pool=row["pool"]
        src,dst=leg["input"],leg["output"]
        amount=int(leg["amount"])
        net=int(leg["received"])
        cash=next(a for a in book[pool] if a in (WETH,*STABLES))
        token=next(a for a in book[pool] if a!=cash)
        funded=amount if src==cash else int(original["trials"][i-1]["paid"]["amount"])
        prep=[tx(WETH,"deposit()",value=funded)] if cash==WETH else [
            tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
               ["uint256","address[]","address","uint256"],[funded,[WETH,cash],OWNER,deadline],value=10**18)]
        if src!=cash:
            prep.extend([approve(cash,FEE,funded),tx(FEE,SWAP,ARGS,[funded,amount,[cash,token],OWNER,deadline])])
        for mode,target in (("wallet",MODULE),("v5",EXEC)):
            def order(floor,end):
                if mode=="wallet":
                    return tx(MODULE,DIRECT,["address"]*3+["uint256"]*3,[pool,src,dst,amount,floor,end])
                step=(17,MODULE,src,dst,10000,encode(["address"],[pool]))
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],
                          [src,amount,dst,floor,end,[step]])
            marks=[balance(t,w) for w in (OWNER,pool,MODULE) for t in (src,dst)]
            marks.append(tx(src,"allowance(address,address)",["address","address"],[MODULE,FEE]))
            fund=prep+[approve(src,target,amount)]
            trade=order(net,deadline)
            ops=fund+marks+[order(net+1,deadline),order(net,0),trade]+marks
            rows=simulate_block(None,reader.tag,states,ops,read_rpc=lambda _,m,p:reader.one(m,p))
            before=rows[len(fund):len(fund)+len(marks)]
            high,expired,paid=rows[len(fund)+len(marks):len(fund)+len(marks)+3]
            after=rows[-len(marks):]
            if any(v["status"]!="0x1" for v in rows[:len(fund)]+before+after) or [v["status"] for v in (high,expired,paid)]!=["0x0","0x0","0x1"]:
                raise ValueError(("fund or settlement",mode,pool,src,amount,paid.get("error")))
            delta=[int(b["returnData"],16)-int(a["returnData"],16) for a,b in zip(before,after,strict=True)]
            if delta!=[-amount,net,*map(int,leg["pool_delta"]),0,0,0] or token_flow(paid["logs"],src,OWNER)!=-amount or token_flow(paid["logs"],dst,OWNER)!=net:
                raise ValueError(("settlement disagreement",pool,delta))
            payments.append(dict(pool=pool,mode=mode,source=src,target=dst,spent=str(amount),received=str(net),
                                 gas=int(paid["gasUsed"],16),approval_gas=int(rows[len(fund)-1]["gasUsed"],16),
                                 failed_minimum=high.get("error"),expired=expired.get("error"),calldata=trade["data"],logs=paid["logs"]))
    return dict(selector=reader.tag,root=reader.header["stateRoot"],owner=OWNER,module=MODULE,executor=EXEC,
                payments=payments,manifest=dict(chain=1,factory=FACTORY,fee_router=FEE,creation=artifact["creation"],
                creation_sha256=sha256(bytes.fromhex(creation[2:])).hexdigest(),runtime_sha256=sha256(bytes.fromhex(runtime[2:])).hexdigest(),runtime="0x"+keccak(hexstr=runtime).hex()))


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact",type=Path)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    args.output.write_text(json.dumps(capture(json.loads(args.artifact.read_bytes())),separators=(",",":")),encoding="utf8")

