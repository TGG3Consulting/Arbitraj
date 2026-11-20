#!/usr/bin/env python3
# Файл: tools/arbitrum_record/ekubo_more.py
# Что делает: дополнительная запись своего узла Arbitrum для Ekubo (решение 454) — только чтение (eth_call, eth_getLogs,
#   eth_getBlockByNumber), без ключей и подписей. Пишет в ту же папку tests/data/arbitrum_ekubo_<дата>/ (или --out):
#   more_meta.json     — блок B, адреса, аргументы;
#   pool_initialized.json — все PoolInitialized ядра за блоки [--from-block, B] (как есть) — история ключей без
#                        PoolKeyIndex; сводка: сколько с нативным ETH, по расширениям, сколько нет в PoolKeyIndex;
#   native_pools.json  — пулы с token0 = 0 (до --max-native): ключ из события, состояние, битмап, тики, MEVCapture,
#                        MEVCaptureRouter.quote в обе стороны (нативный ETH — 18 знаков);
#   twamm_pools.json   — пулы с расширением из --twamm (до --max-twamm): то же + TWAMM.sload: состояние, слова битмапа
#                        времён всех допустимых времён до «время B + --horizon», TimeInfo установленных битов;
#   core_load.json     — замер нагрузки подписки: логи ядра и TWAMM за [B − --load-blocks, B] — число и байты
#                        (logs_per_block, bytes_per_block): столько получала бы КАЖДАЯ пачка Ekubo до решения 454 и
#                        получает один хаб площадки после.
# Кто вызывает: Тигран на своём сервере (узел Arbitrum), вручную, после tools/arbitrum_record/ekubo.py.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459), tools/arbitrum_record/ekubo.py (общие функции). Узел — аргумент --rpc.
# Настройки: нет (адреса — аргументы; по умолчанию — официальные, broadcast DeployAll.s.sol/42161, docs.ekubo.org).
#
# Источники: evm-contracts@61d09daa src/interfaces/ICore.sol L111 (PoolInitialized), TWAMMStorageLayout.sol L20-L64,
#   math/time.sol L17-L65 (допустимые времена), math/timeBitmap.sol L10-L23. topic0 PoolInitialized =
#   keccak256("PoolInitialized(bytes32,(address,address,bytes32),int32,uint96)") = 0x5e4688b3…154d (посчитано
#   pycryptodome при написании; аргумент --topic; сверка с SIGNATURE_HASH — тест tests/arbitrum_ekubo_more_live.rs).
#
# Запуск: python3 tools/arbitrum_record/ekubo_more.py --rpc http://127.0.0.1:8547 --from-block <блок ядра>

import argparse
import datetime
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ekubo as base  # noqa: E402 — общие функции записи Ekubo (решение 447)

TWAMM = "0xd4f1060cb9c1a13e1d2d20379b8aa2cf7541ed9b,0xd47f1b1edcfeabb08f6ebd8fc337c27e636c75ba"
# TWAMMStorageLayout.sol L22, L24.
TIME_BITMAPS_OFFSET = 0x07F3F693B68A1A1B1B3315D4B74217931D60E9DC7F1AF4989F50E7AB31C8820E
TIME_INFOS_OFFSET = 0x70DB18EF1C685B7AA06D1AC5EA2D101C7261974DF22A15951F768F92187043FB


def step_size(cur, t):
    """computeStepSize (time.sol L17-L31)."""
    if t > cur + 4095:
        msb = (t - cur).bit_length() - 1
        return 1 << (msb - msb % 4)
    return 256


def next_valid(cur, after):
    """nextValidTime (time.sol L44-L65); 0 — нет."""
    s = step_size(cur, after)
    n = after + s - (after + s) % s
    s2 = step_size(cur, n)
    if s2 != s:
        n = after + s2 - (after + s2) % s2
    return 0 if n > cur + 0xFFFFFFFF else n


def words_until(last, end):
    """Слова битмапа времён всех допустимых времён в (last, end] — как math/ekubo_time.rs words_until."""
    out, t = [], last
    while True:
        t = next_valid(last, t)
        if t == 0 or t > end:
            return out
        if not out or out[-1] != t >> 16:
            out.append(t >> 16)


