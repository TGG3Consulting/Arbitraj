"""Выплаты BaseStonk при самом обычном вызове SimpleSwapRouter и шаге V4 исходного V5."""
import argparse
import gzip
import json
from collections import defaultdict
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_basestonk_base import KEY, MANAGER, ROUTER, FEES
from record_e3_base import OWNER, EXEC, RUNTIME, STEP, WETH, USDC, UNI, Reader, tx, ok, result
from record_eulerswap_directory import DATA

SWAP="0x"+keccak(text="Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)").hex()


def capture():
    r=Reader("base");directory=json.loads(gzip.decompress((DATA/"basestonk_base_directory.json.gz").read_bytes()))
    src=json.loads(gzip.decompress((DATA/"basestonk_base_listed.json.gz").read_bytes()))
    assert directory["tag"]==r.tag and directory["router_hash"]==r.code_hash(ROUTER)
    h=r.rpc("eth_getBlockByHash",[r.tag["blockHash"],False]);stamp=int(h["timestamp"],16)+2
    code="0x"+RUNTIME.read_text().strip().removeprefix("0x");word=lambda v:"0x"+format(v,"064x")
    state={OWNER:{"balance":hex(2*10**18)},EXEC:{"code":code,"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    assert all(r.rpc("eth_getCode",[a,r.tag])=="0x" for a in (OWNER,EXEC))
    def run(calls):
        b,=r.rpc("eth_simulateV1",[dict(blockStateCalls=[dict(stateOverrides=state,blockOverrides=dict(time=hex(stamp)),calls=calls)],validation=False),r.tag])
        assert b["parentHash"]==r.tag["blockHash"];return b["calls"]
    def order(p,src,dst,amount,minimum,mode,deadline):
        key=p["key"];direction=src==key[0]
        if mode=="official":
            return tx(ROUTER,"swap("+KEY+",bool,uint256,uint256,address)",[KEY,"bool","uint256","uint256","address"],[key,direction,amount,minimum,OWNER])
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],
                  [src,amount,dst,minimum,deadline,[(19,MANAGER,src,dst,10000,encode([KEY],[key]))]])
    def seed(payer_token,amount):
        if payer_token==WETH:return tx(WETH,"deposit()",[],[],amount)
        return tx(UNI,"exactOutputSingle((address,address,uint24,address,uint256,uint256,uint160))",["(address,address,uint24,address,uint256,uint256,uint160)"],[(WETH,USDC,500,OWNER,amount,10**18,0)],10**18)
    def leg(p,a,b,qty,prefix,mode):
        spender=ROUTER if mode=="official" else EXEC
        approve=tx(a,"approve(address,uint256)",["address","uint256"],[spender,qty])
        bal=lambda t:tx(t,"balanceOf(address)",["address"],[OWNER])
        body=prefix+[approve,bal(a),bal(b)];suffix=[bal(a),bal(b)]
        def measure(minimum,deadline):
            call=order(p,a,b,qty,minimum,mode,deadline);calls=run(body+[call]+suffix)
            assert all(ok(x) for x in calls[:len(body)]) and all(ok(x) for x in calls[-2:])
            before=[int.from_bytes(result(x)) for x in calls[len(body)-2:len(body)]]
            after=[int.from_bytes(result(x)) for x in calls[-2:]]
            return calls[len(body)],before[0]-after[0],after[1]-before[1],call,int(calls[len(prefix)]["gasUsed"],16)
        normal,paid,net,call,allowance=measure(1,stamp+90)
        row=dict(id=p["id"],hook=p["hook"],mode=mode,src=a,dst=b,requested=qty,paid=paid,received=net,success=ok(normal),error=normal.get("error"))
        if not ok(normal):
            assert paid==net==0
            return row,None
        assert 0<paid<=qty and net>0
        at,debit,credit,call,_=measure(net,stamp+90)
        assert ok(at) and (debit,credit)==(paid,net)
        strict,_,_,_,_=measure(net+1,stamp+90)
        row.update(gas=int(at["gasUsed"],16),approval_gas=allowance,logs=at["logs"],strict=dict(success=ok(strict),error=strict.get("error")))
        if mode=="v5":
            expired,e_in,e_out,_,_=measure(net,stamp-1)
            assert not ok(strict) and not ok(expired) and e_in==e_out==0
            row["expired"]=expired.get("error")
        fees=[decode(["address","uint256","uint256"],bytes.fromhex(l["data"][2:])) for l in at["logs"] if l["address"]==p["hook"] and l["topics"][:2]==[FEES,p["id"]]]
        row["tax"]=fees;row["pool_swaps"]=sum(l["address"]==MANAGER and l["topics"][:2]==[SWAP,p["id"]] for l in at["logs"])
        return row,prefix+[approve,call]
    rank={t:i for i,t in enumerate(src["api_volume_order"])}
    grouped=defaultdict(list)
    for p in directory["markets"]:
        if p["pair"] in (WETH,USDC) and int(p["liquidity"])>0:grouped[p["hook"]].append(p)
    rows=[]
    for _,markets in grouped.items():
        markets.sort(key=lambda p:(rank.get(p["token"],len(rank)),-int(p["liquidity"])))
        for p in markets[:4]:
            stable=p["pair"]==USDC
            before=len(rows)
            for amount in ([10**5,10**6] if stable else [10**12,10**13]):
                funding=[seed(p["pair"],amount)]
                for mode in ("official","v5"):
                    buy,prefix=leg(p,p["pair"],p["token"],amount,funding,mode);rows.append(buy)
                    if prefix:
                        sell,_=leg(p,p["token"],p["pair"],buy["received"],prefix,mode);rows.append(sell)
            if any(x["success"] for x in rows[before:]):break
    return dict(tag=r.tag,height=r.height,timestamp=stamp,payer=OWNER,executor=EXEC,router=ROUTER,results=rows)


if __name__=="__main__":
    a=argparse.ArgumentParser(description=__doc__);a.add_argument("output",type=Path)
    a.parse_args().output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

