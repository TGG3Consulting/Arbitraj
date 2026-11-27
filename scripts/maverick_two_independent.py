"""Измеряет два способа оплаты независимо, включая узкие диапазоны и малые резервы."""
from eth_abi import decode
from record_base_slipstream_executor import rpc
from record_diamondswap_base import direct_wallet


def independent(c, source, samples, stamp, orders):
    roles = source["roles"]
    router = roles["router"]["address"]
    wrapped = roles["router"]["wrapped"]
    router_eth = int(rpc(c.endpoint, "eth_getBalance", [router, c.selector]), 16)
    # Проверенные исходники токенов допускают прямое сравнение без перевода ставки и символа между сетями.
    exact = {
        8453: {"0x4ed4e862860bed51a9570b96d89af5e1b0efefed", "0x0b3e328455c4059eeb9e3f84b5543f74e24e7e1b"},
        1: {"0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0", "0x56072c95faa701256059aa122697b133aded9279"},
    }[source["chain_id"]]
    watch = [s["pool"] for s in samples if s.get("reason")][:1]
    covered = set()
    records = []
    for pool, market in source["markets"].items():
        tokens = market["tokens"]
        if wrapped not in tokens or market["permissioned_swap"] or market["locked"]:
            continue
        other = tokens[1 - tokens.index(wrapped)]
        if pool not in watch and (other not in exact or other in covered):
            continue
        cash = int(market["reserves"][tokens.index(wrapped)])
        if not cash or not min(map(int, market["reserves"])):
            continue
        covered.add(other)
        build, quote = orders(pool, tokens[0], roles["quoter"]["address"], router)
        row = {"pool": pool, "tokens": tokens, "attempts": []}
        for size in sorted({max(1, min(10**15, cash // 1000)), max(2, min(10**16, cash // 100))}):
            for mode in ("v5", "router"):
                attempt = {"mode": mode, "size": str(size)}
                row["attempts"].append(attempt)
                if mode == "router" and router_eth >= size:
                    attempt["unavailable"] = "wallet WETH debit cannot be attributed with router ETH balance"
                    continue
                try:
                    runs = direct_wallet(c, dict(market, pool=pool), [size], stamp, 0, router,
                                         wrapped=wrapped, modes=[mode], audit=True, keep_partial=True,
                                         build_action=build, quote_call=quote)
                except ValueError as exc:
                    attempt["unavailable"] = str(exc)[:250]
                    continue
                trial, = runs
                for side in ("buy", "sell"):
                    if side not in trial:
                        continue
                    leg = trial[side]
                    raw = leg["official_quote"]
                    if raw["status"] == "0x1":
                        debit, payout, _ = decode(["uint256"] * 3, bytes.fromhex(raw["response"][2:]))
                        leg["matches_quote"] = (debit, payout) == (int(leg["exact"]["spent"]), int(leg["exact"]["received"]))
                attempt.update(trial)
        records.append(row)
    return dict(router_eth=str(router_eth), markets=records)
