"""Prepare conservative single-pool historical CEX→Pancake V2 replay cases."""

import argparse
from collections import Counter, defaultdict
from decimal import Decimal, localcontext
import json
from pathlib import Path
import tempfile

import stored_compare
from stored_trades import book_levels, decision, read, timestamp_ns


TOOLS = Path(__file__).resolve().parent
BSC_USDT = "0x55d398326f99059ff775485246999027b3197955"
DEOD = "0x3510fbbc13090f991ffa523527113a166161683e"
DEFAULT_DECIMALS = {BSC_USDT: 18, DEOD: 18}


def pool_out(qty, coin_reserve, quote_reserve):
    taxed = qty * 9975
    return taxed * quote_reserve // (coin_reserve * 10000 + taxed)


def exact_units(value, places):
    raw = Decimal(value).scaleb(places)
    if raw != raw.to_integral_value():
        raise ValueError("amount not representable in verified token decimals")
    return int(raw)


def close_enough(estimate, recorded):
    return abs(estimate - Decimal(recorded)) <= Decimal("1e-19")


def buy_cost(levels, size):
    cost = Decimal(0)
    for p, q in levels:
        fill = min(size, Decimal(q))
        cost += fill * Decimal(p)
        size -= fill
        if not size:
            return cost
    raise ValueError("planned quantity exceeds recorded ask depth")


def direction_key(direction):
    asset = direction["asset"]
    address = asset.get("contract", asset.get("bsc_contract"))
    return direction["buy_on"], direction["sell_on"], (address or "").lower()


def route_spec(deal, decimals):
    buy, sell, coin = direction_key(deal["direction"])
    plan = deal["plan"]
    quote = plan["sell_quote"]
    route = quote.get("route")
    if sell != "bsc_dex" or not route or plan["buy_quote"].get("route") is not None:
        raise ValueError("not a saved CEX to BSC route")
    if not route.get("executable") or len(route["legs"]) != 1:
        raise ValueError("requires one executable unsplit leg")
    leg = route["legs"][0]
    if len(leg["hops"]) != 1 or Decimal(leg["share"]) != 1:
        raise ValueError("requires one direct hop")
    hop = leg["hops"][0]
    if (hop["kind"], hop["family"]) != ("V2", "pancakeswap_v2"):
        raise ValueError("not standard PancakeSwap V2")
    if coin not in decimals:
        raise ValueError("no verified coin decimals")
    if any(Decimal(c["amount"]) and c["kind"] not in {"Gas", "PoolFee"} for c in quote["costs"]):
        raise ValueError("nonstandard DEX costs or token tax")
    if any(c["kind"] == "PoolFee" and not c["in_price"] for c in quote["costs"]):
        raise ValueError("pool fee is not already in price")
    return buy, coin, route["block"], hop["pool"]["Contract"].lower(), leg["expected_out"]


def from_record(deal, episode, record, spec, decimals, at, basis):
    buy, coin, block, pool, expected = spec
    stamp = timestamp_ns(at)
    if timestamp_ns(record["t"]) >= stamp:
        raise ValueError("market sample at or after plan")
    plan = deal["plan"]
    if Decimal(record["qty"]) != Decimal(plan["qty"]):
        raise ValueError("sample used a different original amount")

    books = [b for b in record["books"] if b["venue"] == buy]
    matching = [p for p in record["pools"] if p["reference"].get("Contract", "").lower() == pool]
    if len(books) != 1 or len(matching) != 1:
        raise ValueError("unique recorded book and pool required")
    book, state = books[0], matching[0]
    if (state["kind"], state["family"], state["block"], state["venue"]) != ("V2", "pancakeswap_v2", block, "bsc_dex"):
        raise ValueError("pool kind, family, block or venue differs")
    tokens = [state["token0"].lower(), state["token1"].lower()]
    if set(tokens) != {coin, BSC_USDT}:
        raise ValueError("pool assets differ from the CEX/base and USDT pair")
    book_stamp = timestamp_ns(book["received_at"])
    pool_stamp = timestamp_ns(state["received_at"])
    if max(book_stamp, pool_stamp) > timestamp_ns(record["t"]):
        raise ValueError("source timestamps later than the market sample")

    asks = book_levels(book, True)
    qty = Decimal(plan["qty"])
    if not close_enough(buy_cost(asks, qty) / qty, plan["buy_quote"]["avg_price"]):
        raise ValueError("saved CEX ladder does not reproduce the plan average")
    reserves = state["state"]["V2"]
    coin_i = tokens.index(coin)
    coin_r = int(reserves[f"reserve{coin_i}"], 16)
    quote_r = int(reserves[f"reserve{1-coin_i}"], 16)
    if min(coin_r, quote_r) <= 0:
        raise ValueError("empty recorded pool")

    coin_places, quote_places = decimals[coin], decimals[BSC_USDT]
    sold = plan["sell_qty"]
    out = pool_out(exact_units(sold, coin_places), coin_r, quote_r)
    if out != exact_units(expected, quote_places):
        raise ValueError("saved reserves and fee do not reproduce plan route output exactly")
    if not close_enough(Decimal(out).scaleb(-quote_places) / Decimal(sold), plan["sell_quote"]["avg_price"]):
        raise ValueError("integer V2 output differs from the plan average")

    return {
        "deal_id": deal["id"],
        "window_id": episode["info"]["id"],
        "decision_utc": at,
        "decision_time_basis": basis,
        "direction": deal["direction"],
        "basis": "cex_episode_book_bsc_v2_fixed_route",
        "buy": {"recv_ns": book_stamp, "source": "episode_book", "levels": asks},
        "pool": {
            "ref": pool, "block": block, "snapshot_ns": pool_stamp,
            "coin_reserve_raw": str(coin_r), "quote_reserve_raw": str(quote_r),
            "coin_decimals": coin_places, "quote_decimals": quote_places,
            "planned_out_raw": str(out),
        },
        "market_sample_utc": record["t"],
        "plan": plan,
        "historical_result_usdt": deal.get("result_usd"),
    }


