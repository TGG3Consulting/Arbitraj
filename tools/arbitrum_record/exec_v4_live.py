#!/usr/bin/env python3
"""Файл: tools/arbitrum_record/exec_v4_live.py
Что делает: решение 459 (T457-02) — запись приёмки исполнителя v4 на живых пулах Arbitrum по правилу решения 400 (как
  tests/data/executor_v3_live_2026-11-17 для v3 на BSC): код contracts/out/ArbExecutor.bin-runtime (сборка v4, решение
  457) подменой состояния ставится на пустой адрес X (слот 0 owner = W, слот 1 wnative = обёртка — раскладка
  contracts/out/ArbExecutor.layout.json), W получает вход подменой баланса, X — разрешение; вызовы execute от W —
  eth_simulateV1 поверх блока H (validation false). Транзакции не отправляются, ключей и подписей нет.
  Виды: 19 Uniswap V4 и 23 Balancer V3 (их судит правило — v4live_mgr.py), 22 Fluid, 24 LB v2.0, 1 Uniswap V3, 0
  Uniswap V2 (свидетельство — v4live_old.py). Вызовы и формат строк — v4live_cases.py.
  Пишет tests/data/executor_v4_live_<дата H>/: README.md (строка «sha256 симулированного bin-runtime: <sha>» — её ищет
  live_rule.rs bound), summary.json, hooks_reject.json, calls.json, slots.json, pools.json, raw_rpc.json.
Кто вызывает: Тигран — через tools/arbitrum_record/record_all.py (или сам: из корня репозитория
  `python3 tools/arbitrum_record/exec_v4_live.py --rpc http://УЗЕЛ:8547`).
От чего зависит: Python 3 (стандартная библиотека), v4live_*.py, curve_crypto_rpc.py, stdhttp.py; узел Arbitrum с
  eth_simulateV1. Адреса по умолчанию — официальные (источник у каждого аргумента).
Ошибки: код ≠ записанному SHA-256, не сеть 42161, узел без eth_simulateV1 — выход с кодом 2 и причиной.
"""

import argparse
import datetime
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import recnode  # noqa: E402 — решение 460: свежий блок состояния
import v4live_cases as cases  # noqa: E402
import v4live_mgr as mgr  # noqa: E402
import v4live_old as old  # noqa: E402
import v4live_slots as slots  # noqa: E402
from curve_crypto_rpc import keccak256  # noqa: E402
from v4live_rpc import Node, w  # noqa: E402
from v4live_write import write  # noqa: E402
from stdhttp import StaleState  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))


