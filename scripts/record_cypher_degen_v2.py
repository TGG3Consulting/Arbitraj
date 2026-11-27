"""Р”РІР° РѕС‚РґРµР»СЊРЅС‹Рµ Ethereum-СЂРµРµСЃС‚СЂР° V2 Рё С„Р°РєС‚РёС‡РµСЃРєРёРµ РѕР±РјРµРЅС‹ РїРѕ С€С‚Р°С‚РЅС‹Рј entrypoints РЅР° С‚РѕРј Р¶Рµ Р±Р»РѕРєРµ."""
import argparse
import gzip
import json
import re
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_classic_directory import optional
from record_base_slipstream_executor import ROOT, OWNER, rpc
from record_diamondswap_base import direct_wallet
from record_ethereum_v2_factories import WETH, STABLES, FUNDING, digest, raw, registry
from record_ethervista_wallets import tx
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
DEPLOYMENTS = {
    "cypher-v2": ("0xcc8e4c2998395e56d06d985ba791138edf48a8d4", "0xadfd0ebc71afcd38070a9fb5cecf9a932cbcc46f"),
    "degenswap": ("0x5c515455efb90308689579993c11a84fc41229c0", "0x4bf3e2287d4ced7796bfab364c0401dfce4a4f7f"),
}

def source(c, address):
    url = "https://eth.blockscout.com/api/v2/smart-contracts/" + address
    reply = c.session.get(url, timeout=20)
    reply.raise_for_status()
    j = reply.json()
    deployed = raw(rpc(c.endpoint, "eth_getCode", [address, c.selector]))
    if not deployed or not j.get("is_verified") or j.get("is_changed_bytecode") or deployed != raw(j["deployed_bytecode"]):
        raise ValueError("verified deployment differs for " + address)
    body = "\n".join(s["source_code"] for s in [j] + j.get("additional_sources", []))
    proof = dict(address=address, runtime=digest(deployed), name=j["name"], source=url,
                 source_hash=digest(body.encode()), exact_source_match=j.get("is_fully_verified",False))
    return proof, deployed, body

def initcode(factory, router):
    matches = re.findall(r"hex[\"']([0-9a-fA-F]{64})[\"']\s*//\s*init code hash", router)
    if len(set(matches)) != 1:
        raise ValueError("ambiguous router CREATE2 code hash")
    wanted = bytes.fromhex(matches[0])
    # Solidity CODECOPY lengths and offsets are checked against the ENTIRE pair creation code.
    for i in range(len(factory)-8):
        if factory[i] != 0x61: continue
        size = int.from_bytes(factory[i+1:i+3],"big")
        if size < 2000: continue
        for j in range(i+3,i+8):
            if factory[j] != 0x61: continue
            start=int.from_bytes(factory[j+1:j+3],"big")
            if start+size<=len(factory) and keccak(factory[start:start+size])==wanted:
                return dict(hash="0x"+wanted.hex(),start=start,bytes=size)
    raise ValueError("router init hash does not identify pair creation code in factory runtime")

