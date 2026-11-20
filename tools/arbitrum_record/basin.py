#!/usr/bin/env python3
# Файл: tools/arbitrum_record/basin.py
# Что делает: решение 485 — запись своего узла Arbitrum для скважин Basin (вид basin_well): официальные «Whitelisted
#   Beanstalk Wells» (docs.basin.exchange/resources/contracts). Только чтение (eth_call, eth_getCode,
#   eth_getBlockByNumber), без ключей, подписей и транзакций. Пишет tests/data/arbitrum_basin_<дата>/ (meta.json,
#   wells.json, README.md). Тест — tests/arbitrum_basin_live.rs (#[ignore] до записи).
# Кто вызывает: Тигран (record_all.py → record_steps.py) или вручную.
# От чего зависит: Python 3.8+ (стандартная библиотека), curve_plain_pools.py (Batch, call, key), curve_sim.py (Sim —
#   симуляция обмена от пустого адреса с подменой баланса и разрешения), curve_crypto_rpc.py (selector, word,
#   addr_word), recnode.py (fresh), stdhttp.py. Узел — аргумент --node.
# Настройки: нет. Адреса — официальные: docs.basin.exchange/resources/contracts (раздел Arbitrum).
#
# Порядок (узел хранит состояние ~128 блоков — решение 460): 1) на «latest» — tokens() скважин и слоты баланса /
#   разрешения их токенов для симуляции (curve_sim.py); 2) фаза состояния на свежем блоке B (recnode.fresh: узел уже
#   удалил состояние — фаза заново на новом блоке): ОДНИМ пакетом — ответы на вызовы бота по (адрес, calldata):
#   чтение load_basin.rs calls (getReserves, totalSupply, wellFunction, tokens, getAParameter() таблицы Stable2) и
#   первый проход проверки verify_basin_judge.rs first_calls (wellImplementation(скважина) Aquifer, getImplementation,
#   aquifer()), ещё numberOfPumps() и pumps() (насосы — для справки); ОДНИМ пакетом — второй проход проверки
#   (wellImplementation(реализация прокси), calcLpTokenSupply(резервы, данные) функции) и getSwapOut(from, to, вход)
#   на долях резерва входа (--fractions) в обе стороны; ОДНИМ пакетом — симуляции swapFrom(from, to, вход, 0, W,
#   2^256 − 1) на тех же суммах (curve_sim.py); eth_getCode скважин, реализаций прокси и функций.
#
# Запуск: python3 tools/arbitrum_record/basin.py --node http://УЗЕЛ:8547

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import recnode  # noqa: E402
from curve_crypto_rpc import addr_word, selector, word  # noqa: E402
from curve_plain_pools import Batch, call, key  # noqa: E402
from curve_sim import SENDER, Sim  # noqa: E402

# docs.basin.exchange/resources/contracts, раздел Arbitrum.
AQUIFER = "0xBA51AAAa8C2f911AE672e783707Ceb2dA6E97521"
IMPLS = ["0xBA5106bd62b342afAcB93f1078fe60177A62d1a9", "0xBA510995783111be5301d93CCfD5dE4e3B28e50B"]
# ConstantProduct2: docs.basin.exchange и BeanstalkFarms/Beanstalk@6db8c07c ReseedBean.sol L68-L69 (оба — Sourcify
# exact_match); какой у скважин — покажет запись (wells.json, function).
CP2 = ["0xBA15000450Bf6d48ec50BD6327A9403E401b72b4", "0xbA1500c28C8965521f47F17Fc21A7829D6E1343e"]
STABLE2 = ["0xba150052e11591D0648b17A0E608511874921CBC"]
LUT = "0xBA51055dAD14d3920e1798D2e8A152d91CaDb461"
WELLS = [
    "0xBeA00Aa8130aCaD047E137ec68693C005f8736Ce",  # BEAN:WETH
    "0xBEa00BbE8b5da39a3F57824a1a13Ec2a8848D74F",  # BEAN:wstETH
    "0xBeA00Cc9F93E9a8aC0DFdfF2D64Ba38eb9C2e48c",  # BEAN:weETH
    "0xBea00DDe4b34ACDcB1a30442bD2B39CA8Be1b09c",  # BEAN:WBTC
    "0xBea00ee04D8289aEd04f92EA122a96dC76A91bd7",  # BEAN:USDC
    "0xbEA00fF437ca7E8354B174339643B4d1814bED33",  # BEAN:USDT
]
SWAP_FROM = "swapFrom(address,address,uint256,uint256,address,uint256)"
MAXU = 2**256 - 1


