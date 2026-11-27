"""Replay an actual input-funded purchase followed by an output-funded sale of the purchased quarter."""

from copy import deepcopy
from eth_abi import decode
from eth_utils import keccak
from record_antfarm_output import ATF, MODULE, ORDER, PAYER, VAULT, WETH, basis, possible
from record_base_slipstream_executor import calldata, rpc, simulate_block

INPUT_MODULE = "0x" + keccak(text="Antfarm original input funding probe control")[-20:].hex()
INPUT_ABI = "(address,address,uint16,uint16,bool,uint256,uint256,uint256,uint256,uint256,uint256,uint256)"


def chained(url, d, input_runtime, output_runtime):
    tag = d["selector"]
    end = d["block_time"] + 300
    if rpc(url, "eth_getCode", [INPUT_MODULE, tag]) != "0x":
        raise ValueError("occupied simulation address")
    overrides = {INPUT_MODULE: {"code": "0x" + input_runtime}, MODULE: {"code": "0x" + output_runtime}}

    def tx(to, fn, types=(), vals=()):
        return {"from": PAYER, "to": to, "data": calldata(fn, types, vals), "gas": hex(1400000)}

    def run(calls):
        return simulate_block(url, tag, overrides, calls)

    def bal(t):
        return tx(t, "balanceOf(address)", ["address"], [PAYER])

    def approve(t, s, n):
        return tx(t, "approve(address,uint256)", ["address", "uint256"], [s, n])

    def number(r):
        if r["status"] != "0x1":
            raise ValueError(("read", r.get("error")))
        return int(r["returnData"], 16)

    def purchase(row, fee):
        a, b = map(int, row["reserves"])
        gross = fee * 1000 // (1000 - row["fee"])
        return b * gross // (a - gross) + 1

    result = []
    for pair, quantities in [
        ("0x5a2c88ddc0dd28cf2c1f9b92e3adde599f3321fa", [10**14, 10**15]),
        ("0x196a843cb27f2ac6c7f9e0164886515d88f17372", [250_000, 1_000_000]),
    ]:
        market = next(m for m in d["pairs"] if m["pair"] == pair)
        coin, cash = market["tokens"]
        tier = 100 if cash == WETH else 250
        fund = next(m for m in d["pairs"] if m["tokens"] == [ATF, cash] and m["fee"] == tier)
        for qty in quantities:
            nominal, fee = basis(d, market, cash, qty)
            cost = purchase(fund, fee)
            budget = qty + cost
            buy = tx(
                INPUT_MODULE,
                "execute(" + INPUT_ABI + ")",
                [INPUT_ABI],
                [(cash, coin, market["fee"], tier, False, qty, budget, nominal, fee, fee, cost, end)],
            )
            prefix = [
                approve(cash, INPUT_MODULE, 0),
                approve(cash, INPUT_MODULE, budget),
                bal(cash),
                bal(coin),
                bal(ATF),
                buy,
                bal(cash),
                bal(coin),
                bal(ATF),
            ]
            prefix.extend(tx(m["pair"], "getReserves()") for m in (market, fund))
            prefix += [tx(market["oracle"], "price1Average()")]
            first = run(prefix)
            if any(r["status"] != "0x1" for r in first):
                raise ValueError(
                    ("first purchase", pair, qty, [r.get("error") for r in first if r["status"] != "0x1"])
                )
            if (
                number(first[2]) - number(first[6]) != budget
                or number(first[7]) - number(first[3]) != nominal
                or number(first[8]) != number(first[4])
            ):
                raise ValueError("first wallet outcome")
            post = deepcopy(d)
            changed = []
            for m, r in zip((market, fund), first[9:11], strict=True):
                a, b, _ = decode(["uint112", "uint112", "uint32"], bytes.fromhex(r["returnData"][2:]))
                updated = next(o for o in post["pairs"] if o["pair"] == m["pair"])
                updated["reserves"] = [str(a), str(b)]
                changed.append(updated)
            post["oracles"][market["oracle"]]["rate_q112"] = str(number(first[11]))
            rest = nominal - (nominal // 2) - (nominal // 4)
            gross, atf, offers = possible(post, changed[0], coin, rest)
            picked = next(o for o in offers if o["tier"] == tier)
            repayment = picked["cost"]
            net = gross - repayment

            def sell(minimum):
                return tx(
                    MODULE,
                    "execute(" + ORDER + ")",
                    [ORDER],
                    [(coin, cash, market["fee"], tier, False, True, rest, minimum, atf, repayment, 0, end)],
                )

            calls = prefix + [
                approve(coin, MODULE, rest),
                bal(coin),
                bal(cash),
                bal(ATF),
                sell(net + 1),
                sell(net),
                bal(coin),
                bal(cash),
                bal(ATF),
            ]
            rs = run(calls)
            before = rs[-8:-5]
            strict, exact = rs[-5:-3]
            after = rs[-3:]
            if strict["status"] != "0x0" or strict.get("logs") or exact["status"] != "0x1":
                raise ValueError(("return leg", pair, qty, strict.get("error"), exact.get("error")))
            if [number(a) - number(b) for a, b in zip(after, before)] != [-rest, net, 0]:
                raise ValueError("second wallet outcome")
            prior_gross, prior_fee, prior_offers = possible(d, market, coin, rest)
            result.append(
                dict(
                    pair=pair,
                    input_module=INPUT_MODULE,
                    cash_budget=str(budget),
                    bought=str(nominal),
                    sold=str(rest),
                    net=str(net),
                    after_fee=str(atf),
                    after_cost=str(repayment),
                    earlier_gross=str(prior_gross),
                    earlier_fee=str(prior_fee),
                    earlier_cost=str(next(o["cost"] for o in prior_offers if o["tier"] == tier)),
                    post_reserves=[m["reserves"] for m in changed],
                    calls=calls,
                    results=rs,
                )
            )
    return result