def args():
    ap = argparse.ArgumentParser(description="Приёмка исполнителя v4 на живых пулах Arbitrum (решения 400, 459)")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--wallet", help="адрес кошелька W (не секрет; по умолчанию — пустой адрес, проверяется)")
    ap.add_argument("--runtime", default=os.path.join(ROOT, "contracts", "out", "ArbExecutor.bin-runtime"))
    ap.add_argument("--sha-file", default=os.path.join(ROOT, "contracts", "out", "ArbExecutor.sha256"))
    ap.add_argument("--data-root", default=os.path.join(ROOT, "tests", "data"))
    ap.add_argument("--out", default=None, help="папка (по умолчанию tests/data/executor_v4_live_<дата H>)")
    # dex.native_wrapped arb_dex — WETH (dex-settings-arbitrum.md §0; Uniswap docs v3-arbitrum-deployments).
    ap.add_argument("--wnative", default="0x82aF49447D8a07e3bd95BD0d56f35241523fBab1")
    ap.add_argument("--base", action="append", default=None, help="предпочитаемый вход (а); по умолчанию WETH")
    ap.add_argument("--pools-per-kind", type=int, default=3)
    ap.add_argument("--bv3-per-hook", type=int, default=2,
                    help="пулов Balancer V3 на каждый вид крючка, который бот берёт (static, surge, mev)")
    ap.add_argument("--max-impact-pct", type=float, default=3.0)
    ap.add_argument("--log-chunk", type=int, default=500000, help="блоков в одном eth_getLogs")
    # Uniswap V4 — Uniswap/docs v4/deployments.mdx L76-L80 (dex-settings-arbitrum.md §2.3).
    ap.add_argument("--v4-pool-manager", default="0x360e68faccca8ca495c1b759fd9eee466db9fb32")
    ap.add_argument("--v4-quoter", default="0x8928074CA1b241D8Ec02815881c1Af11E8bC5219")
    ap.add_argument("--v4-state-view", default="0x76fd297e2d437cd7f76d50f01afe6160f86e9990")
    ap.add_argument("--v4-blocks", type=int, default=20000000, help="окно поиска Initialize до H, блоков")
    ap.add_argument("--v4-scan", type=int, default=300, help="сколько пулов из событий проверить")
    # Balancer V3 — balancer-deployments@b55dc2b8 output/arbitrum.json (docs/FIRST_RUN.md, balancer_v3.py).
    ap.add_argument("--bv3-vault", default="0xbA1333333333a1BA1108E8412f11850A5C319bA9")
    ap.add_argument("--bv3-router", default="0xEAedc32a51c510d35ebC11088fD5fF2b47aACF2E")
    ap.add_argument("--bv3-pool", action="append", default=[], help="пул Balancer V3 (кроме записи balancer_v3.py)")
    # T477-01 (решение 478): правило приёмки требует MIN_POOLS = 2 доказанных пула на класс (live_rule.rs) — пулы
    # Balancer V3 ещё и перечислением getPools() фабрик; сколько пулов, которые бот берёт, проверить (чтение на H).
    ap.add_argument("--bv3-max-pools", type=int, default=40,
                    help="наибольшее число пулов Balancer V3, которые бот берёт, для подбора суммы")
    # Fluid — Instadapp/fluid-contracts-public@404d73f deployments/arbitrum (fluid_t1.py).
    ap.add_argument("--fluid-resolver", default="0x05Bd8269A20C472b148246De20E6852091BF16Ff")
    # LB v2.0 — joe-docs 61c16665 deployment-addresses/arbitrum.md L52-L58 (lb_v20.py).
    ap.add_argument("--lb20-factory", default="0x1886D09C9Ade0c5DB822D85D21678Db67B6c2982")
    ap.add_argument("--lb20-router", default="0x7BFd7192E76D950832c77BB412aaE841049D8D9B")
    ap.add_argument("--lb20-scan", type=int, default=30)
    # Uniswap V3 / V2 — dex-settings-arbitrum.md §2.1, §2.2.
    ap.add_argument("--v3-factory", default="0x1F98431c8aD98523631AE4a59f267346ea31F984")
    ap.add_argument("--v3-quoter", default="0x61fFE014bA17989E743c5F6cB21bF9697530B21e")
    ap.add_argument("--v3-fee", type=int, action="append", default=None, help="уровни (по умолчанию 500, 3000, 10000)")
    ap.add_argument("--v2-factory", default="0xf1D7CC64Fb4452F05c498126312eBE29f30Fbcf9")
    ap.add_argument("--v2-fee-pips", type=int, default=3000)
    a = ap.parse_args()
    a.base = [x.lower() for x in (a.base or [a.wnative])]
    a.v3_fee = a.v3_fee or [500, 3000, 10000]
    return a


def fail(msg):
    print("ОШИБКА:", msg, file=sys.stderr)
    sys.exit(2)


def empty(node, tag, addr):
    return node.ok("eth_getCode", [addr, tag]) == "0x" and int(node.ok("eth_getBalance", [addr, tag]), 16) == 0


