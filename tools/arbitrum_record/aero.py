#!/usr/bin/env python3
"""Файл: tools/arbitrum_record/aero.py
Что делает: решение 470 (T451-01/02) — запись ответов своего узла Arbitrum One для пулов Aero «Basic» (VolatilePool /
StablePool) и Aero «Slipstream V3» (CLPool с крючком DynamicSwapFeeHook). Всё чтение — на ОДНОМ свежем блоке сразу
(узел хранит только недавнее состояние): фабрики (пауза, defaultFee, модули комиссии и MEV-налога, их параметры),
последние --max-pools пулов каждой фабрики перечислением allPools(i) (скана истории нет), по каждому пулу Basic —
metadata, POOL_TYPE, getReserves, factory/token0/token1, getBaseFee(pool, 0, 0, 0, r0, r1), getPool(t0, t1) и
getAmountOut пула на доли резерва в обе стороны; по каждому пулу CL — slot0, liquidity, tickSpacing, fee(),
getPoolSwapHook, tickSpacingToFee, mevTaxModule крючка и его параметры. Пишет tests/data/arbitrum_aero_<дата>/
(record.json + README.md: блок, хеш блока, sha256 файла). Ключей и подписей нет — только eth_call.
Кто вызывает: Тигран вручную или record_all.py (record_steps.py).
От чего зависит: Python 3.8+, stdhttp.py (стандартная библиотека).
Источник адресов: Dromos-Labs/metadex-public 1.1.0 deployment-addresses/pools-arbitrum.json и
Dromos-Labs/metadex-slipstream-public 1.1.0 deployment-addresses/cl-arbitrum.json. Селекторы — keccak256 сигнатур
исходников (тест src/venues/bsc_dex/pools/abi_aero_basic.rs сверяет их же).
Запуск: python3 tools/arbitrum_record/aero.py --rpc http://УЗЕЛ:8547 [--max-pools 20] [--out tests/data]
"""

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

import stdhttp as requests

VOLATILE_FACTORY = "0x8E6a9FCd1d638CC760Ffba397605Fe3355dE76D2"
STABLE_FACTORY = "0x78589E876edcEbdce72ee3436ab3c9AD3a7cEF2D"
CL_FACTORY = "0x88a60ee77392577c355C0acb2A8d476D5a4EC03a"
CL_HOOK = "0xC0ed888AfE87ed1AcF01D9f19bB850159e3ce434"
SEL = {
    "metadata": "392f37e9", "POOL_TYPE": "648da859", "getReserves": "0902f1ac", "getAmountOut": "f140a35a",
    "token0": "0dfe1681", "token1": "d21220a7", "factory": "c45a0155", "getPool2": "531aa03e",
    "isPaused": "b187bd26", "getBaseFee": "df29d5bb", "mevTaxModule": "fa172435", "defaultFee": "5a6c72d0",
    "feeModule": "bae41cbf", "minThreshold": "c85501bb", "priorityFeeMultiplier": "c21c82bd",
    "baseFeeFactor": "d353b05d", "allPoolsLength": "efde4e64", "allPools": "41d1de97", "customFee": "4d419abc",
    "getPoolSwapHook": "d8f75299", "fee": "ddca3f43", "slot0": "3850c7bd", "tickSpacing": "d0c93a7c",
    "tickSpacingToFee": "380dc1c2", "liquidity": "1a686502", "discountRegistry": "b46c70b2",
}


class Rpc:
    """JSON-RPC узла: ошибка узла — исключение с текстом; откат eth_call — строка «revert: …» в записи."""

    def __init__(self, url, deadline):
        self.url, self.n, self.deadline = url, 0, deadline

    def raw(self, method, params):
        if time.time() > self.deadline:
            raise RuntimeError("потолок времени --max-seconds")
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=120)
        r.raise_for_status()
        return r.json()

    def call(self, to, data, tag):
        body = self.raw("eth_call", [{"to": to, "data": "0x" + data}, tag])
        if "error" in body:
            return {"error": body["error"]}
        return {"result": body["result"]}


def word(v):
    return format(v % (1 << 256), "064x")


def addr(a):
    return word(int(a, 16))


def as_int(res):
    return int(res["result"][2:66], 16) if "result" in res and len(res["result"]) >= 66 else None


def as_addr(res):
    v = as_int(res)
    return None if v is None else "0x" + format(v, "040x")


def last_pools(rpc, factory, tag, n):
    """Последние n пулов фабрики перечислением allPools(i). Ошибок нет: сбой — пустой список с записью."""
    length = as_int(rpc.call(factory, SEL["allPoolsLength"], tag)) or 0
    out = []
    for i in range(max(0, length - n), length):
        a = as_addr(rpc.call(factory, SEL["allPools"] + word(i), tag))
        if a:
            out.append(a)
    return length, out


def module_params(rpc, module, tag):
    if not module or int(module, 16) == 0:
        return None
    return {k: rpc.call(module, SEL[k], tag) for k in ("priorityFeeMultiplier", "minThreshold", "baseFeeFactor")}


