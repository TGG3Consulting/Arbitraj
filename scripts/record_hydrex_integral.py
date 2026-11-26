"""Измеряет Hydrex Integral Base через QuoterV2, виртуальный квотер и ArbExecutor V5 на одном состоянии."""
import hashlib
import re

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture, ZERO
from record_base_slipstream_executor import EXEC, RUNTIME, STEP, calldata
from record_curve_ng_deployments import code
from record_woo_base import HOLDERS, WETH, rpc

FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"
DOC = "https://hydrex.gitbook.io/home/resources/security-and-audits.md"
VIRTUAL = "0x" + keccak(text="arb.virtual_quoter")[-20:].hex()
VIRTUAL_FILE = ROOT / "contracts/out/VirtualQuoter.bin-runtime"
QUOTE = "(address,address,address,uint256,uint160)"
RETURN = ["uint256", "uint256", "uint160", "uint32", "uint256", "uint16"]
SIZES = {WETH: [10**14, 10**16], "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913": [10**6, 10**8],
         "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf": [100, 10000]}


def verified(e, address, name):
    url = f"https://sourcify.dev/server/v2/contract/8453/{address}?fields=compilation,abi,runtimeBytecode.onchainBytecode"
    response = requests.get(url, timeout=25)
    response.raise_for_status()
    obj = response.json()
    actual = obj["runtimeBytecode"]["onchainBytecode"]
    if (obj["runtimeMatch"] not in ("match", "exact_match") or obj["compilation"]["name"] != name
            or code(e, address) != "0x" + keccak(bytes.fromhex(actual[2:])).hex()):
        raise ValueError(f"иной развёрнутый контракт {address}")
    abi = [a for a in obj["abi"] if a.get("name") in ("Swap", "globalState", "ticks", "quoteExactInputSingle")]
    return dict(url=url, match=obj["runtimeMatch"], name=name, abi=abi)


def map_ticks(c, pool, observed):
    low, high = min(observed) // 256, max(observed) // 256
    states = {}
    for direction, edge in [("prev", low * 256), ("next", high * 256 + 255)]:
        current, = c.one(pool, direction + "TickGlobal()", returns=["int24"])
        for count in range(1, 513):
            row = c.one(pool, "ticks(int24)", ["int24"], [current],
                        ["uint256", "int128", "int24", "int24", "uint256", "uint256"])
            states[current] = dict(gross=str(row[0]), net=str(row[1]), prev=row[2], next=row[3])
            if (current <= edge if direction == "prev" else current >= edge):
                break
            successor = row[2 if direction == "prev" else 3]
            if (successor >= current if direction == "prev" else successor <= current):
                raise ValueError("список тиков не движется к границе")
            current = successor
        else:
            raise ValueError("недостаточно тиков для полного текущего слова")
        if direction == "prev":
            before = count
    return dict(ticks=states, measured_tick_steps=max(before,count), lower_word=low, upper_word=high)


