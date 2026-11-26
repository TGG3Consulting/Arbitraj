"""Capture Hanji proxy books and actual balance changes on Base using unsigned eth_simulateV1."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, ROOT, call, calldata, rpc
from record_v2_chain_pools import NETWORKS, view

MARKETS = "https://api-base.hanji.io/aggregators/markets"
HELPER = "0xc032b6dfec3511a00cde9ea341d140f4733609de"
CONFIG = ["uint256", "uint256", "address", "address", "bool", "bool", "address", "address",
          "uint64", "uint64", "uint64", "uint64", "bool"]
DEPTH = 16
PROBE = "0x" + keccak(text="Hanji read-only owner balance probe")[-20:].hex()


def capture(cross_levels=False):
    url, weth = NETWORKS["base"]
    network = json.loads((ROOT / "tests/data/base_ethereum_networks.json").read_bytes())["base"]
    tag = {"blockHash": network["hash"], "requireCanonical": True}
    block = rpc(url, "eth_getBlockByHash", [network["hash"], False])
    if int(block["number"], 16) != network["block"] or int(rpc(url, "eth_chainId", []), 16) != 8453:
        raise ValueError("wrong block or network")
    expires = int(block["timestamp"], 16) + 300
    profiles = json.loads((ROOT / "config/first_run_dex.json").read_bytes())
    router = next(p["settings"]["dex.router"] for p in profiles
                  if p["network"] == "base" and p["settings"]["dex.kind"] == "uniswap_v2")
    response = requests.get(MARKETS, timeout=20)
    response.raise_for_status()

    def simulate(calls):
        # The probe returns BALANCE(owner). It cannot change any contract or the simulated market.
        code = "0x73" + OWNER[2:] + "3160005260206000f3"
        state = {"stateOverrides": {OWNER: {"balance": hex(100*10**18)}, PROBE: {"code": code}}, "calls": calls}
        return rpc(url, "eth_simulateV1", [{"blockStateCalls": [state]}, tag])[0]["calls"]

    def balance(token):
        return call(token, calldata("balanceOf(address)", ["address"], [OWNER]))

    def balances(token_x, token_y):
        return [balance(token_x), balance(token_y), call(PROBE, "0x")]

    def approve(token, spender, amount):
        return call(token, calldata("approve(address,uint256)", ["address", "uint256"], [spender, amount]))

    def output(row):
        if int(row["status"], 16) != 1:
            raise ValueError(row.get("error", "simulation reverted"))
        return bytes.fromhex(row["returnData"][2:])

    def levels(proxy, asks):
        data = calldata("assembleOrderbookFromOrders(address,bool,uint24)", ["address", "bool", "uint24"],
                        [proxy, asks, DEPTH])
        prices, shares = decode(["uint72[]", "uint128[]"], view(url, tag, HELPER, data))
        if len(prices) != len(shares) or list(prices) != sorted(prices, reverse=not asks):
            raise ValueError("invalid book")
        return [[p, s] for p, s in zip(prices, shares)]

    def round_trip(proxy, config, bids, asks, limit):
        scale_x, scale_y, token_x, token_y = config[:4]
        funding = max(limit, 10**16) if token_y == weth else (20*10**18 if cross_levels else 10**16)
        prefix = [call(weth, calldata("deposit()"), funding)]
        if token_y != weth:
            prefix += [approve(weth, router, funding), call(router, calldata(
                "swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
                ["uint256", "uint256", "address[]", "address", "uint256"],
                [funding, limit, [weth, token_y], OWNER, expires]))]
        prefix += [approve(token_y, proxy, limit)] + balances(token_x, token_y)
        def buy(price, deadline):
            data = calldata("placeMarketOrderWithTargetValue(bool,uint128,uint72,uint128,bool,uint256)",
                            ["bool", "uint128", "uint72", "uint128", "bool", "uint256"],
                            [False, limit // scale_y, price, 2**128-1, True, deadline])
            return call(proxy, data, gas=5_000_000)
        # Cross only the best ask. Save the actual debit, even when it is less than the budget.
        ask_limit = asks[1 if cross_levels else 0][0]
        entry = prefix + [buy(ask_limit, expires)] + balances(token_x, token_y)
        rows = simulate(entry)
        for row in rows:
            output(row)

        def position(rows):
            x, y, native = [int.from_bytes(output(row), "big") for row in rows[-3:]]
            return (x + (native if token_x == weth else 0),
                    y + (native if token_y == weth else 0), native)

        before_x, before_y, before_native = position(rows[:len(prefix)])
        after_x, after_y, after_native = position(rows)
        buy_result = decode(["uint128"] * 3, output(rows[-4]))
        cash_debit, bought = before_y - after_y, after_x - before_x
        if cash_debit < 0 or cash_debit > limit or bought != buy_result[0] * scale_x:
            raise ValueError(f"buy not reconciled: returned {buy_result}, debit {cash_debit}, received {bought}")
        if cash_debit != (buy_result[1] + buy_result[2]) * scale_y:
            raise ValueError("exchange-reported buy fee/value differs from actual debit")
        expired = simulate(prefix + [buy(asks[0][0], expires - 301)])[-1]
        if int(expired["status"], 16) != 0:
            raise ValueError("expired order accepted")
        result = {"budget_y": limit, "ask_limit": ask_limit, "buy_return": buy_result, "debited_y": cash_debit, "received_x": bought,
                  "buy_native_delta": after_native - before_native, "expired_revert": True}
        if not bought:
            return result
        book_data = calldata("assembleOrderbookFromOrders(address,bool,uint24)", ["address", "bool", "uint24"],
                             [proxy, False, DEPTH])
        fresh_prices, fresh_shares = decode(["uint72[]", "uint128[]"], output(simulate(entry + [call(HELPER, book_data)])[-1]))
        bid_index = min(1 if cross_levels else 0, len(bids)-1, len(fresh_prices)-1)
        if bid_index < 0:
            result["remaining_x"] = bought
            return result
        result.update(bid_before=bids[bid_index], bid_after=[fresh_prices[bid_index], fresh_shares[bid_index]],
                      fresh_bids=list(zip(fresh_prices, fresh_shares)))
        def sell(price):
            return calldata("placeOrder(bool,uint128,uint72,uint128,bool,bool,bool,uint256)",
                        ["bool", "uint128", "uint72", "uint128", "bool", "bool", "bool", "uint256"],
                        [True, bought // scale_x, price, 2**128-1, True, False, True, expires])
        exit_calls = []
        if token_x == weth and after_native > before_native:
            exit_calls.append(call(weth, calldata("deposit()"), after_native - before_native))
        exit_calls += [approve(token_x, proxy, bought)]
        def execute_sell(price):
            return simulate(entry + exit_calls + [call(proxy, sell(price), gas=5_000_000)] + balances(token_x, token_y))
        rows = execute_sell(bids[bid_index][0])
        initial = decode(["uint64", "uint128", "uint128", "uint128"], output(rows[-4]))
        result["sell_at_initial_bid"] = initial
        if fresh_prices[bid_index] != bids[bid_index][0]:
            rows = execute_sell(fresh_prices[bid_index])
        for row in rows:
            output(row)
        sell_result = decode(["uint64", "uint128", "uint128", "uint128"], output(rows[-4]))
        final_x, final_y, final_native = position(rows)
        sold, received = after_x - final_x, final_y - after_y
        if sold != sell_result[1] * scale_x or received != (sell_result[2] - sell_result[3]) * scale_y:
            raise ValueError(f"sell not reconciled: {sell_result}, debited {sold}, received {received}")
        result.update(sell_return=sell_result, debited_x=sold, received_y=received, remaining_x=bought-sold,
                      sell_native_delta=final_native-after_native)
        return result

    result = {"chain": 8453, "block": network["block"], "hash": network["hash"], "source": MARKETS,
              "helper": HELPER, "helper_runtime_keccak": "0x" + keccak(view_code(url, tag, HELPER)).hex(),
              "max_levels_per_side": DEPTH, "markets": []}
    for market in sorted(response.json(), key=lambda m: m["orderbookAddress"]):
        proxy = market["orderbookAddress"].lower()
        config = decode(CONFIG, view(url, tag, proxy, calldata("getConfig()")))
        if list(config[2:4]) != [market[k]["contractAddress"].lower() for k in ("baseToken", "quoteToken")]:
            raise ValueError("REST token addresses disagree with contract")
        bids, asks = levels(proxy, False), levels(proxy, True)
        record = {"registry": market, "runtime_keccak": "0x" + keccak(view_code(url, tag, proxy)).hex(),
                  "scale": config[:2], "fee_rates": config[8:12], "bids": bids, "asks": asks, "fills": []}
        if bids and asks and (not cross_levels or min(len(bids),len(asks)) >= 2):
            unit = 10**market["quoteToken"]["decimals"]
            limits = [unit // 1000, unit // 500] if config[3] == weth else [unit, 3*unit]
            if cross_levels:
                top = asks[0][0] * asks[0][1]
                second = asks[1][0] * min(asks[1][1], max(1, asks[0][1] // 4))
                limits = [(top + second) * config[1]]
            for budget in limits:
                try:
                    record["fills"].append(round_trip(proxy, config, bids, asks, budget))
                except ValueError as error:
                    record["fills"].append({"budget_y": budget, "failure": str(error)[:240]})
        result["markets"].append(record)
        print(market["baseToken"]["symbol"], market["quoteToken"]["symbol"], len(asks), len(bids),
              [(f.get("received_x"), f.get("debited_x"), f.get("failure")) for f in record["fills"]], flush=True)
    return result


def view_code(url, tag, address):
    code = bytes.fromhex(rpc(url, "eth_getCode", [address, tag])[2:])
    if not code:
        raise ValueError("missing deployed code")
    return code


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cross-levels", action="store_true")
    args = parser.parse_args()
    args.output.write_text(json.dumps(capture(args.cross_levels), indent=2) + "\n", encoding="utf8")
