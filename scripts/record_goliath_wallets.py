"""Расчёт обычного кошелька через опубликованный PropAMMRouter. Сохраняет фактическую площадку каждого обмена."""
from eth_abi import decode
from eth_utils import keccak
from record_bitswap_ethereum import FUNDING, OWNER, WETH, balance, tx
from record_concave_ethereum import approval

SWAPPED = "0x" + keccak(text="Swapped(address,address,address,uint256,uint256,address)").hex()
SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"), "big") - 1)


def collect(seed, state, query):
    h = state["parent"]["selector"]
    router = seed["public_router"]["router"]
    venue = seed["venue"]
    prof = seed["public_router"]
    for address, expected in [(router, prof["runtime"]), (prof["implementation"], prof["implementation_runtime"])]:
        actual = query("eth_getCode", [address, h])
        if "0x" + keccak(hexstr=actual).hex() != expected:
            raise ValueError("изменился код PropAMMRouter")
    impl = query("eth_getStorageAt", [router, SLOT, h])
    if "0x" + impl[-40:] != prof["implementation"]:
        raise ValueError("изменилась реализация PropAMMRouter")
    if int(query("eth_call", [tx(router, "isWhitelistedVenue(address)", ["address"], [venue]), h]), 16) != 1:
        raise ValueError("Goliath отсутствует в реестре роутера")
    fallback = decode(["address"], bytes.fromhex(query("eth_call", [tx(router, "fallbackSwapRouter()"), h])[2:]))[0]
    if fallback != prof["fallback"] or query("eth_getCode", [OWNER,h]) != "0x":
        raise ValueError("штатный fallback или тип кошелька изменился")
    if int(query("eth_getBalance", [OWNER,h]),16) != 0:
        raise ValueError("исследовательский кошелёк уже финансирован")
    deadline = int(state["child_timestamp"],16) + 300

    def run(calls):
        # Единственное переопределение — собственный начальный ETH синтетического кошелька.
        blocks = query("eth_simulateV1", [{"blockStateCalls": [{"stateOverrides":{OWNER:{"balance":hex(10**18)}}, "calls":calls}]},h])
        if len(blocks)!=1 or blocks[0]["parentHash"]!=h["blockHash"] or len(blocks[0]["calls"])!=len(calls):
            raise ValueError("дочерний блок, число ответов")
        return blocks[0]["calls"]

    rows = []
    for m in seed["markets"]:
        for src,dst in (m["tokens"],m["tokens"][::-1]):
            for qty in ((10**14,10**16) if src==WETH else (10**6,10**7)):
                fund = tx(WETH,"deposit()",value=qty) if src==WETH else tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[qty,[WETH,src],OWNER,deadline],10**17)
                reads = [balance(t,w) for w in (OWNER,venue,router) for t in (src,dst)]
                prefix = [fund,approval(src,router,qty),*reads]
                def order(minimum,expiry):
                    msg=tx(router,"swapViaVenueV1(address,address,address,uint256,uint256,address,uint256)",["address","address","address","uint256","uint256","address","uint256"],[venue,src,dst,qty,minimum,OWNER,expiry])
                    msg["gas"]=hex(3_000_000)
                    return msg
                calls=run([*prefix,order(1,deadline),*reads])
                if any(c["status"]!="0x1" for c in calls[:len(prefix)]):
                    raise ValueError("собственное финансирование")
                trade=calls[len(prefix)]
                info=dict(input=src,output=dst,amount=qty,requested=venue)
                if trade["status"]!="0x1":
                    rows.append({**info,"error":trade.get("error")})
                    continue
                returned,used=decode(["uint256","address"],bytes.fromhex(trade["returnData"][2:]))
                def values(rs):
                    if any(v["status"]!="0x1" for v in rs):raise ValueError("балансы")
                    return [int(v["returnData"],16) for v in rs]
                before=values(calls[2:len(prefix)]);after=values(calls[-len(reads):]);credited=after[1]-before[1]
                events=[l for l in trade["logs"] if l["address"].lower()==venue and l["topics"][0].lower()==SWAPPED]
                if used not in (venue,fallback) or len(events)!=int(used==venue) or before[0]-after[0]!=qty or credited!=returned or before[2:]!=after[2:] or returned<=0:
                    raise ValueError("физический расход, выплата или её источник")
                if used==venue:
                    ev=events[0];expected=[router,src,dst]
                    if [int(x,16) for x in ev["topics"][1:]]!=[int(x,16) for x in expected] or decode(["uint256","uint256","address"],bytes.fromhex(ev["data"][2:]))!=(qty,returned,OWNER.lower()):
                        raise ValueError("событие Goliath не соответствует суммам кошелька")
                exact=order(returned,deadline);checked=run([*prefix,order(1,1),order(returned+1,deadline),*reads,exact,*reads])
                idx=len(prefix);expired,strict=checked[idx:idx+2];offset=idx+2
                # При отказе Goliath Router может подобрать лучший запасной рынок вместо отката.
                if expired["status"]=="0x1":raise ValueError("deadline")
                if strict["status"]=="0x1":
                    better,alternative=decode(["uint256","address"],bytes.fromhex(strict["returnData"][2:]))
                    if alternative==used or better<returned+1:raise ValueError("минимум")
                    # Проверка минимума расходует собственные средства. Точное исполнение проверяется отдельно.
                    checked=run([*prefix,order(1,1),*reads,exact,*reads])
                    offset=idx+1
                if values(checked[offset:offset+len(reads)])!=before:raise ValueError("отклонённое поручение изменило балансы")
                accepted=checked[offset+len(reads)]
                if accepted["status"]!="0x1" or accepted["returnData"]!=trade["returnData"] or values(checked[-len(reads):])!=after:raise ValueError("конечные балансы")
                rows.append({**info,"actual":used,"fallback":used==fallback,"received":returned,"gas":int(accepted["gasUsed"],16),
                             "approval_gas":int(calls[1]["gasUsed"],16),"before":before,"after":after,"order":exact,"logs":accepted["logs"],
                             "minimum_plus_one":strict,"expired":expired.get("error")})
    return rows
