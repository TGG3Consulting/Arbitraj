"""Сверяет адреса Classic и Elastic с официальной таблицей Kyber, реестром и котировками двух сетей."""
import argparse
import io
import re
import tarfile
from functools import cache
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_curve_ng_deployments import code

FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"
DOC = "https://docs.kyberswap.com/reference/legacy/kyberswap-{kind}/contracts/{kind}-contract-addresses.md"
ELASTIC_REPO = "https://github.com/KyberNetwork/ks-elastic-sc"
ELASTIC_REV = "4ab08c0a"


@cache
def canonical_elastic_sources():
    response = requests.get(f"https://api.github.com/repos/KyberNetwork/ks-elastic-sc/tarball/{ELASTIC_REV}",timeout=25)
    response.raise_for_status()
    with tarfile.open(fileobj=io.BytesIO(response.content)) as archive:
        return {entry.name.split("/",1)[1]: archive.extractfile(entry).read() for entry in archive
                if entry.isfile() and "/contracts/" in entry.name and entry.name.endswith(".sol")}


def verified_elastic(e, factory, pool):
    """Сверяет фактический initcode пула с фабрикой и собственным подтверждённым исходником пула."""
    url=(f"https://sourcify.dev/server/v2/contract/{e['chain_id']}/{pool}?fields=sources,"
         "runtimeBytecode.onchainBytecode,creationBytecode.onchainBytecode")
    response=requests.get(url,timeout=25)
    response.raise_for_status()
    j=response.json()
    chain_code=code(e,pool)
    raw=bytes.fromhex(j["runtimeBytecode"]["onchainBytecode"][2:])
    if j["runtimeMatch"] not in ("exact_match","match") or chain_code != "0x"+keccak(raw).hex():
        raise ValueError("байткод пула Elastic расходится с Sourcify")
    c=Capture(e)
    creation,=c.one(factory,"getCreationCode()",returns=["bytes"])
    init_hash,=c.one(factory,"poolInitHash()",returns=["bytes32"])
    if (j["creationMatch"] not in ("match","exact_match") or keccak(creation)!=init_hash
            or creation!=bytes.fromhex(j["creationBytecode"]["onchainBytecode"][2:])):
        raise ValueError("initcode размещённого пула расходится с его фабрикой")
    canonical=canonical_elastic_sources()
    files={}
    for path, source in j["sources"].items():
        if path.startswith("contracts/") and (path.startswith("contracts/libraries/") or path in
                ("contracts/Pool.sol","contracts/PoolStorage.sol","contracts/PoolTicksState.sol")):
            if canonical.get(path) != source["content"].encode():
                raise ValueError(f"исходник размещения отличается: {path}")
            files[path]=keccak(text=source["content"]).hex()
    if not {"contracts/Pool.sol","contracts/PoolStorage.sol","contracts/PoolTicksState.sol","contracts/libraries/SwapMath.sol"}.issubset(files):
        raise ValueError("неполные исходники Elastic")
    return dict(verified=url, repo=ELASTIC_REPO, revision=ELASTIC_REV, runtime=chain_code, source_keccak=files,
                pool=pool, factory=factory, pool_init_hash="0x"+init_hash.hex(), creation_bytes=len(creation))


def deployment(kind, network):
    article = DOC.format(kind=kind)
    response = requests.get(article, timeout=20)
    response.raise_for_status()
    title = "Base (ChainID: 8453)" if network == "base" else "Ethereum (ChainID: 1)"
    section = response.text.split(f"<summary>{title}</summary>")[1].split("</details>")[0]
    return {label: address.lower() for label, address in
            re.findall(r"\*\*([^*]+):\*\* \[.?(0x[\da-fA-F]{40})", section)}


def contract(c, address, signature, args=(), values=()):
    return c.one(address, signature, args, values, ["address"])[0]


