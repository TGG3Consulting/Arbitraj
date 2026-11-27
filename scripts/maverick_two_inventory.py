"""Строка Maverick V2 по реестру фабрики и архивным измерениям выплат."""

import gzip
import json
from pathlib import Path


def inventory(network):
    path = Path("tests/data/first_run_dex_2026-11-26")
    directory = path / f"maverick_v2_{network}_directory.json.gz"
    wallets = path / f"maverick_v2_{network}_wallets.json.gz"
    with gzip.open(directory, "rt") as source:
        catalog = json.load(source)
    with gzip.open(wallets, "rt") as source:
        payments = json.load(source)
    assert payments["selector"] == catalog["selector"] and payments["roles"] == catalog["roles"]
    roles = catalog["roles"]
    swaps = sum(direction["count"] for pair in catalog["events"].values() for direction in pair.values())
    paid = [market for market in payments["samples"] if market.get("cases")]
    savings = []
    targets = 0
    for market in paid:
        by_size = {}
        for case in market["cases"]:
            by_size.setdefault(case["size"], {})[case["mode"]] = case
        for routes in by_size.values():
            for side in ("buy", "sell"):
                v5, router = (routes[mode][side]["exact"] for mode in ("v5", "router"))
                assert (v5["spent"], v5["received"]) == (router["spent"], router["received"])
                savings.append(int(v5["gas"], 16) - int(router["gas"], 16))
        targets += sum(len(item["legs"]) for item in market["exact_out"])
    separate = payments["independent"]
    paired = []
    for market in separate["markets"]:
        by_size = {}
        for attempt in market["attempts"]:
            by_size.setdefault(attempt["size"], {})[attempt["mode"]] = attempt
        for routes in by_size.values():
            for side in ("buy", "sell"):
                if all(side in routes.get(mode, {}) for mode in ("v5", "router")):
                    compared = [routes[mode][side]["exact"] for mode in ("v5", "router")]
                    assert all((r["spent"], r["received"]) == (compared[0]["spent"], compared[0]["received"]) for r in compared)
                    paired.append(market["pool"])
    tokens = "DEGEN и VIRTUAL" if network == "base" else "SKY"
    text = (
        f"Число пулов в реестре Maverick V2 — {len(catalog['markets'])}. "
        f"На блоках {catalog['first_block']}–{catalog['height']} число событий PoolSwap достигло {swaps} "
        f"за 24 часа. Активных пулов за этот период — {len(catalog['events'])}. "
        f"На {len(paid)} рынках V5 и официальный ERC20 Router дали одинаковый расход и поступление "
        f"в {len(savings)} парных сравнениях. Router сэкономил {min(savings)}–{max(savings)} ед. газа на обмен. "
        f"Ещё {targets} выплат Router проверены с точным выходом и пределом входа. "
        f"Независимые пробы узких рынков добавили {len(paired)} парных сравнений на {len(set(paired))} пулах. "
        f"Прямое поручение разрешает оплату токенами {tokens} при сверке неизменяемого кода входа и выхода. "
        "Радиус чтения 32 тика в каждую сторону проверен сравнением CPU с Quoter для двух размеров и обеих сторон. "
        "За пределами прочитанного диапазона требуется увеличить радиус и повторить расчёт. Минимум проверяется по чистой выплате. При оплате WETH применяется V5. Разрешение ERC20 измеряется отдельно. "
        "[Код токенов](../tests/data/first_run_dex_2026-11-26/maverick_v2_exact_tokens.json) · "
        f"[Реестр](../{directory.as_posix()}) · [Кошельковые измерения](../{wallets.as_posix()})"
    )
    return catalog["source"], [roles["factory"]["address"]], text
