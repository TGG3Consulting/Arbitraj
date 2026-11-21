#!/usr/bin/env python3
# Файл: tools/arbitrum_record/pancake_stable.py
# Что делает: решение 471 (T448-10, T433-064) — запись фактов своего узла Arbitrum для PancakeSwap StableSwap: только
#   чтение (eth_call, eth_getCode, eth_getLogs, eth_getBlockByNumber), без ключей и подписей. Пишет JSON-файлы в
#   tests/data/arbitrum_pancake_stable_<дата>/ и README.md (блок, хеши). По ним — tests/arbitrum_pancake_stable_live.rs
#   (сейчас #[ignore] — записи нет).
# Кто вызывает: Тигран (record_all.py, шаг pancake_stable) или вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459). Узел — --rpc (HTTP JSON-RPC).
# Настройки: нет. Адреса по умолчанию — официальные (решение 471):
#   фабрика PancakeStableSwapFactory 0x5D5fBB19572c4A89846198c3DBEdB2B6eF58a77a — pancakeswap/infinity-universal-router
#   1b6d479 script/deployParameters/mainnet/DeployArbitrum.s.sol L30 (stableFactory; коммит 8b18bad, PR #46),
#   test/stableSwap/StableSwapArb.t.sol L27; пулы — API конфигов PancakeSwap (STABLE_SWAP_API @pancakeswap/
#   stable-swap-sdk 2.0.16 src/config/endpoint.ts) `configs.pancakeswap.com/api/data/cached/stableswaps?chainId=42161`.
# Порядок (решение 460: узел хранит ~128 блоков состояния): (1) короткое чтение реестра фабрики (pairLength,
#   swapPairContract(i)) на голове; (2) логи узким окном [B0 − --log-blocks, B0] по адресам пулов и фабрики; (3) всё
#   чтение состояния — на свежем блоке B, взятом после логов; ответ узла «состояние недоступно» — фаза (3) заново на
#   новом блоке (до 3 раз). Потолок времени — --deadline (проверка между запросами).
#
# Что записывается (на блоке B, номер — в meta.json); у каждого вызова — to, data (calldata), ok (hex) или error:
#   meta.json     — eth_chainId, B, время, фабрика, пулы, аргументы;
#   factory.json  — pairLength(), swapPairContract(i) для i < min(pairLength, --max-pairs);
#   pools.json    — по пулу: вызовы бота (load_stable_u.rs answers: coins(k)/balances(k) k < 8, fee(), A(),
#                   is_killed()), ещё admin_fee(), initial_A/future_A/initial_A_time/future_A_time(), token(),
#                   decimals() монет, реестр фабрики getPairInfo(c0, c1), getPairInfo(c1, c0), getThreePoolPairInfo(c0,
#                   c1) (verify_stable_u.rs / factory_ask.rs), eth_getCode (длина и последние 64 байта — метаданные
#                   компилятора), get_dy(i, j, dx) на долях баланса монеты i (--bps, в 1/10000);
#   logs.json     — eth_getLogs пулов и фабрики за [B0 − --log-blocks, B0].
# Селекторы (keccak256 сигнатур, посчитаны curve_crypto_rpc.keccak256; тест сверяет calldata с alloy): coins(uint256)
#   c6610657, balances(uint256) 4903b0d1, fee() ddca3f43, A() f446c1d0, is_killed() 9c868ac0, admin_fee() fee3f7f9,
#   initial_A() 5409491a, future_A() b4b577ad, initial_A_time() 2081066c, future_A_time() 14052288, token() fc0c546a,
#   decimals() 313ce567, get_dy(uint256,uint256,uint256) 556d6e9f, pairLength() fcc9136c, swapPairContract(uint256)
#   636e66a0, getPairInfo(address,address) 400f7a1e, getThreePoolPairInfo(address,address) 923093cb.
#
# Запуск: python3 tools/arbitrum_record/pancake_stable.py --rpc http://127.0.0.1:8547 (из корня репозитория)

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests

