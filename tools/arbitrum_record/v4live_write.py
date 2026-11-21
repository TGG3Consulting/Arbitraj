"""Файл: tools/arbitrum_record/v4live_write.py
Что делает: решение 459 (T457-02) — запись папки приёмки исполнителя v4 (tests/data/executor_v4_live_<дата>/) в формате
  записи решения 400 (tests/data/executor_v3_live_2026-11-17; бот читает README.md, summary.json, hooks_reject.json —
  src/venues/bsc_dex/exec/executor/live_proof.rs, live_rule.rs, live_row.rs). README несёт строку
  «- sha256 симулированного bin-runtime: <sha> …» — первое слово после двоеточия = SHA-256 кода (live_rule.rs bound).
  Итоги в README — счёт по строкам (факты записи); вердикт по классам выносит бот правилом live_rule.rs.
Кто вызывает: tools/arbitrum_record/exec_v4_live.py.
От чего зависит: стандартная библиотека.
Ошибки: OSError записи — как есть.
"""

import hashlib
import json
import os

NAMES = {19: "Uniswap V4", 23: "Balancer V3", 22: "Fluid DEX T1", 24: "LB v2.0", 1: "Uniswap V3", 0: "Uniswap V2"}


def _dump(out, name, obj):
    with open(os.path.join(out, name), "w", encoding="utf-8") as fh:
        json.dump(obj, fh, ensure_ascii=False, indent=1)


def _counts(rows, kind):
    r = [x for x in rows if x.get("kind") == kind and x["case"] in ("a", "b")]
    ok = [x for x in r if x["status"] == 1]
    same = [x for x in ok if x.get("got") and x.get("got") == (x.get("quoter_out_after_a") if x["case"] == "b"
                                                                   else x.get("quoter_out"))]
    cred = [x for x in ok if x.get("got_eq_swap_credit") is True]
    zero = [x for x in r if isinstance(x.get("x_left"), dict) and set(x["x_left"].values()) == {"0"}]
    pools = {x["pool_id"] for x in r}
    noxl = [x for x in r if x.get("x_left_error")]
    return (f"пулов {len(pools)}; вызовов (а)/(б) {len(r)}, прошли {len(ok)}, получено = квотер в том же состоянии "
            f"{len(same)}, получено = кредит Swap {len(cred)}, на исполнителе 0 — {len(zero)}, остаток не записан "
            f"(balanceOf(X) не ответил, x_left_error) — {len(noxl)}")