def capture(endpoint, key, wallets):
    snap=json.loads((DATA/"ethereum.json").read_bytes())
    c=Capture(dict(snap,endpoint=endpoint))
    head=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if rpc(endpoint,"eth_chainId",[])!="0x1" or head["hash"]!=c.selector["blockHash"] or int(head["number"],16)!=snap["block"] or head["stateRoot"]!=ROOT_HASH:
        raise ValueError("wrong archive")
    f,r=DEPLOYMENTS[key]
    factory,code,pair_source=source(c,f)
    router,_,router_source=source(c,r)
    init=initcode(code,router_source)
    if c.one(r,"factory()",returns=["address"])!=(f,) or c.one(r,"WETH()",returns=["address"])!=(WETH,):
        raise ValueError("router and factory differ")
    markets=registry(c,f,init["hash"])
    if key=="cypher-v2":
        if not all(v in pair_source for v in ("amount0In.mul(fee)", "amount1In.mul(fee)", "FEE_DENOMINATOR = 100000", "function setFee(uint16")):
            raise ValueError("unrecognized Cypher fee")
        fees=posts(c,[(m["pool"],"fee()",[],[]) for m in markets])
        for m,word in zip(markets,fees,strict=True):m["fee_numerator"]=decode(["uint16"],word)[0]
        terms=dict(method="fee()",source="pool",denominator=100000)
    else:
        if c.one(f,"INIT_CODE_PAIR_HASH()",returns=["bytes32"])[0].hex()!=init["hash"][2:]:
            raise ValueError("factory init hash differs")
        extra=posts(c,[(m["pool"],"getReserves()",[],[]) for m in markets]+[(r,"pairFeeAddress(address)",["address"],[m["pool"]]) for m in markets])
        for n,m in enumerate(markets):
            m["base_token"]=decode(["uint112","uint112","uint32","address"],extra[n])[-1]
            m["router_fee_token"]=decode(["address"],extra[len(markets)+n])[0]
        terms=dict(method="adminFee()",raw=str(c.one(r,"adminFee()",returns=["uint256"])[0]),
                   pair_swap="swap(uint256,uint256,uint256,uint256,address,bytes)")
    balance=iter(optional(c,[(t,"balanceOf(address)",["address"],[m["pool"]]) for m in markets for t in m["tokens"]]))
    for m in markets:
        m["balances"]=[str(int.from_bytes(b,"big")) if b is not None else None for b in (next(balance),next(balance))]
    directory=dict(height=snap["block"],selector=c.selector,state_root=head["stateRoot"],market=key,
                   factory=factory,router=router,init=init,fees=terms,markets=markets)
    if not wallets:return directory
    outcomes=[]
    for m in markets:
        cash=next((a for a in (WETH,*STABLES) if a in m["tokens"]),None)
        if cash is None or any(v is None or int(v)<=0 for v in m["balances"]+m["reserves"]):continue
        index=m["tokens"].index(cash)
        depth=min(int(m["balances"][index]),int(m["reserves"][index]))
        standard=10**15 if cash==WETH else 10**6
        end=int(head["timestamp"],16)+120
        def fund(size):
            return tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                      ["uint256","address[]","address","uint256"],[size,[WETH,cash],OWNER,end],10**17)
        for size in sorted({max(1,min(standard,depth//1000)),max(2,min(10*standard,depth//100))}):
            for mode in (("router","v5") if key=="cypher-v2" else ("router","bounded-router")):
                # DEGENSwap has its own six-argument pair.swap; only the published router is eligible.
                try:
                    def bounded(a,b,n,floor,deadline,_mode):
                        # The deployed setter caps adminFee at 100/10000; input-asset selection may change.
                        signed=floor+(floor-1)//99
                        return tx(r,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                                  ["uint256","uint256","address[]","address","uint256"],[n,signed,[a,b],OWNER,deadline])
                    def guaranteed(_a,b,net):
                        current=int(terms["raw"]) if b==m["router_fee_token"] else 0
                        highest_gross=net*10000//(10000-current)
                        return highest_gross-highest_gross//100
                    rounds=direct_wallet(c,m,[size],int(head["timestamp"],16),m.get("fee_numerator",0)*10,
                                         r,wrapped=WETH,modes=(mode,),audit=True,keep_partial=True,cash=cash,
                                         funding=None if cash==WETH else fund,
                                         build_action=bounded if mode=="bounded-router" else None,
                                         guaranteed_floor=guaranteed if mode=="bounded-router" else None)
                    outcomes.append(dict(pool=m["pool"],size=str(size),mode=mode,rounds=rounds))
                except ValueError as exc:
                    outcomes.append(dict(pool=m["pool"],size=str(size),mode=mode,refused=str(exc)[:300]))
    if not outcomes:raise ValueError("no independently funded test size")
    directory["outcomes"]=outcomes
    return directory

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("market",choices=DEPLOYMENTS)
    p.add_argument("output",type=Path)
    p.add_argument("--wallets",action="store_true")
    a=p.parse_args()
    for ep in ENDPOINTS:
        try: data=capture(ep,a.market,a.wallets)
        except (requests.RequestException,ValueError,RuntimeError,KeyError) as exc:
            print(ep,str(exc)[:200],flush=True)
            continue
        payload=(json.dumps(data,separators=(",",":"))+"\n").encode()
        a.output.write_bytes(gzip.compress(payload,mtime=0) if a.output.suffix==".gz" else payload)
        break
    else:raise ValueError("canonical source or archive unavailable")
