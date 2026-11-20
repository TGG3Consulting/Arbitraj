#!/usr/bin/env python3
# Файл: tools/arbitrum_record/forks_482.py
# Что делает: решение 482 — запись своего узла Arbitrum для DEX, подключённых настройками как форки поддержанных видов:
#   V2 — Joe DEX / LFJ V1 (v2_fork), Rubicon Aquila (v2_fork), Swapsicle V1 (v2_fork_executor), Arbitrum Exchange V2
#   (v2_fork_executor, swapFee 25/10000); V3 — Rubicon CLMM и Arbitrum Exchange V3 (v3_fork; часть forks_482_v3.py).
#   ZyberSwap Stableswap (saddle_stableswap) пишет saddle.py отдельным шагом record_steps.py (тот же разбор, что
#   решение 451). Только чтение (eth_call, eth_getCode, eth_getBlockByNumber), без ключей, подписей и транзакций.
#   Поручение координатора к решению 482: источники пулов без индексатора — Swapline V1 (lb_v21, getAllLBPairs) и
#   Metric V1 (metric_omm, перечисление фабрики) — forks_482_reg.py, файл reg.json.
#   Пишет tests/data/arbitrum_forks_482_<дата>/ (meta.json, v2.json, v3.json, reg.json, README.md); тест —
#   tests/arbitrum_forks_482_live.rs (#[ignore] до записи).
# Кто вызывает: Тигран (record_all.py → record_steps.py, шаг forks_482) или вручную.
# От чего зависит: Python 3.8+ (стандартная библиотека), recnode.py (fresh), curve_plain_pools.py (Batch, call),
#   v4live_rpc.py (Node, sel, w, ad, word), v4live_slots.py (_find, _balance_cands), curve_sim.py (SENDER),
#   forks_482_v3.py. Узел — аргумент --rpc.
# Настройки: нет. Адреса — по умолчанию DEXES ниже (источники — docs/notes/arbitrum/dex-settings-arbitrum.md §7).
#
# Порядок (узел хранит состояние ~128 блоков — решение 460): 1) на «latest» — перечисление пар фабрик V2
#   (allPairsLength / allPairs, до --scan пар; выбор до --pairs пар с наибольшим резервом WETH, иначе любые с обоими
#   резервами > 0), поля V3-пулов (token0, token1, fee), слоты balanceOf токенов входа у пары (v4live_slots._find —
#   место слота от блока не зависит); 2) фаза состояния на свежем блоке B (recnode.fresh): пакет B1 — getReserves,
#   factory, token0/1, swapFee() (Arbidex), getPair фабрики; V3 — поля, getPool, балансы; пакет B2 — getAmountsOut
#   роутера (Joe, Aquila), симуляция обмена в паре: eth_call pair.swap(out, SENDER, «») от пустого адреса с подменой
#   баланса входа у пары = резерв + dx (вход ровно dx — как перевод исполнителя), out — формула шага V2 исполнителя
#   (contracts/ArbExecutor.sol _v2Out, комиссия в пипсах), и то же с out + 1 (пара должна откатить проверку K); V3 —
#   TickLens, QuoterV2, виртуальный квотер; пакет B3 — ticks(t) пула по тикам ответов TickLens.
#
# Запуск: python3 tools/arbitrum_record/forks_482.py --rpc http://УЗЕЛ:8547

import argparse
import datetime
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import forks_482_v2 as v2m  # noqa: E402
import forks_482_reg as reg  # noqa: E402
import forks_482_v3 as v3  # noqa: E402
import recnode  # noqa: E402
import v4live_slots as slots  # noqa: E402
from curve_plain_pools import Batch, call  # noqa: E402
from curve_sim import SENDER  # noqa: E402
from v4live_rpc import Node, ad, sel, word  # noqa: E402

