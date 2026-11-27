"""Фиксирует действующие Diamond facets, каталог и точные выплаты пары на архивном блоке Base."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, ROOT, rpc, simulate_block
from record_ethervista_wallets import tx, integer
from record_v2_chain_pools import measure, swap

FACTORY="0x1a62a841e83ecc3d72b0de6002af7a7dbf921cd5"
ROUTER="0x0681363e5da35a248e1abb5cddd6db9cdac9a771"
WETH="0x4200000000000000000000000000000000000006"
REFERENCE="0x2adc726148b26e7d6a0de7c0276afe7a25e97228"
def digest(b):return "0x"+keccak(b).hex()


def facets(c,proxy,signatures):
    root=bytes.fromhex(rpc(c.endpoint,"eth_getCode",[proxy,c.selector])[2:])
    sets,=c.one(proxy,"facets()",returns=["(address,bytes4[])[]"])
    methods={w.hex():a for a,items in sets for w in items}
    selected={}
    for signature in signatures:
        selector=keccak(text=signature)[:4]
        actual,=c.one(proxy,"facetAddress(bytes4)",["bytes4"],[selector],["address"])
        if actual!=methods[selector.hex()] or int(actual,16)==0:raise ValueError("facet not active")
        selected[signature]=actual
    lookup=keccak(text="facetAddress(bytes4)")[:4]
    loupe,=c.one(proxy,"facetAddress(bytes4)",["bytes4"],[lookup],["address"])
    if loupe!=proxy or methods.get(lookup.hex())!=proxy:raise ValueError("loupe is not in the verified shell")
    implementations={}
    lengths={}
    for a in set(selected.values()):
        code=bytes.fromhex(rpc(c.endpoint,"eth_getCode",[a,c.selector])[2:])
        if not code:raise ValueError("missing facet")
        implementations[a]=digest(code)
        lengths[a]=len(code)
    return dict(address=proxy,runtime=digest(root),selectors=selected,implementations=implementations,loupe=loupe,bytes=len(root),implementation_bytes=lengths)


def direct_wallet(c, market, sizes, timestamp, fee_pips, router, fund_sale=None, wrapped=WETH, router_method="swapExactTokensForTokensSupportingFeeOnTransferTokens", modes=("v5","router")):
    token=next(t for t in market["tokens"] if t!=wrapped)
    deadline=timestamp+120
    word=lambda n:"0x"+format(n,"064x")
    env={OWNER:{"balance":hex(10**19)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
         "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(wrapped,16))}}}
    def leg(prefix, input_token, output_token, size, mode):
        target=EXEC if mode=="v5" else router
        approve=tx(input_token,"approve(address,uint256)",["address","uint256"],[target,size])
        def trial(minimum,expiry=deadline):
            action=(swap(market["pool"],input_token,output_token,size,minimum,expiry,fee_pips) if mode=="v5"
                    else tx(router,router_method+"(uint256,uint256,address[],address,uint256)",
                            ["uint256","uint256","address[]","address","uint256"],[size,minimum,[input_token,output_token],OWNER,expiry]))
            reads=[tx(t,"balanceOf(address)",["address"],[who]) for who in (OWNER,EXEC) for t in (input_token,output_token)]
            answers=simulate_block(c.endpoint,c.selector,env,[*prefix,approve,*reads,action,*reads])
            start=len(prefix)+1
            if any(x["status"]!="0x1" for x in answers[:start]):raise ValueError("failed funding or allowance")
            before=list(map(integer,answers[start:start+4]));after=list(map(integer,answers[-4:]))
            result=answers[start+4]
            if any(before[2:]+after[2:]):raise ValueError("executor residue")
            data=dict(status=result["status"],spent=str(before[0]-after[0]),received=str(after[1]-before[1]),
                      gas=result["gasUsed"],approval_gas=answers[start-1]["gasUsed"],error=result.get("error"))
            return data,action
        loose,_=trial(1)
        if loose["status"]!="0x1":raise ValueError(f"could not settle {mode}: {loose['error']}")
        net=int(loose["received"])
        exact,action=trial(net)
        above,_=trial(net+1)
        if (exact["status"],exact["spent"],exact["received"],above["status"],above["spent"],above["received"])!=("0x1",str(size),str(net),"0x0","0","0"):
            raise ValueError("recipient floor or wallet debit differs")
        expired={}
        if router_method=="swapExactTokensForTokens":
            expired,_=trial(net,1)
            if (expired["status"],expired["spent"],expired["received"])!=("0x0","0","0"):
                raise ValueError("expired transaction changed wallet balances")
        return dict(input=input_token,output=output_token,exact=exact,above=above,expired=expired),[*prefix,approve,action]
    trials=[]
    for size in sizes:
        for mode in modes:
            if fund_sale is not None:
                sell,_=leg([fund_sale(size)],token,wrapped,size,mode)
                trials.append(dict(mode=mode,size=str(size),sell=sell))
                continue
            fund=tx(wrapped,"deposit()",value=size)
            buy,next_prefix=leg([fund],wrapped,token,size,mode)
            sell,_=leg(next_prefix,token,wrapped,int(buy["exact"]["received"]),mode)
            trials.append(dict(mode=mode,size=str(size),buy=buy,sell=sell))
    return trials


def capture(endpoint):
    chain=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())["base"]
    tag={"blockHash":chain["hash"],"requireCanonical":True}
    c=Capture(dict(endpoint=endpoint,selector=tag))
    head=rpc(endpoint,"eth_getBlockByHash",[chain["hash"],False])
    if int(rpc(endpoint,"eth_chainId",[]),16)!=8453 or int(head["number"],16)!=chain["block"]:
        raise ValueError("network or block differs")
    f=facets(c,FACTORY,["createPair(address,address)","getPair(address,address)","swapFee()","feesEnabled()",
                        "getTotalPairs()","getPairAddresses(uint256,uint256)"])
    r=facets(c,ROUTER,["factory()","WETH()","getAmountsOut(uint256,address[])","getAmountsIn(uint256,address[])",
                        "getAmountOut(uint256,uint256,uint256,uint32)","swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)"])
    if c.one(ROUTER,"factory()",returns=["address"])[0]!=FACTORY or c.one(ROUTER,"WETH()",returns=["address"])[0]!=WETH:
        raise ValueError("router binding differs")
    fee,=c.one(FACTORY,"swapFee()",returns=["uint32"])
    enabled,=c.one(FACTORY,"feesEnabled()",returns=["bool"])
    creator=f["selectors"]["createPair(address,address)"]
    code=bytes.fromhex(rpc(endpoint,"eth_getCode",[creator,tag])[2:])
    # Два PUSH-литерала длины и смещения у CODECOPY фабрики. Самостоятельная проверка — адрес каждой пары.
    start,size=0x1087,0x2ab7
    if bytes.fromhex("612ab7") not in code[1380:1420]:
        raise ValueError("pair creation size changed")
    if bytes.fromhex("620010878239") not in code[1400:1450]:raise ValueError("pair copy offset changed")
    init=keccak(code[start:start+size])
    length,=c.one(FACTORY,"getTotalPairs()",returns=["uint256"])
    addresses,=c.one(FACTORY,"getPairAddresses(uint256,uint256)",["uint256","uint256"],[0,length],["address[]"])
    if len(addresses)!=length or len(set(addresses))!=length:raise ValueError("incomplete factory directory")
    fields=iter(posts(c,[(a,s,[],[]) for a in addresses for s in ("token0()","token1()","factory()","getReserves()")]))
    rows=[]
    for a in addresses:
        t0,t1,factory=[decode(["address"],next(fields))[0] for _ in range(3)]
        reserve=decode(["uint112","uint112","uint32"],next(fields))[:2]
        salt=keccak(bytes.fromhex(t0[2:]+t1[2:]))
        if factory!=FACTORY or a!="0x"+keccak(b"\xff"+bytes.fromhex(FACTORY[2:])+salt+init)[-20:].hex():
            raise ValueError("wrong pair constructor")
        rows.append(dict(pool=a,tokens=[t0,t1],reserves=[str(x) for x in reserve]))
    controls=[]
    for row in rows:
        for i in (0,1):
            a,b=row["tokens"][i],row["tokens"][1-i]
            controls.extend([(FACTORY,"getPair(address,address)",["address","address"],[a,b]),(a,"balanceOf(address)",["address"],[row["pool"]])])
    checks=iter(posts(c,controls))
    for row in rows:
        row["balances"]=[]
        for _ in (0,1):
            if decode(["address"],next(checks))[0]!=row["pool"]:raise ValueError("getPair binding differs")
            row["balances"].append(str(decode(["uint256"],next(checks))[0]))
    samples=[]
    candidates=sorted([x for x in rows if WETH in x["tokens"] and x["reserves"]==x["balances"] and min(map(int,x["reserves"]))>0],
                      key=lambda x:int(x["reserves"][x["tokens"].index(WETH)]),reverse=True)
    for row in candidates:
        code=bytes.fromhex(rpc(endpoint,"eth_getCode",[row["pool"],tag])[2:])
        data=dict(network="base",dex="diamondswap",factory=FACTORY,runtime_hash=digest(code),
                  creation_code_hash="0x"+init.hex(),fee_fraction=[10000,fee],fee_target=FACTORY,fee_getter="swapFee()",
                  verify_pair_boundary=True,invariant_error="0xcd6fabbb",**row)
        try:
            data=measure(data,chain,endpoint)
        except ValueError as e: data["reason"]=str(e)
        samples.append(data)
        if row["pool"]==REFERENCE and data.get("cases"):break
    if not any(x.get("cases") for x in samples):raise ValueError("no pair boundary settled: "+str([x.get("reason") for x in samples])[:400])
    curve=[]
    sample=next(m for m in rows if m["pool"]==REFERENCE)
    for i in (0,1):
        tokens=sample["tokens"][::1 if i==0 else -1]
        rin,rout=map(int,sample["reserves"][::1 if i==0 else -1])
        for amount in (rin//10000,rin//10):
            path,=c.one(ROUTER,"getAmountsOut(uint256,address[])",["uint256","address[]"],[amount,tokens],["uint256[]"])
            for candidate in (0,1,fee,100,1000):
                out,=c.one(ROUTER,"getAmountOut(uint256,uint256,uint256,uint32)",["uint256"]*3+["uint32"],[amount,rin,rout,candidate],["uint256"])
                if out!=amount*(10000-candidate)*rout//(rin*10000+amount*(10000-candidate)):raise ValueError("router fraction differs")
                if candidate==fee and out!=path[1]:raise ValueError("path fee differs")
                curve.append(dict(tokens=tokens,amount=str(amount),fee=candidate,out=str(out),live=str(path[1])))
    amounts=[int(row["in"]) for entry in samples if entry["pool"]==REFERENCE for row in entry["cases"]]
    wallets=direct_wallet(c,sample,amounts,int(head["timestamp"],16),fee*100,ROUTER)
    return dict(chain=8453,selector=tag,height=chain["block"],state_root=head["stateRoot"],factory=f,router=r,wallets=wallets,
                creation=dict(offset=start,bytes=size,hash="0x"+init.hex()),fee=fee,fees_enabled=enabled,markets=rows,curve=curve,samples=samples)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path)
    ap.add_argument("--endpoint", action="append", help="Archive RPC, repeat to specify fallback order")
    a=ap.parse_args()
    failures=[]
    for url in a.endpoint or ("https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"):
        try:
            evidence=capture(url)
        except (requests.RequestException, ValueError, KeyError, RuntimeError) as err:
            failures.append((url,str(err)[:160]))
            continue
        evidence["endpoint"]=url
        a.output.write_text(json.dumps(evidence,indent=2)+"\n",encoding="utf8")
        break
    else:
        raise RuntimeError(f"No endpoint completed the pinned DiamondSwap replay: {failures}")