def pick(node, tag, a, ctx, cands, rec, limit=None):
    """Выбор пулов вида ДО симуляции: слоты токенов найдены, сумма подобрана квотером. Ошибок-исключений нет."""
    chosen = []
    for p in cands:
        if len(chosen) >= (limit or a.pools_per_kind):
            break
        tin = next((t for t in p["tokens"] if t in a.base), p["tokens"][0])
        tout = next(t for t in p["tokens"] if t != tin)
        bad = None
        for t in (tin, tout):
            if t not in ctx["slots"]:
                s, why = slots.find(node, tag, t, ctx["w"], ctx["x"])
                ctx["slots"][t] = s or {"error": why}
            if "error" in ctx["slots"][t]:
                bad = f"слоты {t}: {ctx['slots'][t]['error']}"
        amt = qa = None
        if not bad:
            amt, qa, probes = cases.size(node, tag, p, a, ctx["w"], tin, tout)
            if amt is None:
                bad = f"сумма не подобрана квотером: {probes}"
        if bad:
            rec["skipped"].append({"kind": p["kind"], "pool_id": p["pool_id"], "why": bad})
            continue
        chosen.append((p, tin, tout, amt, qa))
    return chosen


def state_phase(node, a, code, v4_found, skipped, head):
    """Фаза состояния на блоке H = head (решение 460; ревью t246, п. 2): X и W, подбор пулов (до --bv3-max-pools
    Balancer V3 и квотеры подбора суммы), симуляции (а) / (б) и квотеры (а) в своей симуляции (решение 478) — всё на
    одном свежем H. Узел хранит только ~128 блоков: ответ «historical state … is not available» — stdhttp.StaleState —
    ловит recnode.fresh и повторяет фазу на новом свежем блоке; запросы неудачной попытки убираются из журнала узла
    (raw_rpc.json — только попытка записи). Возвращает: (head, время H, X, W, rec, ctx). Ошибки: fail — выход с
    причиной; StaleState — наружу (recnode.fresh).
    """
    start = len(node.log)
    try:
        return _state_phase(node, a, code, v4_found, skipped, head)
    except StaleState:
        del node.log[start:]
        raise


def _state_phase(node, a, code, v4_found, skipped, head):
    tag = hex(head)
    ht = int(node.ok("eth_getBlockByNumber", [tag, False])["timestamp"], 16)
    x = "0x" + keccak256(b"arb.exec_v4_live.executor").hex()[-40:]
    wl = (a.wallet or "0x" + keccak256(b"arb.exec_v4_live.wallet").hex()[-40:]).lower()
    if not empty(node, tag, x):
        fail(f"адрес X {x} не пуст на H")
    x_ov = {x: {"code": code, "stateDiff": {"0x" + w(0): "0x" + w(int(wl, 16)), "0x" + w(1): "0x" + w(int(a.wnative, 16))}}}
    probe, err = node.simulate(tag, x_ov, [{"from": wl, "to": x, "input": "0x8da5cb5b"}])  # owner()
    if probe is None:
        fail(f"eth_simulateV1 не работает на узле: {err} — без него нет логов вызова (кредит Swap) и (б) после (а)")
    ctx = {"w": wl, "x": x, "deadline": ht + 600, "slots": {}}
    rec = {"skipped": list(skipped), "summary": [], "calls": [], "hooks_reject": [], "pools": {}, "sim_errors": []}
    found = {19: mgr.v4_pools(node, tag, v4_found, a, rec["skipped"]), 23: mgr.bv3_pools(node, tag, a, rec["skipped"]),
             22: old.fluid_pools(node, tag, a, rec["skipped"]), 24: old.lb20_pools(node, tag, a, rec["skipped"])}
    chosen = {k: pick(node, tag, a, ctx, v, rec) for k, v in found.items() if k != 23}
    # Координатор (T457-02): класс 23 у бота покрывает и пулы с крючком — по --bv3-per-hook на вид крючка, который
    # бот берёт (v4live_mgr.bv3_hook_kind); вида нет на Arbitrum — строка «нет пулов» с bv3_hook_kind.
    chosen[23], rec["bv3_hook_missing"] = [], []
    for hk in mgr.BV3_HOOK_KINDS:
        got = pick(node, tag, a, ctx, [p for p in found[23] if p["bv3_hook_kind"] == hk], rec,
                   None if hk == "none" else a.bv3_per_hook)
        chosen[23] += got
        if not got and hk != "none":
            rec["bv3_hook_missing"].append(hk)
            rec["summary"].append({"case": "нет пулов", "kind": 23, "bv3_hook_kind": hk,
                                   "note": "пула Balancer V3 с этим видом крючка, который бот берёт, нет"})
    pairs = []
    for lst in chosen.values():
        for p, tin, tout, _, _ in lst:
            if (tin, tout) not in pairs:
                pairs.append((tin, tout))
    classic = old.classic_pools(node, tag, a, pairs, rec["skipped"])
    for k in (1, 0):
        chosen[k] = pick(node, tag, a, ctx, [p for p in classic if p["kind"] == k], rec)
    tokens = {t: s for t, s in ctx["slots"].items() if "error" not in s}
    fund = {}
    for lst in chosen.values():
        for p, tin, tout, amt, _ in lst:
            fund[tin] = max(fund.get(tin, 0), amt)
    ov, why = slots.overrides(node, tag, tokens, wl, x, fund)
    if ov is None:
        fail(f"подмена состояния: {why}")
    ov.update(x_ov)
    ctx["overrides"] = ov
    for k, lst in chosen.items():
        rec["pools"][k] = [dict(p, token_in=tin, token_out=tout, amount_in=str(amt)) for p, tin, tout, amt, _ in lst]
        if not lst and k in (19, 23):
            rec["summary"].append({"case": "нет пулов", "kind": k, "note": "ни один пул не прошёл подбор (skipped)"})
        for p, tin, tout, amt, qa in lst:
            rows, calls, err = cases.case_ab(node, tag, p, a, ctx, tin, tout, amt, qa)
            rec["summary"] += rows
            rec["calls"] += calls
            if err:
                rec["sim_errors"].append({"kind": k, "pool_id": p["pool_id"], "error": str(err)[:400]})
            print(k, p["pool_id"][:12], [(r["case"], r["status"], r["got"], r.get("quoter_out")) for r in rows],
                  file=sys.stderr, flush=True)
    return head, ht, x, wl, rec, ctx


