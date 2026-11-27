"""Архив фактического списания и выплаты Topaz CL и Maverick V1 для выбранных денежных маршрутов."""
import argparse
import gzip
import json
from pathlib import Path
import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_fraxswap_swaps import bound_runtime
from record_fynd_markets import rpc as read_chain
from record_hello_wallets import BASE_ENDPOINTS,ETH_ENDPOINTS,BASE_WETH,ETH_WETH,DATA,HOLDERS
from record_base_slipstream_executor import EXEC,STEP,RUNTIME,calldata,rpc
from record_registered_v3_module import ADAPTER


def replay(d,dex,url,artifact):
    chain=d["chain"];at=d["selector"];weth=BASE_WETH if chain=="base" else ETH_WETH
    head=rpc(url,"eth_getBlockByHash",[at["blockHash"],False]);end=int(head["timestamp"],16)+300
    if head["stateRoot"]!=d["state_root"] or rpc(url,"eth_getCode",[EXEC,at])!="0x":raise ValueError("неизвестное состояние")
    mav=dex=="maverick";roles=d["roles"]
    router=roles["Router" if mav else "SwapRouter"]["address"];viewer=roles["PoolInformation" if mav else "QuoterV2"]["address"]
    module=None
    if mav:
        if rpc(url,"eth_getCode",[ADAPTER,at])!="0x":raise ValueError("занят адрес изолированной постановки")
        module=bound_runtime(artifact,roles["Factory"]["address"],url,at)
    candidates=[]
    for m in d["markets"]:
        cash=next((t for t in (weth,*HOLDERS[chain]) if t in m["tokens"]),None)
        if cash is None:continue
        # Представитель каждого проверяемого номинала выбирается по остатку той же монеты.
        amount=int(m["balances"][m["tokens"].index(cash)] if mav else m["liquidity"])
        candidates.append((cash,amount,m))
    if mav:
        selected=[]
        for cash in sorted({x[0] for x in candidates}):
            group=sorted((x for x in candidates if x[0]==cash and x[1]),key=lambda x:-x[1])
            selected.extend(group[:2])
        candidates=selected
    samples=[]
    for cash,_,m in candidates:
        other=next(t for t in m["tokens"] if t!=cash);owner=HOLDERS[chain][cash]
        if rpc(url,"eth_getCode",[owner,at])!="0x":continue
        def tx(to,sig,types=(),args=()):return dict(to=to,data=calldata(sig,types,args),gas="0xc00000",**{"from":owner})
        def simulate(calls,wire):
            frame={"calls":calls};changes={}
            if wire in ("v5","module_v5"):
                word=lambda n:"0x"+format(n,"064x")
                changes[EXEC]=dict(code="0x"+RUNTIME.read_text().strip().removeprefix("0x"),stateDiff={word(0):word(int(owner,16)),word(1):word(int(weth,16))})
            if wire in ("module","module_v5"):changes[ADAPTER]={"code":"0x"+module}
            if changes:frame["stateOverrides"]=changes
            r,=rpc(url,"eth_simulateV1",[dict(blockStateCalls=[frame],validation=False),at])
            if r["parentHash"]!=at["blockHash"] or len(r["calls"])!=len(calls):raise ValueError("другой хеш либо число вызовов")
            return r["calls"]
        def leg(wire,prefix,a,b,qty):
            spender={"module":ADAPTER,"module_v5":EXEC,"router":router,"v5":EXEC}[wire]
            approvals=[tx(a,"approve(address,uint256)",["address","uint256"],[spender,n]) for n in (0,qty)]
            balances=[tx(t,"balanceOf(address)",["address"],[who]) for who in (owner,EXEC,ADAPTER) for t in (a,b)]
            if mav:q=tx(viewer,"calculateSwap(address,uint128,bool,bool,uint256)",["address","uint128","bool","bool","uint256"],[m["pool"],qty,a==m["tokens"][0],False,0])
            else:
                qt="(address,address,uint256,int24,uint160)";q=tx(viewer,"quoteExactInputSingle("+qt+")",[qt],[(a,b,qty,m["spacing"],0)])
            base=prefix+approvals+balances+[q];front=simulate(base,wire)
            if any(v["status"]!="0x1" for v in front):return None,dict(stage="quote",calls=base,results=front)
            value=int.from_bytes(bytes.fromhex(front[-1]["returnData"][2:])[:32],"big")
            if not value:return None,dict(stage="zero",calls=base,results=front)
            def order(floor,expiry):
                if wire=="module":return tx(spender,"execute(address,address,address,uint256,uint256,uint256)",["address","address","address","uint256","uint256","uint256"],[m["pool"],a,b,qty,floor,expiry])
                if wire in ("v5","module_v5"):
                    step=(17,ADAPTER,a,b,10000,encode(["address"],[m["pool"]])) if mav else (1,m["pool"],a,b,10000,b"")
                    return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[a,qty,b,floor,expiry,[step]])
                typ="(address,address,address,address,uint256,uint256,uint256,uint256)" if mav else "(address,address,int24,address,uint256,uint256,uint256,uint160)"
                return tx(router,"exactInputSingle("+typ+")",[typ],[(a,b,m["pool"] if mav else m["spacing"],owner,expiry,qty,floor,0)])
            exact=order(value,end);calls=base+[order(value+1,end),order(value,0),exact]+balances;r=simulate(calls,wire)
            if [v["status"] for v in r[-9:-7]]!=["0x0","0x0"]:raise ValueError("провален контроль minOut+1 или срока")
            if r[-7]["status"]!="0x1":return None,dict(stage="payment",calls=calls,results=r)
            pre=[int(v["returnData"],16) for v in front[-7:-1]];post=[int(v["returnData"],16) for v in r[-6:]]
            paid=pre[0]-post[0];net=post[1]-pre[1]
            if paid<=0 or paid>qty or net!=value or pre[2:]!=post[2:]:raise ValueError("не совпала выплата или остаток")
            if wire.startswith("module") and paid!=qty:raise ValueError("модуль провёл частичный вход")
            return prefix+approvals+[exact],dict(stage="paid",input=str(qty),paid=str(paid),net=str(net),full=paid==qty,gas=int(r[-7]["gasUsed"],16),calls=calls,results=r)
        dec=decode(["uint8"],bytes.fromhex(read_chain(url,"eth_call",[tx(cash,"decimals()"),at])[2:]))[0]
        sizes=[10**max(dec-5,0),10**max(dec-3,0)] if cash==weth else [10**max(dec-4,0),10**max(dec-2,0)]
        cases=[]
        for qty in sizes:
            for wire in (("router","module","module_v5") if mav else ("router","v5")):
                prefix,buy=leg(wire,[],cash,other,qty)
                sell=leg(wire,prefix,other,cash,int(buy["net"]))[1] if prefix and buy["full"] else None
                cases.append(dict(wire=wire,input=str(qty),buy=buy,sell=sell))
        samples.append(dict(pool=m["pool"],cash=cash,payer=owner,cases=cases))
    return dict(selector=at,state_root=d["state_root"],total=len(d["markets"]),samples=samples,
                module_hash="0x"+keccak(bytes.fromhex(module)).hex() if module else None)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("dex");p.add_argument("chain");p.add_argument("output",type=Path);p.add_argument("--module",type=Path);a=p.parse_args()
    with gzip.open(DATA/f"{a.dex}_v1_{a.chain}_directory.json.gz") as f:d=json.load(f)
    urls=BASE_ENDPOINTS if a.chain=="base" else ETH_ENDPOINTS
    for u in urls:
        try:v=replay(d,a.dex,u,json.loads(a.module.read_bytes()) if a.module else None);break
        except (ValueError,requests.RequestException):
            if u==urls[-1]:raise
    a.output.write_text(json.dumps(v),encoding="utf8")