def say(text):
    print(text, file=sys.stderr, flush=True)


def words(h):
    """Ответ eth_call → список слов (int)."""
    raw = bytes.fromhex(h[2:])
    return [int.from_bytes(raw[k:k + 32], "big") for k in range(0, len(raw) - len(raw) % 32, 32)]


def dyn_array(h):
    """uint256[] / address[] из ответа (смещение, длина, элементы)."""
    ws = words(h)
    at = ws[0] // 32
    return ws[at + 1:at + 1 + ws[at]]


def call_struct(h):
    """Call(address target, bytes data) из ответа wellFunction(): (адрес, данные hex)."""
    raw = bytes.fromhex(h[2:])
    base = int.from_bytes(raw[0:32], "big")
    target = "0x" + raw[base + 12:base + 32].hex()
    off = base + int.from_bytes(raw[base + 32:base + 64], "big")
    n = int.from_bytes(raw[off:off + 32], "big")
    return target, "0x" + raw[off + 32:off + 32 + n].hex()


def enc_calc_lp(reserves, data_hex):
    """calldata calcLpTokenSupply(uint256[] reserves, bytes data)."""
    d = bytes.fromhex(data_hex[2:])
    head = word(64) + word(64 + 32 * (1 + len(reserves)))
    arr = word(len(reserves)) + "".join(word(r) for r in reserves)
    pad = d + b"\0" * ((32 - len(d) % 32) % 32)
    return selector("calcLpTokenSupply(uint256[],bytes)") + head + arr + word(len(d)) + pad.hex()


def bot_calls(well):
    """(адрес, calldata) первой пачки: load_basin.rs calls, verify_basin_judge.rs first_calls и насосы."""
    out = [(well, selector(s)) for s in ["getReserves()", "totalSupply()", "wellFunction()", "tokens()",
                                          "getImplementation()", "aquifer()", "numberOfPumps()", "pumps()"]]
    out.append((LUT, selector("getAParameter()")))
    out.append((AQUIFER, selector("wellImplementation(address)") + addr_word(well)))
    return out


