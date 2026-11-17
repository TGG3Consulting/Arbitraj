"""Файл: docs/notes/scripts/sigma_series/run.py
Что делает: задача t177 — «было / стало» для оценки σ запаса на движение цены k·σ·√T по критерию решения 360:
доля случаев, где падение цены продажи за T больше запаса, и сумма превышений, по типам направлений.
Части:
 A. Сделки (план σ кода, факт продажи) — «было» на живом боте; сделки с σ = 0 (п. 2 задачи).
 B. Окна стакана: в каждый момент t окна (история от начала окна до t, вперёд — T) σ по истории ряда разными
    способами → запас → падение цены продажи на объёме сделки за T. Это ЗАМЕНА 60-минутного ряда бота (его в выгрузке
    нет): история — секунды-минуты, а не 60 мин; моменты одного окна не независимы. Итог — косвенный.
 C. Скачки (п. 3 задачи): моменты, где один шаг истории несёт ≥ половины Σr², против остальных — что было дальше.
Запуск: python3 run.py [папка выгрузки] (по умолчанию tests/data/sigma_windows_2026-11-16 от корня репозитория).
Кто вызывает: человек. От чего зависит: data.py, series.py, стандартная библиотека.
"""

import collections
import math
import os
import statistics
import sys

import data
import series as S

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", "..", ".."))


def part_a(ds, st):
    """Сделки: запас плана против факта. Печатает таблицу по площадке продажи. Ошибок нет."""
    print("\n## A. Сделки (план бота, факт продажи) — «было»")
    by = collections.defaultdict(lambda: [0, 0, 0.0, 0.0, 0.0])
    zero = []
    for d in ds:
        p, f = d["план"], d["факт"]
        sells = [x for x in f.get("sells") or [] if x.get("avg_price") and float(x.get("filled_qty") or 0) > 0]
        if not sells:
            continue
        ven = d["direction"]["sell_on"]
        q = sum(float(x["filled_qty"]) for x in sells)
        fact = sum(float(x["avg_price"]) * float(x["filled_qty"]) for x in sells) / q
        move = fact / float(p["sell_avg"]) - 1
        proceeds = float(p["proceeds"])
        k = data.setting_for(st, "profit.price_move_k", ven, "dex_cex")
        res = k * float(p["sigma"]) * math.sqrt(float(p["transit_s"]))
        ex = max(0.0, -move - res) * proceeds
        a = by[ven]
        a[0] += 1
        a[1] += ex > 0
        a[2] += ex
        a[3] += res * proceeds
        a[4] += max(0.0, -move) * proceeds
        if float(p["sigma"]) == 0:
            zero.append((d["id"], ven, move, proceeds))
    print("| площадка | сделок | превышений | Σ превышений USDT | Σ запаса USDT | Σ падения USDT |")
    print("|---|---|---|---|---|---|")
    for v, a in sorted(by.items()):
        print("| %s | %d | %d (%.0f %%) | %.2f | %.2f | %.2f |" % (v, a[0], a[1], 100 * a[1] / a[0], a[2], a[3], a[4]))
    print("\nСделки с σ плана = 0 (цена ряда в окне не менялась): %d" % len(zero))
    for i, v, m, pr in zero:
        print("  %s %s движение %+.3f %% (выручка %.2f USDT)" % (i, v, 100 * m, pr))
    return by


VARIANTS = [
    ("было: σ кода, ряд VWAP сигнала", "sig", "code"),
    ("σ_T, ряд VWAP сигнала", "sig", "t"),
    ("max(σ кода, σ_T), ряд VWAP сигнала", "sig", "max"),
    ("σ кода, ряд лучшего бида", "bid", "code"),
    ("σ_T, ряд лучшего бида", "bid", "t"),
    ("σ кода, ряд VWAP объёма сделки", "deal", "code"),
    ("σ кода без наибольшего шага (выброс)", "sig", "trim"),
]


def estimate(s, i_now, t0, now, h, how):
    """σ по истории ряда s до индекса i_now (время now) способом how. None — не измерена."""
    st = S.steps(s, 0, i_now)
    if how == "code":
        return S.sigma_code(st)
    if how == "trim":
        return S.sigma_trim(st)
    t, _ = S.sigma_t(s, t0, now, h)
    if how == "t":
        return t
    c = S.sigma_code(st)
    return None if t is None or c is None else max(t, c)


