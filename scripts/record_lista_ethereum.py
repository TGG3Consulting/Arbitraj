"""Проверяет код и реестр ListaV3 Ethereum, официальный QuoterV2, TickLens и обмены класса V3."""
import hashlib
import posixpath
import re
import time

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, RUNTIME, STEP, calldata
from record_curve_ng_deployments import code
from record_hydrex_integral import VIRTUAL, VIRTUAL_FILE
from record_saddle_eth_registry import HOLDERS as STABLE_HOLDERS
from record_woo_base import rpc

FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"
DOC = "https://docs.bsc.lista.org/for-developer/dex/smart-contract-ethereum.md"
REFERENCE = "0x88e6a0c2ddd26feeb64f039a2c41296fcb3f5640"
HOLDERS = {**STABLE_HOLDERS,
           "0xdc035d45d973e3ec169d2276ddab16f1e407384f":"0x0965fffb37f94d82b6f16b4c291af43a94b24dc6",
           "0x4c9edd5852cd905f086c759e8383e09bff1e68b3":"0x33ae83071432116ae892693b45466949a38ac74c"}
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
PARAM = "(address,address,uint256,uint24,uint160)"
WORD = lambda n: "0x"+n.to_bytes(32).hex()


def get(url, params=None):
    for delay in (1,2,4):
        response = requests.get(url,params=params,timeout=25)
        if response.status_code not in (429,500,502,503,504):
            break
        time.sleep(delay)
    response.raise_for_status()
    return response.json()


def deployed(e,addr):
    url = f"https://sourcify.dev/server/v2/contract/{e['chain_id']}/{addr}"
    obj = get(url,{"fields":"compilation,sources,runtimeBytecode.onchainBytecode"})
    if (obj["runtimeMatch"] not in ("match","exact_match") or
            code(e,addr)!="0x"+keccak(bytes.fromhex(obj["runtimeBytecode"]["onchainBytecode"][2:])).hex()):
        raise ValueError("код расходится с закреплённым блоком")
    return obj,url


def sources(obj):
    """Обходит только зависимости исполняемого контракта. Фиксирует импорты и токены Solidity."""
    todo = [obj["compilation"]["fullyQualifiedName"].split(":")[0]]
    seen, result = set(), {}
    while todo:
        path = todo.pop()
        if path in seen:
            continue
        seen.add(path)
        content = obj["sources"][path]["content"]
        clean = re.sub(r"//[^\n]*|/\*.*?\*/","",content,flags=re.S)
        imports = re.findall(r"""\bimport\s+['"]([^'"]+)['"]\s*;""",clean)
        todo.extend(posixpath.normpath(posixpath.join(posixpath.dirname(path),p)) for p in imports)
        rename = lambda n:n.replace("ListaV3","UniswapV3")
        # Импорты соответствуют тем же единицам обеих компиляций, абсолютный путь Hardhat/Foundry различается.
        for p in imports:
            clean = clean.replace(p,rename(posixpath.basename(p)))
        tokens = re.findall(r"""(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[a-zA-Z_$][\w$]*|\d+|[^\s])""",clean)
        key = rename(posixpath.basename(path))
        if key in result:
            raise ValueError("коллизия имён зависимостей")
        result[key] = hashlib.sha256("\n".join(rename(t) if re.fullmatch(r"[A-Za-z_$][\w$]*",t) else t
                                                for t in tokens).encode()).hexdigest()
    return result


def events(e,factory):
    url = f"https://eth.blockscout.com/api/v2/addresses/{factory}/logs"
    params, labels = {}, []
    while True:
        page = get(url,params)
        labels.extend(page["items"])
        params = page["next_page_params"]
        if not params:
            break
    head = rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    limit = int(head["number"],16)
    logs = []
    for height in sorted({r["block_number"] for r in labels if r["block_number"]<=limit}):
        block = rpc(e,"eth_getBlockByNumber",[hex(height),False])
        records = rpc(e,"eth_getLogs",[{"address":factory,"blockHash":block["hash"]}])
        logs.extend(records)
    listed = {(r["block_number"],r["index"]) for r in labels if r["block_number"]<=limit}
    if {(int(r["blockNumber"],16),int(r["logIndex"],16)) for r in logs}!=listed:
        raise ValueError("страницы индексатора расходятся с журналом фабрики")
    return url,logs