def state_phase(b, sim, fractions, head):
    """Фаза состояния на блоке head. Возвращает: (head, recs, code). StaleState — наружу (recnode.fresh)."""
    tag = head["number"]
    sim.tag, sim.reqs, sim.recs = tag, [], []
    reqs, keys = [], []
    for well in WELLS:
        for to, d in bot_calls(well):
            reqs.append(call(to, d, tag))
            keys.append((well, key(to, d)))
    recs = {wl: {"well": wl, "answers": {}} for wl in WELLS}
    for (wl, k), x in zip(keys, b.run(reqs)):
        recs[wl]["answers"][k] = x
    reqs, keys = [], []
    for wl in WELLS:
        r, a = recs[wl], recs[wl]["answers"]
        get = lambda sig: a.get(key(wl, selector(sig)), {}).get("result")  # noqa: E731
        try:
            r["tokens"] = ["0x%040x" % t for t in dyn_array(get("tokens()"))]
            r["reserves"] = [str(x) for x in dyn_array(get("getReserves()"))]
            r["function"], r["function_data"] = call_struct(get("wellFunction()"))
        except Exception as e:  # ответ скважины не разобран — запись с причиной, остальные скважины идут дальше
            r["error"] = "ответ скважины не разобран: %s" % e
            continue
        imp = get("getImplementation()")
        r["proxy_impl"] = "0x" + imp[-40:] if imp and len(imp) >= 42 else None
        if r["proxy_impl"]:
            d = selector("wellImplementation(address)") + addr_word(r["proxy_impl"])
            reqs.append(call(AQUIFER, d, tag))
            keys.append((wl, key(AQUIFER, d)))
        d = enc_calc_lp([int(x) for x in r["reserves"]], r["function_data"])
        reqs.append(call(r["function"], d, tag))
        keys.append((wl, key(r["function"], d)))
        res = [int(x) for x in r["reserves"]]
        for i in range(2):
            j = 1 - i
            for f in fractions.split(","):
                dx = max(1, int(res[i] * float(f)))
                d = (selector("getSwapOut(address,address,uint256)") + addr_word(r["tokens"][i])
                     + addr_word(r["tokens"][j]) + word(dx))
                reqs.append(call(wl, d, tag))
                keys.append((wl, ("swap_out", i, j, dx)))
                data = (selector(SWAP_FROM) + addr_word(r["tokens"][i]) + addr_word(r["tokens"][j]) + word(dx)
                        + word(0) + addr_word(SENDER) + word(MAXU))
                sim.add(wl, SWAP_FROM, i, j, dx, r["tokens"][i], data=data)
    for (wl, k), x in zip(keys, b.run(reqs)):
        if isinstance(k, tuple):
            _, i, j, dx = k
            recs[wl].setdefault("get_swap_out", []).append({"i": i, "j": j, "dx": str(dx), **x})
        else:
            recs[wl]["answers"][k] = x
    for s in sim.run():
        recs[s["pool"]].setdefault("swap_from_sim", []).append(s)
    code = {}
    addrs = list(WELLS) + [r.get("proxy_impl") for r in recs.values()] + [r.get("function") for r in recs.values()]
    for addr in sorted({x.lower() for x in addrs if x}):
        c = b.one("eth_getCode", [addr, tag])
        h = c.get("result") or ""
        code[addr] = {"len": max(0, len(h) // 2 - 1), "sha256": hashlib.sha256(h.encode()).hexdigest(),
                      "has_swap_from": selector(SWAP_FROM)[2:] in h, "error": c.get("error")}
    return head, recs, code


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: скважины Basin (решение 485)")
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--fractions", default="0.0001,0.01,0.2", help="доли резерва входа для getSwapOut и симуляции")
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    t0 = time.time()
    b = Batch(a.node)
    chain = b.one("eth_chainId", [])["result"]
    sim = Sim(a.node, "latest")
    if not sim.empty():
        say("ОШИБКА: отправитель симуляции %s не пуст (код или баланс)" % SENDER)
        sys.exit(2)
    pre = b.run([call(wl, selector("tokens()"), "latest") for wl in WELLS])
    for wl, x in zip(WELLS, pre):
        if not x.get("result"):
            say("скважина %s: tokens() не ответил: %s" % (wl, x))
            continue
        for t in dyn_array(x["result"]):
            sim.prefill("0x%040x" % t, wl)
    head, recs, code = recnode.fresh(lambda: b.one("eth_getBlockByNumber", ["latest", False])["result"],
                                     lambda h: state_phase(b, sim, a.fractions, h), say=say)
    write(a, chain, head, recs, code, t0)


def write(a, chain, head, recs, code, t0):
    """Файлы записи и README с SHA-256."""
    tag = head["number"]
    files = {
        "meta.json": {"chainId": chain, "block": int(tag, 16), "block_hex": tag,
                      "timestamp": int(head["timestamp"], 16), "aquifer": AQUIFER, "impls": IMPLS, "cp2": CP2,
                      "stable2": STABLE2, "lut": LUT, "wells": WELLS, "sim_sender": SENDER,
                      "fractions": a.fractions, "seconds": round(time.time() - t0, 1)},
        "wells.json": [recs[wl] for wl in WELLS],
        "code.json": code,
    }
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = a.out or os.path.join(root, "tests", "data", "arbitrum_basin_" + day)
    os.makedirs(out, exist_ok=True)
    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Скважины Basin (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/basin.py (решение 485): только чтение, без ключей и транзакций.\n"
                "Блок B = %d, chainId %s; симуляции swapFrom — eth_call с подменой состояния от пустого адреса %s.\n"
                % (int(tag, 16), chain, SENDER))
        f.write("Файлы и порядок записи — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    say("записано в " + out)


if __name__ == "__main__":
    main()
