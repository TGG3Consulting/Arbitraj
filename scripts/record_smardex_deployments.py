"""Сверяет официальные фабрики SmarDex, собственные пары, тарифы и точные котировки основной сети."""
import json
import re

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_curve_ng_deployments import code
from record_dodo_deployments import MULTICALL

REV = "7ab2beb2afe0e5703c06dfd6eb6bf904c63dc72e"
BASE_URL = f"https://raw.githubusercontent.com/SmarDex-Dev/smart-contracts/{REV}/deployments"
LIBRARY = "contracts/core/libraries/SmardexLibrary.sol"


def get(url):
    r = requests.get(url,timeout=20)
    r.raise_for_status()
    return r.json()


def run(network,e):
    prefix = f"{BASE_URL}/{network}"
    f = get(prefix+"/SmardexFactory.json")
    router = get(prefix+"/SmardexRouter.json")["address"]
    factory = f["address"]
    c = Capture(e)
    if code(e,factory) != "0x"+keccak(bytes.fromhex(f["deployedBytecode"][2:])).hex():
        raise ValueError("код фабрики расходится с манифестом")
    code(e,router)
    actual_factory, = c.one(router,"factory()",returns=["address"])
    if actual_factory != factory.lower():
        raise ValueError("роутер другой фабрики")
    compiler_source = f"{prefix}/solcInputs/{f['solcInputHash']}.json"
    source = get(compiler_source)["sources"][LIBRARY]["content"]
    source_hash = "0x"+keccak(text=source).hex()
    if json.loads(f["metadata"])["sources"][LIBRARY]["keccak256"] != source_hash:
        raise ValueError("контрольная сумма библиотеки математики")
    exact_equal = re.search(r"if\s*\(_x\s*==\s*_y\)\s*\{\s*return true;", source) is not None
    count, = c.one(factory,"allPairsLength()",returns=["uint256"])
    def aggregate(calls):
        out, = c.one(MULTICALL,"tryAggregate(bool,(address,bytes)[])",["bool","(address,bytes)[]"],[False,calls],["(bool,bytes)[]"])
        return out
    listings = aggregate([(factory,keccak(text="allPairs(uint256)")[:4]+encode(["uint256"],[i])) for i in range(count)])
    pools = [decode(["address"],data)[0] for ok,data in listings if ok]
    if len(pools)!=count:
        raise ValueError("прерван список пулов")
    owners = aggregate([(pool,keccak(text="factory()")[:4]) for pool in pools])
    fees = aggregate([(pool,keccak(text="getPairFees()")[:4]) for pool in pools])
    lineage = []
    for p,(valid,owner),(has_fee,raw) in zip(pools,owners,fees,strict=True):
        addr = decode(["address"],owner)[0] if valid else None
        lineage.append(dict(pool=p,owner=addr,fees=bool(has_fee and len(raw)==64),own=addr==factory.lower()))
    states = []
    for row in lineage:
        if not (row["own"] and row["fees"]):
            continue
        p = row["pool"]
        a, = c.one(p,"token0()",returns=["address"])
        b, = c.one(p,"token1()",returns=["address"])
        match, = c.one(factory,"getPair(address,address)",["address","address"],[a,b],["address"])
        if match != p:
            continue
        reserves = c.one(p,"getReserves()",returns=["uint256"]*2)
        if min(reserves)<10000:
            continue
        fictive = c.one(p,"getFictiveReserves()",returns=["uint256"]*2)
        price = c.one(p,"getPriceAverage()",returns=["uint256"]*3)
        fee = c.one(p,"getPairFees()",returns=["uint256"]*2)
        quotes = []
        for i in (0,1):
            for fraction in (10000,100):
                amount = reserves[i]//fraction
                raw, = c.batch([(router,"getAmountOutFromPair(uint256,address,address)",["uint256","address","address"],
                                 [amount,(a,b)[i],(a,b)[1-i]])])
                if raw is not None and len(raw)==160:
                    out = decode(["uint256"]*5,raw)[0]
                    if out:
                        quotes.append(dict(i=i,amount_in=str(amount),amount_out=str(out)))
        if len(quotes)==4:
            states.append(dict(pool=p,tokens=[a,b],reserves=list(map(str,reserves)),fictive=list(map(str,fictive)),
                           price=list(map(str,price)),fee=list(map(str,fee)),quotes=quotes))
        if len(states)==3:
            break
    if len(states)<2:
        raise ValueError("недостаточно пулов с котировками")
    tup="("+",".join(["uint256"]*7+["uint128"]*2)+")"
    params=[21,2000,900,100,200,100,200,5000,1000]
    pure = c.one(router,"getAmountOut("+tup+")",[tup],[params],["uint256"]*5)
    head = requests.post(e["endpoint"],json=dict(id=1,jsonrpc="2.0",method="eth_getBlockByHash",
                         params=[e["selector"]["blockHash"],False]),timeout=20).json()["result"]
    proof = dict(chain_id=e["chain_id"],selector=e["selector"],timestamp=int(head["timestamp"],16),source=compiler_source,
                 library_keccak=source_hash,explicit_equality=exact_equal,factory=factory,router=router,lineage=lineage,
                 pools=states,equal_price_example=dict(params=params,result=list(map(str,pure))),calls=c.calls)
    profile = dict(chain_id=e["chain_id"],network=network,id="smardex-"+network,source=prefix+"/SmardexFactory.json",
                   settings={"dex.kind":"smardex_exact_equality" if exact_equal else "smardex",
                             "dex.factory":factory,"dex.quoter_virtual":True})
    return proof,profile


if __name__=="__main__":
    folder = ROOT/"tests/data/first_run_dex_2026-11-26"
    target = ROOT/"config/first_run_dex.json"
    profiles = []
    for net in ("base","ethereum"):
        path = folder/f"{net}.json"
        evidence = read(path)
        proof,profile = run(net,evidence)
        write(path,evidence)
        write(folder/f"smardex_{net}.json",proof)
        profiles.append(profile)
        print(net,len(proof["pools"]),proof["explicit_equality"],flush=True)
    ids={(p["network"],p["id"]) for p in profiles}
    write(target,[r for r in read(target) if (r["network"],r["id"]) not in ids]+profiles)
