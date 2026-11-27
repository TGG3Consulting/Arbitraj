"""Fund a pair on an archived, empty DOCSwap or MarSwap factory in an isolated EVM block."""
import argparse
import json
import gzip
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, call, simulate_block
from record_bitswap_ethereum import read_batch, balance, tx
from record_hello_wallets import BASE_ENDPOINTS
from record_ranges_ethereum import ENDPOINT


DATA = ROOT / "tests/data/first_run_dex_2026-11-26/docswap_empty_directories.json"
LAB = "0x" + keccak(text="arb docswap empty factory laboratory wrapper")[-20:].hex()


# Read EXTCODEHASH for a nested CREATE2 while the isolated block is still in scope.
CODEHASH = "0x"+keccak(text="arb simulation code hash reader")[-20:].hex()

def capture(url, network, mars=False):
    if mars:
        from types import SimpleNamespace
        from record_omni_v2_base import role
        if network != "base":
            raise ValueError("the Ethereum MarSwap registry already has historical funded pools")
        directory = json.loads(gzip.decompress((DATA.parent / "marswap-base.json.gz").read_bytes()))
        # Deployer nonce 8 created the factory, nonce 10 the router. Verify its immutable roles below.
        router = "0xc10f17b52ab0f230cc8e0866be1533ea55a6c0d9"
        source, binding = role(SimpleNamespace(endpoint=url,selector=directory["selector"]), router)
        if source["compilation"]["name"] != "MarsRouter":
            raise ValueError("unexpected verified router")
        factory = directory["factory"]
        roles = {"selector":directory["selector"], "chain":8453, "factory":factory, "router":router,
                 "wrapped":"0x4200000000000000000000000000000000000006",factory:{"hash":directory["factory_code_hash"]},
                 router:{"hash":binding["runtime"]}}
    else:
        roles=json.loads(DATA.read_bytes())[network]
    at=roles["selector"]
    chain,factory,router=roles["chain"],roles["factory"],roles["router"]
    wrapped="0x"+roles["wrapped"][-40:]
    head, chain_id, fcode,rcode,wcode,*empty=read_batch(url,[("eth_getBlockByHash",[at["blockHash"],False]),
        ("eth_chainId",[]),*(("eth_getCode",[a,at]) for a in (factory,router,wrapped,LAB,OWNER,EXEC,CODEHASH))])
    if int(chain_id,16)!=chain or empty!=["0x"]*4 or head["hash"]!=at["blockHash"]:
        raise ValueError("unexpected chain or occupied simulation address")
    for role,code in [(factory,fcode),(router,rcode)]:
        if "0x"+keccak(hexstr=code).hex()!=roles[role]["hash"]:raise ValueError(("role changed",role))
    word=lambda n:"0x"+format(n,"064x")
    overrides={CODEHASH:{"code":"0x6000353f60005260206000f3"},LAB:{"code":wcode},OWNER:{"balance":hex(10**21)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
        "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(wrapped,16))}}}
    def run(calls):
        return simulate_block(url,at,overrides,calls)
    def values(calls):
        result=run(calls)
        for i,r in enumerate(result):
            if r["status"]!="0x1":raise ValueError(("preparation",i,r.get("error")))
        return [bytes.fromhex(r["returnData"][2:]) for r in result]
    count, init, router_factory, router_wrapped = values([
        tx(factory, "allPairsLength()"), tx(factory, "INIT_CODE_PAIR_HASH()"), tx(router, "factory()"), tx(router, "WETH()")])
    if decode(["address"],router_factory)[0] != factory or decode(["address"],router_wrapped)[0] != wrapped:
        raise ValueError("router immutables do not match the selected factory and wrapper")
    integer=lambda x:int.from_bytes(x,"big")
    count = integer(count)
    flat = None
    if mars:
        terms = decode(["address","uint256","uint256","uint256"],values([tx(factory,"lpInfo()")])[0])
        if list(terms)!=directory["terms"] or "0x"+init.hex()!=directory["init_hash"]:
            raise ValueError("historical fee terms or CREATE2 hash changed")
        fee,den,flat=terms[1]+terms[2],10000,terms[3]
    else:
        fee,den=map(integer,values([tx(factory,"totalFee()"),tx(factory,"MAX_TOTAL_FEE_PERCENT()")]))
    if count!=0 or not 0<=fee<den: raise ValueError("unexpected original supply")
    t0,t1=sorted([wrapped,LAB])
    salt=keccak(bytes.fromhex(t0[2:]+t1[2:]))
    pair="0x"+keccak(b"\xff"+bytes.fromhex(factory[2:])+salt+init)[-20:].hex()
    create=tx(factory,"createPair(address,address)",["address","address"],[wrapped,LAB])
    create["gas"]=hex(12_000_000)
    end=int(head["timestamp"],16)+900
    reserve=8*10**18
    approval=lambda asset,target,cap:tx(asset,"approve(address,uint256)",["address","uint256"],[target,cap])
    prefix=[create,tx(wrapped,"deposit()",value=reserve*2),tx(LAB,"deposit()",value=reserve*2),
            approval(wrapped,router,reserve),approval(LAB,router,reserve),
            tx(router,"addLiquidity(address,address,uint256,uint256,uint256,uint256,address,uint256)",
               ["address","address","uint256","uint256","uint256","uint256","address","uint256"],
               [wrapped,LAB,reserve,reserve,reserve,reserve,OWNER,end])]
    views=[tx(factory,"getPair(address,address)",["address","address"],[t1,t0]),tx(factory,"allPairs(uint256)",["uint256"],[0]),
           tx(pair,"getReserves()"),tx(pair,"factory()"),tx(pair,"token0()"),tx(pair,"token1()"),
           call(CODEHASH,"0x"+encode(["address"],[pair]).hex())]
    signs=values(prefix+views)
    address=lambda b:decode(["address"],b)[0]
    if (address(signs[0]),address(signs[-7]),address(signs[-6]),address(signs[-4]),address(signs[-3]),address(signs[-2]))!=(pair,pair,pair,factory,t0,t1):
        raise ValueError("factory registration")
    if decode(["uint112","uint112","uint32"],signs[-5])[:2]!=(reserve,reserve): raise ValueError("seed reserves")
    runtime=signs[-1]
    if len(runtime)!=32 or runtime in (bytes(32),keccak(b"")):raise ValueError("missing pair code")
    rows=[]
    for amount in (10**13,10**17):
        for source,target in [(t0,t1),(t1,t0)]:
            model=amount*(den-fee)*reserve//(reserve*den+amount*(den-fee))
            quoted=decode(["uint256[]"],values(prefix+[tx(router,"getAmountsOut(uint256,address[])",["uint256","address[]"],[amount,[source,target]])])[-1])[0][-1]
            if quoted!=model:raise ValueError(("quote",quoted,model))
            for mode in (("router","v5","flat") if mars else ("router","v5")):
                model=quoted if mode!="flat" else amount*reserve//(reserve+amount)
                dest=EXEC if mode=="v5" else router
                def trade(minimum,deadline):
                    if mode!="v5":
                        return tx(router,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                                  ["uint256","uint256","address[]","address","uint256"],[amount,minimum,[source,target],OWNER,deadline],value=flat if mode=="flat" else 0)
                    step=(0,pair,source,target,10000,encode(["uint32"],[fee*10**6//den]))
                    return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                              ["address","uint256","address","uint256","uint256",STEP+"[]"],[source,amount,target,minimum,deadline,[step]])
                probes=[balance(source,OWNER),balance(target,OWNER),balance(source,EXEC),balance(target,EXEC)]
                for label,minimum,deadline in [("paid",model,end),("above",model+1,end),("expired",1,0)]:
                    result=run([*prefix,approval(source,dest,amount),*probes,trade(minimum,deadline),*probes])
                    if any(x["status"]!="0x1" for x in result[:-5]+result[-4:]): raise ValueError("funding/balances")
                    op=result[-5]
                    b,a=([int(x["returnData"],16) for x in side] for side in (result[-9:-5],result[-4:]))
                    if label=="paid":
                        if op["status"]!="0x1" or (b[0]-a[0],a[1]-b[1])!=(amount,model) or b[2:]!=a[2:]:
                            raise ValueError(("settlement",mode,label,op.get("error"),b,a))
                        rows.append(dict(mode=mode,fee_value_wei=flat if mode=="flat" else 0,source=source,target=target,input=amount,output=model,gas=int(op["gasUsed"],16),
                                         logs=op["logs"]))
                    elif op["status"]!="0x0" or b!=a:raise ValueError(("protection",mode,label))
    return dict(chain=chain,selector=at,router_runtime_hash=roles[router]["hash"],factory_runtime_hash=roles[factory]["hash"],state_root=head["stateRoot"],factory=factory,router=router,original_count=count,
                init_hash="0x"+init.hex(),pair=pair,assets=[t0,t1],lab_token=LAB,fee=fee,denominator=den,
                pair_runtime_hash="0x"+runtime.hex(),source="registered simulated pool on original factory",cases=rows)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("network",choices=("ethereum","base"));cli.add_argument("out",type=Path);cli.add_argument("--mars", action="store_true");args=cli.parse_args()
    for url in BASE_ENDPOINTS if args.network=="base" else (ENDPOINT,"https://ethereum-rpc.publicnode.com"):
        try:
            record=capture(url,args.network,args.mars)
            break
        except (RuntimeError,OSError) as err: print(str(err)[:180])
    else: raise RuntimeError("archive unavailable")
    args.out.write_text(json.dumps(record,separators=(",",":")),encoding="utf8")
