"""Проверяет реестр OmniFactory, конструктор UniversalRouter, выплаты Permit2 и V5 на Base."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, EXEC, RUNTIME, calldata, rpc
from record_diamondswap_base import WETH
from record_ethereum_v2_factories import registry, raw, digest
from record_solidly_v3 import HOLDERS
from record_v2_chain_pools import measure, swap

FACTORY="0x7d9d51267f7e9e6b46a48e0a75c0086f46777087"
ROUTER="0x423fae8ee58833f283e8845479bd705514110ec3"
ENDPOINTS=["https://base.gateway.tenderly.co","https://mainnet.base.org","https://base-rpc.publicnode.com"]


def role(c,addr,chain_id=8453):
    url=f"https://sourcify.dev/server/v2/contract/{chain_id}/{addr}?fields=all"
    response=requests.get(url,timeout=25);response.raise_for_status();data=response.json()
    live=rpc(c.endpoint,"eth_getCode",[addr,c.selector])
    if not data.get("runtimeMatch") or live.lower()!=data["runtimeBytecode"]["onchainBytecode"].lower():
        raise ValueError("Sourcify runtime differs from selected block")
    return data,dict(address=addr,runtime=digest(raw(live)),source=url,bytes=len(raw(live)))


def wallet(c,head,row,permit,sample):
    payer=HOLDERS["base"][WETH]
    token=next(t for t in row["tokens"] if t!=WETH);deadline=int(head["timestamp"],16)+180
    def tx(to,sig,types=(),args=()):
        return dict(to=to,data=calldata(sig,types,args),**{"from":payer,"gas":"0x500000"})
    def run(calls,mode):
        frame={"calls":calls}
        if mode=="v5":
            slot=lambda n:"0x"+format(n,"064x")
            frame["stateOverrides"]={EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                    "stateDiff":{slot(0):slot(int(payer,16)),slot(1):slot(int(WETH,16))}}}
        found,=rpc(c.endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},c.selector])
        if found["parentHash"]!=c.selector["blockHash"]:raise ValueError("canonical parent changed")
        return found["calls"]
    def leg(prefix,mode,x,y,amount,out):
        if mode=="native":
            approve=[tx(x,"approve(address,uint256)",["address","uint256"],[permit,amount]),
                     tx(permit,"approve(address,address,uint160,uint48)",["address","address","uint160","uint48"],[x,ROUTER,amount,deadline])]
            def order(minimum,when):
                payload=encode(["address","uint256","uint256","address[]","bool"],[payer,amount,minimum,[x,y],True])
                check=encode(["address","address","uint256"],[payer,y,base_balance+minimum])
                return tx(ROUTER,"execute(bytes,bytes[],uint256)",["bytes","bytes[]","uint256"],[bytes([8,14]),[payload,check],when])
        else:
            approve=[tx(x,"approve(address,uint256)",["address","uint256"],[EXEC,amount])]
            def order(minimum,when):
                return {**swap(row["pool"],x,y,amount,minimum,when,2500),"from":payer}
        before=[tx(z,"balanceOf(address)",["address"],[payer]) for z in (x,y)]
        prefix=prefix+approve+before
        base_balance=int(run(prefix,mode)[-1]["returnData"],16) if mode=="native" else 0
        calls=prefix+[order(out+1,deadline),order(out,0),order(out,deadline)]+before
        result=run(calls,mode);a,b,c0,p,q=[result[i] for i in range(len(prefix),len(calls))]
        if a["status"]!="0x0" or b["status"]!="0x0" or c0["status"]!="0x1" or any(r["status"]!="0x1" for r in result[len(prefix)-4:len(prefix)] if mode=="native"):
            raise ValueError((mode,x,amount,c0.get("error"),"boundaries",a["status"],b["status"]))
        integer=lambda r:decode(["uint256"],raw(r["returnData"]))[0]
        old=[integer(v) for v in result[len(prefix)-2:len(prefix)]]
        if old[0]-integer(p)!=amount or integer(q)-old[1]!=out:
            raise ValueError("wallet settled less than the exact order")
        return calls,dict(input=str(amount),output=str(out),calls=calls,results=result)
    cases=[]
    for trial in sample["cases"]:
        amount,bought,paid=(int(trial[k]) for k in ("in","bought","returned"));case={}
        for mode in ["native","v5"]:
            prefix,buy=leg([],mode,WETH,token,amount,bought);_,sell=leg(prefix,mode,token,WETH,bought,paid)
            case[mode]=dict(buy=buy,sell=sell)
        cases.append(case)
    return dict(owner=payer,permit2=permit,cases=cases)


def capture(endpoint):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes());c=Capture(dict(snap,endpoint=endpoint))
    chain=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())["base"]
    head=rpc(endpoint,"eth_getBlockByHash",[chain["hash"],False])
    if chain["hash"]!=c.selector["blockHash"] or head["hash"]!=chain["hash"]:raise ValueError("wrong Base parent")
    factory,fac=role(c,FACTORY);router,rt=role(c,ROUTER)
    src=factory["sources"]["src/OmniPair.sol"]["content"]
    if "amount0In.mul(25)" not in src or "amount1In.mul(25)" not in src or "10000**2" not in src:raise ValueError("changed pair invariant")
    words=raw(router["creationBytecode"]["transformationValues"]["constructorArguments"])
    permit,wrapped,origin=decode(["address"]*3,words[:96]);init="0x"+words[160:192].hex()
    stated,=c.one(FACTORY,"INIT_CODE_PAIR_HASH()",returns=["bytes32"])
    if wrapped!=WETH or origin!=FACTORY or init!="0x"+stated.hex():raise ValueError("wrong router constructor")
    _,per=role(c,permit)
    refs=router["runtimeBytecode"]["immutableReferences"]
    code=raw(router["runtimeBytecode"]["onchainBytecode"])
    bindings={}
    for key,value in (("permit2",permit),("factory",FACTORY),("init",init)):
        word=raw(value).rjust(32,b"\0")
        indexes=next((a for a in refs.values() if all(code[i["start"]:i["start"]+i["length"]]==word for i in a)),None)
        if not indexes: raise ValueError("отсутствует неизменяемая привязка Omni")
        bindings[key]=[x["start"] for x in indexes]
    markets=registry(c,FACTORY,init);examples=[]
    for row in sorted((r for r in markets if WETH in r["tokens"]),key=lambda r:-int(r["reserves"][r["tokens"].index(WETH)])):
        code=rpc(endpoint,"eth_getCode",[row["pool"],c.selector])
        entry=dict(network="base",dex="omni-exchange-v2-base",pool=row["pool"],tokens=row["tokens"],factory=FACTORY,creation_code_hash=init,
                   runtime_hash=digest(raw(code)),fee_fraction=[10000,25],verify_pair_boundary=True,invariant_error="OMNI: K")
        try:
            measured=measure(entry,chain,endpoint=endpoint);direct=wallet(c,head,row,permit,measured)
        except (ValueError,KeyError) as exc:
            examples.append(dict(pool=row["pool"],reason=str(exc)[:240]));continue
        examples.append(dict(pool=row["pool"],measured=measured,wallet=direct));break
    if not any("wallet" in r for r in examples):raise ValueError(str(examples[:3])[:560])
    return dict(id="omni-exchange-v2-base",chain_id=8453,selector=c.selector,height=snap["block"],state_root=head["stateRoot"],
                factory=fac,router=rt,permit2=per,bindings=bindings,init_hash=init,markets=markets,examples=examples)

if __name__=="__main__":
    args=argparse.ArgumentParser(description=__doc__);args.add_argument("output",type=Path);args=args.parse_args()
    for ep in ENDPOINTS:
        try:
            d=capture(ep);args.output.write_text(json.dumps(d,indent=2)+"\n",encoding="utf8",newline="\n");break
        except (ValueError,KeyError,OSError) as error:print(ep,str(error)[:450],flush=True)
    else:raise ValueError("не подтверждена единая выплата Omni")