def logs_range(rpc, flt, frm, to, chunk):
    out = []
    while frm <= to:
        end = min(to, frm + chunk - 1)
        out += rpc.call("eth_getLogs", [dict(flt, fromBlock=hex(frm), toBlock=hex(end))])
        frm = end + 1
    return out


def record_key(rpc, pid, t0, t1, config, args, tag):
    """Состояние, битмап, тики, MEVCapture и quote пула по ключу из события (как ekubo.record_pool без реестра)."""
    rec = {"pool_id": pid, "token0": t0, "token1": t1, "config": "0x" + config}
    state = base.sload(rpc, args.core, [int(pid, 16)], tag)[0]
    rec["state"] = "0x" + state
    tick = base.signed((int(state, 16) >> 128) & 0xFFFFFFFF, 32)
    tb = int(config[-8:], 16)
    if tb & 0x80000000:
        sp = tb & 0x7FFFFFFF
        c, _ = base.word_index(tick, sp)
        lo = max(c - args.words, base.word_index(base.MIN_TICK, sp)[0])
        hi = min(c + args.words, base.word_index(base.MAX_TICK, sp)[0])
        ws = list(range(lo, hi + 1))
        got = base.sload(rpc, args.core, [int(pid, 16) + base.BITMAPS_OFFSET + w for w in ws], tag)
        rec["words"] = [lo, hi]
        rec["bitmap"] = [{"word": w, "bits": "0x" + b} for w, b in zip(ws, got) if int(b, 16)]
        ticks = sorted(base.tick_of(e["word"], i, sp) for e in rec["bitmap"] for i in range(256)
                       if int(e["bits"], 16) >> i & 1)
        vals = base.sload(rpc, args.core, [int(pid, 16) + base.TICKS_OFFSET + t for t in ticks], tag) if ticks else []
        rec["ticks"] = [{"tick": t, "info": "0x" + x} for t, x in zip(ticks, vals)]
    ext = "0x" + config[:40]
    if int(ext, 16) == int(args.mev, 16):
        rec["mev_state"] = "0x" + base.sload(rpc, args.mev, [int(pid, 16)], tag)[0]
    dec = []
    for t in (t0, t1):
        if int(t, 16) == 0:
            dec.append(18)  # нативный ETH: 18 знаков (NATIVE_TOKEN_ADDRESS, constants.sol L26)
            continue
        try:
            dec.append(int(rpc.eth_call(t, base.SEL_DECIMALS, tag), 16))
        except Exception as e:
            dec.append(str(e))
    rec["decimals"] = dec
    rec["quotes"] = []
    for is1, d in ((False, dec[0]), (True, dec[1])):
        if not isinstance(d, int):
            continue
        for s in args.sizes.split(","):
            amount = int(float(s) * 10 ** d)
            q = {"is_token1": is1, "amount": str(amount)}
            try:
                q["out"] = rpc.eth_call(args.router, base.quote_data(t0, t1, config, is1, amount), tag)
            except Exception as e:
                q["error"] = str(e)
            rec["quotes"].append(q)
    return rec


def record_twamm(rpc, rec, twamm, ts, args, tag):
    """TWAMM.sload: состояние, слова битмапа времён до ts + horizon, TimeInfo установленных битов."""
    pid = int(rec["pool_id"], 16)
    st = base.sload(rpc, twamm, [pid], tag)[0]
    last = int(st, 16) & 0xFFFFFFFF
    real = ts - (((ts & 0xFFFFFFFF) - last) & 0xFFFFFFFF)
    ws = words_until(real, ts + args.horizon)
    got = base.sload(rpc, twamm, [pid + TIME_BITMAPS_OFFSET + w for w in ws], tag) if ws else []
    times = sorted((w << 16) + (i << 8) for w, b in zip(ws, got) for i in range(256)
                   if int(b, 16) >> i & 1 and (w << 16) + (i << 8) > real)
    infos = base.sload(rpc, twamm, [pid + TIME_INFOS_OFFSET + t for t in times], tag) if times else []
    rec["twamm"] = {"address": twamm, "state": "0x" + st, "until": ts + args.horizon, "words": ws,
                    "bitmap": ["0x" + b for b in got], "times": [{"time": t, "info": "0x" + x}
                                                             for t, x in zip(times, infos)]}


