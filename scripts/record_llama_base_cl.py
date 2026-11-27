"""Reconcile Base V3 factory creations and compare protected wallet and V5 payments."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from fork_fee_rpc import Reader, pool_blocks
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import tx, balance
from record_concave_ethereum import approval

WETH = "0x4200000000000000000000000000000000000006"
CREATE = "0x" + keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
WAGMI = "https://github.com/RealWagmi/"
THICK = "https://docs.equalizer.exchange/equal-fantom/contracts/"
VENUES = {
    "wagmi": (14250787, "0x576a1301b42942537d38fb147895fe83fb418fd4", "0xb5fa77e3929fe198a86aa40fd6c77886785bcd0e", "0x8b741b0d79be80e135c880f7583d427b4d41f015",
              [WAGMI+"v3-sdk/blob/055bffed07f05ab57be7bfab899f3c0df45f1c02/src/constants/addresses.ts",
               WAGMI+"smart-router/blob/8a3869f5bb5facaf154ad1677da0ad3a98cb7c4a/src/constants/exchange.ts",
               WAGMI+"smart-router/blob/8a3869f5bb5facaf154ad1677da0ad3a98cb7c4a/src/constants/v3.ts"]),
    "thick": (6314325, "0xe6da85feb3b4e0d6aed95c41a125fba859bb9d24", "0x1f7a12b40bfc8e8561008bc2ca1fbdc71a36d0e8", "0xf5b7181877feab982a4b49a99e84bb7cbebeff98", [THICK]),
    "2thick": (23864246, "0x7ca1dccfb4f49564b8f13e18a67747fd428f1c40", "0xe4ba08712c404042b8eefc3fdf3b603c977500df", "0xc701e92db1ef14ab64f66f18536b1e07678db678", [THICK]),
}


def registry(reader, cap, name, head):
    start, factory, router, quoter, origins = VENUES[name]
    roles = {"factory":factory, "router":router, "quoter":quoter}
    code = reader.many(("eth_getCode", [a,cap.selector]) for a in roles.values())
    bindings = {k: {"address":a,"bytes":len(c[2:])//2,"runtime":"0x"+keccak(hexstr=c).hex()}
                for (k,a),c in zip(roles.items(),code,strict=True)}
    assert all(x["bytes"] for x in bindings.values())
    for address in (router,quoter):
        assert cap.one(address,"factory()",returns=["address"]) == (factory,)
        assert cap.one(address,"WETH9()",returns=["address"]) == (WETH,)
    keytype = "int24" if name=="2thick" else "uint24"
    if name == "wagmi":
        init=bytes.fromhex("30146866f3a846fe3c636beb2756dbd24cf321bc52c9113c837c21f47470dfeb")
        assert init in bytes.fromhex(code[2][2:])
    else:
        init,=cap.one(factory,"POOL_INIT_CODE_HASH()",returns=["bytes32"])
    total=int(reader.one("eth_getTransactionCount",[factory,cap.selector]),16)
    assert reader.one("eth_getCode",[factory,hex(start-1)])=="0x"
    heights=pool_blocks(reader,factory,start-1,head["block"],0,total)
    # The factory's own deployment contributes the starting account nonce.
    assert heights.get(start,0)>=1
    records=[]
    headers=reader.many(("eth_getBlockByNumber",[hex(h),False]) for h in heights)
    logs=reader.many(("eth_getLogs",[{"address":factory,"blockHash":b["hash"],"topics":[CREATE]}]) for b in headers)
    for (h,change),header,entries in zip(heights.items(),headers,logs,strict=True):
        assert int(header["number"],16)==h and len(entries)==change-int(h==start)
        for log in entries:
            assert log["blockHash"]==header["hash"] and not log.get("removed") and log["address"].lower()==factory
            a,b=[decode(["address"],bytes.fromhex(t[2:]))[0] for t in log["topics"][1:3]]
            fee=int(log["topics"][3],16)
            spacing,p=decode(["int24","address"],bytes.fromhex(log["data"][2:]))
            key=spacing if name=="2thick" else fee
            expected="0x"+keccak(b"\xff"+bytes.fromhex(factory[2:])+keccak(encode(["address","address",keytype],[a,b,key]))+init)[-20:].hex()
            assert p==expected and a<b
            records.append(dict(pool=p,tokens=[a,b],key=key,initial_fee=fee,spacing=spacing,tx=log["transactionHash"],height=h))
    assert len(records)==total-1 and len({m["pool"] for m in records})==len(records)
    calls=[]
    for m in records:
        a,b=m["tokens"];p=m["pool"]
        calls.extend((factory,"getPool(address,address,"+keytype+")",["address","address",keytype],[x,y,m["key"]]) for x,y in [(a,b),(b,a)])
        calls.extend((p,fn,[],[]) for fn in ("token0()","token1()","factory()","tickSpacing()","fee()","liquidity()"))
        calls.extend((t,"balanceOf(address)",["address"],[p]) for t in (a,b))
    values=iter(posts(cap,calls))
    for m in records:
        ts=m["tokens"]; expected=[m["pool"],m["pool"],*ts,factory,m["spacing"]]
        for typ,ex in zip(["address"]*5+["int24"],expected,strict=True):assert decode([typ],next(values))[0]==ex
        m["current_fee"],m["liquidity"],*cash=[int.from_bytes(next(values),"big") for _ in range(4)]
        m["balances"]=list(map(str,cash))
    return dict(name=name,chain_id=8453,height=head["block"],selector=cap.selector,state_root=reader.header["stateRoot"],
                sources=origins,roles=bindings,init="0x"+init.hex(),key_type=keytype,markets=sorted(records,key=lambda row:row["pool"]))


def wallet(reader,cap,book):
    assert reader.many(("eth_getCode",[a,cap.selector]) for a in (EXEC,OWNER)) == ["0x","0x"]
    word=lambda x:"0x"+format(x,"064x")
    overrides={OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
               "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    def canonical_simulation(_endpoint, method, params):
        if method != "eth_simulateV1" or params[-1] != cap.selector:
            raise ValueError("unexpected simulation selector")
        return reader.one(method, params)

    def run(calls):
        return simulate_block(cap.endpoint,cap.selector,overrides,calls,read_rpc=canonical_simulation)
    kind=book["key_type"]; router=book["roles"]["router"]["address"]; quoter=book["roles"]["quoter"]["address"]
    deadline=int(reader.header["timestamp"],16)+300
    funded=sorted((m for m in book["markets"] if WETH in m["tokens"] and m["liquidity"]>0),
                  key=lambda m:-int(m["balances"][m["tokens"].index(WETH)]))
    results=[]; rejected=[]
    for market in funded[:4]:
        a,b=market["tokens"];other=b if a==WETH else a
        for seed in (10**12,10**14):
            for wire in ("router02","v5"):
                target=router if wire=="router02" else EXEC
                prefix=[tx(WETH,"deposit()",value=seed)]; amount=seed
                for a,b in ((WETH,other),(other,WETH)):
                    quotesig="(address,address,uint256,"+kind+",uint160)"
                    quote=tx(quoter,"quoteExactInputSingle("+quotesig+")",[quotesig],[(a,b,amount,market["key"],0)])
                    def order(minimum,expiry):
                        if wire=="v5":
                            step=(1,market["pool"],a,b,10000,b"")
                            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[a,amount,b,minimum,expiry,[step]])
                        shape="(address,address,"+kind+",address,uint256,uint256,uint160)"
                        inner=tx(router,"exactInputSingle("+shape+")",[shape],[(a,b,market["key"],OWNER,amount,minimum,0)])
                        return tx(router,"multicall(uint256,bytes[])",["uint256","bytes[]"],[expiry,[bytes.fromhex(inner["data"][2:])]])
                    before=[balance(t,w) for w in (OWNER,EXEC) for t in (a,b)]
                    permit=approval(a,target,amount); first=prefix+[permit,*before,quote]; i=len(first)
                    r=run(first+[order(1,deadline),*before])
                    if not all(v["status"]=="0x1" for v in r[:i-1]):raise ValueError(("funding and balance",market["pool"],wire))
                    if r[i-1]["status"]!="0x1" or r[i]["status"]!="0x1":
                        rejected.append(dict(pool=market["pool"],wire=wire,input=a,amount=str(amount),quote=r[i-1].get("error"),swap=r[i].get("error")));break
                    assert all(v["status"]=="0x1" for v in r[-4:])
                    cash=lambda rows:[int(x["returnData"],16) for x in rows]
                    old=cash(r[i-5:i-1]); new=cash(r[-4:]); received=new[1]-old[1];spent=old[0]-new[0]
                    q=int.from_bytes(bytes.fromhex(r[i-1]["returnData"][2:])[:32],"big")
                    if q!=received or spent!=amount or new[2:]!=old[2:] or received<=0:
                        rejected.append(dict(pool=market["pool"],wire=wire,input=a,requested=str(amount),quoted=str(q),received=str(received),spent=str(spent)));break
                    exact=order(received,deadline)
                    check=run(first+[order(received+1,deadline),order(received,1),*before,exact,*before])
                    assert [j for j,x in enumerate(check) if x["status"]!="0x1"]==[i,i+1]
                    assert cash(check[i+2:i+6])==old and cash(check[-4:])==new
                    paid=check[i+6]
                    swaps=[v for v in paid["logs"] if v["address"].lower()==market["pool"] and v["topics"][0]=="0x"+keccak(text="Swap(address,address,int256,int256,uint160,uint128,int24)").hex()]
                    assert len(swaps)==1
                    vals=decode(["int256","int256"],bytes.fromhex(swaps[0]["data"][2:])[:64]);side=market["tokens"].index(a)
                    assert vals[side]==spent and vals[1-side]==-received
                    results.append(dict(pool=market["pool"],wire=wire,input=a,output=b,spent=str(spent),received=str(received),
                                        gas=int(paid["gasUsed"],16),approval_gas=int(check[len(prefix)]["gasUsed"],16),
                                        tx=exact,logs=paid["logs"],refusals=[check[i]["error"],check[i+1]["error"]]))
                    prefix.extend((permit,exact)); amount=received
    book.update(cases=results,unavailable=rejected,weth_funded_samples=[m["pool"] for m in funded[:4]])
    return book


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("name",choices=VENUES);ap.add_argument("output",type=Path)
    a=ap.parse_args();snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes()); reader=Reader("base",snap)
    cap=Capture(dict(snap,endpoint="https://base.gateway.tenderly.co"))
    book=wallet(reader,cap,registry(reader,cap,a.name,snap))
    a.output.write_bytes(json.dumps(book,separators=(",",":")).encode())