def main():
    a = args()
    node = Node(a.rpc)
    if int(node.ok("eth_chainId", []), 16) != 42161:
        fail("узел не Arbitrum One (chainId ≠ 42161)")
    # Ревью t233, п. 2: SHA-256 — по тексту с переводами строк LF (как файл в репозитории; checkout с
    # core.autocrlf=true на Windows дал бы CRLF — .gitattributes это запрещает, здесь — вторая защита).
    raw = open(a.runtime, "rb").read().replace(b"\r\n", b"\n")
    sha = hashlib.sha256(raw).hexdigest()
    want = [ln.split()[0] for ln in open(a.sha_file, encoding="utf-8") if ln.strip().endswith("ArbExecutor.bin-runtime")]
    if want != [sha]:
        fail(f"SHA-256 {a.runtime} = {sha}, в {a.sha_file}: {want} — пересоберите contracts/build.sh")
    code = "".join(raw.decode("ascii").split())  # hex без пробелов и переводов строк — в подмену state override
    code = code if code.startswith("0x") else "0x" + code
    # Решение 460: события Initialize Uniswap V4 (окно --v4-blocks, долгий поиск логов) — ДО выбора блока H: узел
    # хранит только недавнее состояние; всё чтение состояния и симуляции — на свежем H после поиска.
    skipped = []
    v4_found = mgr.v4_inits(node, int(node.ok("eth_blockNumber", []), 16), a, skipped)
    head, ht, x, wl, rec, ctx = recnode.fresh(lambda: int(node.ok("eth_blockNumber", []), 16),
                                              lambda h: state_phase(node, a, code, v4_found, skipped, h))
    date = datetime.datetime.fromtimestamp(ht, datetime.timezone.utc).strftime("%Y-%m-%d")
    out = a.out or os.path.join(a.data_root, f"executor_v4_live_{date}")
    for r in rec["summary"]:
        r["chain_id"] = 42161  # пара (версия кода, сеть) записи — план правки live_proof.rs, TASKS T457-02
    write(out, dict(chain_id=42161, a=a, sha=sha, head=head, ht=ht, x=x, w=wl, rec=rec, slots=ctx["slots"], log=node.log))
    print("записано:", out, file=sys.stderr)


if __name__ == "__main__":
    main()
