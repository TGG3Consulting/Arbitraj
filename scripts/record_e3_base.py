"""Реестр E3, фактические котировки пар и расчёт из собственного кошелька через Router/LB V5."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import OWNER, EXEC, RUNTIME, STEP, call, calldata, rpc
from record_eulerswap_directory import DATA, Reader

FACTORY="0x8597db3ba8de6baadeda8cba4dac653e24a0e57b"
ROUTER="0xb9a64ab6b91f5c7a78c2360cff759de8a8a450d5"
WETH="0x4200000000000000000000000000000000000006"
USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
UNI="0x2626664c2603336e57b271c5c0b26f421741e481"
PATH="(uint256[],uint8[],address[])"

def tx(to,signature,types,values,value=0):
    return call(to,calldata(signature,types,values),value,gas=7_000_000)

def ok(row):
    return row["status"]=="0x1"

def result(row):
    assert ok(row),row.get("error")
    return bytes.fromhex(row["returnData"][2:])

def archive(capacity=False):
    r=Reader("base")
    selector=r.tag
    parent=rpc(r.url,"eth_getBlockByHash",[selector["blockHash"],False])
    stamp=int(parent["timestamp"],16)+2
    deadline=stamp+180
    assert all(r.rpc("eth_getCode",[a,selector])=="0x" for a in (OWNER,EXEC))
    count,impl,linked=r.many([(FACTORY,"getNumberOfLBPairs()",(),"uint256"),(FACTORY,"getLBPairImplementation()",(),"address"),(ROUTER,"getFactory()",(),"address")])
    assert linked==FACTORY and count>0
    addresses=r.many([(FACTORY,"getLBPairAtIndex(uint256)",(i,),"address") for i in range(count)])
    word=lambda x:"0x"+format(x,"064x")
    runtime="0x"+RUNTIME.read_text().strip().removeprefix("0x")
    state={OWNER:{"balance":hex((13 if capacity else 5)*10**18)},EXEC:{"code":runtime,"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    def run(calls):
        frame={"blockOverrides":{"time":hex(stamp)},"stateOverrides":state,"calls":calls}
        block,=rpc(r.url,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},selector])
        assert block["parentHash"]==selector["blockHash"]
        return block["calls"]
    def funds(token):
        if token==WETH:
            return tx(WETH,"deposit()",[],[],10**17)
        params="(address,address,uint24,address,uint256,uint256,uint160)"
        for fee in (500,3000,100,10000):
            swap=tx(UNI,"exactInputSingle("+params+")",[params],[(WETH,token,fee,OWNER,10**17,1,0)],10**17)
            if ok(run([swap])[0]):return swap
        raise ValueError("Нет самостоятельного финансирования токена "+token)
    markets=[];payments=[];stopped=[];caps=[]
    for pool in addresses:
        x,y,step,orig,active,reserves,fees=r.many([(pool,s,(),t) for s,t in [
            ("getTokenX()","address"),("getTokenY()","address"),("getBinStep()","uint16"),("getFactory()","address"),("getActiveId()","uint24"),
            ("getReserves()","uint128,uint128"),("getProtocolFees()","uint128,uint128")]])
        info,physical_x,physical_y=r.many([(FACTORY,"getLBPairInformation(address,address,uint256)",(x,y,step),"uint16,address,bool,bool"),
            (x,"balanceOf(address)",(pool,),"uint256"),(y,"balanceOf(address)",(pool,),"uint256")])
        assert info[:2]==[step,pool] and orig==FACTORY
        code=bytes.fromhex(r.rpc("eth_getCode",[pool,selector])[2:])
        assert bytes.fromhex(impl[2:]) in code
        probes=[]
        for is_y,token in ((True,x),(False,y)):
            decimals,=r.many([(token,"decimals()",(),"uint8")]);unit=10**decimals
            for amount in [1,max(1,unit//10_000),unit,10**27]:
                q,alt=r.many([(pool,"getSwapOut(uint128,bool)",(amount,is_y),"uint128,uint128,uint128"),
                    (ROUTER,"getSwapOut(address,uint128,bool)",(pool,amount,is_y),"uint128,uint128,uint128")])
                assert q==alt
                probes.append(dict(for_y=is_y,quantity=amount,result=q))
        markets.append(dict(pool=pool,x=x,y=y,bin_step=step,registration=info,active=active,reserves=reserves,protocol=fees,
                            physical=[physical_x,physical_y],runtime="0x"+code.hex(),probes=probes))
        if not all(reserves):
            if not any(reserves):assert all(not p["result"] or p["result"][1]==0 for p in probes)
            continue
        if capacity:
            from record_e3_capacity import examine
            caps.extend(examine(r,pool,(x,y),step,ROUTER,stamp,run))
            continue
        first=WETH if WETH in (x,y) else USDC
        assert first in (x,y)
        other=y if x==first else x
        mint=funds(first)
        for mode in ("official","v5"):
            spender=ROUTER if mode=="official" else EXEC
            for size in ([10**12,10**13] if first==WETH else [10**6,10**7]):
                prefix=[mint];amount=size
                for src,dst in ((first,other),(other,first)):
                    quote=tx(pool,"getSwapOut(uint128,bool)",["uint128","bool"],[amount,src==x])
                    before=run(prefix+[quote])
                    assert all(map(ok,before))
                    left,net,fee=decode(["uint128"]*3,result(before[-1]))
                    assert left==0 and net>0
                    approve=tx(src,"approve(address,uint256)",["address","uint256"],[spender,amount])
                    balances=[tx(t,"balanceOf(address)",["address"],[owner]) for owner in (OWNER,EXEC) for t in (src,dst)]
                    def order(minimum,end):
                        if mode=="v5":
                            types=["address","uint256","address","uint256","uint256",STEP+"[]"]
                            return tx(EXEC,"execute("+",".join(types)+")",types,[src,amount,dst,minimum,end,[(18,pool,src,dst,10000,b"")]])
                        types=["uint256","uint256",PATH,"address","uint256"]
                        return tx(ROUTER,"swapExactTokensForTokensSupportingFeeOnTransferTokens("+",".join(types)+")",types,
                                  [amount,minimum,([step],[2],[src,dst]),OWNER,end])
                    normal=order(net,deadline)
                    rows=run(prefix+[approve]+balances+[order(net+1,deadline),order(net,stamp-1),normal]+balances)
                    i=len(prefix)
                    assert all(map(ok,rows[:i+5]))
                    prior=[int.from_bytes(result(z)) for z in rows[i+1:i+5]]
                    strict,expired,swap=rows[i+5:i+8]
                    assert not ok(strict) and not ok(expired)
                    post=[int.from_bytes(result(z)) for z in rows[-4:]]
                    if not ok(swap):
                        assert prior==post
                        stopped.append(dict(pool=pool,mode=mode,input=src,output=dst,amount=amount,quote=net,error=swap.get("error")))
                        break
                    assert prior[0]-post[0]==amount and post[1]-prior[1]==net and prior[2:]==post[2:]
                    payments.append(dict(pool=pool,mode=mode,input=src,output=dst,paid=amount,net=net,fee=fee,
                        before=prior,after=post,approval_gas=int(rows[i]["gasUsed"],16),gas=int(swap["gasUsed"],16),logs=swap["logs"],
                        order=normal,strict=strict.get("error"),expired=expired.get("error")))
                    prefix.extend([approve,normal]);amount=net
    assert (caps if capacity else payments) and len(markets)==count
    return dict(selector=selector,stamp=stamp,factory=FACTORY,factory_hash=r.code_hash(FACTORY),implementation=impl,
                impl_hash=r.code_hash(impl),router=ROUTER,router_hash=r.code_hash(ROUTER),payer=OWNER,executor=EXEC,
                markets=markets,payments=payments,refusals=stopped,capacity=caps)

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output",type=Path)
    ap.add_argument("--capacity",action="store_true")
    args=ap.parse_args()
    args.output.write_text(json.dumps(archive(args.capacity),separators=(",",":")),encoding="utf8")

