"""Reconstruct MotoSwap's beacon pairs and measure the public FeeRouter on a canonical Ethereum parent."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from fork_fee_rpc import Reader
from record_base_slipstream_executor import OWNER, ROOT, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx
from record_ethereum_v2_factories import FUNDING, STABLES, WETH
from record_llama_base_cl import CanonicalCalls
from record_mooniswap import ZERO, aggregate

FACTORY = "0x81c9cbc47d700da1777abd831d8da3f526dfae24"
FEE = "0x9f846ef584fd44d075b5e8df00ddb4416da61a80"
IMPLEMENTATION = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
BEACON = "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50"
SWAP = "swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)"
ARGS = ["uint256", "uint256", "address[]", "address", "uint256"]


def word(cap, address, name, kind="address"):
    return cap.one(address, name + "()", returns=[kind])[0]


def slot(reader, address, position):
    raw = reader.one("eth_getStorageAt", [address, position, reader.tag])
    result, = decode(["address"], bytes.fromhex(raw[2:]))
    return result


def code(reader, address):
    raw = reader.one("eth_getCode", [address, reader.tag])
    if raw == "0x":
        raise ValueError(f"missing runtime {address}")
    return "0x" + keccak(hexstr=raw).hex()


def registry(reader, cap):
    roles = dict(factory=FACTORY, fee_router=FEE, core=word(cap, FEE, "router"),
                 wrapped=word(cap, FEE, "WETH"), registry=word(cap, FEE, "registry"),
                 creator=word(cap, FEE, "creatorRegistry"), vault=word(cap, FEE, "creatorVault"),
                 collector=word(cap, FEE, "collector"), beacon=word(cap, FACTORY, "beacon"),
                 pair_impl=word(cap, FACTORY, "implementation"))
    roles.update(factory_impl=slot(reader, FACTORY, IMPLEMENTATION), fee_impl=slot(reader, FEE, IMPLEMENTATION))
    if (word(cap, roles["core"], "factory"), word(cap, roles["core"], "WETH"),
        word(cap, roles["beacon"], "implementation"), word(cap, FACTORY, "quoteRegistry")) != (
            FACTORY, WETH, roles["pair_impl"], roles["registry"]):
        raise ValueError("factory, router, registry and beacon disagree")
    init = word(cap, FACTORY, "pairCodeHash", "bytes32")
    count = word(cap, FACTORY, "allPairsLength", "uint256")
    pools = [decode(["address"], row)[0] for row in aggregate(cap, [(FACTORY, "allPairs(uint256)", ["uint256"], [i]) for i in range(count)])]
    if len(set(pools)) != count or not count:
        raise ValueError("invalid pair enumeration")
    entries = []
    ops = [(p, sig, [], []) for p in pools for sig in ("token0()", "token1()", "factory()", "getReserves()")]
    raws = iter(aggregate(cap, ops))
    for p in pools:
        a, b, factory = (decode(["address"], next(raws))[0] for _ in range(3))
        reserves = decode(["uint112", "uint112", "uint32"], next(raws))
        salt = keccak(bytes.fromhex(a[2:] + b[2:]))
        predicted = "0x" + keccak(b"\xff" + bytes.fromhex(FACTORY[2:]) + salt + init)[-20:].hex()
        if not a < b or p != predicted or factory != FACTORY or slot(reader, p, BEACON) != roles["beacon"]:
            raise ValueError("wrong pair identity " + p)
        entries.append(dict(pool=p, tokens=[a,b], reserves=list(map(str,reserves[:2])), runtime=code(reader,p)))
    ops = []
    for row in entries:
        p = row["pool"]
        a,b = row["tokens"]
        ops.extend([(FACTORY,"getPair(address,address)",["address"]*2,[a,b]),
                    (FACTORY,"getPair(address,address)",["address"]*2,[b,a]),
                    (a,"balanceOf(address)",["address"],[p]), (b,"balanceOf(address)",["address"],[p]),
                    (FEE,"feeOnOutput(address[])",["address[]"],[[a,b]]),
                    (FEE,"feeOnOutput(address[])",["address[]"],[[b,a]])])
    raws = iter(aggregate(cap,ops))
    for row in entries:
        if any(a != row["pool"] for a in [decode(["address"],next(raws))[0] for _ in range(2)]):
            raise ValueError("reverse getPair")
        row["balances"] = [str(decode(["uint256"],next(raws))[0]) for _ in range(2)]
        row["fee_on_output"] = [decode(["bool"],next(raws))[0] for _ in range(2)]
    assets = sorted({t for row in entries for t in row["tokens"]})
    ops = [(target,sig,["address"],[t]) for t in assets for target,sig in [
        (roles["registry"],"isQuote(address)"), (roles["registry"],"quoteRank(address)"),
        (roles["creator"],"activeCreator(address)")]]
    raws = iter(aggregate(cap,ops))
    tokens = {t:dict(quote=decode(["bool"],next(raws))[0],rank=decode(["uint256"],next(raws))[0],
                     creator=decode(["address"],next(raws))[0]) for t in assets}
    allowed = {k:cap.one(FACTORY,"swapAllowed(address)",["address"],[v],["bool"])[0]
               for k,v in (("owner",OWNER),("fee_router",FEE),("core",roles["core"]))}
    return dict(roles=roles,code={k:code(reader,a) for k,a in roles.items()}, tokens=tokens, pairs=entries,
                allowed=allowed, init="0x"+init.hex(), lp_bps=word(cap,FACTORY,"swapFeeBps","uint256"),
                protocol_bps=word(cap,FEE,"protocolFeeBps","uint256"), creator_bps=word(cap,FEE,"creatorFeeBps","uint256"))


def approve(token, spender, amount):
    return tx(token,"approve(address,uint256)",["address","uint256"],[spender,amount])


def trade(reader, market, prep, src, dst, n, minimum, deadline, mode):
    pool = market["pool"]
    core = market["core"]
    target = FEE if mode == "fee" else core
    permissions = [approve(src,target,n)]
    calls = prep + permissions + [balance(t,w) for w in (OWNER,pool) for t in (src,dst)]
    call = tx(target,SWAP,ARGS,[n,minimum,[src,dst],OWNER,deadline])
    pos = len(calls)
    calls.extend([call, *[balance(t,w) for w in (OWNER,pool) for t in (src,dst)]])
    rows = simulate_block(None,reader.tag,{OWNER:{"balance":hex(4*10**18)}},calls,
                          read_rpc=lambda _,method,params:reader.one(method,params))
    if any(r["status"]!="0x1" for i,r in enumerate(rows) if i!=pos):
        raise ValueError("failed funding, approval or balance at "+pool)
    result = rows[pos]
    logs = result["logs"]
    before = [int(r["returnData"],16) for r in rows[pos-4:pos]]
    after = [int(r["returnData"],16) for r in rows[pos+1:]]
    deltas = [b-a for a,b in zip(before,after,strict=True)]
    if result["status"]=="0x1":
        if deltas[0] != -n or deltas[1]<minimum or any(token_flow(logs,t,OWNER)!=x for t,x in zip((src,dst),deltas[:2])):
            raise ValueError("wallet mismatched physical transfers")
        if mode=="fee" and int(result["returnData"],16)!=deltas[1]:
            raise ValueError("router returned a different payout")
    elif deltas!=[0]*4:
        raise ValueError("failed swap changed balances")
    return dict(ok=result["status"]=="0x1",target=target,input=src,output=dst,amount=str(n),minimum=str(minimum),
                spent=str(-deltas[0]),received=str(deltas[1]),pool_delta=list(map(str,deltas[2:])), gas=int(result["gasUsed"],16),
                approval_gas=int(rows[len(prep)]["gasUsed"],16), calldata=call["data"],logs=logs,
                error=result.get("error"),return_data=result.get("returnData"))


def capture():
    source = json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    reader = Reader("ethereum",source)
    if reader.one("eth_getCode",[OWNER,reader.tag])!="0x":
        raise ValueError("occupied simulated payer")
    cap=CanonicalCalls(reader)
    book=registry(reader,cap)
    deadline=int(reader.header["timestamp"],16)+300
    trials=[]
    for row in book["pairs"][:4]:
        # The three MOTO cash markets and the first graduated token.
        cash=next(a for a in row["tokens"] if a in (WETH,*STABLES))
        token=next(a for a in row["tokens"] if a!=cash)
        for size in ((10**15,10**16) if cash==WETH else (10**6,10**7)):
            funding=[tx(WETH,"deposit()",value=size)] if cash==WETH else [
                tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                   ["uint256","address[]","address","uint256"],[size,[WETH,cash],OWNER,deadline],value=10**18)]
            market=dict(**row,core=book["roles"]["core"])
            buy=trade(reader,market,funding,cash,token,size,1,deadline,"fee")
            if not buy["ok"]:
                raise ValueError("failed funded buy in "+row["pool"])
            prep_sell=funding+[approve(cash,FEE,size),tx(FEE,SWAP,ARGS,[size,1,[cash,token],OWNER,deadline])]
            received=int(buy["received"])
            for prep,src,dst,amount in ((funding,cash,token,size),(prep_sell,token,cash,received)):
                trial=buy if src==cash else trade(reader,market,prep,src,dst,amount,1,deadline,"fee")
                if not trial["ok"]:
                    raise ValueError("failed funded reverse in "+row["pool"])
                net=int(trial["received"])
                exact=trade(reader,market,prep,src,dst,amount,net,deadline,"fee")
                high=trade(reader,market,prep,src,dst,amount,net+1,deadline,"fee")
                denied=trade(reader,market,prep,src,dst,amount,1,deadline,"core")
                if not exact["ok"] or high["ok"] or denied["ok"]!=book["allowed"]["owner"]:
                    raise ValueError("router control changed at "+row["pool"])
                trials.append(dict(pool=row["pool"],paid=exact,strict=high,core=denied))
    return dict(chain=1,block=source["block"],selector=reader.tag,root=reader.header["stateRoot"],wallet=OWNER,
                source="https://docs.motoswap.org/dev/addresses",directory=book,trials=trials)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output",type=Path)
    args=cli.parse_args()
    args.output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

