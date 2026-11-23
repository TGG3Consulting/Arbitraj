#!/usr/bin/env python3
"""Снимает запись приёмки ArbExecutor V5 на живых пулах BSC (решения 400, 461, 478).

Из корня репозитория: python3 tools/bsc_record/exec_v5_bsc.py --rpc http://127.0.0.1:8545.
Фиксированные кандидаты из записи BSC от 17.11 служат только для обнаружения. Слоты, объём,
квоты, execute, события Swap и остатки получаются заново на одном свежем блоке. Только чтение
через RPC, без ключей. Вердикт по файлам записи выносит Rust live_rule в GitHub Actions.
"""
import argparse
import datetime
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/arbitrum_record"))
from curve_crypto_rpc import keccak256
from exec_v5_live import empty, pick
from recnode import fresh
from stdhttp import StaleState
import v4live_cases as cases
import v4live_slots as slots
from v4live_rpc import Node, ad, w

# Адреса и ключи — три записи Initialize (BSC PoolManager) из raw_rpc.json.zst приёмки V3
# от 17.11, раздел «Адреса из настроек» её README. Координаты событий есть в pools.json.
USDT = "0x55d398326f99059ff775485246999027b3197955"
WBNB = "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c"
MANAGER = "0x28e2ea090877bf75740558f6bfb36a5ffee9e9df"
QUOTER = "0x95c086d7fe28add842ee220fef225a777474d781"
ZERO = "0x" + "0" * 40
V4_KEYS = (
    ("0x92e5d26cf484a2dc2a48bf0f96e50e7b29cf54dfb520de664b7db88a959c1ddf",
     0x7F68AA7, "0x9bafc8d4b487cebff201721702507a3e2c67ad79", 989800, 9898),
    ("0xb12f0c5fe62a9a7eecce88bcf7926044ac0842e8935f43fcdab119c62b450e0d",
     0x7FC915B, "0xa67c48f86fc6d0176dca38883ca8153c76a532c7", 999901, 1),
    ("0xc89b1f76929a5e88de04d43706e9a6aec4dc9593243aa9d962e9e98250e92817",
     0x7FA5C85, "0x36c51c465f5f8f25dc201f0152357cca394d2510", 75, 1),
)


def v4_pools():
    pools = []
    for pid, block, coin, fee, spacing in V4_KEYS:
        c0, c1 = sorted((USDT, coin))
        key = ad(c0) + ad(c1) + w(fee) + w(spacing) + ad(ZERO)
        if "0x" + keccak256(bytes.fromhex(key)).hex() != pid:
            raise ValueError(f"V4 PoolKey не соответствует ID {pid}")
        pools.append(dict(kind=19, fam="v4", pool_id=pid, tokens=[c0, c1], hooks=ZERO, manager=MANAGER,
                          pool_manager=MANAGER, step_pool=MANAGER, data="0x" + key, initialize_block=block))
    return pools


def read_runtime():
    folder = ROOT / "contracts/out"
    raw = (folder / "ArbExecutor.bin-runtime").read_bytes().replace(b"\r\n", b"\n")
    sha = hashlib.sha256(raw).hexdigest()
    manifest = [line.split()[0] for line in (folder / "ArbExecutor.sha256").read_text().splitlines()
                if line.endswith(" ArbExecutor.bin-runtime")]
    if manifest != [sha] or sha != "36643fab78603cb0ac92620dae73729bceb07b4bb44f04be74a16a5ef57150ab":
        raise ValueError("Runtime не совпал с зафиксированной сборкой V5 или её манифестом")
    return sha, "0x" + raw.decode("ascii").strip().removeprefix("0x")


