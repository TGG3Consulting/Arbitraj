"""Replay maker orders on Rubicon Classic Base, on their original canonical book."""
import argparse
import json
from collections import defaultdict
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT, OWNER, calldata, simulate_block
from record_bitswap_ethereum import balance, tx
from record_woo_base import HOLDERS

MARKET = "0x9a5215e96e1185d4e6002c95c3cc0ab6eead354f"
ROUTER = "0x929675f6a6ac12d7cc3463be1df7221ca35b8a00"
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"
MODULE = "0x" + keccak(text="Rubicon Classic isolated wallet on Base")[-20:].hex()
OFFER = ["uint256", "address", "uint256", "address"]
ACTION = "(uint256,uint128)[]"


def capture(url, artifact):
    snap = json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    at = snap["selector"]
    def rpc(method, params):
        response = requests.post(url, json=dict(jsonrpc="2.0", id=1, method=method, params=params), timeout=35)
        response.raise_for_status()
        reply = response.json()
        if "error" in reply:
            raise RuntimeError(str(reply["error"])[:240])
        return reply["result"]
    def view(to, signature, types=(), args=(), returns=("uint256",)):
        result = rpc("eth_call",[dict(to=to,data=calldata(signature,types,args)),at])
        return decode(returns,bytes.fromhex(result[2:]))
    head = rpc("eth_getBlockByHash", [at["blockHash"], False])
    if int(rpc("eth_chainId",[]),16)!=8453 or head["hash"]!=at["blockHash"] or rpc("eth_getCode",[MODULE,at])!="0x":
        raise ValueError("canonical parent / simulation address")
    slot = hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"),"big")-1)
    def role(proxy):
        impl="0x"+rpc("eth_getStorageAt",[proxy,slot,at])[-40:]
        roles={}
        for a in (proxy,impl):
            code=rpc("eth_getCode",[a,at])
            if code=="0x":raise ValueError("missing role")
            roles[a]="0x"+keccak(hexstr=code).hex()
        return dict(proxy=proxy,implementation=impl,hashes=roles)
    market,router=role(MARKET),role(ROUTER)
    if market["implementation"]!="0x78f8ac461b4602083d652410f74a2b6f2b69323a":
        raise ValueError("different market implementation")
    runtime=rpc("eth_call",[{"from":OWNER,"data":"0x"+artifact["creation"]+encode(["address"],[MARKET]).hex()},at])
    expected=bytearray.fromhex(artifact["runtime"])
    bound=bytes.fromhex(runtime[2:])
    values=set()
    for refs in artifact["immutableReferences"].values():
        observed={bound[ref["start"]:ref["start"]+32] for ref in refs if ref["length"]==32}
        if len(observed)!=1 or any(ref["length"]!=32 for ref in refs):raise ValueError("inconsistent immutable")
        value,=observed
        values.add(value)
        for ref in refs:expected[ref["start"]:ref["start"]+32]=value
    if values!={encode(["address"],[MARKET]),bytes.fromhex(market["hashes"][MARKET][2:])}:
        raise ValueError("unexpected market identity")
    if runtime!="0x"+expected.hex():raise ValueError("deployed code mismatch")
    total,=view(MARKET,"last_offer_id()")
    fees=[view(MARKET,sig)[0] for sig in ("getFeeBPS()","makerFee()")]
    offers=[]
    for first in range(1,total+1,96):
        ids=list(range(first,min(total+1,first+96)))
        calls=[(MARKET,False,bytes.fromhex(calldata("getOffer(uint256)",["uint256"],[i])[2:])) for i in ids]
        found,=view(MULTICALL,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[calls],["(bool,bytes)[]"])
        for i,(ok,data) in zip(ids,found,strict=True):
            if not ok:raise ValueError("incomplete market enumeration")
            quantity,target,price,source=decode(OFFER,data)
            if quantity:offers.append(dict(id=i,quantity=quantity,target=target,price=price,source=source))
    books=defaultdict(dict)
    for o in offers:books[(o["target"],o["source"])][o["id"]]=o
    controls=[]
    for (target,source),rows in books.items():
        order,=view(MARKET,"getBestOffer(address,address)",["address","address"],[target,source])
        reverse,=view(MARKET,"getBestOffer(address,address)",["address","address"],[source,target])
        ranked=[]
        while order:
            if order in ranked or order not in rows:raise ValueError("bad offer chain")
            ranked.append(order)
            order,=view(MARKET,"getWorseOffer(uint256)",["uint256"],[order])
        if set(ranked)!=set(rows):raise ValueError("orphaned offers")
        wallet=HOLDERS[source]
        for divisor in (2,1):
            actions=[(i,rows[i]["quantity"]//divisor) for i in ranked]
            dues=[q*rows[i]["price"]//rows[i]["quantity"] for i,q in actions]
            debits=sum(x+sum(x*f//100_000 for f in fees) for x in dues)
            budget=debits+1
            nominal=sum(q for _,q in actions)
            def op(a):return dict(a,**{"from":wallet})
            approve=op(tx(source,"approve(address,uint256)",["address","uint256"],[MODULE,budget]))
            before=[op(balance(t,w)) for w in (wallet,MODULE) for t in (source,target)]
            def trade(minimum,deadline,maximum=budget):
                return op(tx(MODULE,"execute(address,address,(uint256,uint128)[],uint256,uint256,uint256)",
                    ["address","address",ACTION,"uint256","uint256","uint256"],[source,target,actions,maximum,minimum,deadline]))
            def attempt(minimum,deadline,maximum=budget):
                return simulate_block(url,at,{MODULE:{"code":runtime}},[approve,*before,trade(minimum,deadline,maximum),*before],read_rpc=lambda _,m,p:rpc(m,p))
            end=int(head["timestamp"],16)+300
            trial=attempt(1,end)
            if any(r["status"]!="0x1" for r in trial[:5]+trial[-4:]):raise ValueError("allowance/balance")
            result=trial[-5]
            if result["status"]!="0x1":
                if [r["returnData"] for r in trial[1:5]]!=[r["returnData"] for r in trial[-4:]]:raise ValueError("revert debited wallet")
                controls.append(dict(source=source,target=target,reverse=reverse,actions=actions,error=result.get("error")))
                continue
            old,new=([int(x["returnData"],16) for x in side] for side in (trial[1:5],trial[-4:]))
            spent,received=old[0]-new[0],new[1]-old[1]
            if spent!=debits or not 0 < received <= nominal or old[2:]!=new[2:]:raise ValueError("wallet/fee calculation")
            quote=op(tx(MODULE,"quote(address,address,(uint256,uint128)[],uint256)",
                        ["address","address",ACTION,"uint256"],[source,target,actions,budget]))
            quoted,=simulate_block(url,at,{MODULE:{"code":runtime}},[quote],read_rpc=lambda _,m,p:rpc(m,p))
            if quoted["status"]!="0x1" or decode(["uint256","uint256"],bytes.fromhex(quoted["returnData"][2:]))!=(debits,nominal):
                raise ValueError("official fee and order quote")
            for limit,deadline,maximum in ((received+1,end,budget),(received,0,budget),(received,end,debits-1)):
                rejected=attempt(limit,deadline,maximum)
                if rejected[-5]["status"]!="0x0" or [r["returnData"] for r in rejected[1:5]]!=[r["returnData"] for r in rejected[-4:]]:
                    raise ValueError("wallet guard failed")
            controls.append(dict(source=source,target=target,wallet=wallet,reverse=reverse,actions=actions,spent=spent,
                received=received,nominal=nominal,deadline=end,budget=budget,gas=int(result["gasUsed"],16),logs=result["logs"]))
    for o in offers:
        for key in ("quantity","price"):
            o[key]=str(o[key])
    for row in controls:
        row["actions"]=[[i,str(q)] for i,q in row["actions"]]
        for key in ("spent","received","nominal","budget"):
            if key in row:row[key]=str(row[key])
    return dict(selector=at,state_root=head["stateRoot"],market=market,router=router,read_offers=total,active=offers,fees=fees,
                module=MODULE,module_hash="0x"+keccak(hexstr=runtime).hex(),controls=controls)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("artifact",type=Path)
    cli.add_argument("output",type=Path)
    args=cli.parse_args()
    for url in ("https://base-rpc.publicnode.com","https://mainnet.base.org","https://base.gateway.tenderly.co"):
        try:
            captured=capture(url,json.loads(args.artifact.read_bytes()))
            args.output.write_text(json.dumps(captured,separators=(",",":")),encoding="utf8")
            break
        except (RuntimeError,requests.RequestException) as exc:
            print(url,str(exc)[:220],flush=True)
    else:
        raise RuntimeError("archive unavailable")
