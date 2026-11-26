"""Memebox: два плательщика swapFrom, доступные резервы и чистая выплата на одном блоке Base."""
import argparse
import json
from pathlib import Path
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, STEP, rpc, simulate_block
from record_fraxswap_swaps import bound_runtime
from record_memebox_base import FACTORY, WETH, tx
from record_oasisswap_v5 import allowance, balance, scalar, word

ADAPTER="0x"+keccak(text="isolated Arbitraj Memebox module")[-20:].hex()


def capture(market, artifact, endpoint):
    tag=market["selector"]
    head=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if int(rpc(endpoint,"eth_chainId",[]),16)!=8453 or head["hash"]!=tag["blockHash"]:
        raise ValueError("unexpected Base parent")
    deadline=int(head["timestamp"],16)+120
    runtime=bound_runtime(artifact,FACTORY,endpoint,tag)
    v5=RUNTIME.read_text().strip().removeprefix("0x")
    env={OWNER:{"balance":hex(2*10**18)},ADAPTER:{"code":"0x"+runtime},
         EXEC:{"code":"0x"+v5,"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    def run(calls):
        return simulate_block(endpoint,tag,env,calls)
    def leg(prefix,mode,pool,snapshot,a,b):
        amount=int(snapshot["input"]);net=int(snapshot["net"]);gross=int(snapshot["gross"])
        target=EXEC if mode=="v5" else ADAPTER
        approve=tx(a,"approve(address,uint256)",["address","uint256"],[target,amount])
        inputs=[balance(a,OWNER),balance(b,OWNER),balance(a,EXEC),balance(b,EXEC),
                balance(a,ADAPTER),balance(b,ADAPTER),allowance(a,OWNER,target)]
        read=tx(ADAPTER,"getAmountOut(address,address,uint256)",["address","address","uint256"],[a,b,int(snapshot["pair_input"])])
        def trade(floor):
            if mode=="v5":
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                   ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,amount,b,floor,deadline,[(29,ADAPTER,a,b,10000,b"")]])
            return tx(ADAPTER,"execute(address,address,address,uint256,uint256,uint256)",
                      ["address","address","address","uint256","uint256","uint256"],[pool,a,b,amount,floor,deadline])
        prefix=prefix+[approve]
        calls=prefix+inputs+[read,trade(net+1)]+inputs+[trade(net)]+inputs
        res=run(calls);i=len(prefix);n=len(inputs)
        if any(r["status"]!="0x1" for r in res[:i]):raise ValueError("funding, prior leg or approval failed")
        before=list(map(scalar,res[i:i+n]));q,rejected=res[i+n:i+n+2]
        unchanged=list(map(scalar,res[i+n+2:i+n+2+n]));done=res[-n-1];after=list(map(scalar,res[-n:]))
        if (scalar(q),rejected["status"],unchanged,scalar(done),before[0]-after[0],after[1]-before[1])!=(gross,"0x0",before,net,amount,net):
            raise ValueError("bound module quote, wallet debit, receipt or floor differs")
        if any(before[2:6]+after[2:6]):raise ValueError("module or executor holds tokens")
        return dict(input=a,output=b,amount=str(amount),paid=str(net),gross=str(gross),gas=int(done["gasUsed"],16),
                    approval_gas=int(res[i-1]["gasUsed"],16),above=rejected.get("error"),balances_before=list(map(str,before)),
                    balances_after=list(map(str,after))),prefix+[trade(net)]
    cases=[]
    bypool={m["pool"]:m for m in market["markets"]}
    for p in market["cases"]:
        pool=p["pool"];token=next(t for t in bypool[pool]["tokens"] if t!=WETH)
        qty=int(p["buy"]["input"])
        for mode in ("module","v5"):
            buy,prefix=leg([tx(WETH,"deposit()",value=qty)],mode,pool,p["buy"],WETH,token)
            sell,_=leg(prefix,mode,pool,p["sell"],token,WETH)
            cases.append(dict(mode=mode,pool=pool,buy=buy,sell=sell))
    return dict(chain=8453,selector=tag,factory=FACTORY,adapter=ADAPTER,
                module_hash="0x"+keccak(bytes.fromhex(runtime)).hex(),v5_hash="0x"+keccak(bytes.fromhex(v5)).hex(),cases=cases)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    for key in ("market","module","output"):parser.add_argument(key,type=Path)
    a=parser.parse_args()
    j=capture(json.loads(a.market.read_bytes()),json.loads(a.module.read_bytes()),"https://mainnet.base.org")
    a.output.write_text(json.dumps(j,indent=2)+"\n",encoding="utf8")