def basic(rpc, factory, tag, n):
    rec = {k: rpc.call(factory, SEL[k], tag) for k in ("isPaused", "defaultFee", "feeModule", "mevTaxModule")}
    rec["mevModuleParams"] = module_params(rpc, as_addr(rec["mevTaxModule"]), tag)
    rec["allPoolsLength"], pools = last_pools(rpc, factory, tag, n)
    rec["pools"] = []
    for p in pools:
        x = {"pool": p}
        for k in ("metadata", "POOL_TYPE", "getReserves", "factory", "token0", "token1"):
            x[k] = rpc.call(p, SEL[k], tag)
        t0, t1 = as_addr(x["token0"]), as_addr(x["token1"])
        res = x["getReserves"].get("result", "0x")
        r0, r1 = (int(res[2:66], 16), int(res[66:130], 16)) if len(res) >= 130 else (0, 0)
        x["getBaseFee"] = rpc.call(factory, SEL["getBaseFee"] + addr(p) + word(0) * 3 + word(r0) + word(r1), tag)
        if t0 and t1:
            x["getPool"] = rpc.call(factory, SEL["getPool2"] + addr(t0) + addr(t1), tag)
        x["quotes"] = []
        for tok, r in ((t0, r0), (t1, r1)):
            for div in (10000, 1000, 100, 10):
                amt = r // div
                if tok and amt > 0:
                    q = rpc.call(p, SEL["getAmountOut"] + word(amt) + addr(tok), tag)
                    x["quotes"].append({"tokenIn": tok, "amountIn": str(amt), "out": q})
        rec["pools"].append(x)
    return rec


def cl(rpc, tag, n):
    rec = {"allPoolsLength": None, "hook": {}}
    hook_mev = as_addr(rpc.call(CL_HOOK, SEL["mevTaxModule"], tag))
    rec["hook"] = {"mevTaxModule": hook_mev, "mevModuleParams": module_params(rpc, hook_mev, tag),
                   "discountRegistry": rpc.call(CL_FACTORY, SEL["discountRegistry"], tag)}
    rec["allPoolsLength"], pools = last_pools(rpc, CL_FACTORY, tag, n)
    rec["pools"] = []
    for p in pools:
        x = {"pool": p}
        for k in ("slot0", "liquidity", "tickSpacing", "fee", "token0", "token1", "factory"):
            x[k] = rpc.call(p, SEL[k], tag)
        x["getPoolSwapHook"] = rpc.call(CL_FACTORY, SEL["getPoolSwapHook"] + addr(p), tag)
        ts = as_int(x["tickSpacing"])
        if ts is not None:
            if ts >= 1 << 255:
                ts -= 1 << 256
            x["tickSpacingToFee"] = rpc.call(CL_FACTORY, SEL["tickSpacingToFee"] + word(ts), tag)
        rec["pools"].append(x)
    return rec


def main():
    ap = argparse.ArgumentParser(description="Запись Aero Basic и Slipstream V3 своим узлом Arbitrum (решение 470)")
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--max-pools", type=int, default=20)
    ap.add_argument("--max-seconds", type=int, default=600)
    ap.add_argument("--out", default="tests/data")
    a = ap.parse_args()
    rpc = Rpc(a.rpc, time.time() + a.max_seconds)
    head = rpc.raw("eth_getBlockByNumber", ["latest", False])["result"]
    tag = head["number"]
    rec = {"block": int(tag, 16), "blockHash": head["hash"], "timestamp": int(head["timestamp"], 16),
           "baseFeePerGas": head.get("baseFeePerGas"),
           "volatile": basic(rpc, VOLATILE_FACTORY, tag, a.max_pools),
           "stable": basic(rpc, STABLE_FACTORY, tag, a.max_pools),
           "cl": cl(rpc, tag, a.max_pools)}
    day = datetime.date.today().isoformat()
    d = os.path.join(a.out, "arbitrum_aero_" + day)
    os.makedirs(d, exist_ok=True)
    body = json.dumps(rec, indent=1, sort_keys=True).encode()
    with open(os.path.join(d, "record.json"), "wb") as f:
        f.write(body)
    with open(os.path.join(d, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Запись Aero Basic и Slipstream V3 (решение 470)\n\n"
                "Скрипт: tools/arbitrum_record/aero.py. Узел Arbitrum One, все eth_call на одном блоке.\n\n"
                f"- блок: {rec['block']}\n- хеш блока: {rec['blockHash']}\n"
                f"- record.json sha256: {hashlib.sha256(body).hexdigest()}\n"
                f"- пулов: volatile {len(rec['volatile']['pools'])}, stable {len(rec['stable']['pools'])}, "
                f"CL {len(rec['cl']['pools'])}\n")
    print("записано:", d)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:  # узел недоступен, потолок времени — причина и код 1
        print("ошибка:", e)
        sys.exit(1)