def main():
    ap = argparse.ArgumentParser(description="Запись фактов узла Arbitrum для Ekubo (решение 454)")
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--from-block", type=int, required=True, help="с какого блока искать PoolInitialized ядра")
    ap.add_argument("--chunk", type=int, default=50000, help="блоков в одном eth_getLogs")
    ap.add_argument("--topic", default="0x5e4688b340694b7c7fd30047fd082117dc46e32acfbf81a44bb1fac0ae65154d",
                    help="topic0 PoolInitialized (сверяет тест Rust по SIGNATURE_HASH)")
    ap.add_argument("--twamm", default=TWAMM)
    ap.add_argument("--horizon", type=int, default=86400, help="секунд времён TWAMM после блока B")
    ap.add_argument("--max-native", type=int, default=20)
    ap.add_argument("--max-twamm", type=int, default=20)
    ap.add_argument("--load-blocks", type=int, default=2000)
    ap.add_argument("--words", type=int, default=4)
    ap.add_argument("--sizes", default="0.001,0.01,0.1,1,10,100,1000")
    ap.add_argument("--core", default=base.CORE)
    ap.add_argument("--key-index", default=base.KEY_INDEX)
    ap.add_argument("--mev", default=base.MEV_CAPTURE)
    ap.add_argument("--router", default=base.ROUTER)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    rpc = base.Rpc(args.rpc)
    head = rpc.call("eth_getBlockByNumber", ["latest", False])
    b, tag, ts = int(head["number"], 16), head["number"], int(head["timestamp"], 16)
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    # Решение 459 (ревью t233, п. 7): та же дата, что у ekubo.py этого запуска record_all.py (ARB_RECORD_DATE).
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_ekubo_" + day)
    os.makedirs(out, exist_ok=True)
    twamms = [a.strip().lower() for a in args.twamm.split(",") if a.strip()]
    files = {"more_meta.json": {"block": b, "block_hex": tag, "timestamp": ts, "args": vars(args)}}

    inits = logs_range(rpc, {"address": args.core, "topics": [args.topic]}, args.from_block, b, args.chunk)
    summary = {"total": len(inits), "native": 0, "extensions": {}, "not_in_key_index": 0}
    natives, tw = [], []
    for lg in inits:
        d = lg["data"][2:]
        pid, t0, t1, cfg = "0x" + d[0:64], "0x" + d[88:128], "0x" + d[152:192], d[192:256]
        ext = "0x" + cfg[:40]
        summary["extensions"][ext] = summary["extensions"].get(ext, 0) + 1
        if int(t0, 16) == 0:
            summary["native"] += 1
        key = rpc.eth_call(args.key_index, base.SEL_KEY + pid[2:], tag)[2:]
        if int(key[88:128], 16) == 0:
            summary["not_in_key_index"] += 1
        try:
            if int(t0, 16) == 0 and len(natives) < args.max_native:
                natives.append(record_key(rpc, pid, t0, t1, cfg, args, tag))
            if ext.lower() in twamms and len(tw) < args.max_twamm:
                rec = record_key(rpc, pid, t0, t1, cfg, args, tag)
                record_twamm(rpc, rec, ext, ts, args, tag)
                tw.append(rec)
        except Exception as e:
            (natives if int(t0, 16) == 0 else tw).append({"pool_id": pid, "error": str(e)})
    files["pool_initialized.json"] = {"from": args.from_block, "to": b, "summary": summary, "logs": inits}
    files["native_pools.json"] = natives
    files["twamm_pools.json"] = tw

    frm = max(0, b - args.load_blocks)
    logs = logs_range(rpc, {"address": [args.core] + twamms}, frm, b, args.chunk)
    size = sum(len(json.dumps(x)) for x in logs)
    n = b - frm + 1
    files["core_load.json"] = {"from": frm, "to": b, "blocks": n, "logs": len(logs), "bytes": size,
                               "logs_per_block": len(logs) / n, "bytes_per_block": size / n}

    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README_454.md"), "w", encoding="utf-8") as f:
        f.write("# Ekubo (Arbitrum One) — дополнительная запись (решение 454)\n\n")
        f.write("tools/arbitrum_record/ekubo_more.py: только чтение. Блок B = %d (%s), время %d.\n" % (b, tag, ts))
        f.write("PoolInitialized: %s.\n\n## SHA-256\n\n" % json.dumps(summary, ensure_ascii=False))
        for h, nm in hashes:
            f.write("%s  %s\n" % (h, nm))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