def classic(c, factory, quoter, candidates):
    total, = c.one(factory, "allPoolsLength()", returns=["uint256"])
    rows = []
    for index in range(min(total, 16)):
        candidates.append(contract(c, factory, "allPools(uint256)", ["uint256"], [index]))
    for pool in dict.fromkeys(candidates):
        if contract(c, pool, "factory()") != factory:
            continue
        tokens = [contract(c, pool, f"token{i}()") for i in range(2)]
        registered, = c.one(factory, "isPool(address,address,address)", ["address"]*3,
                            tokens + [pool], ["bool"])
        if not registered:
            raise ValueError("пул сообщил фабрику, которая его не подтверждает")
        trade = c.one(pool, "getTradeInfo()", returns=["uint112"]*4 + ["uint256"])
        quotes = []
        for i in range(2):
            for divisor in (10000, 100):
                dx = trade[i] // divisor
                if not dx:
                    continue
                raw, = c.batch([(quoter, "getAmountsOut(uint256,address[],address[])",
                                  ["uint256", "address[]", "address[]"], [dx, [pool], [tokens[i], tokens[1-i]]])])
                if raw:
                    amounts, = decode(["uint256[]"], raw)
                    if len(amounts) == 2 and amounts[0] == dx and amounts[1] > 0:
                        quotes.append(dict(i=i, dx=str(dx), out=str(amounts[1])))
        rows.append(dict(pool=pool, tokens=tokens, trade=list(map(str, trade)), quotes=quotes))
        if sum(len(row["quotes"]) == 4 for row in rows) == 2:
            break
    return dict(factory_count=total, pools=rows)


def elastic_ticks(c, reader, pool):
    chunk = 1000  # Параметр решения 475. Здесь проверяются части такой же длины на обоих узлах.
    start = -887272
    initialized = []
    while True:
        ticks, = c.one(reader, "getTicksInRange(address,int24,uint32)",
                       ["address","int24","uint32"], [pool,start,chunk], ["int24[]"])
        if len(ticks) != chunk or ticks[0] != start:
            raise ValueError("разрыв списка тиков")
        end = 887272 in ticks
        cut = ticks[:ticks.index(887272)+1] if end else ticks
        if any(a >= b for a, b in zip(cut, cut[1:])):
            raise ValueError("тики читателя не возрастают")
        initialized.extend(cut if not initialized else cut[1:])
        if end:
            break
        start = cut[-1]
    states = []
    for offset in range(0,len(initialized),40):
        group=initialized[offset:offset+40]
        query=[(pool,False,keccak(text="ticks(int24)")[:4]+encode(["int24"],[tick])) for tick in group]
        result,=c.one("0xca11bde05977b3631167028862be2a173976ca11", "aggregate3((address,bool,bytes)[])",
                      ["(address,bool,bytes)[]"],[query],["(bool,bytes)[]"])
        for tick, (ok, raw) in zip(group,result,strict=True):
            if not ok:
                raise ValueError("не прочитан тик")
            gross, net, _, _ = decode(["uint128","int128","uint256","uint128"],raw)
            states.append(dict(tick=tick,gross=str(gross),net=str(net)))
    return states


def elastic(c, s, candidates):
    rows = []
    for pool in candidates:
        if contract(c, pool, "factory()") != s["dex.factory"]:
            continue
        tokens = [contract(c, pool, f"token{i}()") for i in range(2)]
        fee, = c.one(pool, "swapFeeUnits()", returns=["uint24"])
        distance, = c.one(pool, "tickDistance()", returns=["int24"])
        spacing, = c.one(s["dex.factory"], "feeAmountTickDistance(uint24)", ["uint24"], [fee], ["int24"])
        actual = contract(c, s["dex.factory"], "getPool(address,address,uint24)",
                          ["address", "address", "uint24"], tokens + [fee])
        if actual != pool or distance != spacing or spacing <= 0:
            raise ValueError(f"{pool} не совпадает с реестром")
        state = c.one(pool, "getPoolState()", returns=["uint160","int24","int24","bool"])
        liquidity = c.one(pool, "getLiquidityState()", returns=["uint128"]*3)
        ticks = elastic_ticks(c,s["dex.tick_reader"],pool)
        quotes = []
        for i in range(2):
            balance, = c.one(tokens[i], "balanceOf(address)", ["address"], [pool], ["uint256"])
            for divisor in (100000, 10000):
                dx = balance // divisor
                if not dx:
                    continue
                abi = "(address,address,uint256,uint24,uint160)"
                raw, = c.batch([(s["dex.quoter"], f"quoteExactInputSingle({abi})", [abi],
                                 [(tokens[i], tokens[1-i], dx, fee, 0)])])
                if raw:
                    used, out, after, crossed, gas = decode(["uint256","uint256","uint160","uint32","uint256"], raw)
                    if used == dx and out:
                        quotes.append(dict(i=i,dx=str(dx),out=str(out),after=str(after),crossed=crossed,gas=gas))
        rows.append(dict(pool=pool,tokens=tokens,fee=fee,spacing=spacing,ticks=ticks,
                         price=str(state[0]),tick=state[1],nearest=state[2],locked=state[3],
                         liquidity=list(map(str,liquidity)),quotes=quotes))
    return dict(pools=rows)