def exchange(e,pool,quoter,source,target,dx,fee,deadline,bytecode):
    sender = HOLDERS[source]
    query = calldata("quoteExactInputSingle("+PARAM+")",[PARAM],[(source,target,dx,fee,0)])
    bal = lambda who:calldata("balanceOf(address)",["address"],[who])
    approve = calldata("approve(address,uint256)",["address","uint256"],[EXEC,dx])
    def execute(limit):
        return calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
            ["address","uint256","address","uint256","uint256",STEP+"[]"],
            [source,dx,target,limit,deadline,[(1,pool,source,target,10000,b"")]])
    caller = {"from":sender,"to":quoter,"data":query}
    quote = decode(["uint256","uint160","uint32","uint256"],bytes.fromhex(rpc(e,"eth_call",[caller,e["selector"]])[2:]))
    out = quote[0]
    entries = [("quote",quoter,query),("approve",source,approve),("in_before",source,bal(sender)),
               ("out_before",target,bal(sender)),("above",EXEC,execute(out+1)),("exact",EXEC,execute(out)),
               ("in_after",source,bal(sender)),("out_after",target,bal(sender)),
               ("left_in",source,bal(EXEC)),("left_out",target,bal(EXEC))]
    calls = [{"from":sender,"to":a,"data":d,"gas":hex(1_200_000)} for _,a,d in entries]
    overrides = {EXEC:{"code":bytecode,"stateDiff":{WORD(0):WORD(int(sender,16)),WORD(1):WORD(int(WETH,16))}}}
    current, = rpc(e,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls,"stateOverrides":overrides}],
                                       "validation":False},e["selector"]])
    if current["parentHash"]!=e["selector"]["blockHash"]:
        raise ValueError("другой блок")
    result = {k:item for (k,_,_),item in zip(entries,current["calls"],strict=True)}
    n = lambda name:int(result[name]["returnData"],16)
    for name,item in result.items():
        if item["status"]!=("0x0" if name=="above" else "0x1"):
            raise ValueError(f"{pool} {name} {item.get('error')}")
    if (not out or int(result["quote"]["returnData"][2:66],16)!=out or n("exact")!=out
            or n("in_before")-n("in_after")!=dx or n("out_after")-n("out_before")!=out
            or n("left_in") or n("left_out")):
        raise ValueError("котировка, балансы или исполнитель расходятся")
    return dict(sender=sender,source=source,target=target,dx=str(dx),out=str(out),quote=[str(v) for v in quote],
                calls=calls,results=result)


