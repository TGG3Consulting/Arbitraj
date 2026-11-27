"""Authenticate all Ethereum Solidly V2 factory entries and the actual fallback targets at the archived head."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture,posts
from record_base_classic_directory import optional
from record_base_slipstream_executor import ROOT,rpc
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

DOC="https://docs.solidly.com/resources/contract-addresses.html"
FACTORY="0x777de5fe8117caaa7b44f396e93a401cf5c9d4d6"
ROUTER="0x77784f96c936042a3adb1dd29c91a55eb2a4219f"
ADDR=lambda b:decode(["address"],b)[0]


def record(ep):
    template=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c=Capture(dict(template,endpoint=ep));tag=c.selector
    h=rpc(ep,"eth_getBlockByHash",[tag["blockHash"],False])
    if h["stateRoot"]!=ROOT_HASH or int(h["number"],16)!=template["block"] or rpc(ep,"eth_chainId",[])!="0x1":raise ValueError("incorrect archive")
    doc=requests.get(DOC,timeout=16);doc.raise_for_status()
    if any(addr not in doc.text.lower() for addr in [FACTORY,ROUTER]):raise ValueError("official roles changed")
    role_calls=[(a,fn+"()",[],[]) for a,fn in [(FACTORY,'logicAddress'),(FACTORY,'childSubImplementationAddress'),(FACTORY,'implementationAddress'),(FACTORY,'childInterfaceAddress'),(ROUTER,'logicAddress'),(ROUTER,'implementationAddress')]]
    fac_logic,pair_logic,fac_interface,pair_interface,router_logic,router_interface=map(ADDR,posts(c,role_calls))
    count,=c.one(FACTORY,"allPairsLength()",returns=["uint256"])
    init,=c.one(FACTORY,"pairCodeHash()",returns=["bytes32"])
    paused,=c.one(FACTORY,"isPaused()",returns=["bool"])
    pairs=list(map(ADDR,posts(c,[(FACTORY,"allPairs(uint256)",["uint256"],[i]) for i in range(count)])))
    if len(set(pairs))!=count:raise ValueError("duplicate pool")
    raw=posts(c,[(p,fn+"()",[],[]) for p in pairs for fn in ["metadata","factoryAddress","subImplementationAddress","implementationAddress","fees"]])
    rows=[];query=[]
    for i,p in enumerate(pairs):
        m,pf,pl,pi,fee_to=raw[i*5:i*5+5]
        dec0,dec1,r0,r1,stable,x,y,fee=decode(["uint256"]*4+["bool","address","address","uint256"],m)
        if ADDR(pf)!=FACTORY or ADDR(pl)!=pair_logic or x>=y or not dec0 or not dec1:raise ValueError("pair roles")
        salt=keccak(bytes.fromhex(x[2:]+y[2:])+bytes([int(stable)]))
        derived="0x"+keccak(b"\xff"+bytes.fromhex(FACTORY[2:])+salt+init)[-20:].hex()
        if derived!=p:raise ValueError(("CREATE2",p))
        rows.append(dict(pool=p,init="0x"+init.hex(),current_init=True,tokens=[x,y],stable=stable,scales=[str(dec0),str(dec1)],
                         reserves=[str(r0),str(r1)],fee=str(fee),fee_recipient=ADDR(fee_to),interface=ADDR(pi)))
        query.extend([(FACTORY,"getPair(address,address,bool)",["address","address","bool"],[a,b,stable]) for a,b in [(x,y),(y,x)]])
        query.extend([(FACTORY,"isPair(address)",["address"],[p]),(FACTORY,"poolFees(address)",["address"],[p])])
    checked=posts(c,query)
    physical=optional(c,[(t,"balanceOf(address)",["address"],[m["pool"]]) for m in rows for t in m["tokens"]])
    hashes={}
    def code(a):
        wire=bytes.fromhex(rpc(ep,"eth_getCode",[a,tag])[2:])
        if not wire:raise ValueError(("missing runtime",a))
        return "0x"+keccak(wire).hex()
    for i,m in enumerate(rows):
        a,b,listed,fee=checked[i*4:i*4+4]
        if ADDR(a)!=m["pool"] or ADDR(b)!=m["pool"] or not decode(["bool"],listed)[0]:raise ValueError("reverse registration")
        m["factory_fee"]=str(int.from_bytes(fee,"big"))
        m["balances"]=[str(int.from_bytes(v,"big")) if v is not None and len(v)==32 else None for v in physical[i*2:i*2+2]]
    # Every factory CREATE2 preimage is identical apart from the salt; the live runtime is also compared.
    for m in rows: m["runtime"]=code(m["pool"])
    roles={}
    for label,(addr,expected) in {"factory":(FACTORY,"SolidlyProxy"),"factory_logic":(fac_logic,"BaseV2Factory"),
                              "pair_logic":(pair_logic,"BaseV2Pair"),"pair_proxy":(pairs[0],"SolidlyChildProxy"),
                              "router":(ROUTER,"SolidlyProxy"),"router_logic":(router_logic,"BaseV2Router01")}.items():
        url="https://sourcify.dev/server/v2/contract/1/"+addr
        resp=requests.get(url,params={"fields":"compilation,runtimeBytecode.onchainBytecode"},timeout=16);resp.raise_for_status()
        j=resp.json();digest=code(addr)
        if j["compilation"]["name"]!=expected or "0x"+keccak(hexstr=j["runtimeBytecode"]["onchainBytecode"]).hex()!=digest:
            raise ValueError(("source mismatch",label,addr))
        roles[label]=dict(address=addr,keccak256=digest,contract=expected,source=url)
    if any(m["runtime"]!=roles["pair_proxy"]["keccak256"] for m in rows): raise ValueError("unexpected pair shell")
    return dict(chain_id=1,id="solidlydex",height=template["block"],selector=tag,state_root=ROOT_HASH,doc=DOC,roles=roles,
                factory=FACTORY,router=ROUTER,factory_interface=fac_interface,router_interface=router_interface,pair_interface=pair_interface,
                init="0x"+init.hex(),factory_paused=paused,markets=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path);a=parser.parse_args();errs=[]
    for ep in ENDPOINTS:
        try:d=record(ep);break
        except Exception as e:errs.append((ep,str(e)[:350]))
    else:raise RuntimeError(errs)
    raw=json.dumps(d,separators=(",",":")).encode()
    a.output.write_bytes(gzip.compress(raw,mtime=0) if a.output.suffix==".gz" else raw)
