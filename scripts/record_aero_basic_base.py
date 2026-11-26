"""Aero Basic Base: проверяет реестры обеих кривых, код и фактический вход/выход V5 в обоих направлениях."""
import hashlib

import requests
from bs4 import BeautifulSoup
from eth_utils import keccak

from record_lista_ethereum import Capture, EXEC, FOLDER, RUNTIME, STEP, calldata, deployed, read, rpc, write
from record_solidly_v3 import BASE_WETH, HOLDERS

DOC = "https://app.aero.xyz/docs/security/"
ZERO = "0x" + "00" * 20
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
MARKETS = {
    "stable": [(USDC, "0x877014e21c32fea108b6a1f45f367efc9a2d9b9f"),
               (BASE_WETH, "0x7ba6f01772924a82d9626c126347a28299e98c98")],
    "volatile": [(USDC, BASE_WETH), (USDC, "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf")],
}


def round_trip(e, pool, source, target, amount, deadline, runtime, kind=26, owner=None, funding=None):
    owner = owner or HOLDERS["base"][source]
    word = lambda n: "0x" + format(n, "064x")
    override = {EXEC: {"code": "0x"+runtime, "stateDiff": {
        word(0): word(int(owner, 16)), word(1): word(int(BASE_WETH, 16))}}}

    def call(to, data, sender=owner):
        return {"to": to, "from": sender, "data": data, "gas": hex(1200000)}

    def quote(token, n):
        method = "getAmountOut(uint256,address)" if kind == 3 else "getAmountOutWithTotalFee(uint256,address)"
        return call(pool, calldata(method,
                                   ["uint256", "address"], [n, token]), EXEC)

    def swap(token_in, token_out, n, minimum):
        signature = "execute(address,uint256,address,uint256,uint256,"+STEP+"[])"
        types = ["address", "uint256", "address", "uint256", "uint256", STEP+"[]"]
        return call(EXEC, calldata(signature, types, [token_in, n, token_out, minimum, deadline,
                    [(kind, pool, token_in, token_out, 10000, b"")]]))

    def approve(token, n):
        return call(token, calldata("approve(address,uint256)", ["address", "uint256"], [EXEC, n]))

    def balance(token, who):
        return call(token, calldata("balanceOf(address)", ["address"], [who]))

    def simulate(entries):
        calls = [v for _, v in entries]
        stage, = rpc(e, "eth_simulateV1", [{"blockStateCalls": [{"stateOverrides": override, "calls": calls}],
                                          "validation": False}, e["selector"]])
        if stage["parentHash"] != e["selector"]["blockHash"]:
            raise ValueError("хеш исходного блока изменился")
        results = dict(zip((k for k, _ in entries), stage["calls"], strict=True))
        for k, r in results.items():
            if r["status"] != ("0x0" if k.startswith("above") else "0x1"):
                raise ValueError((pool, amount, k, r.get("error")))
        return results

    prefix = [] if funding is None else [("fund_wallet",call(source,calldata("transfer(address,uint256)",["address","uint256"],[owner,amount]),funding))]
    head = prefix+[("before_in", balance(source, owner)), ("before_out", balance(target, owner)),
            ("approve_in", approve(source, amount)), ("quote_buy", quote(source, amount))]
    first = simulate(head)
    n = lambda results, k: int(results[k]["returnData"], 16)
    bought = n(first, "quote_buy")
    buy = head + [("above_buy", swap(source, target, amount, bought+1)),
                  ("buy", swap(source, target, amount, bought)),
                  ("buy_in", balance(source, owner)), ("buy_out", balance(target, owner)),
                  ("after_reserves", call(pool, calldata("getReserves()"))),
                  ("quote_sell", quote(target, bought))]
    sold = n(simulate(buy), "quote_sell")
    entries = buy + [("approve_out", approve(target, bought)), ("above_sell", swap(target, source, bought, sold+1)),
                     ("sell", swap(target, source, bought, sold)), ("sell_in", balance(target, owner)),
                     ("sell_out", balance(source, owner)), ("left_source", balance(source, EXEC)),
                     ("left_target", balance(target, EXEC))]
    results = simulate(entries)
    value = lambda k: n(results, k)
    if (not sold or (value("before_in")-value("buy_in"), value("buy_out")-value("before_out")) != (amount,bought)
            or (value("buy_out")-value("sell_in"), value("sell_out")-value("buy_in")) != (bought,sold)
            or (value("buy"),value("sell")) != (bought,sold) or value("left_source") or value("left_target")):
        raise ValueError((pool, "расхождение фактической выплаты или остатка исполнителя"))
    return dict(owner=owner, token_in=source, amount=str(amount), bought=str(bought), sold=str(sold),
                calls=[c for _,c in entries], results=results)


