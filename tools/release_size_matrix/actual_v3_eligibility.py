"""Conservative BSC V3 own-quote eligibility from archived episodes, without network access."""

import argparse
from collections import Counter
from decimal import Decimal, localcontext
import json
from pathlib import Path
import re


Q96 = 1 << 96
MAX_FEE = 1_000_000
MAX_TICK = 887_272
BSC_USDT = "0x55d398326f99059ff775485246999027b3197955"
V3_FAMILIES = {"pancakeswap-v3-bsc", "uniswap-bsc"}


class Blocked(Exception):
    """The archive lacks an input needed to reproduce this sampled quote."""


def need(condition, reason):
    if not condition:
        raise Blocked(reason)


def num(n):
    return Decimal(str(n))


def time_key(s):
    """Compare UTC strings without truncating their nanosecond fractions."""
    need(isinstance(s, str) and s.endswith("Z"), "timestamp_missing_or_not_utc")
    chars = s[:-1].partition(".")
    need(len(chars[0]) == 19, "timestamp_not_iso8601")
    return chars[0], int(chars[2].ljust(9, "0") or "0")


def load_tick_math(path):
    """Use the checked-in Pancake/Uniswap TickMath constants, avoiding float powers."""
    code = path.read_text(encoding="utf-8-sig")
    masks = re.search(r"const MULS: \[u128; 19\] = \[(.*?)\];", code, re.S)
    masks = [int(c.replace("_", ""), 16) for c in re.findall(r"0x[0-9a-f_]+", masks[1])]
    odd = re.search(r"U256::from\((0x[0-9a-f_]+)u128\)", code)
    assert len(masks) == 19 and odd is not None
    masks.insert(0, int(odd[1].replace("_", ""), 16))

    def sqrt_at(tick):
        need(abs(tick) <= MAX_TICK, "tick_out_of_range")
        ratio = 1 << 128
        for bit, multiplier in enumerate(masks):
            if abs(tick) & (1 << bit):
                ratio = ratio * multiplier >> 128
        if tick > 0:
            ratio = ((1 << 256) - 1) // ratio
        return div_up(ratio, 1 << 32)

    assert sqrt_at(-MAX_TICK) == 4_295_128_739
    assert sqrt_at(0) == Q96
    assert sqrt_at(MAX_TICK) == 1461446703485210103287273052203988822378723970342
    return sqrt_at


