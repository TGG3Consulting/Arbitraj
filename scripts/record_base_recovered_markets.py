"""Перепроверяет Base-размещения, найденные после временного отказа каталога, на одном хеше."""
import argparse
import json
from pathlib import Path
from eth_abi import decode,encode
from eth_utils import keccak

from record_base_launch_bindings import read
from record_base_launch_hooks import INIT,KEY

def record(folder):
    snap=json.loads((folder/"roles.json").read_bytes())
    at=snap["selector"];calls=[]
    def call(a,sig,returns,args=(),values=()):
        data="0x"+(keccak(text=sig)[:4]+encode(args,values)).hex()
        raw=read("eth_call",[dict(to=a,data=data),at]);calls.append(dict(to=a,data=data,result=raw,name=sig))
        return decode(returns,bytes.fromhex(raw[2:]))
    def contract(addr):
        code=bytes.fromhex(read("eth_getCode",[addr,at])[2:])
        if not code:raise ValueError("нет кода "+addr)
        return dict(address=addr,bytes=len(code),hash="0x"+keccak(code).hex())
    # Сравниваем счета и документацию с getter фабрики и действующим роутером в этой сети.
    rf="0xdf97b25a935eb72378e0c2d4dc15955ece612b49";rr="0x8e20a1534a204de569e9d62d410c19795b81df70"
    rq="0xd4b5dd638d09ab7b6eb4ec3490f02a96d0dd4100"
    if call(rr,"factory()",["address"])[0]!=rf:raise ValueError("Ryze: другая фабрика роутера")
    pooln=call(rf,"getPoolCount()",["uint256"])[0]
    known={r["address"].lower():r for r in json.loads((folder/"ryze-pools.txt").read_bytes())["pools"]}
    ryze=[]
    for i in range(pooln):
        a=call(rf,"pools(uint256)",["address"],["uint256"],[i])[0]
        tokens=[call(a,"getPoolAsset(uint256)",["address"],["uint256"],[n])[0] for n in (0,1)]
        ryze.append(dict(pool=contract(a),tokens=tokens,official=known.get(a)))
    third="0xefcb993e113ea8197c17c6f4959495929be0b68e";tm=[]
    registry=call(third,"defaultPluginFactory()",["address"])[0];deployer=call(third,"poolDeployer()",["address"])[0]
    for row in json.loads((folder/"thirdfy-gecko.json").read_bytes())["rows"]:
        a=row["attributes"]["address"].lower();x=call(a,"token0()",["address"])[0];y=call(a,"token1()",["address"])[0]
        factory=call(a,"factory()",["address"])[0]
        if factory!=third or call(third,"poolByPair(address,address)",["address"],["address","address"],[x,y])[0]!=a:raise ValueError("Thirdfy: нет членства")
        pl=call(a,"plugin()",["address"])[0]
        state=call(a,"globalState()",["uint160","int24","uint16","uint8","uint16","bool"])
        tm.append(dict(pool=contract(a),tokens=[x,y],plugin=contract(pl),state=list(state),liquidity=str(call(a,"liquidity()",["uint128"])[0])))
    ref=json.loads((folder/"baseline-pair.txt").read_bytes())["pair"]
    birth=read("eth_getTransactionReceipt",[ref["createdAtTxnId"]])
    block=read("eth_getBlockByNumber",[hex(ref["createdAtBlockNumber"]),False])
    if birth["blockHash"]!=block["hash"] or birth["status"]!="0x1":raise ValueError("Baseline: неканоническое развёртывание")
    l,=[l for l in birth["logs"] if l["topics"][0]==INIT and l["address"]==snap["manager"]]
    x,y=["0x"+w[-40:] for w in l["topics"][2:]];fee,step,hook,_,_=decode(["uint24","int24","address","uint160","int24"],bytes.fromhex(l["data"][2:]))
    key=[x,y,fee,step,hook];pid="0x"+keccak(encode([KEY],[key])).hex()
    if {x,y}!={ref["asset0Id"].lower(),ref["asset1Id"].lower()} or pid!=l["topics"][1]:raise ValueError("Baseline: другой рынок")
    return dict(height=snap["height"],selector=at,state_root=snap["state_root"],calls=calls,
                ryze=dict(factory=contract(rf),router=contract(rr),queries=contract(rq),count=pooln,markets=ryze),
                thirdfy=dict(factory=contract(third),deployer=contract(deployer),plugin_factory=contract(registry),markets=tm),
                baseline=dict(index=ref,initialize=l,pool_id=pid,key=key,hook=contract(hook),btoken=contract(x),receipt=birth))

if __name__=="__main__":
    args=argparse.ArgumentParser(description=__doc__);args.add_argument("source",type=Path);args.add_argument("output",type=Path);a=args.parse_args()
    a.output.write_text(json.dumps(record(a.source),indent=2)+"\n",encoding="utf8")
