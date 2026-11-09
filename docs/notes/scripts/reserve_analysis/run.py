#!/usr/bin/env python3
"""Файл: docs/notes/scripts/reserve_analysis/run.py
Что делает: разбор «запас на движение цены против реального движения» (решение 240, docs/notes/reserve-analysis-2026-11-09.md)
на сервере рядом с ботом: читает бота (только GET) и узел BSC (только чтение), считает (А) упущенные биржа → DEX,
(Б) DEX → биржа по сделкам, (В) значения существующих настроек запаса с наибольшим итогом на истории; пишет out/.
Идемпотентен: ответы бота и узла кэшируются в out/cache_*, повторный запуск продолжает с места обрыва.
Кто вызывает: человек / ИИ на сервере: python3 run.py --out /home/ubuntu/arb-analysis/out
От чего зависит: модули этой папки; только стандартная библиотека Python 3.8+.
"""

import argparse
import os
import sys
import time
from decimal import Decimal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import keccak  # noqa: E402
from botapi import Bot  # noqa: E402
from common import dump_json, log, set_log, ts, iso  # noqa: E402
from dexmove import t_real_table, lookup, rows_cf, deals_model  # noqa: E402
from evaluate import variants, run_variant, best, GRIDS  # noqa: E402
from records import reject_rows, episodes, deals  # noqa: E402
from report import summary_a, summary_b, move_vs_reserve, deals_check_stats, write_tables  # noqa: E402
from reserve import Settings, thresholds, current, model_check  # noqa: E402
from summary_text import readme, brief, RELEVANT  # noqa: E402


def args():
    p = argparse.ArgumentParser(description="Разбор запаса на движение цены (только чтение бота и узла)")
    p.add_argument("--bot", default="http://127.0.0.1:8787")
    p.add_argument("--host", default="127.0.0.1:8787", help="заголовок Host (= --listen бота, src/api/guard.rs)")
    p.add_argument("--node", default="http://127.0.0.1:8545")
    p.add_argument("--out", default="out")
    p.add_argument("--rps", type=float, default=15.0, help="запросов к узлу в секунду (не больше 20)")
    p.add_argument("--chunk", type=int, default=200, help="окно eth_getLogs, блоков")
    p.add_argument("--lookback", type=int, default=20000, help="сколько блоков назад искать базу пула")
    p.add_argument("--gap-s", type=int, default=120, help="разрыв эпизода, секунды")
    p.add_argument("--journal-page", type=int, default=3000)
    p.add_argument("--journal-max-pages", type=int, default=0, help="0 — весь журнал")
    p.add_argument("--hours", type=float, default=0, help="только последние N часов журнала (0 — весь)")
    p.add_argument("--min-pair-deals", type=int, default=5, help="минимум сделок для T_real пары/источника")
    p.add_argument("--deals-days", type=float, default=0, help="проверка метода на сделках за N дней (0 — все)")
    p.add_argument("--refresh-api", action="store_true", help="перечитать бота (кэш узла сохраняется)")
    p.add_argument("--skip-node", action="store_true", help="без узла: только сделки и пересчёт запаса")
    a = p.parse_args()
    if a.rps > 20:
        p.error("--rps больше 20 запрещено условием")
    return a


def prepare(S, rows):
    """Параметры запаса строки (cur) и сверка записи с формулой; k по записи — если не сошлось и доли нет."""
    bad = 0
    for r in rows:
        r["sqrtT"] = r["T"].sqrt() if r["T"] and r["T"] > 0 else Decimal(0)
        cur = current(S, r)
        ok, k_impl = model_check(r, cur)
        r["model_consistent"] = ok
        if not ok:
            bad += 1
            if cur["share"] is None and k_impl is not None:
                cur = dict(cur, k=k_impl)
        r["cur"] = cur
    return bad


