"""Settle whole registered shares from a separately funded public wallet at a pinned Ethereum block."""
import argparse
import gzip
import json
from pathlib import Path

import requests

from record_base_slipstream_executor import OWNER, ROOT, rpc, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx

UINT = "uint256"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
FUND = "0xe592427a0aece92de3edee1f18e0157c05861564"
NATIVE_ROUTER = "0x03f0d7fde338ff228f2ac038acda35a0b3bca8e0"
SWAP = "(address,address,uint24,address,uint256,uint256,uint256,uint160)"
DIRECTORY = ROOT / "tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_directory.json.gz"


def approve(token, spender, amount):
    return tx(token, "approve(address,uint256)", ["address", UINT], [spender, amount])


def verdict(row):
    return {"status": row["status"], "return": row.get("returnData"), "gas": int(row["gasUsed"], 16),
            "error": row.get("error")}


def measure(endpoint, directory):
    tag = directory["selector"]
    block = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"], False])
    if (rpc(endpoint, "eth_chainId", []) != "0x1" or block["stateRoot"] != directory["state_root"]
            or rpc(endpoint, "eth_getCode", [OWNER, tag]) != "0x"):
        raise ValueError("wrong parent or occupied wallet")
    deadline = int(block["timestamp"], 16) + 900
    environment = {OWNER: {"balance": hex(100 * 10**18)}}

    def run(calls):
        return simulate_block(endpoint, tag, environment, calls)

    registry = directory["directories"][0]["registry"]
    native_registry, = run([tx(NATIVE_ROUTER, "brokerbotRegistry()")])
    assert int(native_registry["returnData"], 16) == int(registry, 16)
    out = []
    for market in directory["markets"].values():
        if not (market["settings"] & 1 and int(market.get("shares") or 0) > 0):
            continue
        cash, share = market["cash"], market["share"]
        assert market["share_decimals"] == "0"
        broker, hub = market["address"], market["hub"]
        registered = market["registered_pair"] == [cash, share]
        for units in [1, min(10, int(market["shares"]))]:
            quotes = run([tx(broker, "getBuyPrice(uint256)", [UINT], [units])])
            assert quotes[0]["status"] == "0x1"
            cost = int(quotes[0]["returnData"], 16)
            assert cost > 0
            budget = 10 * 10**18
            fund = [tx(WETH, "deposit()", value=budget), approve(WETH, FUND, budget),
                    tx(FUND, "exactOutputSingle(" + SWAP + ")", [SWAP],
                       [(WETH, cash, 3000, OWNER, deadline, cost, budget, 0)])]
            modes = ["hub", "router"] if registered else ["hub"]
            for mode in modes:
                if mode == "hub":
                    buy = tx(hub, "payAndNotify(address,uint256,bytes)", ["address", UINT, "bytes"], [broker, cost, b""])
                    spender = hub
                else:
                    buy = tx(NATIVE_ROUTER, "exactOutputSingle(" + SWAP + ")", [SWAP],
                             [(cash, share, 0, OWNER, deadline, units, cost, 0)])
                    spender = NATIVE_ROUTER
                prep = [*fund, approve(cash, spender, cost)]
                prefix = [*prep, balance(cash, OWNER), balance(share, OWNER), buy,
                          balance(cash, OWNER), balance(share, OWNER), tx(broker, "getSellPrice(uint256)", [UINT], [units])]
                initial = run(prefix)
                row = dict(market=broker, cash=cash, share=share, mode=mode, units=units, cost=str(cost),
                           registered=registered, initial_cash=market["cash_balance"], funding=verdict(initial[2]))
                out.append(row)
                if initial[2]["status"] != "0x1":
                    continue
                assert all(x["status"] == "0x1" for x in initial[:6])
                row["buy"] = verdict(initial[6])
                if initial[6]["status"] != "0x1":
                    continue
                before, old, after, new, sell_price = [int(initial[i]["returnData"], 16) for i in (4,5,7,8,9)]
                assert before-after == cost and new-old == units
                row.update(buy_spent=str(before-after), buy_received=str(new-old), sell_price_after_buy=str(sell_price))
                sells = {"direct": [tx(share, "transferAndCall(address,uint256,bytes)", ["address", UINT, "bytes"], [broker,units,b""])]}
                if registered:
                    # This original router receives an intentionally impossible sale minimum.
                    sells["router_above_quote"] = [approve(share, NATIVE_ROUTER, units),
                        tx(NATIVE_ROUTER, "exactInputSingle(" + SWAP + ")", [SWAP],
                           [(share, cash, 0, OWNER, deadline, units, sell_price+1, 0)])]
                row["sells"] = {}
                for wire, calls in sells.items():
                    seq = run([*prefix, *calls, balance(cash, OWNER), balance(share, OWNER)])
                    assert seq[6]["status"] == "0x1"
                    result = verdict(seq[-3])
                    result["net_cash"] = str(int(seq[-2]["returnData"],16)-int(seq[7]["returnData"],16))
                    result["spent_shares"] = str(int(seq[8]["returnData"],16)-int(seq[-1]["returnData"],16))
                    result["quoted"] = str(int(seq[9]["returnData"],16))
                    result["logs"] = seq[-3]["logs"]
                    result["request"] = calls[-1]
                    if seq[-3]["status"] != "0x1":
                        assert result["net_cash"] == result["spent_shares"] == "0"
                    else:
                        # Record nominal-vs-net discrepancies from the original contract verbatim.
                        assert token_flow(result["logs"], cash, OWNER) == int(result["net_cash"])
                        assert -token_flow(result["logs"], share, OWNER) == int(result["spent_shares"])
                    row["sells"][wire] = result
    if not any(x.get("buy_received") for x in out):
        raise ValueError("no paid shares")
    return dict(block=directory["height"], selector=tag, router=NATIVE_ROUTER, registry=registry, wallet=OWNER, controls=out)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    with gzip.open(DIRECTORY) as stream:
        archive = json.load(stream)
    errors = []
    for url in ("https://rpc.mevblocker.io", "https://eth.drpc.org"):
        try:
            result = measure(url, archive)
            break
        except (requests.RequestException, ValueError, RuntimeError) as err:
            errors.append((url, str(err)[:250]))
    else:
        raise RuntimeError(errors)
    args.output.write_bytes(json.dumps(result, separators=(",", ":")).encode())