FACTORY = "0x5D5fBB19572c4A89846198c3DBEdB2B6eF58a77a"
POOLS = [
    "0xE80bD6fE6CB209E5546153AF19F151a3e651FA57",  # mBTC-WBTC (API конфигов, id 18)
    "0xd0f0be815a76eFE677c92b07b39a5e972BAf22bD",  # DLP-mDLP (id 17)
    "0x73ed25e04Aa673ddf7411441098fC5ae19976CE0",  # PENDLE-mPENDLE (id 16)
]
MAX_COINS = 8  # pools/abi_curve.rs MAX_COINS — столько coins(k)/balances(k) читает бот
STALE = ("historical state", "is not available", "missing trie node")


class Stale(Exception):
    """Узел не отдал состояние блока (удалено) — фаза заново на новом блоке."""


class Rpc:
    """JSON-RPC узла с потолком времени. Ошибка узла — RuntimeError (текст ответа); состояние удалено — Stale."""

    def __init__(self, url, deadline):
        self.url, self.end, self.n = url, time.time() + deadline, 0

    def call(self, method, params):
        if time.time() > self.end:
            raise RuntimeError("потолок времени --deadline вышел (%s)" % method)
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=120)
        r.raise_for_status()
        body = r.json()
        if "error" in body:
            text = json.dumps(body["error"])
            if any(s in text for s in STALE):
                raise Stale(text)
            raise RuntimeError(text)
        return body["result"]


def w(v):
    return format(v, "064x")


def aw(a):
    return w(int(a, 16))


def ec(rpc, to, data, tag):
    """eth_call → {to, data, ok} или {to, data, error}; Stale — наружу."""
    rec = {"to": to, "data": "0x" + data}
    try:
        rec["ok"] = rpc.call("eth_call", [{"to": to, "input": "0x" + data}, tag])
    except Stale:
        raise
    except Exception as e:  # noqa: BLE001 — откат контракта пишется как факт
        rec["error"] = str(e)
    return rec


def num(rec):
    return int(rec["ok"], 16) if "ok" in rec and len(rec["ok"]) > 2 else None


def registry(rpc, factory, tag, cap):
    """pairLength() и swapPairContract(i) для i < min(pairLength, cap)."""
    n = ec(rpc, factory, "fcc9136c", tag)
    out = {"pairLength": n, "swapPairContract": []}
    for i in range(min(num(n) or 0, cap)):
        out["swapPairContract"].append(ec(rpc, factory, "636e66a0" + w(i), tag))
    return out


def pool_state(rpc, factory, pool, bps, tag):
    rec = {"pool": pool, "bot_calls": []}
    for k in range(MAX_COINS):
        rec["bot_calls"].append(ec(rpc, pool, "c6610657" + w(k), tag))
        rec["bot_calls"].append(ec(rpc, pool, "4903b0d1" + w(k), tag))
    for sel in ("ddca3f43", "f446c1d0", "9c868ac0"):  # fee(), A(), is_killed() — порядок answers() бота
        rec["bot_calls"].append(ec(rpc, pool, sel, tag))
    for name, sel in [("admin_fee", "fee3f7f9"), ("initial_A", "5409491a"), ("future_A", "b4b577ad"),
                      ("initial_A_time", "2081066c"), ("future_A_time", "14052288"), ("token", "fc0c546a")]:
        rec[name] = ec(rpc, pool, sel, tag)
    coins, bals = [], []
    for k in range(MAX_COINS):
        c, b = rec["bot_calls"][2 * k], rec["bot_calls"][2 * k + 1]
        if "ok" not in c:
            break
        coins.append("0x" + c["ok"][-40:])
        bals.append(num(b) or 0)
    rec["decimals"] = [ec(rpc, c, "313ce567", tag) for c in coins]
    if len(coins) >= 2:
        rec["getPairInfo"] = [ec(rpc, factory, "400f7a1e" + aw(coins[0]) + aw(coins[1]), tag),
                              ec(rpc, factory, "400f7a1e" + aw(coins[1]) + aw(coins[0]), tag)]
        rec["getThreePoolPairInfo"] = ec(rpc, factory, "923093cb" + aw(coins[0]) + aw(coins[1]), tag)
    code = rpc.call("eth_getCode", [pool, tag])
    rec["code"] = {"bytes": (len(code) - 2) // 2, "tail64": "0x" + code[-128:],
                   "sha256": hashlib.sha256(code.encode()).hexdigest()}
    quotes = []
    for i in range(len(coins)):
        for j in range(len(coins)):
            if i == j:
                continue
            for b in bps:
                dx = bals[i] * b // 10000
                if dx == 0:
                    continue
                q = ec(rpc, pool, "556d6e9f" + w(i) + w(j) + w(dx), tag)
                q.update({"i": i, "j": j, "dx": str(dx), "bps": b})
                quotes.append(q)
    rec["get_dy"] = quotes
    return rec


