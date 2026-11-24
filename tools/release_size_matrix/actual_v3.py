"""Prepare and stage the certified TRADOOR single-tick replay without altering the bot."""

import argparse
from decimal import Decimal
import json
from pathlib import Path
import tempfile

import stored_compare
from stored_trades import read, timestamp_ns


TOOLS = Path(__file__).resolve().parent
TARGET_DEAL = "20261122144730803TRADOOR"


def extract(args):
    archive = args.archive
    rows = read(archive / "actual_v3_eligibility.json")["rows"]
    result, = [r for r in rows if r["id"] == TARGET_DEAL and r["status"] == "eligible_fixed_route_own_qty"]
    deal, = [d for d in read(archive / "deal_inputs_for_windows.json") if d["id"] == TARGET_DEAL]
    episode = read(archive / f'episode-{result["episode"]}.json')
    row, = [r for r in episode["records"] if r.get("t") == result["snapshot_at"] and r["type"] == "calc"]
    book, = [
        b for b in row["books"]
        if b["venue"] == result["cex_venue"] and b["received_at"] == result["cex_quote_at"]
    ]
    pool, = [
        p for p in row["pools"]
        if p["reference"] == result["pool"] and p["block"] == result["block"]
    ]
    plan = deal["plan"]
    assert result["dex_side"] == "Buy" and result["cex_side"] == "Sell"
    assert deal["direction"]["buy_on"] == "bsc_dex" and deal["direction"]["sell_on"] == result["cex_venue"]
    assert Decimal(plan["qty"]) == Decimal(result["dex_qty_human"]) == Decimal(plan["sell_qty"])
    evidence = {
        "deal_id": deal["id"], "window_id": result["episode"], "decision_utc": result["plan_at"],
        "decision_time_basis": "archived_plan_at",
        "direction": deal["direction"], "basis": "cex_episode_book_bsc_v3_fixed_single_tick",
        "sell": {"recv_ns": timestamp_ns(book["received_at"]), "source": "episode_book",
                 "levels": [[level["price"], level["qty"]] for level in book["bids"]]},
        "thresholds": {
            "min_usdt": "0.005", "min_pct": "0.00001",
            "source": "profit_threshold_history.json",
            "usdt_scope": "BingX DEX→CEX",
            "pct_scope": "global, no BingX DEX→CEX override",
        },
        "historical_result_usdt": deal.get("result_usd"),
        "market_sample_utc": result["snapshot_at"],
        "pool": {**{k: result[k] for k in (
            "block", "current_tick", "nearest_possible_tick", "sqrt_price_x96",
            "boundary_sqrt_price_x96", "next_sqrt_price_x96", "liquidity", "tick_spacing",
            "fee_pips", "capacity_raw", "quote_result_raw", "asset_decimals", "quote_decimals",
            "decimals_evidence",
        )}, "ref": pool["reference"]["Contract"], "snapshot_ns": timestamp_ns(pool["received_at"])},
        "plan": plan,
    }
    args.output.write_text(json.dumps({"cases": [evidence]}, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"eligible": 1, "deal": TARGET_DEAL, "recorded_budget": plan["size"]["budget"]}))


def stage(args):
    cases = read(args.cases)["cases"]
    if len(cases) != 1 or cases[0]["deal_id"] != TARGET_DEAL:
        return {"status": "unavailable", "reason": "expected the one independently certified TRADOOR cell"}
    missing = [
        path for path in ("src/core/size.rs", "src/core/uncapped_recompute_ctx.rs",
                          "src/venues/bsc_dex/math/swap_step.rs")
        if not (args.target / path).is_file()
    ]
    if missing:
        return {"status": "unavailable", "reason": f"native files missing: {missing}"}
    swaps = (args.target / "src/venues/bsc_dex/math/swap_step.rs").read_text(encoding="utf-8-sig")
    signature = "pub fn step_v3(cur: U256, target: U256, l: u128, amount: I256, fee: u32) -> Option<Step>"
    if signature not in swaps:
        return {"status": "unavailable", "reason": "native step_v3 signature differs"}
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
            return meta
        native = (temp / "tests/core_size_stored_compare.rs").read_text(encoding="utf-8")
    placement = "inside_search" if '"inside_search"' in native else "after_search"
    start = native.index("async fn replay(")
    end = native.index("#[tokio::test]", start)
    native = native[:start] + (TOOLS / "actual_v3.rs").read_text(encoding="utf-8-sig") + "\n" + native[end:]
    native = native.replace("NATIVE_THRESHOLD_PLACEMENT", placement)
    native = native.replace("use arb::core::uncapped_recompute_cfg::{presence, step_of};",
                            "use arb::core::uncapped_recompute_cfg::presence;")
    native = native.replace("step: step_of(plan, book),", "step: observed_step(plan, sell),")
    native = native.replace("guess: budget / book.asks[0].price,", "guess: budget / plan.buy_quote.avg_price,")
    assert "book.asks[0]" not in native and "step: observed_step(plan, sell)," in native
    native = native.replace("size_stored_cases.json", "size_actual_v3_cases.json")
    native = native.replace("stored_trade_matrix", "actual_v3_matrix").replace("STORED_TRADE_MATRIX", "ACTUAL_V3_MATRIX")
    native = native.replace('"min_usdt": "0.05", "min_pct": "0"', '"min_usdt": case["thresholds"]["min_usdt"], "min_pct": case["thresholds"]["min_pct"], "threshold_source": case["thresholds"]["source"]')
    native = native.replace('"model": {', '"market_sample_utc": case["market_sample_utc"], "pool_block": case["pool"]["block"], "model": {')
    tests = args.target / "tests"
    (tests / "core_size_actual_v3.rs").write_text(native, encoding="utf-8")
    (tests / "size_actual_v3_cases.json").write_text(json.dumps(cases), encoding="utf-8")
    return {"status": "staged", "cases": 1, "expected_rows": 2}


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    modes = cli.add_subparsers(dest="mode", required=True)
    extract_cli = modes.add_parser("extract")
    extract_cli.add_argument("archive", type=Path)
    extract_cli.add_argument("--output", type=Path, required=True)
    stage_cli = modes.add_parser("stage")
    stage_cli.add_argument("target", type=Path)
    stage_cli.add_argument("cases", type=Path)
    args = cli.parse_args()
    if args.mode == "extract":
        extract(args)
    else:
        print(json.dumps(stage(args)))


if __name__ == "__main__":
    main()
