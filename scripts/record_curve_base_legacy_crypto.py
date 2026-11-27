"""Сопоставляет пять каталогов Curve Base и проверяет платежи первой фабрики CryptoSwap с одного блока."""
import argparse
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from fork_fee_rpc import Reader
from record_bitswap_ethereum import balance, tx
from record_base_slipstream_executor import ROOT, OWNER, EXEC, RUNTIME, STEP, simulate_block
from record_pendle_registry import call, read_many

FACTORIES = {
    "stable": "0x3093f9b57a428f3eb6285a589cb35bea6e78c336",
    "crypto": "0x5ef72230578b3e399e6c6f4f6360edf95e83bbfd",
    "twocrypto": "0xc9fe0c63af9a39402e8a5514f9c43af0322b665f",
    "tricrypto": "0xa5961898870943c68037f6848d2d866ed2016bcb",
    "stable_ng": "0xd2002373543ce3527023c75e7518c274a51ce712",
}
WETH = "0x4200000000000000000000000000000000000006"
SOURCE = "https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/curve/index.ts#L340-L357"
VIEW = ["A","gamma","D","mid_fee","out_fee","fee_gamma","future_A_gamma_time","price_scale"]


def directory(r):
    at = r.tag
    views = lambda rows: read_many(rows,at,r.one)
    counts = views([call(a,"pool_count()",["uint256"]) for a in FACTORIES.values()])
    lists = {}
    for (name,a),(count,) in zip(FACTORIES.items(),counts,strict=True):
        lists[name] = [v[0] for v in views([call(a,"pool_list(uint256)",["address"],["uint256"],[i]) for i in range(count)])]
        if len(set(lists[name]))!=count: raise ValueError("duplicate factory registration")
    a=FACTORIES["crypto"]; addresses=lists["crypto"]
    listed=views([call(a,"get_coins(address)",["address[2]"],["address"],[p]) for p in addresses])
    code=r.many(("eth_getCode",[p,at]) for p in addresses)
    roles={}; pools=[]
    for p,c,(pair,) in zip(addresses,code,listed,strict=True):
        if len(c)!=92 or not c.startswith("0x363d3d373d3d3d363d73") or not c.endswith("5af43d82803e903d91602b57fd5bf3"):raise ValueError("unexpected pool runtime")
        template="0x"+c[22:62]
        if template not in roles:
            impl=r.one("eth_getCode",[template,at])
            roles[template]={"runtime":"0x"+keccak(hexstr=impl).hex(),"bytes":(len(impl)-2)//2,"suffix":impl[-64:]}
        q=[call(p,n+"()",["uint256"]) for n in VIEW]
        for i in range(2):
            q.extend([call(p,"coins(uint256)",["address"],["uint256"],[i]),call(p,"balances(uint256)",["uint256"],["uint256"],[i])])
        outputs=views(q)
        if [outputs[8][0],outputs[10][0]]!=list(pair):raise ValueError("pool currencies differ from factory")
        raw={"0x"+wire.hex():{"result":"0x"+encode(abi,res).hex()} for (_,wire,abi),res in zip(q,outputs,strict=True) if res}
        balances=[outputs[9][0],outputs[11][0]]
        extra=views([call(t,"balanceOf(address)",["uint256"],["address"],[p]) for t in pair]+[call(t,"decimals()",["uint8"]) for t in pair])
        ladder=[(i,max(1,balances[i]*ppm//10**6)) for i in range(2) for ppm in [1,10,1000,10000]] if min(balances)>2 else []
        out=views([call(p,"get_dy(uint256,uint256,uint256)",["uint256"],["uint256"]*3,[i,1-i,amount]) for i,amount in ladder])
        pools.append(dict(pool=p,template=template,coins=pair,balances=list(map(str,balances)),erc20_balances=[str(v[0]) if v else None for v in extra[:2]],
                          native_balance=str(int(r.one("eth_getBalance",[p,at]),16)) if WETH in pair else None,
                          decimals=[v[0] if v else None for v in extra[2:]],calls=raw,code=c,
                          get_dy=[dict(i=i,j=1-i,dx=str(amount),get_dy={"result":"0x"+encode(["uint256"],result).hex()} if result else {"error":"reverted"}) for (i,amount),result in zip(ladder,out,strict=True)]))
    return {"source":SOURCE,"selector":at,"timestamp":int(r.header["timestamp"],16),"factories":FACTORIES,"registry":lists,"templates":roles,"pools":pools}


def wallets(r,j):
    runtime="0x"+RUNTIME.read_text().strip().removeprefix("0x")
    if r.many(("eth_getCode",[a,r.tag]) for a in [EXEC,OWNER])!=["0x","0x"]: raise ValueError("simulation addresses occupied")
    word=lambda n:"0x"+format(n,"064x")
    env={OWNER:{"balance":hex(10**18)},EXEC:{"code":runtime,"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    sim=lambda ops: simulate_block("",r.tag,env,ops,lambda _,method,params:r.one(method,params))
    output=[]
    for p in j["pools"]:
        if WETH not in p["coins"] or min(map(int,p["balances"]))<=2:continue
        i=p["coins"].index(WETH); other=p["coins"][1-i]; pool=p["pool"]
        for qty in [10**12,10**14]:
            for mode in ["pool","v5"]:
                case={"pool":pool,"amount":str(qty),"wire":mode};output.append(case)
                prefix=[tx(WETH,"deposit()",value=qty)]
                for name,k,amount in [("forward",i,qty),("reverse",1-i,0)]:
                    if name=="reverse": amount=int(case["forward"]["credit"])
                    src,dst=p["coins"][k],p["coins"][1-k]
                    approved=prefix+[tx(src,"approve(address,uint256)",["address","uint256"],[pool if mode=="pool" else EXEC,amount])]
                    reads=[balance(t,OWNER) for t in (src,dst)]
                    def probe(floor,deadline):
                        if mode=="pool":
                            op=tx(pool,"exchange(uint256,uint256,uint256,uint256)",["uint256"]*4,[k,1-k,amount,floor])
                        else:
                            step=(9,pool,src,dst,10000,encode(["uint256","uint256"],[k,1-k]))
                            op=tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[src,amount,dst,floor,deadline,[step]])
                        rows=sim(approved+reads+[op]+reads); idx=len(approved)+2
                        if any(row["status"]!="0x1" for n,row in enumerate(rows) if n!=idx):raise ValueError(("funds/balance",pool,name))
                        vals=lambda segment:[int(row["returnData"],16) for row in segment]
                        return op,rows[idx],vals(rows[idx-2:idx]),vals(rows[idx+1:]),int(rows[len(approved)-1]["gasUsed"],16)
                    deadline=j["timestamp"]+180
                    _,result,b,a,_=probe(1,deadline)
                    if result["status"]!="0x1" or a[1]<=b[1]:
                        case[name]={"stage":"unavailable","error":result.get("error")};break
                    credit=a[1]-b[1]
                    op,result,b,a,approval=probe(credit,deadline)
                    if result["status"]!="0x1" or b[0]-a[0]!=amount or a[1]-b[1]!=credit:raise ValueError(("wallet payment",pool))
                    _,bad,pre,post,_=probe(credit+1,deadline)
                    strict_reverted=bad["status"]=="0x0"
                    if strict_reverted and pre!=post:raise ValueError(("rollback",pool))
                    if mode=="v5" and not strict_reverted:raise ValueError(("wallet minimum",pool))
                    if mode=="v5":
                        _,expired,pre,post,_=probe(credit,1)
                        if expired["status"]!="0x0" or pre!=post:raise ValueError(("expiry",pool))
                    case[name]=dict(stage="paid",input=src,output=dst,debit=str(amount),credit=str(credit),before=list(map(str,b)),after=list(map(str,a)),
                                    swap_gas=int(result["gasUsed"],16),approval_gas=approval,transaction=op,logs=result["logs"],strict_reverted=strict_reverted,strict=bad.get("error"))
                    prefix=approved+[op]
    return output


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);args=ap.parse_args()
    head=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    reader=Reader("base",head);record=directory(reader);record["wallets"]=wallets(reader,record)
    args.output.write_bytes(json.dumps(record,separators=(",",":")).encode())
