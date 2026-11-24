"""Extract replayable historical markets from saved, decoded window API responses.

No network requests. The output describes market and plan inputs, not trade
outcomes under a different release. Each window input starts at offset zero.
"""

import argparse
from collections import Counter
import datetime as dt
from decimal import Decimal
import json
from pathlib import Path
import re


EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)
DEX_VENUES = {"bsc_dex", "arb_dex"}


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"), parse_float=str)


def timestamp_ns(value):
    match = re.fullmatch(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?Z", value)
    if not match:
        raise ValueError(f"Expected UTC timestamp: {value}")
    second = dt.datetime.fromisoformat(match[1]).replace(tzinfo=dt.timezone.utc)
    return int((second - EPOCH).total_seconds()) * 10**9 + int((match[2] or "").ljust(9, "0"))


def decision(deal):
    event = next((at for at, msg in deal["timeline"] if msg.startswith("План")), None)
    plan_at = deal["plan"].get("at")
    start = event or deal.get("started_at")
    if plan_at and (not start or timestamp_ns(plan_at) <= timestamp_ns(start)):
        return plan_at, "saved_plan_at"
    if start:
        return start, "plan_timeline" if event else "trade_start"
    raise ValueError("no saved decision timestamp")

def book_levels(book, buy, expected_count=None):
    side = "asks" if buy else "bids"
    levels = book.get(side, [])
    if not levels or (expected_count is not None and len(levels) != expected_count):
        raise ValueError(f"missing or truncated {side} ladder")
    values = [(Decimal(row["price"]), Decimal(row["qty"])) for row in levels]
    if any(p <= 0 or q <= 0 for p, q in values):
        raise ValueError(f"non-positive {side} level")
    values.sort(reverse=not buy)
    if len({p for p, _ in values}) != len(values):
        raise ValueError(f"duplicate {side} price")
    return [[str(p), str(q)] for p, q in values]


def book_at(frames, venue, buy):
    trade = fallback = None
    for frame in frames:
        if frame["venue"] != venue:
            continue
        decoded = frame.get("decoded", {}).get("Ok", {})
        if frame["stream"].lower() == "book":
            view = decoded.get("view", {})
            if isinstance(view, dict) and "Book" in view:
                trade = frame["recv_ns"], view["Book"]
            elif isinstance(view, dict) and "Other" in view:
                trade = None
            continue
        book = frame.get("book")
        if book is not None:
            fallback = frame["recv_ns"], book
    if trade:
        recv, book = trade
        return {"recv_ns": recv, "source": "trading_snapshot", "levels": book_levels(book, buy)}
    if fallback and fallback[1]["ok"]:
        recv, book = fallback
        depth = book["depth"][1 if buy else 0]
        return {"recv_ns": recv, "source": "gap_free_depth", "levels": book_levels(book, buy, depth)}
    raise ValueError(f"{venue}: no valid book before decision")


def before_decision(window, stamp):
    if window["info"].get("lost", 0):
        raise ValueError("window lost frames")
    frames = window["frames"]
    if not frames or [f["i"] for f in frames] != list(range(len(frames))):
        raise ValueError("frames do not start at zero or have gaps")
    if window["truncated"] or window["frames_read"] != len(frames):
        raise ValueError("decoded window omits frames; use a complete response")
    # Preserve the order seen by the original recorder, as window_at does.
    # Frames from different venues can have non-monotonic receipt timestamps.
    return [f for f in frames if f["recv_ns"] < stamp]


def extract(deal, window):
    planned = deal["plan"]
    if not planned:
        raise ValueError("trade has no saved plan")
    at, at_basis = decision(deal)
    stamp = timestamp_ns(at)
    info = window["info"]
    if timestamp_ns(info["started_at"]) >= stamp:
        raise ValueError("window opened after decision")
    frames = before_decision(window, stamp)
    direction = deal["direction"]
    buy, sell = direction["buy_on"], direction["sell_on"]
    buy_quote = planned.get("buy_quote") or {}
    sell_quote = planned.get("sell_quote") or {}
    if buy in DEX_VENUES or buy_quote.get("route") is not None:
        raise ValueError("DEX purchase needs historical pool state, not the plan")
    source = book_at(frames, buy, True)
    if sell in DEX_VENUES or sell_quote.get("route") is not None:
        quote = sell_quote
        if not quote or Decimal(quote["avg_price"]) <= 0:
            raise ValueError("missing positive DEX plan quote")
        basis = "cex_window_dex_flat_plan"
        destination = {"avg_price": quote["avg_price"], "source": "dex_flat_plan"}
    else:
        basis = "cex_window_books"
        destination = book_at(frames, sell, False)
    return {
        "deal_id": deal["id"],
        "window_id": info["id"],
        "decision_utc": at,
        "decision_time_basis": at_basis,
        "direction": direction,
        "basis": basis,
        "buy": source,
        "sell": destination,
        "plan": {k: planned[k] for k in (
            "arrive", "buy_quote", "costs", "invested", "proceeds",
            "qty", "sell_quote", "sell_qty", "size", "sigma", "transit_s",
            "units_b_per_a", "transfer_sent_a",
        ) if k in planned},
        "historical_result_usdt": deal.get("result_usd"),
    }


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--deals", required=True, type=Path)
    cli.add_argument("--windows", required=True, type=Path)
    cli.add_argument("--output", required=True, type=Path)
    args = cli.parse_args()
    deals = {d["id"]: d for d in read(args.deals)}
    cases = {}
    skipped = {}
    for path in sorted(args.windows.glob("window-*.json")):
        window = read(path)
        for entry in window["info"]["deals"]:
            deal_id = entry["id"]
            if deal_id in cases:
                continue
            deal = deals.get(deal_id)
            if not deal or not deal.get("buy") or Decimal(deal["buy"]["filled_qty"]) <= 0:
                skipped[deal_id] = "no stored executed purchase"
                continue
            try:
                cases[deal_id] = extract(deal, window)
            except (ValueError, KeyError) as exc:
                skipped[deal_id] = str(exc)
            else:
                skipped.pop(deal_id, None)
    payload = {"cases": list(cases.values()), "skipped": skipped}
    args.output.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({
        "eligible": len(cases),
        "by_basis": Counter(case["basis"] for case in cases.values()),
        "skipped": len(skipped),
    }))


if __name__ == "__main__":
    main()
