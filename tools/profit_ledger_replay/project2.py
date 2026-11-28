"""Обезличенная проекция сделок для прогона по релизам (вариант 28.11.2026, без Codex).

Что делает: читает две выгрузки /api/deals бота («исходная» и «последняя»), оставляет только
финансовые поля, нужные расчёту итога сделки и стоимости возврата USDT (как project.py Codex),
и группы: всё / последние сутки / прошлые сутки / день UTC / биржа сделки. Монеты, адреса,
ID ордеров и сделок, тексты событий удаляются. Две сделки TLOS с проверенной квитанцией
помечаются флагом tlos (номер ID не передаётся) — квитанцию проверяет сам прогон в CI (tlos_fix.py).
Кто вызывает: владелец/аналитик локально перед шифрованием (envelope.py seal).
Зависит от: формата карточек /api/deals (поля как в project.py).
Ошибки: AssertionError — сделка не закрыта или «исходная» содержит сделку, которой нет в «последней».
"""
import argparse
import gzip
import hashlib
import json
from collections import Counter
from pathlib import Path

from project import end_at, financial_part, read_cards, timestamp, timedelta

# Те же две сделки, что в correction_map project.py (проверены по квитанции 0x2901e120…).
TLOS = ("20261124052119344TLOS", "20261124052115166TLOS")


def project(card, cutoff):
    """Принимает карточку и момент отсечки; возвращает обезличенную строку для replay2.rs."""
    when = end_at(card)
    groups = ["all"]
    if when:
        if cutoff - timedelta(days=1) <= when < cutoff:
            groups.append("current_24")
        if cutoff - timedelta(days=2) <= when < cutoff - timedelta(days=1):
            groups.append("previous_24")
        if cutoff.replace(hour=0, minute=0, second=0, microsecond=0) <= when < cutoff:
            groups.append("utc_day")
    direction = card.get("direction") or {}
    for side in ("buy_on", "sell_on"):
        venue = direction.get(side)
        if venue and venue != "bsc_dex":
            groups.append("venue:" + venue)
    groups = list(dict.fromkeys(groups))
    parts = [financial_part(x) for x in card.get("usdt_return_parts") or []]
    group = card.get("group") or {}
    return dict(
        groups=groups,
        has_end=bool(when),
        invested=card["invested"], proceeds=card["proceeds"], spent=card["spent"],
        recorded=card.get("result_usd"), return_cost=card.get("return_cost_usd"),
        first=financial_part(card.get("usdt_return")), rest=parts,
        pieces=group.get("pieces"), tlos=card["id"] in TLOS, verified_credit=None,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("original", "latest", "output"):
        parser.add_argument(name, type=Path)
    parser.add_argument("--cutoff", required=True, type=timestamp)
    args = parser.parse_args()
    original, latest = read_cards(args.original), read_cards(args.latest)
    assert original.keys() <= latest.keys()
    result = {}
    for name, cards in (("original", original), ("latest", latest)):
        rows = []
        for key in sorted(cards, key=lambda k: hashlib.sha256(k.encode()).digest()):
            card = cards[key]
            assert card["step"] == "Closed"
            rows.append(project(card, args.cutoff))
        result[name] = rows
        print(name, len(rows), sum(r["tlos"] for r in rows), Counter(g for r in rows for g in r["groups"]))
    with gzip.open(args.output, "wt", encoding="utf-8", compresslevel=7) as f:
        json.dump(result, f, separators=(",", ":"))


if __name__ == "__main__":
    main()
