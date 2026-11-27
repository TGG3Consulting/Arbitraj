"""Record Ethereum Aero Basic registry and compare Metarouter/V5 against the owner's ERC20 deltas."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from fork_fee_rpc import Reader
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx
from record_ethereum_v2_factories import FUNDING, WETH
from record_llama_base_cl import CanonicalCalls
from record_motoswap import approve
from record_mooniswap import ZERO, aggregate

# Official directory https://app.aero.xyz/docs/security/ lists these All chains roles.
FACTORIES = {"stable": "0x78589e876edcebdce72ee3436ab3c9ad3a7cef2d", "volatile": "0x8e6a9fcd1d638cc760ffba397605fe3355de76d2"}
META = "0x0fef9e88a7b76bcdbf451780c6774fe32965ee98"
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
DAI = "0x6b175474e89094c44da98b954eedeac495271d0f"
LINK = "0x514910771af9ca656af840dff83e8264ecf986ca"
FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"


def directory(reader):
    cap = CanonicalCalls(reader)
    roles = {}
    def bind(address):
        raw = reader.one("eth_getCode", [address, reader.tag])
        if raw == "0x":
            raise ValueError("empty runtime " + address)
        roles[address] = "0x" + keccak(hexstr=raw).hex()
        return raw

    bind(META)
    if cap.one(META, "WETH()", returns=["address"]) != (WETH,):
        raise ValueError("wrong network wrapped token")
    result = {}
    for name, factory in FACTORIES.items():
        bind(factory)
        fee, impl, mev = (cap.one(factory, method+"()", returns=["address"])[0] for method in ("feeModule", "implementation", "mevTaxModule"))
        bind(fee)
        bind(impl)
        if mev != ZERO or cap.one(factory, "isPaused()", returns=["bool"])[0]:
            raise ValueError("review current pause or MEV module")
        pools, = cap.one(factory,"allPools()",returns=["address[]"])
        if not pools or cap.one(factory,"allPoolsLength()",returns=["uint256"])[0] != len(set(pools)) or len(set(pools)) != len(pools):
            raise ValueError("incomplete registry")
        markets = []
        for pool in pools:
            if bind(pool) != "0x363d3d373d3d3d363d73"+impl[2:]+"5af43d82803e903d91602b57fd5bf3":
                raise ValueError("unrecognized pool clone")
            u0,u1,r0,r1,t0,t1 = cap.one(pool,"metadata()",returns=["uint256"]*4+["address"]*2)
            ops = [(factory,"getPool(address,address)",["address"]*2,ts) for ts in ([t0,t1],[t1,t0])]
            ops += [(pool,"factory()",[],[]),(pool,"POOL_TYPE()",[],[])]
            ops += [(t,"balanceOf(address)",["address"],[pool]) for t in (t0,t1)]
            ops += [(factory,"getBaseFee(address,address,uint256,uint256,uint256,uint256)",["address"]*2+["uint256"]*4,[pool,caller,i,j,r0,r1])
                    for caller,i,j in ((ZERO,0,0),(META,u0,0),(EXEC,0,u1))]
            rows = aggregate(cap,ops)
            a,b,origin = (decode(["address"],raw)[0] for raw in rows[:3])
            if (a,b,origin)!=(pool,pool,factory) or decode(["bytes32"],rows[3])[0].rstrip(b"\0").decode()!="V2_"+name.upper():
                raise ValueError("wrong registry binding")
            balances = [int.from_bytes(x) for x in rows[4:6]]
            fees = [int.from_bytes(x) for x in rows[6:]]
            markets.append(dict(pool=pool,tokens=[t0,t1],units=list(map(str,(u0,u1))),reserves=list(map(str,(r0,r1))),
                                balances=list(map(str,balances)),fees=fees))
        result[name] = dict(factory=factory,implementation=impl,fee_module=fee,mev_module=mev,markets=markets)
    return result, roles


def order(mode, pool, src, dst, budget, want, expiry):
    if mode=="v5":
        step = (26,pool,src,dst,10000,b"")
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[src,budget,dst,want,expiry,[step]])
    if mode=="in":
        body = encode(["(address[],address,(uint8,uint256),bool,uint256,address)"],[([pool],src,(0,budget),True,want,OWNER)])
    else:
        body = encode(["(address[],address,uint256,bool,uint256,address)"],[([pool],src,want,True,budget,OWNER)])
    return tx(META,"execute(bytes,bytes[],uint256)",["bytes","bytes[]","uint256"],[bytes([8 if mode=="in" else 9]),[body],expiry])


def leg(reader, state, prefix, mode, pool, src, dst, budget, want, end):
    target = EXEC if mode=="v5" else META
    marks = [balance(t,w) for w in (OWNER,pool,target) for t in (src,dst)]
    funded = prefix+[approve(src,target,budget)]
    def simulate(commands):
        rows = simulate_block(None,reader.tag,state, funded+marks+commands+marks, read_rpc=lambda _,m,p:reader.one(m,p))
        ix=len(funded)+len(marks)
        if any(x["status"]!="0x1" for x in rows[:len(funded)]):
            raise ValueError(("funding stage",pool,[x.get("error") for x in rows[:len(funded)] if x["status"]!="0x1"]))
        before,after=rows[len(funded):ix],rows[-len(marks):]
        if any(x["status"]!="0x1" for x in before+after):
            raise ValueError("unreadable physical balance")
        delta=[int(b["returnData"],16)-int(a["returnData"],16) for a,b in zip(before,after,strict=True)]
        return rows[ix:-len(marks)],delta,int(rows[len(prefix)]["gasUsed"],16)
    first=order(mode,pool,src,dst,budget,want,end)
    (trial,),delta,gas_approve=simulate([first])
    identity=dict(mode=mode,pool=pool,input=src,output=dst,budget=str(budget),target=str(want))
    if trial["status"]=="0x0":
        if any(delta): raise ValueError("reverted original moved funds")
        return dict(**identity,ok=False,error=trial.get("error")),None
    spent,net=-delta[0],delta[1]
    if not 0<spent<=budget or net<want or any(delta[-2:]) or (mode!="out" and spent!=budget):
        raise ValueError(("wrong custody or payment",pool,delta))
    if any(token_flow(trial["logs"],t,OWNER)!=d for t,d in zip((src,dst),delta[:2])):
        raise ValueError("unexplained owner movement")
    # Exact-output consumes its independently computed input. Tighten its cap to the observed debit.
    pay=order(mode,pool,src,dst,spent,net,end)
    denied = order(mode,pool,src,dst,spent-1,net,end) if mode=="out" else order(mode,pool,src,dst,spent,net+1,end)
    (strict,expired,paid),fresh,_=simulate([denied,order(mode,pool,src,dst,spent,net,0),pay])
    if [x["status"] for x in (strict,expired,paid)]!=["0x0","0x0","0x1"] or delta!=fresh:
        raise ValueError(("cash or boundary changed",mode,pool,spent,net,strict.get("error"),paid.get("error")))
    return dict(**identity,ok=True,spent=str(spent),received=str(net),pool_delta=list(map(str,delta[2:4])),
                gas=int(paid["gasUsed"],16),approval_gas=gas_approve,strict=strict.get("error"),expired=expired.get("error"),
                transaction=pay,logs=paid["logs"]),funded+[pay]


def capture():
    reader=Reader("ethereum",json.loads((FOLDER/"ethereum.json").read_bytes()))
    books,roles=directory(reader)
    end=int(reader.header["timestamp"],16)+180
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    word=lambda n:"0x"+format(n,"064x")
    if reader.many(("eth_getCode",[a,reader.tag]) for a in (EXEC,OWNER))!=["0x","0x"]:
        raise ValueError("occupied simulated address")
    state={OWNER:{"balance":hex(4*10**18)},EXEC:{"code":"0x"+runtime,"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    trials=[]
    for book in books.values():
        for m in book["markets"]:
            pool=m["pool"]; tokens=m["tokens"]
            src=next(t for t in (USDC,DAI,WETH,LINK) if t in tokens)
            dst=next(t for t in tokens if t!=src)
            i=tokens.index(src); reserve=int(m["reserves"][i]); unit=int(m["units"][i])
            # Include the smallest positive input and thin reserves without losing ordinary $0.10 and $1 trials.
            sizes=sorted({max(1,min(reserve//n,max(1,unit//d))) for n,d in [(100,10),(10,1)]})
            if min(map(int,m["reserves"])) <= 10:
                # Some pools need a large fraction of their input reserve to produce one output atom.
                sizes=sorted(set(sizes)|{max(1,reserve),max(1,reserve*2)})
            for budget in sizes:
                fund=[tx(WETH,"deposit()",value=budget)] if src==WETH else [
                    tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],
                       [budget,[WETH,src],OWNER,end],value=10**18)]
                for mode in ("v5","in","out"):
                    known=next((r for r in trials if r["pool"]==pool and r["input"]==src and r["budget"]==str(budget) and r["mode"]=="in" and r["ok"]),None)
                    if mode=="out" and not known:continue
                    want=max(1,int(known["received"])//2) if mode=="out" else 1
                    buy,prefix=leg(reader,state,fund,mode,pool,src,dst,budget,want,end);trials.append(buy)
                    if prefix is None:continue
                    qty=int(buy["received"]);sell_want=max(1,int(buy["spent"])//4) if mode=="out" else 1
                    sell,_=leg(reader,state,prefix,mode,pool,dst,src,qty,sell_want,end);trials.append(sell)
    return dict(chain=1,selector=reader.tag,root=reader.header["stateRoot"],owner=OWNER,executor=EXEC,
                runtime="0x"+keccak(hexstr=runtime).hex(),router=META,roles=roles,factories=books,trials=trials)


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("output",type=Path)
    parser.parse_args().output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