def write(out, d):
    """Пишет файлы и README.md с SHA-256 файлов. Принимает: папку и данные прогона. Ошибки: OSError."""
    os.makedirs(out, exist_ok=True)
    a, rec = d["a"], d["rec"]
    _dump(out, "summary.json", rec["summary"])
    _dump(out, "hooks_reject.json", rec["hooks_reject"])
    _dump(out, "calls.json", rec["calls"])
    _dump(out, "slots.json", d["slots"])
    _dump(out, "pools.json", {"chosen": {str(k): v for k, v in rec["pools"].items()}, "skipped": rec["skipped"],
                              "sim_errors": rec["sim_errors"]})
    _dump(out, "raw_rpc.json", d["log"])
    lines = [
        f"# {os.path.basename(out)} — живая проверка ArbExecutor {d.get('label', 'v4')} на Arbitrum One (eth_simulateV1, свой узел)", "",
        "Только чтение: eth_call / eth_getLogs / eth_getCode / eth_getBalance / eth_simulateV1. Ничего не подписано и "
        f"не отправлено, ключи не использовались. Скрипт — tools/arbitrum_record/{d.get('script', 'exec_v4_live.py')} (решения 400, 459).", "",
        "## Параметры",
        f"- Код: contracts/out/ArbExecutor.bin-runtime ({d.get('build', 'сборка v4, решение 457')})",
        f"- chainId: {d['chain_id']} (Arbitrum One; запись судит кошельки только этой сети — TASKS T457-02)",
        f"- sha256 симулированного bin-runtime: {d['sha']} (совпал со строкой ArbExecutor.bin-runtime в "
        "ArbExecutor.sha256)",
        f"- H (блок симуляции и всех eth_call) = {d['head']}, время блока H = {d['ht']}, deadline = H.time + 600 = "
        f"{d['ht'] + 600}; validation: false",
        f"- X (пустой адрес, на H: код 0x, баланс 0) = {d['x']}",
        f"- W (кошелёк, --wallet или пустой адрес) = {d['w']}",
        f"- Подмена — X: code + слот 0 = W, слот 1 = wnative {a.wnative}; токены: баланс W = текущий + сумма входа, "
        "allowance(W, X) = 2^256−1 (слоты — slots.json)",
        f"- Выбор пулов — до симуляции, по квотеру на H (сумма — наибольшая из 10^(d+3)…10^(d−6) с выходом ≥ "
        f"(1 − {a.max_impact_pct}/100)·10·выход(сумма/10)); не больше {a.pools_per_kind} пулов на вид; пропуски — "
        "pools.json skipped",
        "- x_left: ключ «usdt» — токен входа (а) (имя формата записи 17.11, live_row.rs), «coin» — второй токен",
        "- Квотер (а) (quoter_out строки «a») — своя eth_simulateV1 [квотер] поверх H: тот же симулированный блок H + 1, "
        "что у покупки (sim_block_a = sim_block_quoter_a); квотер на H — quoter_out_h (не сравнивается; T477-01, "
        "решение 478). Квотер (б) — quoter_out_after_a (та же симуляция после (а); у Balancer V3 — своя [buy, квотер])",
        "", "## Адреса",
        f"- Uniswap V4: PoolManager {a.v4_pool_manager}, V4Quoter {a.v4_quoter}, StateView {a.v4_state_view} "
        f"(Initialize за {a.v4_blocks} блоков до начала поиска; блок H взят после поиска — решение 460; без крючка, "
        "обе валюты ERC-20)",
        f"- Balancer V3: Vault {a.bv3_vault}, Router {a.bv3_router} (пулы — запись balancer_v3.py + --bv3-pool + getPools() фабрик "
        f"списков, не больше {a.bv3_max_pools} пулов, которые бот берёт, — поле src в pools.json; "
        "hooks строки = 0 — у шага 23 класс только «без крючка», крючок пула — bv3_hook, вид — bv3_hook_kind: none / "
        f"static / surge / mev, как pools/balancer_v3_hooks.rs; без крючка до {a.pools_per_kind}, с крючком до "
        f"{a.bv3_per_hook} на вид; нет пула вида: {', '.join(rec.get('bv3_hook_missing', [])) or '—'})",
        f"- Fluid: резолвер {a.fluid_resolver}; LB v2.0: фабрика {a.lb20_factory}, роутер {a.lb20_router}",
        f"- Uniswap V3: фабрика {a.v3_factory}, QuoterV2 {a.v3_quoter}, уровни {a.v3_fee}; Uniswap V2: фабрика "
        f"{a.v2_factory}, комиссия {a.v2_fee_pips} pips",
        "", "## Счёт по видам (вердикт по классам — правило бота live_rule.rs; виды 0, 1, 22, 24 правило не судит)"]
    for k in (19, 23, 22, 24, 1, 0):
        empty = not any(x.get("kind") == k and x["case"] in ("a", "b") for x in rec["summary"])
        lines.append(f"- Вид {k}, {NAMES[k]}: " + ("НЕТ ПУЛОВ (НЕПРОВЕРЕН)" if empty else _counts(rec["summary"], k)))
        if k == 23:
            for hk in ("none", "static", "surge", "mev"):
                rows = [x for x in rec["summary"] if x.get("bv3_hook_kind") == hk]
                lines.append(f"  - крючок {hk}: " + ("нет пула" if hk in rec.get("bv3_hook_missing", []) or not rows
                                                     else _counts(rows, 23)))
    if rec["sim_errors"]:
        lines.append(f"- Ошибки eth_simulateV1: {len(rec['sim_errors'])} (pools.json sim_errors)")
    lines += ["", "## Файлы",
              "- summary.json — по вызову: case, kind, pool_id, manager, pool_manager, hooks, token_in, token_out, "
              "amount_in, got, quoter_out ((а) — в той же симуляции), quoter_out_h ((а) — на H), sim_block_a, "
              "sim_block_quoter_a, quoter_out_after_a (у (б)), got_eq_swap_credit, x_left, status, revert, gas",
              "- calls.json — по вызову: status, returnData, gasUsed, логи целиком, откат, x_left",
              "- hooks_reject.json — откаты крючков V4 (пулы V4 с крючком не выбираются — пусто; откат пула Balancer V3 "
              "с крючком — необъяснённое: класс 23 у бота один)",
              "- slots.json, pools.json (выбранные, пропущенные, ошибки симуляции), raw_rpc.json — все запросы",
              "", "## SHA-256 файлов"]
    for name in sorted(os.listdir(out)):
        if name != "README.md":
            with open(os.path.join(out, name), "rb") as fh:
                lines.append(f"{hashlib.sha256(fh.read()).hexdigest()}  {name}")
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