def part_b(fr, st, med, step_s, probe):
    """Окна: «было / стало» по вариантам σ. Печатает таблицу; возвращает список моментов для части C."""
    print("\n## B. Окна стакана: запас по истории окна против падения цены продажи за T")
    acc = collections.defaultdict(lambda: [0, 0, 0.0, 0.0, 0, set(), 0])
    moments = []
    for (w, ven, tk), frames in fr.items():
        if ven not in med:
            continue
        h, deal_usd, _ = med[ven]
        k = data.setting_for(st, "profit.price_move_k", ven, "dex_cex")
        sig = S.grid(frames, lambda b, a: S.vwap(b, probe / a) if a else None, step_s)
        bid = S.grid(frames, lambda b, a: b[0][0], step_s)
        deal = S.grid(frames, lambda b, a: S.vwap(b, deal_usd / a) if a else None, step_s)
        rows = {"sig": sig, "bid": bid, "deal": deal}
        if not deal or deal[-1][0] - deal[0][0] < 2 * h:
            continue
        t0 = deal[0][0]
        first = True
        for i, (now, p_now) in enumerate(deal):
            if now - t0 < h or now + h > deal[-1][0] + 1e-9 or i % int(round(1 / step_s)):
                continue
            drop = 1 - S.at(deal, now + h) / p_now
            hist = S.steps(sig, 0, min(i, len(sig) - 1)) if sig else []
            moments.append((ven, w, hist, drop))
            for name, ser, how in VARIANTS:
                s = rows[ser]
                if len(s) <= i:
                    continue
                sg = estimate(s, i, t0, now, h, how)
                if sg is None:
                    continue
                res = k * sg * math.sqrt(h)
                ex = max(0.0, drop - res)
                for key in ((ven, name, "все"), (ven, name, "первый")) if first else ((ven, name, "все"),):
                    a = acc[key]
                    a[0] += 1
                    a[1] += ex > 0
                    a[2] += ex * deal_usd
                    a[3] += res * deal_usd
                    a[4] += res >= 1
                    a[5].add(w)
            first = False
    print("| площадка | моменты | вариант σ | случаев | окон | превышений | Σ превышений USDT | Σ запаса USDT | запас ≥ 100 % |")
    print("|---|---|---|---|---|---|---|---|---|")
    for (ven, name, which), a in sorted(acc.items(), key=lambda x: (x[0][0], x[0][2], [v[0] for v in VARIANTS].index(x[0][1]))):
        print("| %s | %s | %s | %d | %d | %d (%.1f %%) | %.3f | %.2f | %d |" % (
            ven, which, name, a[0], len(a[5]), a[1], 100 * a[1] / a[0], a[2], a[3], a[4]))
    return moments


def part_c(moments):
    """Скачки в истории: что было дальше. Печатает таблицу. Ошибок нет."""
    print("\n## C. Скачок в истории (один шаг ≥ половины Σr² истории ряда сигнала) — что дальше за T")
    acc = collections.defaultdict(list)
    for ven, w, hist, drop in moments:
        sq = sum(r * r for r, _ in hist)
        if sq == 0:
            kind = "цена не менялась"
        else:
            kind = "скачок" if max(r * r for r, _ in hist) >= sq / 2 else "без скачка"
        acc[(ven, kind)].append(drop)
    print("| площадка | история | моментов | доля падений > 0 | среднее |движение| за T, % | 90-й процентиль падения, % |")
    print("|---|---|---|---|---|---|")
    for (ven, kind), xs in sorted(acc.items()):
        q = sorted(xs)
        p90 = q[min(len(q) - 1, int(0.9 * len(q)))]
        print("| %s | %s | %d | %.0f %% | %.3f | %.3f |" % (
            ven, kind, len(xs), 100 * sum(x > 0 for x in xs) / len(xs), 100 * statistics.mean(abs(x) for x in xs), 100 * p90))


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "tests", "data", "sigma_windows_2026-11-16")
    st = data.settings(root)
    ds = data.deals(root)
    med = data.deal_medians(ds)
    step_s = float(st["signal.eval_min_interval_ms"][0]["value"]) / 1000
    probe = float(st["signal.probe_usd"][0]["value"])
    print("Медианы плана сделок (T с, выручка USDT, сделок):", {v: med[v] for v in sorted(med)})
    print("Шаг замеров %.1f с, объём сигнала %.0f USDT" % (step_s, probe))
    part_a(ds, st)
    moments = part_b(data.frames(root), st, med, step_s, probe)
    part_c(moments)


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    main()