UNI_LENS = "0xbfd8137f7d1516D3ea5cA83523914859ec47F573"  # TickLens Uniswap V3 (v3-arbitrum-deployments.mdx L17)
DEXES_V2 = [
    {"id": "joe-dex@arbitrum", "factory": "0xaE4EC9901c3076D0DdBe76A520F9E90a6227aCB7",
     "router": "0xbeE5c10Cf6E4F68f831E11C1D9E59B43560B3642", "fee_pips": 3000, "fee_pct": "0.3"},
    {"id": "rubicon-aquila@arbitrum", "factory": "0xEca3EA559b7566e610d113bbA8D1B15B085C9c68",
     "router": "0x9C42Fae9e3C63150BCBe4Aaba292681d5b0b0720", "fee_pips": 3000, "fee_pct": "0.3"},
    {"id": "swapsicle-v1@arbitrum", "factory": "0x2f0c7c98462651bb2102f6cd05acdad333e031b0",
     "router": None, "fee_pips": 3000, "fee_pct": "0.3"},
    {"id": "arbidex@arbitrum", "factory": "0x1C6E968f2E6c9DEC61DB874E28589fd5CE3E1f2c",
     "router": None, "fee_pips": 2500, "fee_pct": "0.25", "fee_method": "swapFee()"},
]
DEXES_V3 = [
    {"id": "rubicon-clmm-arbitrum@arbitrum", "factory": "0x045B7012CbD158C1b48874310F985Adb48aA62ba",
     "tick_lens": "0xdf7e8987421F541C50f821f8Cf22A6bDA5e1E05b", "quoter": "0x22066f5A85076630CF9DEEB0A12186b73CD3284C",
     "pools": ["0xee57310555a957cec1debb65b1e165d7c9ed700f", "0x31168cfc7ffe206ba651224b017170953c2ee1b8"]},
    {"id": "arbidex-v3@arbitrum", "factory": "0x855f2c70cf5cb1d56c15ed309a4dfefb88ed909e", "tick_lens": None,
     "quoter": None, "pools": ["0x837823ed246a7e34a59aa96701c6f2de9e96d592", "0x06537920bde4960713e220bd9cb4844a1461101c",
                               "0x48d7e1a9d652ba5f5d80a8dc396df37993659f35"]},
]


def say(text):
    print(text, file=sys.stderr, flush=True)


def phase(b, pre, a, head):
    """Фаза состояния на блоке head: пакеты B1, B2, B3. StaleState — наружу (recnode.fresh)."""
    tag = head["number"]
    fr = [float(x) for x in a.fractions.split(",")]
    v2, v3recs = [], []
    reqs, owners = [], []
    for dex, pairs, _ in pre["v2"]:
        for p, t0, t1 in pairs:
            rq = v2m.v2_state_reqs(dex, p, t0, t1, tag)
            owners.append(("v2", dex, {"dex": dex["id"], "pair": p, "token0": t0, "token1": t1}, len(rq)))
            reqs += rq
    for dex, pools in pre["v3"]:
        for pool, tok in pools:
            rq = v3.state_reqs(dex, pool, tok, tag)
            owners.append(("v3", dex, {"dex": dex["id"], "pool": pool, "token0": tok[0], "token1": tok[1],
                                       "fee": tok[2]}, len(rq)))
            reqs += rq
    got, at = b.run(reqs), 0
    for kind, dex, rec, n in owners:
        part, at = got[at:at + n], at + n
        if kind == "v2":
            names = ["getReserves()", "factory()", "token0()", "token1()", "getPair"] + \
                    ([dex["fee_method"]] if dex.get("fee_method") else [])
            rec["state"] = dict(zip(names, part))
            if rec["state"]["getReserves()"].get("result"):
                v2.append((dex, rec))
        else:
            rec["state"] = v3.parse_state(part)
            v3recs.append((dex, rec))
    reqs, owners = [], []
    for dex, rec in v2:
        rq, mk = v2m.v2_quote_reqs(dex, rec, pre["slots"], fr, tag)
        owners.append((rec, mk))
        reqs += rq
    for dex, rec in v3recs:
        dex = dict(dex, uniswap_lens=UNI_LENS, words=a.words)
        rq, mk = v3.quote_reqs(dex, rec["pool"], (rec["token0"], rec["token1"], rec["fee"]), rec["state"], fr,
                               pre["vq"], tag)
        owners.append((rec, mk))
        reqs += rq
    got, at = b.run(reqs), 0
    for rec, mk in owners:
        part, at = got[at:at + len(mk)], at + len(mk)
        rec["answers"] = [{"mark": list(m), **x} for m, x in zip(mk, part)]
    reqs, owners = [], []
    for _, rec in v3recs:
        ticks = sorted({t for x in rec["answers"] if x["mark"][0] == "lens"
                        for t in v3.lens_ticks(x.get("result"))})
        owners.append((rec, ticks))
        reqs += v3.ticks_reqs(rec["pool"], ticks, tag)
    got, at = b.run(reqs), 0
    for rec, ticks in owners:
        part, at = got[at:at + len(ticks)], at + len(ticks)
        rec["ticks"] = [{"tick": t, **x} for t, x in zip(ticks, part)]
    return head, [r for _, r in v2], [r for _, r in v3recs]


