"""Проверяет обе физические выплаты через опубликованный Universal Router и V5 на каноническом родителе Base."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
from record_base_launch_hooks import DATA, KEY
from record_base_slipstream_executor import EXEC,RUNTIME,STEP,calldata,rpc
from record_woo_base import WETH,HOLDERS
from record_omni_v2_base import ENDPOINTS
from record_balancer_deployments import ROOT

ZERO="0x"+"00"*20
MC="0xca11bde05977b3631167028862be2a173976ca11"
USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
PAYER=dict(HOLDERS,**{ZERO:HOLDERS[WETH],"0xb20000000000000000000078ee7ce2fe4908108c":"0x2a1b16a4df05fe96afedd50dd9aa6df2f515deeb"})
word=lambda n:"0x"+format(n,"064x")


def replay(group,endpoint,directory=None,wrapped_module=None):
    if directory is None:
        with gzip.open(DATA/"base_launch_hooks.json.gz") as f:directory=json.load(f)
    d=directory
    holders=json.loads((DATA/"o1_launch_holders.json").read_bytes())
    if holders["selector"]!=d["selector"]:raise ValueError("держатели на другой высоте")
    payers={**PAYER,**{t:s["payer"] for t,s in holders["tokens"].items()}}
    at=d["selector"];header=rpc(endpoint,"eth_getBlockByHash",[at["blockHash"],False]);end=int(header["timestamp"],16)+120
    if d["state_root"]!=header["stateRoot"]:raise ValueError("изменилось состояние родителя")
    cfg=next(p["settings"] for p in json.loads((ROOT/"config/first_run_dex.json").read_bytes()) if p["chain_id"]==8453 and p["settings"]["dex.kind"]=="uniswap_v4")
    flux=d.get("flux")
    if flux:cfg=flux
    key_type="(address,address,address,address,uint24,bytes32)" if flux else KEY
    router,permit,manager,quoter=[cfg["dex."+k].lower() for k in ("universal_router","permit2","pool_manager","quoter")]
    if manager!=d["manager"] or rpc(endpoint,"eth_getCode",[EXEC,at])!="0x":raise ValueError("другой manager или занятый тестовый адрес")
    runtime="0x"+RUNTIME.read_text().strip().removeprefix("0x")
    def simulate(payer,calls,mode):
        frame=dict(calls=calls)
        if mode in ("wrapped","wrapped_v5"):
            address,code=wrapped_module
            frame["stateOverrides"]={address:dict(code=code)}
        if mode in ("v5","wrapped_v5"):
            frame.setdefault("stateOverrides",{})[EXEC]=dict(code=runtime,stateDiff={word(0):word(int(payer,16)),word(1):word(int(WETH,16))})
        b,=rpc(endpoint,"eth_simulateV1",[dict(blockStateCalls=[frame],validation=False),at])
        if b["parentHash"]!=at["blockHash"] or len(calls)!=len(b["calls"]):raise ValueError("другая база симуляции")
        return b["calls"]
    def leg(k,owner,prefix,mode,x,y,qty):
        forward=x==k[0]
        if mode in ("wrapped","wrapped_v5"):
            x=WETH if x==ZERO else x
            y=WETH if y==ZERO else y
        def tx(to,sig,ts,vs):return dict(to=to,data=calldata(sig,ts,vs),gas="0xe00000",**{"from":owner})
        def bal(asset):
            return tx(MC,"getEthBalance(address)",["address"],[owner]) if asset==ZERO else tx(asset,"balanceOf(address)",["address"],[owner])
        approvals=[]
        if x!=ZERO:
            spender=EXEC if mode in ("v5","wrapped_v5") else (wrapped_module[0] if mode=="wrapped" else permit)
            allowance=tx(x,"allowance(address,address)",["address","address"],[owner,spender])
            checked=simulate(owner,prefix+[allowance],mode)[-1]
            if checked["status"]!="0x1":return None,dict(input=str(qty),stage="allowance",call=allowance,result=checked)
            if int(checked["returnData"],16)<qty:
                approvals.append(tx(x,"approve(address,uint256)",["address","uint256"],[spender,qty]))
            if mode=="router":approvals.append(tx(permit,"approve(address,address,uint160,uint48)",["address","address","uint160","uint48"],[x,router,qty,end]))
        def trade(minimum,deadline):
            if mode=="wrapped":
                return tx(wrapped_module[0],"execute(bytes32,address,address,uint256,uint256,uint256)",["bytes32","address","address","uint256","uint256","uint256"],[keccak(encode([KEY],[k])),x,y,qty,minimum,deadline])
            if mode in ("v5","wrapped_v5"):
                step=(17,wrapped_module[0],x,y,10000,keccak(encode([KEY],[k]))) if mode=="wrapped_v5" else (
                    20 if flux else 19,cfg["dex.vault"] if flux else manager,x,y,10000,encode([key_type],[k]))
                return tx(EXEC,f"execute(address,uint256,address,uint256,uint256,{STEP}[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[x,qty,y,minimum,deadline,[step]])
            if flux:
                params=encode([f"({key_type},bool,uint128,uint128,bytes)"],[(k,forward,qty,minimum,b"")])
            else:
                params=encode([f"({KEY},bool,uint128,uint128,uint256,bytes)"],[(k,forward,qty,minimum,0,b"")])
            body=encode(["bytes","bytes[]"],[bytes([6,12,15]),[params,encode(["address","uint256"],[x,qty]),encode(["address","uint256"],[y,minimum])]])
            args=tx(router,"execute(bytes,bytes[],uint256)",["bytes","bytes[]","uint256"],[b"\x10",[body],deadline])
            if x==ZERO:args["value"]=hex(qty)
            return args
        exact=f"({key_type},bool,uint128,bytes)"
        quote=tx(quoter,f"quoteExactInputSingle({exact})",[exact],[(k,forward,qty,b"")])
        base=prefix+approvals+[bal(x),bal(y),quote];front=simulate(owner,base,mode)
        if any(v["status"]!="0x1" for v in front):return None,dict(input=str(qty),stage="quote",calls=base,results=front)
        gross=int(front[-1]["returnData"][2:66],16)
        if not gross:return None,dict(input=str(qty),stage="zero",calls=base,results=front)
        call=trade(gross,end)
        calls=base+[trade(gross+1,end),trade(gross,0),call,bal(x),bal(y)]
        r=simulate(owner,calls,mode);paid=int(r[-8]["returnData"],16)-int(r[-2]["returnData"],16);received=int(r[-1]["returnData"],16)-int(r[-7]["returnData"],16)
        good=[v["status"] for v in r[-5:-2]]==["0x0","0x0","0x1"] and paid==qty and received==gross
        if r[-5]["status"]=="0x1" or r[-4]["status"]=="0x1":raise ValueError("строгий минимум либо срок не сработал")
        return (prefix+approvals+[call] if good else None),dict(input=str(qty),quoted=str(gross),paid=str(paid),net=str(received),gas=int(r[-3]["gasUsed"],16),stage="accepted" if good else "settlement",calls=calls,results=r)
    markets=[]
    for row in [r for r in d["rows"] if r["group"]==group and "key" in r]:
        key=row["key"]
        if wrapped_module and ZERO not in key[:2]:continue
        cash=row["terms"].get("quote") or next((t for t in key[:2] if t in (WETH,USDC)),None)
        item=dict(pool=row["id"],key=key,cash=cash,cases=[]);markets.append(item)
        owner=payers.get(WETH if wrapped_module and cash==ZERO else cash)
        if not owner:item["reason"]="нет подтверждённого независимого держателя котируемого актива";continue
        if rpc(endpoint,"eth_getCode",[owner,at])!="0x":raise ValueError("пробный держатель не является EOA")
        item["payer"]=owner;asset=next(t for t in key[:2] if t!=cash)
        sizes=[10**12,10**14] if cash in (WETH,ZERO) else ([10**6,10**7] if cash==USDC else [100,1000])
        if group=="bankr-index-base":
            sizes += [10**15,10**16] if cash==WETH else [10**8,10**9]
        if cash=="0xacfe6019ed1a7dc6f7b508c02d1b04ec88cc21bf":
            sizes=[10**13,10**15]
        wire_key=(*key[:-1],bytes.fromhex(key[-1][2:])) if flux else key
        for qty in sizes:
            modes=["wrapped","wrapped_v5"] if wrapped_module else (["router"] if ZERO in key[:2] else ["router","v5"])
            for mode in modes:
                prefix,buy=leg(wire_key,owner,[],mode,cash,asset,qty)
                sell=leg(wire_key,owner,prefix,mode,asset,cash,int(buy["net"]))[1] if prefix else None
                item["cases"].append(dict(mode=mode,amount=str(qty),buy=buy,sell=sell))
    return dict(group=group,height=d["height"],selector=at,state_root=header["stateRoot"],roles=cfg,v5_hash="0x"+keccak(bytes.fromhex(runtime[2:])).hex(),markets=markets)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("group",choices=("bankr","bankr-index-base","o1-launchpad"));ap.add_argument("output",type=Path)
    a=ap.parse_args()
    for ep in ENDPOINTS:
        try:
            data=replay(a.group,ep);a.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8");break
        except (ValueError,OSError) as exc:print(ep,str(exc)[:350],flush=True)
    else:raise ValueError("не удалось собрать архив выбранного протокола")