def capture():
    e = read(FOLDER/"base.json")
    c = Capture(e)
    head = rpc(e, "eth_getBlockByHash", [e["selector"]["blockHash"], False])
    deadline = int(head["timestamp"], 16)+300
    runtime = RUNTIME.read_text().strip().removeprefix("0x")
    page = requests.get(DOC,timeout=22)
    page.raise_for_status()
    table = [tr.find_all("td") for tr in BeautifulSoup(page.content,"html.parser").select("tr")]
    docs = {ts[0].get_text(strip=True):ts[2].get_text(strip=True).lower() for ts in table
            if len(ts)==3 and ts[1].get_text(strip=True)=="All chains"}
    factories, contracts = [], {}
    for curve, pairs in MARKETS.items():
        factory = docs[curve.title()+"PoolFactory"]
        fee = c.one(factory, "feeModule()", returns=["address"])[0]
        impl = c.one(factory, "implementation()", returns=["address"])[0]
        mev = c.one(factory, "mevTaxModule()", returns=["address"])[0]
        paused = c.one(factory, "isPaused()", returns=["bool"])[0]
        if fee != docs[f"CustomFeeModule ({curve} pools)"] or mev != ZERO or paused:
            raise ValueError("состав модулей или пауза изменились")
        for a, name in [(factory, curve.title()+"PoolFactory"), (fee, "CustomFeeModule"), (impl, curve.title()+"Pool")]:
            obj, url = deployed(e, a)
            if obj["compilation"]["name"] != name:
                raise ValueError("другой исходник размещения")
            code = bytes.fromhex(obj["runtimeBytecode"]["onchainBytecode"][2:])
            path = obj["compilation"]["fullyQualifiedName"].split(":")[0]
            contracts[a] = dict(contract=name,url=url,bytes=len(code),keccak256="0x"+keccak(code).hex(),
                                source_sha256=hashlib.sha256(obj["sources"][path]["content"].encode()).hexdigest())
        all_pools = c.one(factory, "allPools()", returns=["address[]"])[0]
        count = c.one(factory, "allPoolsLength()", returns=["uint256"])[0]
        if count!=len(set(all_pools)) or count!=len(all_pools):
            raise ValueError("длина или уникальность реестра")
        markets = []
        for source, target in pairs:
            pool = c.one(factory, "getPool(address,address)", ["address"]*2, [source,target], ["address"])[0]
            clone = rpc(e,"eth_getCode",[pool,e["selector"]])
            if pool not in all_pools or c.one(pool,"factory()",returns=["address"])[0]!=factory or clone!=(
                    "0x363d3d373d3d3d363d73"+impl[2:]+"5af43d82803e903d91602b57fd5bf3"):
                raise ValueError((pool, "несовпадение реестра или кода клона",clone[:110]))
            kind = c.one(pool,"POOL_TYPE()",returns=["bytes32"])[0].rstrip(b"\0").decode()
            dec0,dec1,r0,r1,t0,t1 = c.one(pool,"metadata()",returns=["uint256"]*4+["address"]*2)
            if kind != "V2_"+curve.upper() or {t0,t1}!={source,target}:
                raise ValueError("кривая или пара изменились")
            fee_signature = "getBaseFee(address,address,uint256,uint256,uint256,uint256)"
            fees = []
            for caller, a0, a1 in [(ZERO,0,0),(EXEC,dec0,0),(EXEC,0,dec1)]:
                fees.append(c.one(factory,fee_signature,["address"]*2+["uint256"]*4,
                                  [pool,caller,a0,a1,r0,r1],["uint256"])[0])
            if len(set(fees))!=1:
                raise ValueError("комиссия зависит от входа или плательщика")
            cases = [round_trip(e,pool,source,target,n,deadline,runtime)
                     for n in ((10**14,10**15) if source==BASE_WETH else (10**6,10**7))]
            markets.append(dict(pool=pool,tokens=[t0,t1],curve=kind,precision=[str(dec0),str(dec1)],
                                reserves=[str(r0),str(r1)],base_fee=fees[0],cases=cases))
        factories.append(dict(curve=curve,factory=factory,fee_module=fee,mev_module=mev,implementation=impl,
                              count=count,markets=markets))
    return dict(chain_id=e["chain_id"],selector=e["selector"],endpoint=e["endpoint"],source=DOC,
                v5_runtime="0x"+keccak(bytes.fromhex(runtime)).hex(),contracts=contracts,factories=factories,reads=c.calls)


if __name__=="__main__":
    write(FOLDER/"aero_basic_base.json",capture())
