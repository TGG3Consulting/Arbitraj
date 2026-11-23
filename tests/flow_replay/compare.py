"""Compare delivered Book identities to a recorded plan at its original sell quantity (CI only)."""

import gzip
import json
import sys
from bisect import bisect_right
from collections import Counter, defaultdict
from datetime import datetime
from decimal import Decimal, getcontext
from pathlib import Path

ARCHIVE = Path("tests/data/sigma_windows_2026-11-16")
SOL = Path("tests/data/market_flow_2026-11-23")
getcontext().prec = 60


def ns(value):
    """Parse recorded UTC RFC3339 without truncating the nine-digit fractional part."""
    head, dot, fraction = value.removesuffix("Z").partition(".")
    second = int(datetime.fromisoformat(head + "+00:00").timestamp())
    return second * 1_000_000_000 + (int(fraction.ljust(9, "0")) if dot else 0)


def key(book):
    """The capture key used in both the market adapter replay and the comparison."""
    return book["venue"], book["ticker"], book["ns"]


def proceeds(qty, levels):
    """Consume recorded bid depth in order, returning None if the captured bids cannot fill it."""
    total = Decimal(0)
    for price, size in levels:
        take = min(qty, Decimal(size))
        total += take * Decimal(price)
        qty -= take
        if qty == 0:
            return total
    return None


def archive_cases():
    """Match only same direction and a unique window containing the plan timestamp."""
    windows = json.loads((ARCHIVE / "windows_index.json").read_text())
    books = defaultdict(dict)
    with gzip.open(ARCHIVE / "book_frames.jsonl.gz", "rt") as source:
        for line in source:
            book = json.loads(line)
            if not book["channel"].startswith("window "):
                books[book["window"], book["venue"], book["ticker"]][key(book)] = book
    covered, cases = Counter(), []
    for line in (ARCHIVE / "deals_sell_on_cex.jsonl").read_text().splitlines():
        deal = json.loads(line)
        plan, route = deal["план"], deal["direction"]
        venue, ticker = route["sell_on"], route["asset"]["ticker"]
        direction = route["buy_on"] + "→" + venue
        at = ns(plan["plan_at"])
        matches = []
        for window in windows:
            if window["ticker"] != ticker or direction not in window["directions"]:
                continue
            tape = books.get((window["window"], venue, ticker))
            if tape and ns(window["started"]) <= at <= max(k[2] for k in tape):
                matches.append(tape)
        if len(matches) != 1:
            covered["ambiguous_window" if matches else "no_window"] += 1
            continue
        cases.append(price_case(plan, at, matches[0].values()))
        covered["unique_window"] += 1
    return cases, covered


def price_case(plan, at, tape):
    """Price a fixed original quantity on each subsequent captured snapshot."""
    qty = Decimal(plan["sell_qty"])
    quotes = {key(b): proceeds(qty, b["bids"]) for b in tape if b["ns"] >= at}
    assert qty > 0 and quotes
    return Decimal(plan["proceeds"]), quotes


def compare(cases, delivered):
    """Count (plan, frame) pairs and distinct plans; maximum is per-plan gross, never summed."""
    result = Counter()
    worst = Decimal(0)
    for target, quotes in cases:
        full = {k: v for k, v in quotes.items() if v is not None}
        received = {k: v for k, v in full.items() if k in delivered}
        passed = {k for k, v in full.items() if v >= target}
        observed = passed & delivered.keys()
        timeline = sorted((k[2], value) for k, value in quotes.items())
        times = [at for at, _ in timeline]
        latest_pass, latest_fail, latest_unknown = 0, 0, 0
        for k in observed:
            when = delivered[k]
            assert when >= k[2]
            latest = timeline[bisect_right(times, when) - 1][1]
            if when > times[-1] or latest is None:
                latest_unknown += 1
            elif latest >= target:
                latest_pass += 1
            else:
                latest_fail += 1
        result.update(
            plan_frames=len(quotes), insufficient_depth=len(quotes) - len(full),
            full_gross_at_least_plan_frames=len(passed), delivered_gross_at_least_plan_frames=len(observed),
            full_gross_at_least_plan_plans=bool(passed), delivered_gross_at_least_plan_plans=bool(observed),
            all_qualifying_updates_replaced_plans=bool(passed and not observed),
            plans_without_delivered_fillable_frame=bool(full and not received),
            qualifying_received_latest_also_qualifies=latest_pass,
            qualifying_received_latest_below_target=latest_fail,
            qualifying_received_latest_unknown=latest_unknown,
            plans_with_qualifying_received_and_latest=bool(latest_pass),
        )
        if received:
            worst = max(worst, max(full.values()) - max(received.values()))
    return dict(result, max_best_gross_decrease_usdt=str(worst))


def sol_margin(plan, quotes, delivered):
    """Hold the recorded DEX leg and proportional cost rates fixed; calculate a conditional margin."""
    p = {k: Decimal(plan[k]) for k in ("proceeds", "trade_fee", "reserve", "costs_usdt", "invested", "profit_usd")}
    variable = p["trade_fee"] + p["reserve"]
    fixed = p["invested"] + p["costs_usdt"] - variable

    def margin(gross):
        return gross * (1 - variable / p["proceeds"]) - fixed

    assert abs(margin(p["proceeds"]) - p["profit_usd"]) < Decimal("1e-22")
    qualifying = {k for k, value in quotes.items() if value is not None and margin(value) >= Decimal("0.05")}
    return {"fixed_dex_and_rates_margin_ge_0_05_full": len(qualifying),
            "fixed_dex_and_rates_margin_ge_0_05_delivered": len(qualifying & delivered.keys())}


def main(delivery, output):
    """Write an aggregate JSON artifact. Input contains only identities emitted by the Rust test."""
    flows, summaries = defaultdict(dict), {}
    for line in Path(delivery).read_text().splitlines():
        row = json.loads(line)
        item = row.get("summary", row)
        profile = item["case"], item["width"], item["capacity"]
        if "summary" in row:
            summaries[profile] = item
        else:
            capture = row["venue"], row["ticker"], row["id"]
            assert capture not in flows[profile], (profile, capture)
            flows[profile][capture] = row["drained_at"]
    cases, coverage = archive_cases()
    single = json.loads((SOL / "sol_book.json").read_text())
    sol = price_case(single["plan"], ns(single["plan"]["at"]), single["frames"])
    expected = {"archive_books": 6584, "sol_books": 7, "sol_pool_blocks": 36}
    assert len(summaries) == 14 and cases and sum(coverage.values()) == 98
    for profile, item in summaries.items():
        case = profile[0]
        assert item["input"] == expected[case] and len(flows[profile]) == item["delivered"]
        assert flows[profile].keys() <= flows[case, 0, 1].keys()
        if case == "archive_books":
            item["gross_sale_vs_plan"] = compare(cases, flows[profile])
        if case == "sol_books":
            item["gross_sale_vs_plan"] = compare([sol], flows[profile])
            item.update(sol_margin(single["plan"], sol[1], flows[profile]))
    result = {"archive_plan_coverage": coverage, "profiles": list(summaries.values())}
    Path(output).write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main(*sys.argv[1:])