def div_up(n, d):
    return -(-n // d)


def delta(a, b, liquidity, token0, round_up):
    a, b = sorted((a, b))
    divide = div_up if round_up else lambda n, d: n // d
    if token0:
        return divide(liquidity * Q96 * (b - a), a * b)
    return divide(liquidity * (b - a), Q96)


def walk_book(book, quote):
    qty = num(quote["qty"])
    remaining = qty
    amount = Decimal(0)
    side = quote["side"]
    levels = book["asks" if side == "Buy" else "bids"]
    need(qty > 0 and num(quote["fillable_qty"]) >= qty, "cex_quote_qty_not_fully_fillable")
    last = None
    for item in levels:
        price, offered = num(item["price"]), num(item["qty"])
        need(offered > 0 and price > 0, "cex_nonpositive_book_level")
        need(last is None or (price >= last if side == "Buy" else price <= last), "cex_side_not_sorted")
        take = min(remaining, offered)
        amount += price * take
        last = price
        remaining -= take
        if remaining == 0:
            average = amount / qty
            need(
                abs(average - num(quote["avg_price"])) <= max(average, Decimal(1)) * Decimal("1e-23"),
                "cex_walk_avg_price_differs_from_plan",
            )
            return amount, price
    raise Blocked("recorded_cex_depth_does_not_cover_qty")


def raw_decimal(qty, digits):
    value = qty * 10**digits
    need(value > 0 and value == value.to_integral_value(), "quote_qty_not_integral_token_units")
    return int(value)


def reproduce(sqrt_at, pool, quote, block, decimals):
    token0, token1 = pool["token0"].lower(), pool["token1"].lower()
    target = quote["asset"].get("bsc_contract") or quote["asset"].get("contract")
    need(isinstance(target, str) and {token0, token1} == {BSC_USDT, target.lower()}, "pool_token_identity_differs")
    target = target.lower()
    side = quote["side"]
    need(side in {"Buy", "Sell"}, "dex_side_unsupported")
    qty = num(quote["qty"])
    need(num(quote["fillable_qty"]) >= qty, "dex_quote_qty_not_fully_fillable")
    need(
        all(num(c["amount"]) == 0 for c in quote.get("costs", [])
            if c["kind"] in {"BuyTax", "SellTax", "HookImpact"}),
        "nonzero_dex_token_tax_or_hook",
    )
    selected = [decimals.get((addr, block), decimals.get((addr, None))) for addr in (target, BSC_USDT)]
    need(all(selected), "source_backed_token_decimals_missing")
    asset_meta, quote_meta = selected
    own_amount = raw_decimal(qty, asset_meta["decimals"])
    sqrt, liquidity, tick = (
        int(str(pool["state"]["V3"]["sqrt_price_x96"]), 0),
        int(pool["state"]["V3"]["liquidity"]),
        int(pool["state"]["V3"]["tick"]),
    )
    gap, fee = pool.get("tick_spacing"), pool.get("fee_pips")
    need(isinstance(gap, int) and gap > 0, "tick_spacing_missing_or_invalid")
    need(isinstance(fee, int) and 0 <= fee < MAX_FEE, "v3_fee_missing_or_invalid")
    need(0 < liquidity < 1 << 128, "nonpositive_or_invalid_v3_liquidity")
    need(sqrt_at(tick) <= sqrt <= sqrt_at(tick + 1), "sqrt_price_inconsistent_with_tick")
    input_token = target if side == "Sell" else BSC_USDT
    zero_for_one = input_token == token0
    lo = (tick // gap) * gap
    barrier_tick = max(-MAX_TICK, lo) if zero_for_one else min(MAX_TICK, lo + gap)
    boundary = sqrt_at(barrier_tick)
    need(boundary < sqrt if zero_for_one else boundary > sqrt, "state_starts_at_possible_tick_boundary")

    keep = MAX_FEE - fee
    if side == "Sell":
        evaluated = own_amount * keep // MAX_FEE
        capacity = delta(sqrt, boundary, liquidity, zero_for_one, True)
        need(0 < evaluated < capacity, "own_sell_qty_reaches_possible_initialized_tick")
        if zero_for_one:
            next_price = div_up(liquidity * Q96 * sqrt, liquidity * Q96 + evaluated * sqrt)
        else:
            next_price = sqrt + evaluated * Q96 // liquidity
        result_raw = delta(sqrt, next_price, liquidity, not zero_for_one, False)
        expected = num(quote["route"]["legs"][0]["expected_out"])
    else:
        evaluated = own_amount
        capacity = delta(sqrt, boundary, liquidity, not zero_for_one, False)
        need(0 < evaluated < capacity, "own_buy_qty_reaches_possible_initialized_tick")
        if zero_for_one:
            next_price = sqrt - div_up(evaluated * Q96, liquidity)
        else:
            denominator = liquidity * Q96 - evaluated * sqrt
            need(denominator > 0, "own_buy_liquidity_insufficient")
            next_price = div_up(liquidity * Q96 * sqrt, denominator)
        input_before_fee = delta(sqrt, next_price, liquidity, zero_for_one, True)
        result_raw = input_before_fee + div_up(input_before_fee * fee, keep)
        expected = qty * num(quote["avg_price"])
    need(
        boundary < next_price < sqrt if zero_for_one else sqrt < next_price < boundary,
        "integer_next_price_does_not_stay_strictly_inside_tick",
    )
    tolerance_raw = Decimal(2) if side == "Buy" else Decimal(0)
    expected_raw = expected * 10**quote_meta["decimals"]
    need(abs(Decimal(result_raw) - expected_raw) <= tolerance_raw, "historical_dex_quote_raw_result_differs")
    return {
        "block": block, "current_tick": tick, "nearest_possible_tick": barrier_tick,
        "sqrt_price_x96": str(sqrt), "boundary_sqrt_price_x96": str(boundary),
        "next_sqrt_price_x96": str(next_price), "liquidity": str(liquidity),
        "tick_spacing": gap, "fee_pips": fee, "dex_side": side,
        "dex_qty_human": str(qty), "dex_qty_raw": str(own_amount),
        "capacity_raw": str(capacity), "capacity_used_raw": str(evaluated),
        "capacity_basis": "fee_adjusted_input" if side == "Sell" else "requested_output",
        "quote_result_raw": str(result_raw), "archived_quote_raw": str(expected_raw),
        "token0": token0, "token1": token1, "asset_decimals": asset_meta["decimals"],
        "quote_decimals": quote_meta["decimals"], "asset_decimals_source": asset_meta["evidence"],
        "quote_decimals_source": quote_meta["evidence"],
        "decimals_evidence": (
            "historical_exact_block"
            if asset_meta.get("block") == quote_meta.get("block") == block
            else "later_decimals_cross_checked_against_archived_own_quote"
        ),
    }


def decimals_map(path):
    if path is None:
        return {}
    raw = json.loads(path.read_text(encoding="utf-8-sig"))
    entries = (dict(meta, address=addr) for addr, meta in raw.items()) if isinstance(raw, dict) else raw
    out = {}
    for item in entries:
        assert isinstance(item["decimals"], int) and 0 <= item["decimals"] <= 255
        assert item["evidence"]
        key = (item["address"].lower(), item.get("block"))
        assert key not in out, f"duplicate decimals entry: {key}"
        out[key] = item
    return out


def audit(deal, match, episodes, decimals, sqrt_at):
    plan = deal["plan"]
    dex = plan[match["quote"]]
    cex = plan["buy_quote" if match["quote"] == "sell_quote" else "sell_quote"]
    dex_at, cex_at = (time_key(q["at"]) for q in (dex, cex))
    cutoff = time_key(match["plan_at"])
    need(max(cex_at, dex_at) <= cutoff, "saved_quote_is_later_than_own_decision")
    route = dex.get("route") or {}
    legs = route.get("legs", [])
    need(
        len(legs) == 1 and len(legs[0]["hops"]) == 1
        and num(legs[0]["share"]) == 1 and route.get("executable") is True,
        "not_one_direct_executable_dex_route",
    )
    hop = legs[0]["hops"][0]
    need(
        hop["kind"] == "V3" and hop["family"] in V3_FAMILIES
        and hop["pool"] == match["pool"] and route["block"] == match["block"],
        "cached_route_identity_differs_from_original_quote",
    )
    cex_contract = cex["asset"].get("bsc_contract") or cex["asset"].get("contract")
    dex_contract = dex["asset"].get("bsc_contract") or dex["asset"].get("contract")
    need(
        isinstance(cex_contract, str) and isinstance(dex_contract, str)
        and cex_contract.lower() == dex_contract.lower(),
        "cex_and_dex_plan_asset_identity_not_established",
    )
    need(
        num(dex["qty"]) == num(plan["qty"] if dex["side"] == "Buy" else plan["sell_qty"]),
        "dex_quote_qty_differs_from_plan_order_qty",
    )
    counts = Counter()
    failures = []
    for eid in match["episodes"]:
        for row in episodes(eid):
            if row.get("type") != "calc":
                continue
            for pool in row.get("pools", []):
                if not (
                    pool.get("reference") == match["pool"] and pool.get("block") == match["block"]
                    and pool.get("kind") == "V3" and pool.get("family") == hop["family"]
                    and pool.get("venue") == dex["venue"]
                ):
                    continue
                counts["exact_pool_block_rows"] += 1
                if time_key(row["t"]) > cutoff or time_key(pool["received_at"]) > dex_at:
                    continue
                counts["before_own_decision_rows"] += 1
                for book in row.get("books", []):
                    book_contract = book["asset"].get("bsc_contract") or book["asset"].get("contract")
                    if (
                        book["venue"] != cex["venue"] or time_key(book["received_at"]) != cex_at
                        or not isinstance(book_contract, str) or book_contract.lower() != cex_contract.lower()
                    ):
                        continue
                    counts["exact_predecision_book_rows"] += 1
                    try:
                        cex_amount, worst = walk_book(book, cex)
                        expected_qty = num(plan["qty"] if cex["side"] == "Buy" else plan["sell_qty"])
                        need(num(cex["qty"]) == expected_qty, "cex_quote_qty_differs_from_plan_order_qty")
                        if cex["side"] == "Buy":
                            need(worst == num(plan["buy_worst"]), "cex_buy_worst_price_differs_from_plan")
                        counts["cex_walk_verified_rows"] += 1
                        return {
                            "status": "eligible_fixed_route_own_qty", "episode": eid,
                            "snapshot_at": row["t"], "cex_quote_at": cex["at"], "dex_quote_at": dex["at"],
                            "cex_venue": cex["venue"], "cex_side": cex["side"],
                            "cex_qty": cex["qty"], "cex_gross_usdt": str(cex_amount),
                            "cex_worst_price": str(worst), "counts": dict(counts),
                            **reproduce(sqrt_at, pool, dex, match["block"], decimals),
                        }
                    except Blocked as err:
                        failures.append(str(err))
    if failures:
        status = failures[0]
    elif not counts["exact_pool_block_rows"]:
        status = "no_recorded_exact_v3_pool_block"
    elif not counts["before_own_decision_rows"]:
        status = "exact_v3_pool_not_captured_predecision"
    else:
        status = "no_exact_predecision_cex_book"
    return {"status": status, "all_blockers": sorted(set(failures)), "counts": dict(counts)}


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("archive", type=Path)
    cli.add_argument("--decimals", type=Path)
    cli.add_argument("--output", type=Path, required=True)
    args = cli.parse_args()

    def read(name):
        return json.loads((args.archive / name).read_text(encoding="utf-8-sig"))

    def episodes(eid):
        name = f"episode-{eid}.json"
        return read(name)["records"] if (args.archive / name).is_file() else []

    deals = {row["id"]: row for row in read("deal_inputs_for_windows.json")}
    matches = [m for m in read("episode_fixed_route_matches.json") if m["kind"] == "V3"]
    decimals = decimals_map(args.decimals)
    root = Path(__file__).resolve().parents[2]
    sqrt_at = load_tick_math(root / "src/venues/bsc_dex/math/tick_math.rs")
    rows = []
    with localcontext() as context:
        context.prec = 90
        for match in matches:
            try:
                result = audit(deals[match["id"]], match, episodes, decimals, sqrt_at)
            except Blocked as err:
                result = {"status": str(err)}
            rows.append({**match, **result})
    summary = {
        "matches": len(matches),
        "distinct_deals": len({row["id"] for row in rows}),
        "outcomes": dict(Counter(row["status"] for row in rows)),
        "scope": (
            "Same archived single-hop V3 state and CEX book at the original planned quantity. "
            "Proves one local quote under recorded inputs without crossing any possible initialized tick. "
            "No alternate route, end-to-end execution, realized outcome, or live-cap proof."
        ),
    }
    args.output.write_text(json.dumps({"summary": summary, "rows": rows}, indent=2), encoding="utf-8")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
