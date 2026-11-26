"""Повтор собственного выпуска и погашения с ограничениями кошелька на развёрнутом Bond и локально собранном модуле."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, simulate_block
from record_mint_club import Archive, BOND, TOKEN, leg, unpack


MODULE="0x"+keccak(text="Arbitraj isolated Mint Club receipt module")[-20:].hex()
SIG="execute(address,address,bool,uint256,uint256,uint256,uint256)"
ARGS=["address","address","bool","uint256","uint256","uint256","uint256"]


def bound(arc,art):
    clone=keccak(bytes.fromhex("363d3d373d3d3d363d73"+TOKEN+"5af43d82803e903d91602b57fd5bf3"))
    kw=encode(["address","address"],[BOND,"0x"+TOKEN]).hex()
    runtime=unpack(arc.rpc("eth_call",[{"from":OWNER,"data":"0x"+art["creation"]+kw},arc.tag]))
    expected=bytearray.fromhex(art["runtime"])
    fixed=set()
    for ranges in art["immutableReferences"].values():
        for row in ranges:
            start,n=row["start"],row["length"]
            x=runtime[start:start+n]
            if n!=32 or x not in (clone,encode(["address"],[BOND])):raise ValueError("unknown bound value")
            fixed.add(x)
            expected[start:start+n]=x
    if len(fixed)!=2 or bytes(expected)!=runtime or arc.code(MODULE):
        raise ValueError("constructor binding or deployment address differs")
    return "0x"+runtime.hex()


def record(arc,source,artifact):
    runtime=bound(arc,artifact)
    if source["selector"]!=arc.tag or arc.header["stateRoot"]!=source["state_root"]:raise ValueError("different market parent")
    expiry=int(arc.header["timestamp"],16)+120
    reserve=arc.weth
    env={OWNER:{"balance":hex(10**20)},MODULE:{"code":runtime}}
    bal=lambda a,w:leg(a,"balanceOf(address)",["address"],[w])
    allow=lambda a,owner,to:leg(a,"allowance(address,address)",["address","address"],[owner,to])
    approve=lambda a,n:leg(a,"approve(address,uint256)",["address","uint256"],[MODULE,n])
    scalar=lambda row:int(row["returnData"],16)
    def run(seq):return simulate_block(None,arc.tag,env,seq,read_rpc=lambda _,m,p:arc.rpc(m,p))
    def trial(m,t):
        token=m["token"];qty=int(t["size"]);cost=int(t["mint_cost"]);refund=int(t["redeem"])
        def swap(mint,max_debit,min_credit,deadline=expiry):
            return leg(MODULE,SIG,ARGS,[token,reserve,mint,qty,max_debit,min_credit,deadline])
        reads=[bal(reserve,OWNER),bal(token,OWNER),bal(reserve,BOND),bal(reserve,MODULE),bal(token,MODULE),
               allow(reserve,MODULE,BOND),allow(token,MODULE,BOND),allow(reserve,OWNER,MODULE),allow(token,OWNER,MODULE)]
        def leg_of(prefix,mint,paid,received):
            src=reserve if mint else token
            # Исходная allowance отсутствует; строгие стоимость, минимум и срок проверяются после разрешения.
            seq=prefix+[swap(mint,paid,received),approve(src,paid)]+reads
            rejects=[swap(mint,paid-1,received),swap(mint,paid,received+1),swap(mint,paid,received,1)]
            seq+=rejects+reads+[swap(mint,paid,received)]+reads
            res=run(seq); k=len(prefix);n=len(reads)
            bad={k,k+2+n,k+3+n,k+4+n}
            if any((v["status"]=="0x0")!=(i in bad) for i,v in enumerate(res)):
                raise ValueError(("module control",token,qty,mint,[(i,v.get("error")) for i,v in enumerate(res) if v["status"]!="0x1"]))
            old=list(map(scalar,res[k+2:k+2+n]));rolled=list(map(scalar,res[k+5+n:k+5+2*n]))
            done=res[-n-1];after=list(map(scalar,res[-n:]))
            if old!=rolled or any(old[3:7]+after[3:7]) or after[7:]!=[0,0]:
                raise ValueError("rollback, custody or allowances differ")
            delta=tuple(a-b for a,b in zip(after[:3],old[:3],strict=True))
            if delta!=((-paid,received,paid) if mint else (received,-paid,-received)) or decode(["uint256","uint256"],unpack(done["returnData"]))!=(paid,received):
                raise ValueError("sender or reserve cash mismatch")
            return dict(before=list(map(str,old)),after=list(map(str,after)),gas=int(done["gasUsed"],16),
                        approve_gas=int(res[k+1]["gasUsed"],16),refused=[res[i]["error"] for i in sorted(bad)],logs=done["logs"]),prefix+[approve(src,paid),swap(mint,paid,received)]
        buy,prefix=leg_of([leg(reserve,"deposit()",value=cost)],True,cost,qty)
        sell,_=leg_of(prefix,False,qty,refund)
        return dict(token=token,qty=str(qty),cost=str(cost),refund=str(refund),buy=buy,sell=sell)
    return dict(chain=arc.chain,selector=arc.tag,state_root=arc.header["stateRoot"],bond=BOND,module=MODULE,
                module_hash="0x"+keccak(hexstr=runtime).hex(),reserve=reserve,owner=OWNER,
                cases=[trial(m,t) for m in source["markets"] for t in m["trials"]])


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("network",choices=["base","ethereum"])
    for name in ("market","artifact","out"):p.add_argument(name,type=Path)
    a=p.parse_args()
    result=record(Archive(a.network),json.loads(a.market.read_bytes()),json.loads(a.artifact.read_bytes()))
    a.out.write_text(json.dumps(result,separators=(",",":"))+"\n",encoding="utf8")
