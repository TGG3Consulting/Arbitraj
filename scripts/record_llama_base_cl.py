"""Reconcile Base V3 factory creations and compare protected wallet and V5 payments."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from fork_fee_rpc import Reader, pool_blocks
from record_balancer_markets import posts
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import tx, balance
from record_concave_ethereum import approval

WETH = "0x4200000000000000000000000000000000000006"
CREATE = "0x" + keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
WAGMI = "https://github.com/RealWagmi/"
THICK = "https://docs.equalizer.exchange/equal-fantom/contracts/"
VENUES = {
    "goblins": (21481309, "0xe82fa4d4ff25bad8b07c4d1ebd50e83180dd5eb8",
                "0xe29a829a1c86516b1d24b7966af14eb1be2cd5d4", "0xcbf872c837bad15b0a0283a5c5936fbd1b66beaf",
                ["https://repo.sourcify.dev/8453/0xe82fa4d4ff25bad8b07c4d1ebd50e83180dd5eb8",
                 "https://repo.sourcify.dev/8453/0xcbf872c837bad15b0a0283a5c5936fbd1b66beaf",
                 "https://repo.sourcify.dev/8453/0xe29a829a1c86516b1d24b7966af14eb1be2cd5d4"]),
    "wagmi": (14250787, "0x576a1301b42942537d38fb147895fe83fb418fd4", "0xb5fa77e3929fe198a86aa40fd6c77886785bcd0e", "0x8b741b0d79be80e135c880f7583d427b4d41f015",
              [WAGMI+"v3-sdk/blob/055bffed07f05ab57be7bfab899f3c0df45f1c02/src/constants/addresses.ts",
               WAGMI+"smart-router/blob/8a3869f5bb5facaf154ad1677da0ad3a98cb7c4a/src/constants/exchange.ts",
               WAGMI+"smart-router/blob/8a3869f5bb5facaf154ad1677da0ad3a98cb7c4a/src/constants/v3.ts"]),
    "thick": (6314325, "0xe6da85feb3b4e0d6aed95c41a125fba859bb9d24", "0x1f7a12b40bfc8e8561008bc2ca1fbdc71a36d0e8", "0xf5b7181877feab982a4b49a99e84bb7cbebeff98", [THICK]),
    "2thick": (23864246, "0x7ca1dccfb4f49564b8f13e18a67747fd428f1c40", "0xe4ba08712c404042b8eefc3fdf3b603c977500df", "0xc701e92db1ef14ab64f66f18536b1e07678db678", [THICK]),
}


class CanonicalCalls:
    """Decode one Ethereum call on the same verified block across archive providers."""
    def __init__(self, reader):
        self.reader = reader
        self.selector = reader.tag
        self.endpoint = reader.endpoints[0]

    def one(self, target, signature, args=(), values=(), returns=()):
        call = tx(target, signature, args, values)
        # Multicall may contain 64 independent tick reads.
        call["gas"] = hex(24_000_000)
        response = self.reader.one("eth_call", [call, self.selector])
        return decode(returns, bytes.fromhex(response[2:]))


def registry(reader, cap, name, head):
    start, factory, router, quoter, origins = VENUES[name]
    lens = {"goblins":"0xdea74cd879a3b11cb849a07bbe76b68f50d531f8","thick":"0xb80a72281c76464cc0e1fbf2431a897393e0d757",
            "2thick":"0x547db6759b0af9e050855106993e3cefc16e937e"}.get(name,"0x0cdee061c75d43c82520ed998c23ac2991c9ac6d")
    roles = {"factory":factory, "router":router, "quoter":quoter, "lens":lens}
    code = reader.many(("eth_getCode", [a,cap.selector]) for a in roles.values())
    bindings = {k: {"address":a,"bytes":len(c[2:])//2,"runtime":"0x"+keccak(hexstr=c).hex()}
                for (k,a),c in zip(roles.items(),code,strict=True)}
    assert all(x["bytes"] for x in bindings.values())
    for address in (router,quoter):
        assert cap.one(address,"factory()",returns=["address"]) == (factory,)
        assert cap.one(address,"WETH9()",returns=["address"]) == (WETH,)
    keytype = "int24" if name=="2thick" else "uint24"
    published_init = {
        "wagmi": "30146866f3a846fe3c636beb2756dbd24cf321bc52c9113c837c21f47470dfeb",
        "goblins": "e34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54",
    }
    if name in published_init:
        init=bytes.fromhex(published_init[name])
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
    # Validate the selected TickLens against both original storage getters on each
    # pool. An empty bitmap is valid evidence only for that word.
    slots=posts(cap, [(m["pool"],"slot0()",[],[]) for m in records])
    checks=[]
    for m, slot in zip(records,slots,strict=True):
        tick=decode(["uint160","int24"],slot[:64])[1]
        word=(tick//m["spacing"])//256
        for index in range(max(-32768,word-1),min(32767,word+1)+1):
            checks.append((m,index))
    ops=[]
    for m,index in checks:
        ops.extend([(m["pool"],"tickBitmap(int16)",["int16"],[index]),
                    (lens,"getPopulatedTicksInWord(address,int16)",["address","int16"],[m["pool"],index])])
    words=iter(posts(cap,ops));ticks=[]
    for m,index in checks:
        bitmap=int.from_bytes(next(words),"big")
        points,=decode(["(int24,int128,uint128)[]"],next(words))
        initialized={(index*256+bit)*m["spacing"] for bit in range(256) if (bitmap>>bit)&1}
        assert len(points)==len(initialized) and {p[0] for p in points}==initialized
        m.setdefault("lens_words",[]).append([index,str(bitmap),len(points)])
        ticks.extend((m["pool"],row) for row in points)
    values=posts(cap,[(p,"ticks(int24)",["int24"],[row[0]]) for p,row in ticks])
    for (_,(_,net,gross)),actual in zip(ticks,values,strict=True):
        assert decode(["uint128","int128"],actual[:64])==(gross,net)
    return dict(name=name,chain_id=8453,height=head["block"],selector=cap.selector,state_root=reader.header["stateRoot"],
                sources=origins,roles=bindings,init="0x"+init.hex(),key_type=keytype,markets=sorted(records,key=lambda row:row["pool"]))


VVV = "0xacfe6019ed1a7dc6f7b508c02d1b04ec88cc21bf"
DIEM = "0xf4d97f2da56e8c3098f3a8d538db630a2606a024"


def fund_quote_asset(cap, run, seed, deadline, asset):
    # Acquire the measured input using only ETH belonging to the payer.
    factory = "0x33128a8fc17869897dce68ed026d694621f6fdfd"
    pool, = cap.one(factory,"getPool(address,address,uint24)",["address","address","uint24"],[WETH,asset,500],["address"])
    assert cap.one(pool,"factory()",returns=["address"]) == (factory,)
    prefix = [tx(WETH,"deposit()",value=seed), approval(WETH,EXEC,seed)]
    step=(1,pool,WETH,asset,10000,b"")
    swap=tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
            ["address","uint256","address","uint256","uint256",STEP+"[]"],[WETH,seed,asset,1,deadline,[step]])
    balances=[balance(t,OWNER) for t in (WETH,asset)]
    rows=run(prefix+balances+[swap]+balances)
    assert all(row["status"]=="0x1" for row in rows)
    before=[int(r["returnData"],16) for r in rows[2:4]]
    after=[int(r["returnData"],16) for r in rows[5:]]
    assert before[0]-after[0]==seed and after[1]>before[1]
    received=after[1]-before[1]
    return prefix+[swap],received,dict(source=pool,spent=str(seed),received=str(received),gas=int(rows[4]["gasUsed"],16))


def wallet(reader,cap,book,artifact=None,*,selected_targets=None,funding_source=None):
    assert reader.many(("eth_getCode",[a,cap.selector]) for a in (EXEC,OWNER)) == ["0x","0x"]
    word=lambda x:"0x"+format(x,"064x")
    overrides={OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
               "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    wires = (book.get("native_wire","router02"), "v5")
    constructor = []
    if artifact:
        import hashlib
        from record_empire_module import MODULE as ADAPTER, deployment
        from record_fraxswap_swaps import bound_runtime
        factory = book["roles"]["factory"]["address"]
        assert book["key_type"] == "uint24" and reader.one("eth_getCode", [ADAPTER, cap.selector]) == "0x"
        code = bound_runtime(artifact, factory, cap.endpoint, cap.selector)
        create, identity = deployment(cap.endpoint,cap.selector,factory,artifact)
        assert identity["runtime"] == "0x"+keccak(hexstr=code).hex()
        constructor.append(create)
        book["module_hash"] = identity["runtime"]
        book["deployment"] = dict(kind="registered_uni_v3",chainId=8453,factory=factory,
            factoryHash=book["roles"]["factory"]["runtime"],sourceBlock=cap.selector["blockHash"],
            runtimeKeccak=identity["runtime"],data=create["data"],
            creationSha256=hashlib.sha256(bytes.fromhex(create["data"][2:])).hexdigest(),
            runtimeSha256=hashlib.sha256(bytes.fromhex(code)).hexdigest())
        wires = ("module",)
    def canonical_simulation(_endpoint, method, params):
        if method != "eth_simulateV1" or params[-1] != cap.selector:
            raise ValueError("unexpected simulation selector")
        return reader.one(method, params)

    def run(calls):
        result = simulate_block(cap.endpoint,cap.selector,overrides,constructor+calls,read_rpc=canonical_simulation)
        if constructor:
            assert result[0]["status"] == "0x1"
            book["deployment_gas"] = int(result[0]["gasUsed"],16)
        return result[len(constructor):]
    kind=book["key_type"]; router=book["roles"]["router"]["address"]; quoter=book["roles"]["quoter"]["address"]
    deadline=int(reader.header["timestamp"],16)+300
    funded=sorted((m for m in book["markets"] if WETH in m["tokens"] and m["liquidity"]>0),
                  key=lambda m:-int(m["balances"][m["tokens"].index(WETH)]))
    results=[]; rejected=[]
    targets=[(m,WETH) for m in funded[:4]]
    if book["name"]=="2thick":
        targets.extend((m,VVV) for m in book["markets"] if set(m["tokens"])=={VVV,DIEM} and m["liquidity"]>0)
    if book.get("native_wire")=="slipstream":
        targets=[(m,WETH if WETH in m["tokens"] else book["counter"]) for m in book["markets"]
                 if WETH in m["tokens"] or book["counter"] in m["tokens"]]
    if selected_targets is not None:
        targets = selected_targets
    for market,base_token in targets:
        other=next(t for t in market["tokens"] if t!=base_token)
        for seed in (10**12,10**14):
            if base_token==WETH:
                initial=[tx(WETH,"deposit()",value=seed)];available=seed
            else:
                initial,available,funding=(funding_source(cap,run,seed,deadline) if funding_source is not None
                                          else fund_quote_asset(cap,run,seed,deadline,base_token))
                book.setdefault("funding",[]).append(funding)
            for wire in wires:
                target = ADAPTER if wire == "module" else (router if wire in ("router02","slipstream") else EXEC)
                prefix=initial.copy();amount=available
                for a,b in ((base_token,other),(other,base_token)):
                    quotesig="(address,address,uint256,"+kind+",uint160)"
                    quote=tx(quoter,"quoteExactInputSingle("+quotesig+")",[quotesig],[(a,b,amount,market["key"],0)])
                    def order(minimum,expiry):
                        if wire == "module":
                            return tx(target, "execute(address,address,address,uint256,uint256,uint256)",
                                      ["address","address","address","uint256","uint256","uint256"],
                                      [market["pool"],a,b,amount,minimum,expiry])
                        if wire=="v5":
                            step=(1,market["pool"],a,b,10000,b"")
                            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[a,amount,b,minimum,expiry,[step]])
                        if wire=="slipstream":
                            shape="(address,address,int24,address,uint256,uint256,uint256,uint160)"
                            return tx(router,"exactInputSingle("+shape+")",[shape],[(a,b,market["key"],OWNER,expiry,amount,minimum,0)])
                        shape="(address,address,"+kind+",address,uint256,uint256,uint160)"
                        inner=tx(router,"exactInputSingle("+shape+")",[shape],[(a,b,market["key"],OWNER,amount,minimum,0)])
                        return tx(router,"multicall(uint256,bytes[])",["uint256","bytes[]"],[expiry,[bytes.fromhex(inner["data"][2:])]])
                    custody = target if wire == "module" else EXEC
                    before=[balance(t,w) for w in (OWNER,custody) for t in (a,b)]
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
    ap.add_argument("--module",type=Path,help="Verify the factory-bound net-settlement wallet on the same recorded markets")
    a=ap.parse_args();snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes()); reader=Reader("base",snap)
    cap=CanonicalCalls(reader)
    if a.module:
        archive = ROOT / "tests/data/first_run_dex_2026-11-26/wagmi_thick_base.json.gz"
        book = json.loads(gzip.decompress(archive.read_bytes()))[a.name]
        assert book["selector"] == cap.selector and book["state_root"] == reader.header["stateRoot"]
        native = {(c["pool"],c["input"],c["spent"]): c for c in book["cases"] if c["wire"] == "router02"}
        book = wallet(reader,cap,book,json.loads(a.module.read_bytes()))
        checked = {(c["pool"],c["input"],c["spent"]): c for c in book["cases"]}
        assert native and checked.keys() == native.keys()
        assert all(c["received"] == native[k]["received"] for k,c in checked.items())
    else:
        book=wallet(reader,cap,registry(reader,cap,a.name,snap))
    a.output.write_bytes(json.dumps(book,separators=(",",":")).encode())
