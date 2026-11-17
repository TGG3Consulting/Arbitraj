"""Файл: docs/notes/scripts/sigma_series/series.py
Что делает: ряды цены продажи из кадров стакана и оценки σ — те же формулы, что у бота:
- σ кода (src/core/vol.rs measure): σ² = Σ(ln pᵢ/pᵢ₋₁)² / ΣΔt по замерам ряда;
- σ_T (src/core/vol_horizon.rs of): σ_T² = ΣRⱼ² / (m·T), Rⱼ — изменение лог-цены на непересекающихся отрезках длины T
  назад от «сейчас», цена отрезка — последний замер не позже его конца, m — полные отрезки;
- σ без наибольшего шага — пример «отбросить выброс» (для проверки п. 3 задачи t177; в боте такой оценки нет).
Замеры — на сетке signal.eval_min_interval_ms (предыдущее значение), как у t161 (scratchpad t161/win2.py).
Кто вызывает: run.py.
От чего зависит: только стандартная библиотека.
"""

import math


def vwap(bids, qty):
    """Средняя цена продажи `qty` монет по бидам [(цена, объём)]. Не хватает глубины 10 уровней — средняя по
    исполнимому (как fillable_qty котировки сигнала: prices.rs берёт min исполнимого). None — бидов нет."""
    left, filled, notional = qty, 0.0, 0.0
    for p, q in bids:
        t = min(left, q)
        filled += t
        notional += t * p
        left -= t
        if left <= 0:
            break
    return notional / filled if filled > 0 else None


def grid(frames, price_of, step_s):
    """Ряд [(сек, цена)] на сетке шага step_s от первого кадра: значение — по последнему кадру не позже узла.
    price_of(кадр) → цена или None (кадр пропускается)."""
    pts = [(t, price_of(b, a)) for t, b, a in frames]
    pts = [(t, p) for t, p in pts if p and p > 0]
    if len(pts) < 2:
        return []
    out, i, t = [], 0, pts[0][0]
    while t <= pts[-1][0] + 1e-9:
        while i + 1 < len(pts) and pts[i + 1][0] <= t:
            i += 1
        out.append((t, pts[i][1]))
        t += step_s
    return out


def at(s, t):
    """Цена ряда s в момент t — последний замер не позже t. None — t раньше первого замера."""
    lo, hi, ans = 0, len(s) - 1, None
    while lo <= hi:
        mid = (lo + hi) // 2
        if s[mid][0] <= t + 1e-9:
            ans = s[mid][1]
            lo = mid + 1
        else:
            hi = mid - 1
    return ans


def steps(s, i0, i1):
    """Лог-доходности и Δt шагов ряда между индексами i0..i1 включительно: [(r, dt)]."""
    return [(math.log(s[i][1] / s[i - 1][1]), s[i][0] - s[i - 1][0]) for i in range(i0 + 1, i1 + 1)]


def sigma_code(st):
    """σ кода (vol.rs measure) по шагам [(r, dt)]. None — ΣΔt = 0."""
    dt = sum(d for _, d in st)
    return math.sqrt(sum(r * r for r, _ in st) / dt) if dt > 0 else None


def sigma_trim(st):
    """σ кода без одного шага с наибольшим |r| (оценка «без выброса»; в боте её нет). None — шагов < 2."""
    if len(st) < 2:
        return None
    j = max(range(len(st)), key=lambda i: abs(st[i][0]))
    return sigma_code(st[:j] + st[j + 1:])


def sigma_t(s, t0, now, h):
    """σ_T (vol_horizon.rs of) по ряду s на отрезке истории [t0; now], горизонт h с. Возвращает (σ_T, m);
    σ_T None — нет ни одного полного отрезка."""
    m = int((now - t0) // h)
    if m <= 0:
        return None, 0
    total = 0.0
    for j in range(m):
        end, start = now - j * h, now - (j + 1) * h
        pe, ps = at(s, end), at(s, start)
        total += math.log(pe / ps) ** 2
    return math.sqrt(total / (m * h)), m


def rho1(st):
    """Автокорреляция соседних шагов ρ₁ = Σrᵢrᵢ₋₁ / Σrᵢ² (как vol_horizon.rs). None — Σr² = 0."""
    sq = sum(r * r for r, _ in st)
    if sq == 0:
        return None
    return sum(st[i][0] * st[i - 1][0] for i in range(1, len(st))) / sq