def simulate(e, pool, quoter, token0, source, target, dx, state, deadline, sender=None, flat_quoter=False, wrapped=WETH):
    sender = sender or HOLDERS[source]
    if flat_quoter:
        official = calldata("quoteExactInputSingle(address,address,address,uint256,uint160)",
                            ["address","address","address","uint256","uint160"], [source,target,ZERO,dx,0])
    else:
        official = calldata("quoteExactInputSingle(" + QUOTE + ")", [QUOTE], [(source,target,ZERO,dx,0)])
    virtual = calldata("quoteExactIn(address,bool,uint256)", ["address","bool","uint256"], [pool,source==token0,dx])
    def execute(limit):
        return calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                        ["address","uint256","address","uint256","uint256",STEP+"[]"],
                        [source,dx,target,limit,deadline,[(2,pool,source,target,10000,b"")]])
    bal = lambda who: calldata("balanceOf(address)", ["address"], [who])
    approve = calldata("approve(address,uint256)", ["address","uint256"], [EXEC,dx])
    word = lambda x: "0x" + x.to_bytes(32).hex()
    overrides = {EXEC:{"code":state[EXEC], "stateDiff":{word(0):word(int(sender,16)),word(1):word(int(wrapped,16))}},
                 VIRTUAL:{"code":state[VIRTUAL]}}
    def run(items):
        calls = [{"from":sender,"to":to,"data":data} for _,to,data in items]
        obj, = rpc(e, "eth_simulateV1", [{"blockStateCalls":[{"stateOverrides":overrides,"calls":calls}],
                                        "validation":False},e["selector"]])
        if obj["parentHash"] != e["selector"]["blockHash"]:
            raise ValueError("другой родительский блок")
        results = {label:row for (label,_,_),row in zip(items,obj["calls"],strict=True)}
        for label, row in results.items():
            if row["status"] != ("0x0" if label=="above" else "0x1"):
                raise ValueError(f"{pool} {label} {row.get('error')}")
        return calls, results
    quotes = [("official",quoter,official),("virtual",VIRTUAL,virtual)]
    _, sample = run(quotes)
    out = int(sample["official"]["returnData"][2:66],16)
    calls, results = run(quotes + [("allow",source,approve),("in_before",source,bal(sender)),
        ("out_before",target,bal(sender)),("above",EXEC,execute(out+1)),("exact",EXEC,execute(out)),
        ("in_after",source,bal(sender)),("out_after",target,bal(sender)),
        ("executor_in",source,bal(EXEC)),("executor_out",target,bal(EXEC)),("next_fee",pool,calldata("fee()"))])
    raw = lambda k: bytes.fromhex(results[k]["returnData"][2:])
    n = lambda k: int.from_bytes(raw(k))
    native = decode(["uint256","uint16"] if flat_quoter else RETURN,raw("official"))
    if (not out or native[0]!=out or (not flat_quoter and native[1]!=dx) or decode(["uint256","uint256"],raw("virtual"))!=(dx,out)
            or n("in_before")-n("in_after")!=dx or n("out_after")-n("out_before")!=out or n("exact")!=out
            or n("executor_in") or n("executor_out")):
        raise ValueError("расход, выплата, квотеры и исполнитель не совпали")
    types = ["int256","int256","uint160","uint128","int24"]
    if not flat_quoter:
        types += ["uint24","uint24"]
    topic = "0x"+keccak(text="Swap(address,address,"+",".join(types)+")").hex()
    raw_logs = [r for r in results["exact"]["logs"] if r["address"].lower()==pool]
    own = [r for r in raw_logs if r["topics"][0]==topic]
    events = [decode(types,bytes.fromhex(r["data"][2:])) for r in own]
    if flat_quoter:
        fee_topic = "0x"+keccak(text="SwapFee(address,uint24,uint24)").hex()
        fees = [r for r in raw_logs if r["topics"][0]==fee_topic]
        if [f["topics"][1] for f in fees] != [r["topics"][1] for r in own]:
            raise ValueError("Размещения и плательщики SwapFee/Swap не совпадают")
        events = [ev+decode(["uint24","uint24"],bytes.fromhex(f["data"][2:])) for ev,f in zip(events,fees,strict=True)]
    event, = [r for log,r in zip(own,events,strict=True) if log["topics"][1][-40:]==EXEC[2:]]
    expected_deltas = (dx,-out) if source==token0 else (-out,dx)
    if event[:2]!=expected_deltas:
        raise ValueError("дельты события Swap не совпали с балансами")
    return dict(sender=sender,source=source,target=target,dx=str(dx),out=str(out),fee_field=native[1] if flat_quoter else native[5],
                tick_after=event[4],override_fee=event[5],plugin_fee=event[6],next_fee=n("next_fee"),
                event_fees=[[r[5],r[6]] for r in events],
                crossed=None if flat_quoter else native[3],calls=calls,results=results)