def prepare(b, node, a):
    """Шаг 1 (на «latest»): пары V2, поля V3-пулов, слоты балансов, код виртуального квотера."""
    pre = {"v2": [], "v3": [], "slots": {}}
    for dex in DEXES_V2:
        try:
            pairs = v2m.pick_pairs(b, dex, a.scan, a.pairs)
        except Exception as e:  # noqa: BLE001 — причина в запись
            say(f"{dex['id']}: пары не перечислены: {e}")
            pairs = []
        pre["v2"].append((dex, pairs, None))
        for p, t0, t1 in pairs:
            for t in (t0, t1):
                found, why = slots._find(node, "latest", t, slots._balance_cands(p),
                                         "0x" + sel("balanceOf(address)") + ad(p))
                pre["slots"][(t.lower(), p.lower())] = found[2] if found else None
                if not found:
                    say(f"{t} у {p}: слот баланса не найден — {why}")
    for dex in DEXES_V3:
        pools = []
        for pool in dex["pools"]:
            g = b.run([call(pool, "0x" + sel(s), "latest") for s in ("token0()", "token1()", "fee()")])
            if all(x.get("result") for x in g):
                pools.append((pool, ("0x" + g[0]["result"][-40:], "0x" + g[1]["result"][-40:],
                                     word(g[2]["result"], 0))))
        pre["v3"].append((dex, pools))
    pre["vq"] = "0x" + open(a.vq_runtime).read().strip().replace("0x", "")
    return pre


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: форки решения 482")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--fractions", default="0.0001,0.003,0.05", help="доли резерва / баланса входа")
    ap.add_argument("--scan", type=int, default=300, help="сколько последних пар фабрики V2 смотреть")
    ap.add_argument("--pairs", type=int, default=3, help="сколько пар V2 на DEX писать")
    ap.add_argument("--words", type=int, default=8, help="слов битмапа V3 по обе стороны от текущего")
    ap.add_argument("--vq-runtime", default="contracts/out/VirtualQuoter.bin-runtime")
    ap.add_argument("--reg-log-blocks", type=int, default=2_000_000,
                    help="окно логов событий создания Swapline / Metric (решение 482, forks_482_reg.py)")
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    b, node = Batch(a.rpc), Node(a.rpc)
    pre = prepare(b, node, a)
    head, v2recs, v3recs = recnode.fresh(lambda: b.one("eth_getBlockByNumber", ["latest", False])["result"],
                                         lambda h: phase(b, pre, a, h), say=say)
    try:
        reg_rec = reg.record(b, a.reg_log_blocks, say)
    except (Exception, recnode.StaleState) as e:  # noqa: BLE001 — причина в запись, остальное уже записано
        reg_rec = {"error": f"{type(e).__name__}: {str(e)[:300]}"}
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = a.out or os.path.join(root, "tests", "data", "arbitrum_forks_482_" + day)
    os.makedirs(out, exist_ok=True)
    meta = {"chainId": b.one("eth_chainId", [])["result"], "block": int(head["number"], 16),
            "timestamp": int(head["timestamp"], 16), "fractions": a.fractions, "words": a.words,
            "dexes_v2": DEXES_V2, "dexes_v3": DEXES_V3, "uniswap_lens": UNI_LENS, "sender": SENDER}
    files = {"meta.json": meta, "v2.json": v2recs, "v3.json": v3recs, "reg.json": reg_rec}
    hashes = []
    for name, data in files.items():
        text = json.dumps(data, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write(f"# Форки решения 482 (Arbitrum One) — запись своего узла, {day}\n\nЗаписано "
                f"tools/arbitrum_record/forks_482.py: только чтение. Блок B = {meta['block']}, chainId "
                f"{meta['chainId']}. Пар V2: {len(v2recs)}, пулов V3: {len(v3recs)}. Файлы — шапка скрипта.\n\n"
                "## SHA-256\n\n" + "".join(f"{h}  {n}\n" for h, n in hashes))
    say(f"записано в {out}")


if __name__ == "__main__":
    main()