def main():
    a = args()
    os.makedirs(a.out, exist_ok=True)
    set_log(os.path.join(a.out, "progress.log"))
    log("старт: %s" % " ".join(sys.argv[1:]))
    bad = keccak.self_check()
    if bad:
        log("СТОП: keccak не сошёлся с записанными topic0: %s" % bad)
        sys.exit(2)
    bot = Bot(a.bot, a.host, os.path.join(a.out, "cache_api"))
    S_items = bot.cached("settings", "/api/settings", refresh=a.refresh_api)
    hist = bot.cached("settings_history", "/api/settings/history", {"limit": 20000}, refresh=a.refresh_api)
    raw_deals = bot.cached("deals", "/api/deals", refresh=a.refresh_api)
    wins = bot.windows(refresh=a.refresh_api)
    since = Decimal(time.time() - a.hours * 3600) if a.hours else None
    entries, jst = bot.journal_kind("core.candidate_rejected", a.journal_page, a.journal_max_pages, since,
                                    refresh=a.refresh_api)
    S = Settings(S_items)
    th = thresholds(S)
    rows, rcnt = reject_rows(entries, th)
    dl, excl = deals(raw_deals)
    p0, p1 = ts(jst["oldest_at"]), ts(jst["newest_at"])
    for d in dl:
        d["period"] = p0 is not None and p0 <= d["at"] <= p1
    inconsistent = prepare(S, rows + dl)
    eps = episodes(rows, a.gap_s)
    ro = []
    for i, ep in enumerate(eps, 1):
        ep["ro"] = [r for r in ep["rows"] if r["reserve_only"]]
        for r in ep["ro"]:
            r["ep"] = i
        ro.extend(ep["ro"])
    tab = t_real_table(dl, a.min_pair_deals)
    for r in ro:
        t = lookup(tab, r)
        if t:
            r["T_real"], r["T_real_p75"] = t[0], t[1]
    for w in wins:
        w["_st"], w["_en"] = ts(w.get("started_at")), ts(w.get("ended_at"))
    node_stats = None
    if not a.skip_node:
        from node import Node
        node = Node(a.node, a.rps, os.path.join(a.out, "cache_node.jsonl"), a.chunk)
        node.refresh_head()
        log("узел: голова %d (%s)" % (node.head, iso(node.head_ts)))
        rows_cf(node, ro, tab, a.lookback)
        lim = Decimal(time.time() - a.deals_days * 86400) if a.deals_days else None
        deals_model(node, [d for d in dl if lim is None or d["at"] >= lim], a.lookback)
        node_stats = node.stats()
        node.close()
    va = sorted({r["A"] for r in rows + dl})
    vb = sorted({r["B"] for r in rows + dl})
    dp = [d for d in dl if d["period"]]
    table = []
    vs = variants(va, vb)
    for i, var in enumerate(vs, 1):
        if i % 100 == 0:
            log("варианты: %d из %d" % (i, len(vs)))
        table.extend(run_variant(var, dp, dl, eps, th))
    summary = {
        "generated_at": iso(time.time()),
        "journal": {k: jst.get(k) for k in ("pages", "seen", "oldest_id", "oldest_at", "newest_id", "newest_at")},
        "thresholds": th,
        "settings_now": {n: [it for it in S_items if it["name"] == n] for n in RELEVANT},
        "settings_history_relevant": [h for h in hist if isinstance(h, dict) and h.get("name") in RELEVANT],
        "reject_rows": dict(rcnt, rows=len(rows), threshold=sum(r["threshold"] for r in rows),
                            reserve_only_by_type=_by_type(ro), episodes_reserve_only=sum(1 for ep in eps if ep["ro"])),
        "reserve_formula_mismatch_rows": inconsistent,
        "deals": {"clean": len(dl), "excluded": excl, "in_journal_period": len(dp),
                  "return_cost_unknown": sum(1 for d in dl if not d["return_cost_known"])},
        "T_real_by_pair": {"%s %s→%s" % k: v for k, v in tab[0].items()},
        "T_real_by_source": {"%s %s" % k: v for k, v in tab[1].items()},
        "A_cex_dex_missed": summary_a(eps, th) if not a.skip_node else "не посчитано: --skip-node",
        "B_dex_cex": summary_b(dl, eps, wins, th),
        "move_vs_reserve_deals_all": move_vs_reserve(dl),
        "move_vs_reserve_deals_period": move_vs_reserve(dp),
        "method_check_deals": deals_check_stats(dl) if not a.skip_node else "не посчитано: --skip-node",
        "C_best": best(table, _currents(S, va, vb)),
        "grids": GRIDS,
        "node": node_stats,
    }
    write_tables(a.out, eps, ro, dl, table)
    dump_json(os.path.join(a.out, "summary.json"), summary)
    with open(os.path.join(a.out, "README.txt"), "w", encoding="utf-8") as f:
        f.write(readme(a))
    print(brief(summary))
    log("готово: %s" % a.out)


def _currents(S, va, vb):
    """Текущие значения настроек в областях перебора — для выбора при равенстве итога. Ошибок нет."""
    out = {}
    for st in GRIDS:
        if st in ("profit.price_move_k", "profit.sigma_min_changes"):
            v = S.exact(st)
            out[(st, "global")] = None if v is None else Decimal(str(v))
            continue
        for sid in (va if st == "profit.transit_manual_s" else vb):
            v = S.exact(st, "Venue", sid)
            out[(st, sid)] = None if v is None else Decimal(str(v))
    return out


def _by_type(rs):
    out = {}
    for r in rs:
        out[r["type"]] = out.get(r["type"], 0) + 1
    return out


if __name__ == "__main__":
    main()