def state_phase(rpc, factory, pools, bps, cap):
    """Чтение состояния на свежем блоке; Stale — заново на новом блоке (до 3 раз)."""
    for k in range(3):
        head = rpc.call("eth_getBlockByNumber", ["latest", False])
        tag = head["number"]
        try:
            reg = registry(rpc, factory, tag, cap)
            return head, reg, [pool_state(rpc, factory, p, bps, tag) for p in pools]
        except Stale as e:
            print("состояние блока %s недоступно (%s) — заново" % (tag, e), file=sys.stderr)
            if k == 2:
                raise


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: PancakeSwap StableSwap (решение 471)")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--factory", default=FACTORY, help="PancakeStableSwapFactory (по умолчанию официальная)")
    ap.add_argument("--pool", action="append", default=[], help="ещё пул (0x…); пулы реестра фабрики — всегда")
    ap.add_argument("--bps", default="1,10,100,1000,5000", help="доли баланса монеты входа для get_dy, 1/10000")
    ap.add_argument("--max-pairs", type=int, default=200, help="предел swapPairContract(i)")
    ap.add_argument("--log-blocks", type=int, default=20000, help="окно логов до головы, блоков")
    ap.add_argument("--deadline", type=int, default=600, help="потолок времени скрипта, с")
    ap.add_argument("--out", default=None, help="папка (по умолчанию tests/data/arbitrum_pancake_stable_<дата>)")
    args = ap.parse_args()
    bps = [int(x) for x in args.bps.split(",")]

    rpc = Rpc(args.rpc, args.deadline)
    chain = rpc.call("eth_chainId", [])
    # (1) короткое чтение реестра на голове — адреса пулов для окна логов.
    head0 = rpc.call("eth_getBlockByNumber", ["latest", False])
    b0 = int(head0["number"], 16)
    reg0 = registry(rpc, args.factory, head0["number"], args.max_pairs)
    found = ["0x" + r["ok"][-40:] for r in reg0["swapPairContract"] if "ok" in r]
    pools = []
    for p in POOLS + args.pool + found:
        if p.lower() not in [q.lower() for q in pools]:
            pools.append(p)
    # (2) логи узким окном (до чтения состояния).
    frm = hex(max(0, b0 - args.log_blocks))
    try:
        logs = {"from": frm, "to": head0["number"], "logs": rpc.call(
            "eth_getLogs", [{"address": pools + [args.factory], "fromBlock": frm, "toBlock": head0["number"]}])}
    except Exception as e:  # noqa: BLE001 — логи необязательны тесту математики; причина — в файле
        logs = {"from": frm, "to": head0["number"], "error": str(e)}
    # (3) состояние на свежем блоке.
    head, reg, recs = state_phase(rpc, args.factory, pools, bps, args.max_pairs)
    b = int(head["number"], 16)

    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_pancake_stable_" + day)
    os.makedirs(out, exist_ok=True)
    files = {
        "meta.json": {"chainId": chain, "block": b, "block_hex": head["number"], "timestamp": int(head["timestamp"], 16),
                      "factory": args.factory, "pools": pools, "api_pools": POOLS, "bps": bps,
                      "log_blocks": args.log_blocks},
        "factory.json": reg,
        "pools.json": recs,
        "logs.json": logs,
    }
    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# PancakeSwap StableSwap (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/pancake_stable.py (решение 471): только чтение, без ключей и подписей.\n")
        f.write("Блок B = %d (%s), время %d, chainId %s. Фабрика %s. Пулы: %s. Логи — [%s, %s].\n\n" % (
            b, head["number"], files["meta.json"]["timestamp"], chain, args.factory, ", ".join(pools), frm,
            head0["number"]))
        f.write("Файлы — шапка скрипта. Тест — tests/arbitrum_pancake_stable_live.rs.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, "; пулов", len(recs), "; запросов", rpc.n, file=sys.stderr)


if __name__ == "__main__":
    main()