def extract(args):
    scale = DEFAULT_DECIMALS | {k.lower(): v for k, v in (read(args.decimals) if args.decimals else {}).items()}
    by_direction = defaultdict(list)
    for file in sorted(args.episodes.glob("episode-*.json")):
        data = read(file)
        by_direction[direction_key(data["info"]["direction"])].append(data)
    found = []
    skipped = Counter()
    with localcontext() as ctx:
        ctx.prec = 80
        for deal in read(args.deals):
            direction = direction_key(deal["direction"])
            if not (episodes := by_direction.get(direction)) or not deal.get("plan"):
                continue
            if not deal.get("buy") or Decimal(deal["buy"]["filled_qty"]) <= 0:
                continue
            try:
                spec = route_spec(deal, scale)
                at, basis = decision(deal)
            except (KeyError, ValueError) as error:
                skipped[str(error)] += 1
                continue
            candidates = []
            reasons = Counter()
            for episode in episodes:
                info = episode["info"]
                if not info.get("ended_at") or not timestamp_ns(info["started_at"]) <= timestamp_ns(at) <= timestamp_ns(info["ended_at"]):
                    continue
                for record in episode["records"]:
                    if record["type"] != "calc":
                        continue
                    try:
                        candidates.append(from_record(deal, episode, record, spec, scale, at, basis))
                    except (KeyError, ValueError) as error:
                        reasons[str(error)] += 1
            if candidates:
                found.append(max(candidates, key=lambda x: timestamp_ns(x["market_sample_utc"])))
            else:
                skipped[reasons.most_common(1)[0][0] if reasons else "no decision-time calc sample"] += 1
    payload = {"cases": found, "skipped": skipped}
    args.output.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"eligible": len(found), "skipped": skipped}, ensure_ascii=False))


def stage(args):
    cases = read(args.cases)["cases"]
    if not cases:
        print(json.dumps({"status": "unavailable", "reason": "no verified fixed-route historical cases"}))
        return
    # Reuse the context and Params compatibility logic of the CEX test.
    # Only its own generated output is transformed in this temporary staging tree.
    with tempfile.TemporaryDirectory() as directory:
        temp = Path(directory)
        (temp / "tests").mkdir()
        (temp / "src/core").mkdir(parents=True)
        for name in ("size.rs", "uncapped_recompute_ctx.rs"):
            (temp / "src/core" / name).write_bytes((args.target / "src/core" / name).read_bytes())
        stub = temp / "cases.json"
        stub.write_text(json.dumps({"cases": [{"basis": "cex_window_books"}]}), encoding="utf-8")
        meta = stored_compare.stage(temp, stub)
        if meta["status"] != "staged":
            print(json.dumps(meta))
            return
        native = (temp / "tests/core_size_stored_compare.rs").read_text(encoding="utf-8")
    placement = "inside_search" if '"inside_search"' in native else "after_search"
    start = native.index("async fn replay(")
    end = native.index("#[tokio::test]", start)
    native = native[:start] + (TOOLS / "actual_v2.rs").read_text(encoding="utf-8-sig") + "\n" + native[end:]
    native = native.replace("NATIVE_THRESHOLD_PLACEMENT", placement)
    native = native.replace("size_stored_cases.json", "size_actual_v2_cases.json")
    native = native.replace("stored_trade_matrix", "actual_v2_matrix").replace("STORED_TRADE_MATRIX", "ACTUAL_V2_MATRIX")
    native = native.replace('"model": {', '"market_sample_utc": case["market_sample_utc"], "pool_block": case["pool"]["block"], "model": {')
    tests = args.target / "tests"
    (tests / "core_size_actual_v2.rs").write_text(native, encoding="utf-8")
    (tests / "size_actual_v2_cases.json").write_text(json.dumps(cases), encoding="utf-8")
    print(json.dumps({"status": "staged", "cases": len(cases), "expected_rows": 2 * len(cases)}))


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    modes = cli.add_subparsers(dest="mode", required=True)
    extract_cli = modes.add_parser("extract")
    for name in ("deals", "episodes", "output"):
        extract_cli.add_argument("--" + name, type=Path, required=True)
    extract_cli.add_argument("--decimals", type=Path)
    stage_cli = modes.add_parser("stage")
    stage_cli.add_argument("target", type=Path)
    stage_cli.add_argument("cases", type=Path)
    args = cli.parse_args()
    (extract if args.mode == "extract" else stage)(args)


if __name__ == "__main__":
    main()
