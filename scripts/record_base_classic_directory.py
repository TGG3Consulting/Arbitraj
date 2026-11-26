"""Registry, actual balances, fee method and verified bytecode for each Base Solidly V1 deployment."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT, rpc
from record_equalizer_generations import generations

# The released contract source and PairCreated receipt identify these factories. Archly publishes both generations.
DEPLOYMENTS = {
    "supernova-amm": ("0x5aef44edfc5a7edd30826c724ea12d7be15bdc30", "factory_pool", 10000),
    "baso-finance": ("0xcefbebf0b85b1638c19b01ce2a02c262f421b07d", "factory_stable", 10000),
    "infusion": ("0x2d9a3a2bd6400ee28d770c7254ca840c82faf23f", "factory_stable", 10000),
    "archly-base": ("0x12508dd9108abab2c5fd8fc6e4984e46a3cf7824", "factory_stable", 10000),
    "archly-v1-base": ("0xba06043a777652baf540ccc785edafd94ee05b37", "factory_stable", 10000),
    "hydrex-classic": ("0xc47f17c4fd96f50efd2a2448cede5c185c084bf0", "pool", 100000),
    "velocimeter-base": ("0xe21aac7f113bd5dc2389e4d8a8db854a87fd6951", "factory_address", 10000),
    "equalizer-base": ("0xed8db60acc29e14bc867a497d94ca6e3ceb5ec04", "factory_address", 10**18),
}
SLOT = "0x"+format(int.from_bytes(keccak(text="eip1967.proxy.implementation"),"big")-1,"064x")
ABI = ["uint256","uint256","uint256","uint256","bool","address","address"]
ENDPOINTS = ["https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"]
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"


def source(c, addr, chain=8453):
    url=f"https://sourcify.dev/server/v2/contract/{chain}/"+addr
    resp=requests.get(url,params={"fields":"sources,compilation,runtimeBytecode.onchainBytecode"},timeout=30)
    resp.raise_for_status()
    doc=resp.json()
    runtime=rpc(c.endpoint,"eth_getCode",[addr,c.selector])
    if runtime=="0x" or runtime.lower()!=doc["runtimeBytecode"]["onchainBytecode"].lower():
        raise ValueError("verified code differs at "+addr)
    result=dict(url=url,address=addr,contract=doc["compilation"]["fullyQualifiedName"],runtime_hash="0x"+keccak(hexstr=runtime).hex())
    return result, doc["sources"]


def optional(c, calls):
    replies=[]
    for i in range(0,len(calls),64):
        block=calls[i:i+64]
        data=[(to,True,keccak(text=sig)[:4]+encode(types,values)) for to,sig,types,values in block]
        results,=c.one(MULTICALL,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[data],["(bool,bytes)[]"])
        if len(results)!=len(block):raise ValueError("short multicall")
        replies.extend(raw if ok and len(raw)==32 else None for ok,raw in results)
    return replies


def capture(endpoint, key):
    network="ethereum" if key=="supernova-amm" else "base"
    base=json.loads((ROOT/f"tests/data/first_run_dex_2026-11-26/{network}.json").read_bytes())
    chain=base["chain_id"]
    c=Capture(dict(endpoint=endpoint,selector=base["selector"]))
    head=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"], False])
    if (int(rpc(endpoint,"eth_chainId",[]),16)!=chain or int(head["number"],16)!=base["block"] or head["hash"]!=c.selector["blockHash"]):
        raise ValueError("wrong snapshot")
    fac, source_kind, denom=DEPLOYMENTS[key]
    factory, texts=source(c,fac,chain)
    res=c.session.post(endpoint,json={"jsonrpc":"2.0","id":1,"method":"eth_getStorageAt","params":[fac,SLOT,c.selector]},timeout=20)
    res.raise_for_status()
    impl=int(res.json()["result"],16)
    if impl:
        if impl.bit_length()>160: raise ValueError("invalid EIP-1967 slot")
        factory["implementation"],texts=source(c,"0x"+format(impl,"040x"),chain)
    physical=fac
    if key=="supernova-amm":
        physical,=c.one(fac,"pairGenerator()",returns=["address"])
        if c.one(physical,"factory()",returns=["address"])[0]!=fac:
            raise ValueError("generator points to another PairFactory")
        factory["generator"],texts=source(c,physical,chain)
    contract_sources={name:"0x"+keccak(text=row["content"]).hex() for name,row in texts.items()
                      if "function _get_y(" in row["content"] and "getAmountOut(" in row["content"]}
    if not contract_sources: raise ValueError("verified implementation has no compiled pair source")
    init,=c.one(fac,"pairCodeHash()",returns=["bytes32"])
    count,=c.one(fac,"allPairsLength()",returns=["uint256"])
    pools=[decode(["address"],r)[0] for r in posts(c,[(fac,"allPairs(uint256)",["uint256"],[i]) for i in range(count)])]
    if len(set(pools))!=count: raise ValueError("duplicate factory market")
    historical=generations(endpoint,fac,pools,texts,source,SLOT) if key=="equalizer-base" else []
    templates=[init]+[bytes.fromhex(g["init"][2:]) for g in historical]
    markets=[]
    fee_calls=[]
    checks=[]
    for pool,raw in zip(pools,posts(c,[(p,"metadata()",[],[]) for p in pools]),strict=True):
        d0,d1,r0,r1,stable,a,b=decode(ABI,raw)
        packed=bytes.fromhex(a[2:]+b[2:])+bytes([stable])
        prefix=b"\xff"+bytes.fromhex(physical[2:])+keccak(packed)
        matches=[h for h in templates if "0x"+keccak(prefix+h)[-20:].hex()==pool]
        if len(matches)!=1:
            raise ValueError("no unique confirmed CREATE2 template for "+pool)
        matched=matches[0]
        if a>=b: raise ValueError(f"{key}: tokens are unsorted at {pool}")
        checks.extend([(fac,"getPair(address,address,bool)",["address","address","bool"],[a,b,stable]),
                       (a,"balanceOf(address)",["address"],[pool]),(b,"balanceOf(address)",["address"],[pool])])
        if source_kind=="factory_pool":fee=(fac,"getFee(address,bool)",["address","bool"],[pool,stable])
        elif source_kind=="factory_stable": fee=(fac,"getFee(bool)",["bool"],[stable])
        elif source_kind=="pool": fee=(pool,"getFee()",[],[])
        else: fee=(fac,"getRealFee(address)" if key=="equalizer-base" else "getFee(address)",["address"],[pool])
        fee_calls.append(fee)
        markets.append(dict(pool=pool,current_init=matched==init,init="0x"+matched.hex(),tokens=[a,b],scales=[str(d0),str(d1)],reserves=[str(r0),str(r1)],stable=stable))
    results=iter(optional(c,checks))
    for m,fee in zip(markets,optional(c,fee_calls),strict=True):
        listed,left,right=next(results),next(results),next(results)
        if not listed or decode(["address"],listed)[0]!=m["pool"]: raise ValueError("registry round trip differs")
        m["balances"]=[None if v is None else str(int.from_bytes(v,"big")) for v in (left,right)]
        m["fee"]=None if fee is None else str(int.from_bytes(fee,"big"))
    if key=="equalizer-base":
        controls=iter(posts(c,[(fac,fn,["address"],[m["pool"]]) for m in markets
                               for fn in ("feesOverrideStatic(address)","feesOverrideDynamic(address)")]))
        for m in markets:
            fixed,=decode(["uint256"],next(controls)); manager,=decode(["address"],next(controls))
            m["fixed_fee"]=str(fixed)
            m["fee_manager"]=manager
    paused,=c.one(fac,"isPaused()",returns=["bool"])
    return dict(id=key,chain_id=chain,block=base["block"],selector=c.selector,state_root=head["stateRoot"],
                factory=factory,pair_sources=contract_sources,historical=historical,init="0x"+init.hex(),fee_source=source_kind,
                fee_method=fee_calls[0][1] if fee_calls else None,denominator=str(denom),factory_paused=paused,markets=markets)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("id",choices=DEPLOYMENTS);p.add_argument("output",type=Path)
    args=p.parse_args()
    from record_ethereum_x7 import ENDPOINTS as ETH_ENDPOINTS
    for endpoint in (ETH_ENDPOINTS if args.id=="supernova-amm" else ENDPOINTS):
        try:
            obj=capture(endpoint,args.id)
            args.output.write_text(json.dumps(obj,indent=2)+"\n",encoding="utf8",newline="\n")
            break
        except (ValueError,KeyError,OSError,RuntimeError) as error:
            print(endpoint,str(error)[:300],flush=True)
    else: raise ValueError("no canonical registry result for "+args.id)