def state(node, a, code, height):
    tag = hex(height)
    header = node.ok("eth_getBlockByNumber", [tag, False])
    stamp = int(header["timestamp"], 16)
    wallet, executor = ["0x" + keccak256(("arb.v5.bsc." + label).encode()).hex()[-40:]
                        for label in ("wallet", "executor")]
    if not all(empty(node, tag, addr) for addr in (wallet, executor)):
        raise ValueError("Один из адресов симуляции не пуст")
    ctx = dict(w=wallet, x=executor, deadline=stamp + 600, slots={})
    rec = dict(summary=[], calls=[], skipped=[], pools=[])
    selected = []
    for kind in a.kinds:
        if kind == 19:
            candidates = v4_pools()
        else:
            import v5_bsc_cl
            candidates = v5_bsc_cl.pools(node, tag)
        selected.extend(pick(node, tag, a, ctx, candidates, rec))
    valid_slots = {t: s for t, s in ctx["slots"].items() if "error" not in s}
    fund = {}
    for _, tin, _, amount, _ in selected:
        fund[tin] = max(amount, fund.get(tin, 0))
    override, error = slots.overrides(node, tag, valid_slots, wallet, executor, fund)
    if error:
        raise RuntimeError(error)
    override[executor] = {"code": code, "stateDiff": {"0x" + w(0): "0x" + ad(wallet),
                                                      "0x" + w(1): "0x" + ad(WBNB)}}
    ctx["overrides"] = override
    for p, tin, tout, amt, qa in selected:
        rec["pools"].append(dict(p, token_in=tin, token_out=tout, amount_in=str(amt)))
        rows, calls, error = cases.case_ab(node, tag, p, a, ctx, tin, tout, amt, qa)
        if error:
            raise RuntimeError(f"Не завершена симуляция выбранного пула {p['pool_id']}: {error}")
        for row in rows:
            row["chain_id"] = 56
        rec["summary"].extend(rows)
        rec["calls"].extend(calls)
    if node.ok("eth_getBlockByNumber", [tag, False])["hash"] != header["hash"]:
        raise StaleState("сменился хеш блока H")
    return header, ctx, rec


def write_record(out, header, ctx, rec, log, sha):
    out.mkdir(parents=True, exist_ok=False)
    docs = {"summary": rec["summary"], "calls": rec["calls"], "slots": ctx["slots"], "hooks_reject": [],
            "pools": {"chosen": rec["pools"], "skipped": rec["skipped"]}, "raw_rpc": log}
    for name, value in docs.items():
        (out / f"{name}.json").write_text(json.dumps(value, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    (out / "README.md").write_text(
        "# Приёмка ArbExecutor V5 на BSC\n\n"
        f"- chainId: 56\n- sha256 симулированного bin-runtime: {sha}\n"
        f"- Блок H: {int(header['number'], 16)}, hash: {header['hash']}, timestamp: {int(header['timestamp'], 16)}\n"
        f"- X: {ctx['x']}, W: {ctx['w']}, WBNB: {WBNB}\n\n"
        "Коллектор: tools/bsc_record/exec_v5_bsc.py. Кандидаты до симуляций — из записи BSC от 17.11. "
        "Выбранные ключи, суммы и пропуски — в pools.json. Реальные ответы узла на блоке H — в raw_rpc.json. "
        "В eth_simulateV1 подменены только код и начальные слоты X, балансы и разрешения токенов. "
        "Квотер покупки вызывается в симуляции того же блока, квотер продажи — после покупки в симуляции продажи. "
        "Транзакции не подписывались и не отправлялись. Непроверенные классы не получают допуска.\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rpc", required=True)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--kind", choices=(19, 20), type=int, action="append")
    a = parser.parse_args()
    a.kinds = sorted(set(a.kind or (19, 20)))
    a.pools_per_kind, a.max_impact_pct, a.base, a.v4_quoter = 3, 3.0, [USDT], QUOTER
    sha, code = read_runtime()
    node = Node(a.rpc)
    if int(node.ok("eth_chainId", []), 16) != 56:
        raise ValueError("Требуется BSC chainId 56")
    def attempt(height):
        start = len(node.log)
        try:
            return state(node, a, code, height)
        except StaleState:
            del node.log[start:]
            raise
    header, ctx, rec = fresh(lambda: int(node.ok("eth_blockNumber", []), 16), attempt)
    date = datetime.datetime.fromtimestamp(int(header["timestamp"], 16), datetime.timezone.utc).date()
    out = a.out or ROOT / f"tests/data/executor_v5_bsc_live_{date}"
    write_record(out, header, ctx, rec, node.log, sha)
    print(f"{out}: {len(rec['pools'])} выбранных пулов, {len(rec['summary'])} строк")


if __name__ == "__main__":
    main()