def main():
    e = read(FOLDER/"base.json")
    c = Capture(e)
    page = requests.get(DOC,timeout=25)
    page.raise_for_status()
    addresses = dict((k,a.lower()) for k,a in re.findall(r"<td>([^<]+)</td><td>(0x[0-9a-fA-F]{40})</td>",page.text))
    factory, quoter = addresses["Factory"], addresses["QuoterV2"]
    if c.one(factory,"poolDeployer()",returns=["address"])[0]!=addresses["Pool Deployer"]:
        raise ValueError("фабрика с другим Pool Deployer")
    source = {quoter:verified(e,quoter,"QuoterV2")}
    for addr in [factory,addresses["Router"],addresses["Pool Deployer"]]:
        code(e,addr)
    sample = read(FOLDER/"catalog_recheck.json")["base"]["pages"]["hydrex-integral"]["samples"]
    state = {EXEC:"0x"+RUNTIME.read_text().strip(),VIRTUAL:"0x"+VIRTUAL_FILE.read_text().strip()}
    for a in [*state,*set(HOLDERS.values())]:
        if rpc(e,"eth_getCode",[a,e["selector"]])!="0x":
            raise ValueError(f"непустой код {a}")
    header = rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    deadline = int(header["timestamp"],16)+300
    directory, markets = [], []
    for row in sample:
        pool = row["address"].lower()
        fields = c.batch([(pool,sig,[],[]) for sig in ["token0()","token1()","factory()"]])
        tokens = [decode(["address"],raw)[0] for raw in fields[:2]]
        default, = c.one(factory,"poolByPair(address,address)",["address","address"],tokens,["address"])
        listed = decode(["address"],fields[2])[0]==factory and default==pool
        directory.append(dict(pool=pool,tokens=tokens,default=default,listed=listed))
        if not listed or not all(t in SIZES for t in tokens):
            continue
        source[pool] = verified(e,pool,"AlgebraPool")
        gs = c.one(pool,"globalState()",returns=["uint160","int24","uint16","uint8","uint16","bool"])
        plugin, = c.one(pool,"plugin()",returns=["address"])
        fee, = c.one(pool,"fee()",returns=["uint16"])
        liquidity, = c.one(pool,"liquidity()",returns=["uint128"])
        if not gs[5] or not liquidity:
            raise ValueError("пул закрыт")
        m = dict(pool=pool,tokens=tokens,price=str(gs[0]),tick=gs[1],last_fee=gs[2],config=gs[3],
                 current_fee=fee,plugin=plugin,liquidity=str(liquidity),cases=[])
        for i,t in enumerate(tokens):
            for dx in SIZES[t]:
                native = c.one(quoter,"quoteExactInputSingle("+QUOTE+")",[QUOTE],[(t,tokens[1-i],ZERO,dx,0)],RETURN)
                if native[1]!=dx or not native[0]:
                    raise ValueError("неполная котировка по хешу")
                case = simulate(e,pool,quoter,tokens[0],t,tokens[1-i],dx,state,deadline)
                case["at_block"] = [str(x) for x in native]
                m["cases"].append(case)
        m.update(map_ticks(c,pool,[m["tick"],*(case["tick_after"] for case in m["cases"])]))
        markets.append(m)
    if len(markets)<2:
        raise ValueError("недостаточно проверенных пар")
    hashes = {a:hashlib.sha256(bytes.fromhex(v[2:])).hexdigest() for a,v in state.items()}
    write(FOLDER/"hydrex_base.json",dict(chain_id=8453,selector=e["selector"],endpoint=e["endpoint"],
          official=DOC,addresses=addresses,source=source,executor=EXEC,virtual=VIRTUAL,runtime_sha256=hashes,
          deadline=deadline,directory=directory,markets=markets,calls=c.calls))
    base = read(FOLDER/"base.json")
    base["contracts"].update(e["contracts"])
    write(FOLDER/"base.json",base)
    path = ROOT/"config/first_run_dex.json"
    rows = [p for p in read(path) if (p["network"],p["id"])!=("base","hydrex-integral")]
    rows.append(dict(network="base",chain_id=8453,id="hydrex-integral",source=DOC,
                     settings={"dex.kind":"algebra_integral_hydrex","dex.factory":factory,"dex.quoter_virtual":True}))
    write(path,rows)
    print("Кандидаты, подтверждённые фабрикой, измеренные пулы, исполнения:",
          len(directory),sum(d["listed"] for d in directory),len(markets),sum(len(m["cases"]) for m in markets))


if __name__ == "__main__":
    main()