def main():
    e = read(FOLDER/"ethereum.json")
    c = Capture(e)
    response = requests.get(DOC,timeout=20)
    response.raise_for_status()
    rows = [(role.strip(),address.lower()) for role,address in
            re.findall(r"\|\s*(\w+)\s*\|\s*(0x[0-9a-fA-F]{40})\s*\|",response.text)]
    addresses = dict(rows)
    factory, quoter = addresses["ListaV3Factory"], addresses["QuoterV2"]
    documented = {addr for role,addr in rows if role=="ListaV3Pool"}
    profiles = read(ROOT/"config/first_run_dex.json")
    reference = next(p["settings"] for p in profiles if p["network"]=="ethereum" and p["id"]=="uniswap_v3")
    lens = reference["dex.tick_lens"].lower()
    ref,ref_url = deployed(e,REFERENCE)
    canonical = sources(ref)
    factory_info,factory_url = deployed(e,factory)
    quoter_info,quoter_url = deployed(e,quoter)
    if (factory_info["compilation"]["name"]!="ListaV3Factory" or quoter_info["compilation"]["name"]!="QuoterV2"
            or c.one(quoter,"factory()",returns=["address"])[0]!=factory):
        raise ValueError("другая версия фабрики или квотера")
    code(e,lens)
    logs_url,logs = events(e,factory)
    fee_topic = "0x"+keccak(text="FeeAmountEnabled(uint24,int24)").hex()
    pool_topic = "0x"+keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
    fees, markets = {}, []
    head = rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    runtime = "0x"+RUNTIME.read_text().strip()
    for a in [EXEC,VIRTUAL,*set(HOLDERS.values())]:
        if rpc(e,"eth_getCode",[a,e["selector"]])!="0x":
            raise ValueError("адрес исполнителя/держателя занят кодом")
    for log in logs:
        topics = log["topics"]
        if topics[0]==fee_topic:
            fee,spacing = (int(v,16) for v in topics[1:])
            if c.one(factory,"feeAmountTickSpacing(uint24)",["uint24"],[fee],["int24"])[0]!=spacing:
                raise ValueError("изменился шаг комиссии")
            fees[fee] = spacing
        if topics[0]!=pool_topic:
            continue
        tokens = ["0x"+v[-40:] for v in topics[1:3]]
        fee = int(topics[3],16)
        spacing,pool = decode(["int24","address"],bytes.fromhex(log["data"][2:]))
        if (c.one(factory,"getPool(address,address,uint24)",["address","address","uint24"],[*tokens,fee],["address"])[0]!=pool
                or c.one(pool,"factory()",returns=["address"])[0]!=factory
                or any(c.one(pool,"token"+str(i)+"()",returns=["address"])[0]!=t for i,t in enumerate(tokens))
                or c.one(pool,"tickSpacing()",returns=["int24"])[0]!=spacing
                or c.one(pool,"fee()",returns=["uint24"])[0]!=fee):
            raise ValueError("запись PoolCreated не совпала с пулом и фабрикой")
        obj,url = deployed(e,pool)
        if obj["compilation"]["name"]!="ListaV3Pool" or sources(obj)!=canonical:
            raise ValueError("исходник пула или зависимостей отличается")
        price,tick,*_ = c.one(pool,"slot0()",returns=["uint160","int24","uint16","uint16","uint16","uint8","bool"])
        liquidity, = c.one(pool,"liquidity()",returns=["uint128"])
        center = (tick//spacing)//256
        grid = {}
        for w in range(center-1,center+2):
            ticks, = c.one(lens,"getPopulatedTicksInWord(address,int16)",["address","int16"],[pool,w],
                           ["(int24,int128,uint128)[]"])
            grid[w] = [[i,str(net),str(gross)] for i,net,gross in ticks]
        m = dict(pool=pool,tokens=tokens,fee=fee,spacing=spacing,source=url,price=str(price),tick=tick,
                 liquidity=str(liquidity),words=grid,cases=[],virtual=[])
        for i,t in enumerate(tokens):
            dec, = c.one(t,"decimals()",returns=["uint8"])
            for amount in (1,100,10):
                dx = amount*10**dec
                query = {"from":HOLDERS[t],"to":VIRTUAL,"data":calldata("quoteExactIn(address,bool,uint256)",
                                    ["address","bool","uint256"],[pool,i==0,dx])}
                result = rpc(e,"eth_call",[query,e["selector"],{VIRTUAL:{"code":"0x"+VIRTUAL_FILE.read_text().strip()}}])
                used,out = decode(["uint256","uint256"],bytes.fromhex(result[2:]))
                m["virtual"].append(dict(i=i,dx=str(dx),used=str(used),out=str(out),request=query,response=result))
                if used!=dx or not out:
                    native = c.one(quoter,"quoteExactInputSingle("+PARAM+")",[PARAM],[(t,tokens[1-i],dx,fee,0)],
                                   ["uint256","uint160","uint32","uint256"])
                    if out!=native[0]:
                        raise ValueError("неполная котировка не совпала с QuoterV2")
                    m["virtual"][-1]["native"] = list(map(str,native))
                    continue
                case = exchange(e,pool,quoter,t,tokens[1-i],dx,fee,int(head["timestamp"],16)+300,runtime)
                if int(case["out"])!=out:
                    raise ValueError("официальный и виртуальный квотеры расходятся")
                m["cases"].append(case)
                if sum(case["source"]==t for case in m["cases"])==2:
                    break
        markets.append(m)
    if {m["pool"] for m in markets}!=documented:
        raise ValueError("список официальной таблицы и события отличаются")
    write(FOLDER/"lista_ethereum.json",dict(chain_id=1,selector=e["selector"],endpoint=e["endpoint"],official=DOC,
          factory=factory,quoter=quoter,lens=lens,factory_source=factory_url,quoter_source=quoter_url,
          reference_source=ref_url,implementation_files=canonical,log_index=logs_url,logs=logs,tiers=fees,markets=markets,
          executor=EXEC,runtime_sha256=hashlib.sha256(bytes.fromhex(runtime[2:])).hexdigest(),calls=c.calls,
          virtual_quoter=VIRTUAL,virtual_sha256=hashlib.sha256(bytes.fromhex(VIRTUAL_FILE.read_text().strip())).hexdigest()))
    base = read(FOLDER/"ethereum.json")
    base["contracts"].update(e["contracts"])
    write(FOLDER/"ethereum.json",base)
    path = ROOT/"config/first_run_dex.json"
    profiles = [p for p in read(path) if (p["network"],p["id"])!=("ethereum","lista-dex-v3")]
    profiles.append(dict(network="ethereum",id="lista-dex-v3",chain_id=1,source=DOC,settings={
        "dex.kind":"v3_fork","dex.factory":factory,"dex.quoter_virtual":True,"dex.tick_lens":lens,
        "dex.v3_fee_tiers":[str(f) for f in sorted(fees)]}))
    write(path,profiles)
    print(len(markets),len(canonical),len(fees),sum(len(m["cases"]) for m in markets))


if __name__=="__main__":
    main()
