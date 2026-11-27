"""Сравнивает отдельные зарегистрированные пулы одной физической пары в том же состоянии Base."""
import argparse
import json
from pathlib import Path

import requests

from record_alien_base_tier import FACTORY, SOURCE, WETH, UINT
from record_balancer_markets import Capture
from record_balancer_v1 import rpc
from record_base_slipstream_executor import EXEC, OWNER, ROOT, STEP
from record_bitswap_ethereum import tx
from record_diamondswap_base import direct_wallet

USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
UNISWAP = "0x33128a8fc17869897dce68ed026d694621f6fdfd"
TIERS = (100, 200, 300, 400, 750, 3000, 10000)


def record(endpoint):
    saved = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    at = saved["selector"]
    c = Capture(dict(endpoint=endpoint, selector=at))
    header = rpc(endpoint, "eth_getBlockByNumber", [hex(saved["block"]), False])
    if header["hash"] != at["blockHash"] or int(rpc(endpoint, "eth_chainId", []), 16) != 8453:
        raise ValueError("Wrong chain or root")
    stamp = int(header["timestamp"], 16)
    if any(rpc(endpoint, "eth_getCode", [owner, at]) != "0x" for owner in (OWNER, EXEC)):
        raise ValueError("Simulation addresses occupied")

    def registered(factory, fee, a=WETH, b=USDC):
        return c.one(factory, "getPool(address,address,uint24)", ["address", "address", "uint24"],
                     [a, b, fee], ["address"])[0]

    funding = registered(UNISWAP, 500)
    if int(funding, 16) == 0 or c.one(funding, "factory()", returns=["address"])[0] != UNISWAP:
        raise ValueError("Unverified independent funding pool")

    def order(pool, x, y, quantity, minimum, deadline):
        return tx(EXEC, "execute(address,uint256,address,uint256,uint256," + STEP + "[])",
                  ["address", UINT, "address", UINT, UINT, STEP + "[]"],
                  [x, quantity, y, minimum, deadline, [(1, pool, x, y, 10000, b"")]])

    def fund_sale(quantity):
        amount = 10**17
        return [tx(WETH, "deposit()", value=amount),
                tx(WETH, "approve(address,uint256)", ["address", UINT], [EXEC, amount]),
                order(funding, WETH, USDC, amount, quantity, stamp + 120)]

    markets = []
    for tier in TIERS:
        pool = registered(FACTORY, tier)
        if pool != registered(FACTORY, tier, USDC, WETH):
            raise ValueError("Factory reversed key disagrees")
        row = dict(tier=tier, pool=pool)
        markets.append(row)
        if not int(pool, 16):
            continue
        spacing = c.one(FACTORY, "feeAmountTickSpacing(uint24)", ["uint24"], [tier], ["int24"])[0]
        values = [c.one(pool, getter + "()", returns=[kind])[0] for getter, kind in
                  [("factory", "address"), ("token0", "address"), ("token1", "address"), ("fee", "uint24"),
                   ("tickSpacing", "int24"), ("liquidity", "uint128")]]
        if values[:5] != [FACTORY, WETH, USDC, tier, spacing] or pool == funding:
            raise ValueError("Pool, input or fee identity mismatch")
        row.update(spacing=spacing, liquidity=str(values[5]), attempts=[])
        for label, sizes, fund in (("WETH", [10**15, 10**16], None), ("USDC", [10**6, 10**7], fund_sale)):
            for size in sizes:
                attempt = dict(source=label, size=str(size))
                row["attempts"].append(attempt)
                try:
                    legs, = direct_wallet(c, dict(tokens=[WETH, USDC], pool=pool), [size], stamp, 0, EXEC,
                                          modes=["v5"], fund_sale=fund, keep_partial=True, audit=True,
                                          build_action=lambda x, y, n, floor, end, mode: order(pool, x, y, n, floor, end))
                    if "sell_error" in legs and not legs["sell_error"].startswith("could not settle v5:"):
                        raise ValueError(legs["sell_error"])
                    attempt.update(legs)
                except ValueError as error:
                    if not str(error).startswith("could not settle v5:"):
                        raise
                    attempt["reverted"] = str(error)[:250]
    paired = [m for m in markets if any("buy" in a for a in m.get("attempts", []))]
    if len(paired) < 2 or not any(m["tier"] == 750 for m in paired):
        raise ValueError("Could not compare new and previously indexed fee tiers")
    return dict(chain_id=8453, block=saved["block"], selector=at, state_root=header["stateRoot"], source=SOURCE,
                factory=FACTORY, tokens=[WETH, USDC], parent_base_fee=header["baseFeePerGas"],
                funding=dict(factory=UNISWAP, fee=500, pool=funding),
                roles=c.calls, markets=markets)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    errors = []
    for url in ("https://mainnet.base.org", "https://base.drpc.org", "https://base.gateway.tenderly.co"):
        try:
            result = record(url)
            args.output.write_bytes(json.dumps(result, separators=(",", ":")).encode())
            break
        except (requests.RequestException, RuntimeError, ValueError, KeyError) as error:
            errors.append((url, str(error)[:220]))
    else:
        raise ValueError(errors)