def elastic_fees(c, e, factory, pools):
    # Журнал Ethereum задаёт кандидатов и для Base. Каждая сеть подтверждает ступени своей фабрикой.
    topic = "0x" + keccak(text="SwapFeeEnabled(uint24,int24)").hex()
    eth = read(FOLDER / "ethereum.json")
    params = dict(module="logs",action="getLogs",fromBlock=0,toBlock=eth["block"],address=factory,topic0=topic)
    response = requests.get("https://eth.blockscout.com/api",params=params,timeout=20)
    response.raise_for_status()
    logs = response.json()["result"]
    if not isinstance(logs,list) or not 0 < len(logs) < 1000:
        raise ValueError("неполная страница журнала Ethereum")
    candidates = {int(ev["topics"][1],16) for ev in logs}
    candidates.update(p["fee"] for p in pools)
    confirmed = {}
    for fee in sorted(candidates):
        spacing,=c.one(factory,"feeAmountTickDistance(uint24)",["uint24"],[fee],["int24"])
        if spacing > 0:
            confirmed[str(fee)]=spacing
    return dict(ethereum_event_source=response.url, ethereum_events=logs, chain_id=e["chain_id"],
                fee_tick_distance=confirmed)


def collect(network, snapshots):
    e = read(FOLDER / f"{network}.json")
    c = Capture(e)
    samples = read(snapshots / f"{network}-integration-2026-11-26/sources/sample-pools.json")
    def markets(dex):
        return [r["address"].lower() for r in samples[dex]["samples"]]
    assigned = []
    def profile(dex, source, settings):
        for val in settings.values():
            if isinstance(val,str) and val.startswith("0x"):
                code(e,val)
        assigned.append(dict(network=network, chain_id=e["chain_id"], id=dex,
                             source=DOC.format(kind=source), settings=settings))
    evidence = {}
    if network == "ethereum":
        dex = "kyberswap_classic_ethereum"
        addresses = deployment("classic", network)
        for suffix, prefix in [("", "DMM"), ("-static", "KS")]:
            factory, router = (addresses[prefix+k] for k in ("Factory","Router"))
            data = classic(c, factory, router, markets(dex))
            evidence[dex+suffix] = data
            if any(len(row["quotes"]) == 4 for row in data["pools"]):
                profile(dex+suffix, "classic", {"dex.kind":"kyber_dmm","dex.factory":factory,"dex.quoter":router})
        if len(assigned) == 2:
            assigned[0]["settings"]["dex.sub_dex"] = [dex+"-static"]
    dex = "kyberswap-elastic-base" if network == "base" else "kyberswap_elastic"
    addresses = deployment("elastic", network)
    s = {"dex.kind":"kyber_elastic", **{"dex."+key: addresses[label] for key,label in
         [("factory","Factory"),("router","Router"),("quoter","QouterV2"),("tick_reader","TicksFeesReader")]}}
    data = elastic(c, s, markets(dex))
    data["source_match"]=verified_elastic(e,s["dex.factory"],data["pools"][0]["pool"])
    evidence[dex] = data
    data["documented_contracts"] = s
    proven = [r for r in data["pools"] if not r["locked"] and len(r["quotes"]) == 4]
    if proven:
        s["dex.elastic_ticks_chunk"] = 1000
        data["fees"] = elastic_fees(c,e,s["dex.factory"],data["pools"])
        s["dex.v3_fee_tiers"] = list(data["fees"]["fee_tick_distance"])
        profile(dex, "elastic", s)
    write(FOLDER / f"kyber_{network}.json", dict(network=network, selector=e["selector"],
          endpoint=c.endpoint, groups=evidence, calls=c.calls))
    write(FOLDER / f"{network}.json", e)
    print(network, [(dex,len(group["pools"]),[len(p["quotes"]) for p in group["pools"]]) for dex,group in evidence.items()])
    return assigned


if __name__ == "__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("snapshots",type=Path)
    snapshots=parser.parse_args().snapshots
    rows = collect("base", snapshots) + collect("ethereum", snapshots)
    path=ROOT/"config/first_run_dex.json"
    managed={"kyberswap_classic_ethereum","kyberswap_classic_ethereum-static","kyberswap-elastic-base","kyberswap_elastic"}
    write(path,[p for p in read(path) if p["id"] not in managed or p["network"] not in ("base","ethereum")]+rows)
