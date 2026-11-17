"""Файл: docs/notes/scripts/sigma_series/data.py
Что делает: читает выгрузку живого бота за 48 ч (14.11 21:20 — 16.11 21:25 UTC, tests/data/sigma_windows_2026-11-16):
кадры стакана площадки продажи в окнах, сделки с продажей на бирже, настройки. Ничего не досчитывает.
Кто вызывает: run.py.
От чего зависит: только стандартная библиотека Python 3.8+.
"""

import collections
import gzip
import json
import os
import statistics


def _lines(path):
    """Строки JSONL (обычный файл или .gz). Возвращает генератор словарей."""
    op = gzip.open if path.endswith(".gz") else open
    with op(path, "rt", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                yield json.loads(line)


def frames(root):
    """Кадры стакана по окну и площадке: {(окно, площадка, тикер): [(сек, биды [(цена, объём)], лучший аск)]},
    по времени приёма ботом (ns). Кадры без бидов пропускаются (в них нечего продавать)."""
    out = collections.defaultdict(list)
    for j in _lines(os.path.join(root, "book_frames.jsonl.gz")):
        if not j.get("bids"):
            continue
        bids = [(float(p), float(q)) for p, q in j["bids"]]
        ask = float(j["best_ask"][0]) if j.get("best_ask") else None
        out[(j["window"], j["venue"], j["ticker"])].append((j["ns"] / 1e9, bids, ask))
    for v in out.values():
        v.sort(key=lambda x: x[0])
    return out


def deals(root):
    """Сделки с продажей на бирже: план (sigma, transit_s, proceeds, sell_avg) и факт продажи. Список словарей."""
    return list(_lines(os.path.join(root, "deals_sell_on_cex.jsonl")))


def settings(root):
    """Настройки бота со всеми областями: {имя: [{scope, value}] | текст «нет такой настройки»}."""
    with open(os.path.join(root, "settings.json"), encoding="utf-8") as f:
        return json.load(f)


def setting_for(st, name, venue, kind):
    """Значение настройки для продажи на `venue` и типа направления `kind` (dex_cex, cex_dex, cex_cex): пара
    «площадка:тип» (решение 242), иначе площадка, иначе вся система. None — не задана."""
    rows = st.get(name)
    if not isinstance(rows, list):
        return None
    by = {}
    for r in rows:
        sc = r["scope"]
        by[(sc["kind"], sc.get("id"))] = r["value"]
    for key in (("VenueDir", "%s:%s" % (venue, kind)), ("Venue", venue), ("Global", None)):
        if key in by:
            return float(by[key])
    return None


def deal_medians(ds):
    """Медианы плана по площадке продажи: {площадка: (T плана, с; выручка плана, USDT; число сделок)}."""
    by = collections.defaultdict(lambda: ([], []))
    for d in ds:
        p = d["план"]
        ven = d["direction"]["sell_on"]
        by[ven][0].append(float(p["transit_s"]))
        by[ven][1].append(float(p["proceeds"]))
    return {v: (statistics.median(t), statistics.median(pr), len(t)) for v, (t, pr) in by.items()}
