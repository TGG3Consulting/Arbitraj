"""Обследует все пулы Lista StableSwap Ethereum, текущие паузы и исторические обмены на той же реализации."""
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
import hashlib

import requests
from eth_abi import decode
from eth_utils import keccak

from record_lista_ethereum import FOLDER, HOLDERS, ROOT, Capture, code, deployed, get, read, rpc, write
from record_pancake_stable_exchange import simulate
from record_woo_base import SLOT

API = "https://api.lista.org/api/moolah/"
REFERENCE = "https://raw.githubusercontent.com/lista-dao/moolah/eb7d73a/src/dex/StableSwapPool.sol"
HISTORIC_POOL = "0x35c9a4dae1ff05788f24b5b32721d89340cbb636"
PAUSE_HEIGHT = 26009038


def market_directory():
    page = lambda i:get(API+"borrow/marketList", {"page":i,"pageSize":200})["data"]
    first = page(1)
    with ThreadPoolExecutor(3) as threads:
        pages = [first,*threads.map(page,range(2,(first["total"]+199)//200+1))]
        rows = [row for batch in pages for row in batch["list"]]
        if len(rows)!=first["total"] or len({(r["chain"],r["marketId"]) for r in rows})!=len(rows):
            raise ValueError("неполный или повторный рынок API")
        ids = [r["marketId"] for r in rows if r["chain"]=="ethereum" and r["isSmartLending"]]
        def detail(ident):
            result = get(API+"market/"+ident,{"chain":"ethereum"})["data"]["smartCollateralConfig"]
            return {k:result[k].lower() for k in ("swapPool","token0","token1")}
        return dict(total=first["total"],pages=len(pages),eth_smart=dict(zip(ids,threads.map(detail,ids),strict=True)))


def state(c,p):
    fields = {}
    for key in ["initial_A","future_A","initial_A_time","future_A_time","fee","admin_fee",
                "price0DiffThreshold","price1DiffThreshold"]:
        fields[key] = str(c.one(p,key+"()",returns=["uint256"])[0])
    for key in ["paused","skipPriceDiff"]:
        fields[key] = c.one(p,key+"()",returns=["bool"])[0]
    for key in ["balances","RATES"]:
        fields[key] = [str(c.one(p,key+"(uint256)",["uint256"],[i],["uint256"])[0]) for i in (0,1)]
    return fields


def main():
    e = read(FOLDER/"ethereum.json")
    c = Capture(e)
    directory = market_directory()
    api_pools = {r["swapPool"] for r in directory["eth_smart"].values()}
    v3 = {m["pool"] for m in read(FOLDER/"lista_ethereum.json")["markets"]}
    if HISTORIC_POOL not in api_pools:
        raise ValueError("нет опорной пары USDC/USDT в каталоге Lista")
    factory, = c.one(HISTORIC_POOL,"STABLESWAP_FACTORY()",returns=["address"])
    count, = c.one(factory,"pairLength()",returns=["uint256"])
    new_pool_impl, = c.one(factory,"swapImpl()",returns=["address"])
    pool_impl = "0x"+rpc(e,"eth_getStorageAt",[HISTORIC_POOL,SLOT,e["selector"]])[-40:]
    fact_impl = "0x"+rpc(e,"eth_getStorageAt",[factory,SLOT,e["selector"]])[-40:]
    for a in [factory,fact_impl,pool_impl,new_pool_impl]:
        code(e,a)
    factory_source = f"https://eth.blockscout.com/api/v2/smart-contracts/{fact_impl}"
    if get(factory_source)["name"]!="StableSwapFactory":
        raise ValueError("неизвестная реализация фабрики")
    obj,pool_source = deployed(e,pool_impl)
    path = obj["compilation"]["fullyQualifiedName"].split(":")[0]
    actual = obj["sources"][path]["content"]
    response = requests.get(REFERENCE,timeout=20)
    response.raise_for_status()
    reference = response.text
    # Две версии различаются только проверкой паузы при внесении ликвидности и donate_admin_fees.
    # Заголовок exchange и все его зависимости сравниваются целиком, включая пробелы.
    start = reference.index("  function add_liquidity(")
    end = reference.index("{",start)+1
    normalized = reference[:start]+reference[start:end].replace("whenNotPaused ","")+reference[end:]
    normalized = normalized.replace("function donate_admin_fees() external onlyRole(MANAGER)",
                                    "function donate_admin_fees() external whenNotPaused onlyRole(MANAGER)")
    if actual!=normalized:
        raise ValueError("изменён исходник математики, обмена или защиты")
    pools = []
    observed = set()
    for i in range(count):
        p, = c.one(factory,"swapPairContract(uint256)",["uint256"],[i],["address"])
        tokens = [c.one(p,"coins(uint256)",["uint256"],[i],["address"])[0] for i in (0,1)]
        ident, = c.one(p,"STABLESWAP_FACTORY()",returns=["address"])
        registered, = c.one(factory,"getPairInfos(address,address)",["address","address"],tokens,
                            ["(address,address,address,address)[]"])
        impl = "0x"+rpc(e,"eth_getStorageAt",[p,SLOT,e["selector"]])[-40:]
        if ident!=factory or impl!=pool_impl or all(r[0]!=p or list(r[1:3])!=tokens for r in registered):
            raise ValueError("пул, реализация или монеты отличаются от реестра")
        observed.add(p)
        entry = dict(pool=p,tokens=tokens,implementation=impl,paused=c.one(p,"paused()",returns=["bool"])[0],reverts=[])
        for side in (0,1):
            call = (p,"exchange(uint256,uint256,uint256,uint256)",["uint256"]*4,[side,1-side,1,0])
            c.batch([call])
            reply = c.calls[-1]["response"]
            if not entry["paused"] or not isinstance(reply,dict) or reply.get("data")!="0x"+keccak(text="EnforcedPause()")[:4].hex():
                raise ValueError("отказ не доказан как EnforcedPause")
            entry["reverts"].append(dict(side=side,response=reply["data"]))
        pools.append(entry)
    if observed | v3 != api_pools:
        raise ValueError("фабрики и каталог кредитных рынков расходятся")
    pause_block = rpc(e,"eth_getBlockByNumber",[hex(PAUSE_HEIGHT),False])
    topic = "0x"+keccak(text="Paused(address)").hex()
    pause_log, = rpc(e,"eth_getLogs",[{"blockHash":pause_block["hash"],"address":HISTORIC_POOL,"topics":[topic]}])
    historic = deepcopy(e)
    historic["selector"] = {"blockHash":pause_block["parentHash"],"requireCanonical":True}
    old_header = rpc(historic,"eth_getBlockByHash",[historic["selector"]["blockHash"],False])
    old = Capture(historic)
    st = state(old,HISTORIC_POOL)
    if st["paused"] or "0x"+rpc(historic,"eth_getStorageAt",[HISTORIC_POOL,SLOT,historic["selector"]])[-40:]!=pool_impl:
        raise ValueError("на предшествующем блоке контракт не активен или имеет другую реализацию")
    tokens = next(r["tokens"] for r in pools if r["pool"]==HISTORIC_POOL)
    swaps = []
    for i in (0,1):
        for dx in (10**6,100*10**6):
            out, = old.one(HISTORIC_POOL,"get_dy(uint256,uint256,uint256)",["uint256"]*3,[i,1-i,dx],["uint256"])
            quote = dict(i=i,j=1-i,dx=str(dx),dy=str(out))
            market = dict(pool=HISTORIC_POOL,coins=tokens)
            expected,_ = simulate(historic,market,quote,None,HOLDERS)
            _,case = simulate(historic,market,quote,expected,HOLDERS)
            swaps.append(dict(**quote,out=str(expected),**case))
    data = dict(selector=e["selector"],endpoint=c.endpoint,directory=directory,factory=factory,
                factory_implementation=fact_impl,factory_source=factory_source,pool_implementation=pool_impl,
                implementation_for_new_pools=new_pool_impl,
                pool_source=pool_source,reference_source=REFERENCE,source_sha256=hashlib.sha256(actual.encode()).hexdigest(),
                pools=pools,calls=c.calls,historical=dict(selector=historic["selector"],pool=HISTORIC_POOL,
                block=int(old_header["number"],16),timestamp=int(old_header["timestamp"],16),
                pause_log=pause_log,state=st,swaps=swaps,calls=old.calls))
    write(FOLDER/"lista_stable_ethereum.json",data)
    store = read(FOLDER/"ethereum.json")
    store["contracts"].update(e["contracts"])
    write(FOLDER/"ethereum.json",store)
    config = ROOT/"config/first_run_dex.json"
    profiles = [p for p in read(config) if (p["network"],p["id"])!=("ethereum","lista-dex-stableswap")]
    profiles.append(dict(chain_id=1,network="ethereum",id="lista-dex-stableswap",source=API+"borrow/marketList",settings={
        "dex.kind":"lista_stableswap_proxy","dex.factory":factory,"dex.pool_implementation":pool_impl}))
    write(config,profiles)
    print(count,len(directory["eth_smart"]),len(swaps))


if __name__=="__main__":
    main()
