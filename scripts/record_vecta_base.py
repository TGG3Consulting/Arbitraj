"""Перечисляет все поколения пула VectaSwap по nonce/событиям и проверяет физические выплаты на Base."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_native_v3_directory import CanonicalReads, directory as pools
from record_fraxswap_swaps import bound_runtime
from record_registered_v3_module import ADAPTER
from record_base_slipstream_executor import EXEC, RUNTIME, STEP, calldata, rpc
from record_hello_wallets import BASE_ENDPOINTS, BASE_WETH, DATA, HOLDERS, QPARAM
from record_spectra_base import AT, BLOCK, call, many, read

FACTORY="0xe84fa17c2ea423e0dea3c986ee3e1560d684bb2d"
REFERENCE="0x4c36388be6f416a29c8d8eee81c771ce6be14b18"
USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
ORDER="(address,address,uint24,address,uint256,uint256,uint256,uint160)"
ROLES={"factory":0,"manager":3,"router":4,"quoter":5,"tick_lens":6}
NAMES=["UniswapV3Factory","NonfungiblePositionManager","SwapRouter","QuoterV2","TickLens"]


def source(a):
    url=f"https://sourcify.dev/server/v2/contract/8453/{a}"
    r=requests.get(url,params={"fields":"sources,runtimeBytecode,creationBytecode,compilation,deployment,abi"},timeout=25)
    r.raise_for_status();v=r.json()
    if v["runtimeMatch"] not in ("match","exact_match") or v["runtimeBytecode"]["onchainBytecode"].lower()!=read("eth_getCode",[a,AT]):raise ValueError("несовпадение первичного runtime")
    return v


def hash_bytes(code):return "0x"+keccak(code).hex()


def directory():
    info=source(FACTORY);deploy=info["deployment"];sender=deploy["deployer"].lower()
    header=read("eth_getBlockByNumber",[hex(BLOCK),False])
    if header["hash"]!=AT["blockHash"]:raise ValueError("другой канон")
    d=dict(block=BLOCK,selector=AT,state_root=header["stateRoot"])
    n=int(read("eth_getTransactionCount",[FACTORY,AT]),16)
    markets=pools(CanonicalReads(d),d,FACTORY,FACTORY,int(deploy["blockNumber"]),n-1)
    ref=source(REFERENCE);ir=ref["runtimeBytecode"]["immutableReferences"]
    def strip(code):
        b=bytearray(code)
        for refs in ir.values():
            for item in refs:b[item["start"]:item["start"]+item["length"]]=bytes(item["length"])
        return bytes(b)
    reference=strip(bytes.fromhex(ref["runtimeBytecode"]["onchainBytecode"][2:]))
    init=hash_bytes(bytes.fromhex(ref["creationBytecode"]["onchainBytecode"][2:]))
    roles={}
    for (role,i),name in zip(ROLES.items(),NAMES,strict=True):
        address="0x"+keccak(b"\xd6\x94"+bytes.fromhex(sender[2:])+bytes([i or 128]))[-20:].hex()
        s=info if role=="factory" else source(address)
        if s["compilation"]["name"]!=name:raise ValueError("чужая роль")
        if role in ("manager","router","quoter") and many([call(address,"factory()","address"),call(address,"WETH9()","address")])!=[FACTORY,BASE_WETH]:raise ValueError("роли factory/WETH")
        bytecode=bytes.fromhex(s["runtimeBytecode"]["onchainBytecode"][2:])
        roles[role]=dict(address=address,code=dict(bytes=len(bytecode),keccak256=hash_bytes(bytecode)),source=f"https://sourcify.dev/server/v2/contract/8453/{address}",tx=s["deployment"]["transactionHash"])
    for m in markets:
        p=m["pool"];a,b=m["tokens"];fee=m["fee"]
        code=bytes.fromhex(read("eth_getCode",[p,AT])[2:]); salt=keccak(encode(["address","address","uint24"],[a,b,fee]))
        if strip(code)!=reference or "0x"+keccak(b"\xff"+bytes.fromhex(FACTORY[2:])+salt+bytes.fromhex(init[2:]))[-20:].hex()!=p:raise ValueError("runtime/init отличается от Uniswap V3")
        actual=many([call(p,s,t) for s,t in (("factory()","address"),("token0()","address"),("token1()","address"),("fee()","uint24"),("tickSpacing()","int24"),("liquidity()","uint128"))])
        if actual[:5]!=[FACTORY,a,b,fee,m["spacing"]]:raise ValueError("несовпадение адресов/ставки")
        m.update(liquidity=str(actual[-1]),runtime=hash_bytes(code))
    return dict(d,markets=markets,roles=roles,nonce=n,init_hash=init,reference=REFERENCE,template_hash=hash_bytes(reference),immutable_slots=ir)


def wallets(d,url,artifact=None):
    head=rpc(url,"eth_getBlockByHash",[AT["blockHash"],False]);end=int(head["timestamp"],16)+300
    if head["stateRoot"]!=d["state_root"] or rpc(url,"eth_getCode",[EXEC,AT])!="0x":raise ValueError("другой архив")
    module=None
    if artifact:
        if rpc(url,"eth_getCode",[ADAPTER,AT])!="0x":raise ValueError("постановочный адрес уже занят")
        module=bound_runtime(artifact,FACTORY,url,AT)
    router=d["roles"]["router"]["address"];quoter=d["roles"]["quoter"]["address"];markets=[]
    for m in d["markets"]:
        x=USDC if USDC in m["tokens"] else BASE_WETH;y=next(t for t in m["tokens"] if t!=x);owner=HOLDERS["base"][x]
        if rpc(url,"eth_getCode",[owner,AT])!="0x":raise ValueError("контрольный плательщик не EOA")
        def tx(to,sig,types=(),args=()):return dict(to=to,data=calldata(sig,types,args),gas="0xa00000",**{"from":owner})
        def simulate(calls,mode):
            block=dict(calls=calls)
            if mode=="v5":
                word=lambda n:"0x"+format(n,"064x")
                block["stateOverrides"]={EXEC:dict(code="0x"+RUNTIME.read_text().strip().removeprefix("0x"),stateDiff={word(0):word(int(owner,16)),word(1):word(int(BASE_WETH,16))})}
            if mode=="module":block["stateOverrides"]={ADAPTER:dict(code="0x"+module)}
            r,=rpc(url,"eth_simulateV1",[dict(blockStateCalls=[block],validation=False),AT])
            if r["parentHash"]!=AT["blockHash"] or len(r["calls"])!=len(calls):raise ValueError("другая высота или неполный набор")
            return r["calls"]
        def leg(mode,prefix,a,b,qty):
            spender={"v5":EXEC,"router":router,"module":ADAPTER}[mode]
            approval=tx(a,"approve(address,uint256)",["address","uint256"],[spender,qty])
            balances=[tx(t,"balanceOf(address)",["address"],[who]) for who in (owner,ADAPTER if mode=="module" else EXEC) for t in (a,b)]
            q=tx(quoter,"quoteExactInputSingle("+QPARAM+")",[QPARAM],[(a,b,qty,m["fee"],0)])
            base=prefix+[approval,*balances,q];front=simulate(base,mode)
            if any(r["status"]!="0x1" for r in front):return None,dict(stage="quote",calls=base,results=front)
            out=decode(["uint256","uint160","uint32","uint256"],bytes.fromhex(front[-1]["returnData"][2:]))[0]
            if not out:return None,dict(stage="zero",calls=base,results=front)
            def order(minimum,expiry):
                if mode=="module":return tx(ADAPTER,"execute(address,address,address,uint256,uint256,uint256)",["address","address","address","uint256","uint256","uint256"],[m["pool"],a,b,qty,minimum,expiry])
                if mode=="router":return tx(router,"exactInputSingle("+ORDER+")",[ORDER],[(a,b,m["fee"],owner,expiry,qty,minimum,0)])
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[a,qty,b,minimum,expiry,[(1,m["pool"],a,b,10000,b"")]])
            exact=order(out,end);calls=base+[order(out+1,end),order(out,0),exact,*balances];r=simulate(calls,mode)
            if [v["status"] for v in r[-7:-5]]!=["0x0","0x0"]:raise ValueError("мин/срок")
            if r[-5]["status"]!="0x1":return None,dict(stage="payment",calls=calls,results=r)
            before=[int(v["returnData"],16) for v in front[-5:-1]];after=[int(v["returnData"],16) for v in r[-4:]]
            paid=before[0]-after[0];net=after[1]-before[1]
            if not 0<paid<=qty or net!=out or before[2:]!=after[2:]:raise ValueError("платёж кошелька")
            return prefix+[approval,exact],dict(stage="paid",input=str(qty),paid=str(paid),net=str(net),full=paid==qty,gas=int(r[-5]["gasUsed"],16),calls=calls,results=r)
        cases=[]
        for qty in ([1000,10**6] if x==USDC else [10**12,10**14]):
            for mode in (("router","v5","module") if module else ("router","v5")):
                prefix,buy=leg(mode,[],x,y,qty)
                sell=leg(mode,prefix,y,x,int(buy["net"]))[1] if prefix and buy["full"] else None
                cases.append(dict(mode=mode,input=str(qty),buy=buy,sell=sell))
        markets.append(dict(pool=m["pool"],cash=x,payer=owner,cases=cases))
    return dict(selector=AT,state_root=d["state_root"],markets=markets,module_hash=hash_bytes(bytes.fromhex(module)) if module else None)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("action",choices=("directory","wallets"));ap.add_argument("output",type=Path);ap.add_argument("--module",type=Path);a=ap.parse_args()
    if a.action=="directory":v=directory()
    else:
        d=json.loads((DATA/"vecta_base_directory.json").read_bytes())
        for url in BASE_ENDPOINTS:
            try:v=wallets(d,url,json.loads(a.module.read_bytes()) if a.module else None);break
            except (ValueError,requests.RequestException) as exc:
                if url==BASE_ENDPOINTS[-1]:raise
    raw=(json.dumps(v,indent=2)+"\n").encode();a.output.write_bytes(gzip.compress(raw,mtime=0) if a.output.suffix==".gz" else raw)
