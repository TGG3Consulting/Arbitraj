#!/usr/bin/env python3
# Файл: tools/arbitrum_record/ekubo.py
# Что делает: запись фактов своего узла Arbitrum для семейства пулов Ekubo (решение 447, задача T433-F8 / T433-079) —
#   только чтение (eth_call, eth_getLogs, eth_getBlockByNumber), без ключей и подписей. Пишет JSON-файлы в
#   tests/data/arbitrum_ekubo_<дата>/ и README.md (что записано, блок, хеши файлов). По этим файлам идут тесты
#   tests/arbitrum_ekubo_live.rs (сейчас #[ignore] — записи нет).
# Кто вызывает: Тигран на своём сервере (узел Arbitrum), вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459). Узел — аргумент --rpc (HTTP JSON-RPC). GeckoTerminal — по желанию (--gt).
# Настройки: нет (адреса — аргументы; по умолчанию — официальные: docs.ekubo.org/reference/contracts/evm-v3 и
#   EkuboProtocol/evm-contracts broadcast/DeployAll.s.sol/42161).
#
# Что записывается (на одном блоке B = «latest» на момент запуска, номер — в meta.json):
#   meta.json        — eth_chainId, номер и время блока B, адреса, аргументы запуска;
#   gt_pools.json    — ответы GeckoTerminal GET /api/v2/networks/arbitrum/dexes/ekubo-v3-arbitrum/pools?page=N (как есть);
#   pools.json       — по каждому PoolId (из GeckoTerminal и/или --pool): ответ PoolKeyIndex.poolKeyById (сырой hex),
#                      Core.sload(состояние), слова битмапа (±--words от слова текущего тика), тики установленных битов,
#                      MEVCapture.sload(PoolId) у пула с этим расширением, decimals токенов, ответы
#                      MEVCaptureRouter.quote на суммы --sizes в обе стороны (сырой hex; откат — текст ошибки);
#   core_logs.json   — eth_getLogs всех логов ядра за блоки [B − --log-blocks, B] (анонимный обмен log0, PositionUpdated …).
# Селекторы (keccak256 сигнатур, проверено при написании): sload() 0x380eb4e0, poolKeyById(bytes32) 0xea6c8bc7,
#   quote((address,address,bytes32),bool,int128,uint96,uint256) 0x3bc52842, decimals() 0x313ce567.
#
# Запуск: python3 tools/arbitrum_record/ekubo.py --rpc http://127.0.0.1:8547 --gt [--pool 0x…] [--words 4]

import argparse
import datetime
import hashlib
import json
import os
import sys

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests

CORE = "0x00000000000014aA86C5d3c41765bb24e11bd701"
KEY_INDEX = "0x898956fc2aed01d5f81f556ff5dcb10534285718"
MEV_CAPTURE = "0x5555fF9Ff2757500BF4EE020DcfD0210CFfa41Be"
ROUTER = "0xd26f20001a72a18C002b00e6710000d68700ce00"
GT = "https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/ekubo-v3-arbitrum/pools"

SEL_SLOAD = "380eb4e0"
SEL_KEY = "ea6c8bc7"
SEL_QUOTE = "3bc52842"
SEL_DECIMALS = "313ce567"

# CoreStorageLayout.sol L23, L31 (evm-contracts@61d09daa); tickBitmap.sol L10; constants.sol L10, L14.
TICKS_OFFSET = 0x435A5EB89A296820174331CF5A3902D9FCA683928D56726D8E7ACD6EFB28C568
BITMAPS_OFFSET = 0x3DEF450D0010A2FEF515CE5EBA4B363B5A0F42FDD4C53E1C737975DB05A2E3A5
BITMAP_OFFSET = 89421695
MIN_TICK, MAX_TICK = -88722835, 88722835
MOD = 1 << 256


class Rpc:
    """JSON-RPC узла: один запрос — один вызов; ошибка узла — исключение с текстом."""

    def __init__(self, url):
        self.url = url
        self.n = 0

    def call(self, method, params):
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=60)
        r.raise_for_status()
        body = r.json()
        if "error" in body:
            raise RuntimeError(json.dumps(body["error"]))
        return body["result"]

    def eth_call(self, to, data, tag):
        return self.call("eth_call", [{"to": to, "input": "0x" + data}, tag])


def word(v):
    """Число → 32 байта hex без 0x."""
    return format(v % MOD, "064x")


def sload(rpc, at, slots, tag):
    """Core/MEVCapture.sload(): значения слотов подряд (ExposedStorage.sol L15-L20)."""
    raw = rpc.eth_call(at, SEL_SLOAD + "".join(word(s) for s in slots), tag)[2:]
    return [raw[i * 64:(i + 1) * 64] for i in range(len(slots))]


def word_index(tick, spacing):
    """tickToBitmapWordAndIndex (tickBitmap.sol L14-L20): floor(tick / spacing) + смещение; слово — >> 8."""
    raw = tick // spacing + BITMAP_OFFSET
    return raw >> 8, raw & 0xFF


def tick_of(w, i, spacing):
    """bitmapWordAndIndexToTick (tickBitmap.sol L24-L29)."""
    return ((w << 8) + i - BITMAP_OFFSET) * spacing


def signed(v, bits):
    return v - (1 << bits) if v >> (bits - 1) else v


def gt_pages(pages):
    out = []
    for page in range(1, pages + 1):
        r = requests.get(GT, params={"page": page}, headers={"Accept": "application/json"}, timeout=60)
        out.append({"page": page, "status": r.status_code, "body": r.text})
        if r.status_code != 200 or not json.loads(r.text).get("data"):
            break
    return out


def quote_data(t0, t1, config, is_token1, amount):
    """quote(PoolKey, isToken1, int128 amount, uint96 0, uint256 0) — статический кортеж, слова подряд."""
    return SEL_QUOTE + word(int(t0, 16)) + word(int(t1, 16)) + config + word(1 if is_token1 else 0) + word(amount) \
        + word(0) + word(0)


def record_pool(rpc, pid, args, tag):
    rec = {"pool_id": pid}
    key = rpc.eth_call(args.key_index, SEL_KEY + pid[2:], tag)[2:]
    rec["poolKeyById"] = "0x" + key
    t0, t1, config = "0x" + key[24:64], "0x" + key[88:128], key[128:192]
    if int(t1, 16) == 0:
        rec["note"] = "нет в PoolKeyIndex"
        return rec
    state = sload(rpc, args.core, [int(pid, 16)], tag)[0]
    rec["state"] = "0x" + state
    v = int(state, 16)
    tick = signed((v >> 128) & 0xFFFFFFFF, 32)
    tb = int(config[-8:], 16)
    ext = "0x" + config[:40]
    if tb & 0x80000000:
        spacing = tb & 0x7FFFFFFF
        c, _ = word_index(tick, spacing)
        lo = max(c - args.words, word_index(MIN_TICK, spacing)[0])
        hi = min(c + args.words, word_index(MAX_TICK, spacing)[0])
        words = list(range(lo, hi + 1))
        got = sload(rpc, args.core, [int(pid, 16) + BITMAPS_OFFSET + w for w in words], tag)
        rec["words"] = [lo, hi]
        rec["bitmap"] = [{"word": w, "bits": "0x" + b} for w, b in zip(words, got) if int(b, 16)]
        ticks = []
        for e in rec["bitmap"]:
            bits = int(e["bits"], 16)
            ticks += [tick_of(e["word"], i, spacing) for i in range(256) if bits >> i & 1]
        ticks.sort()
        vals = sload(rpc, args.core, [int(pid, 16) + TICKS_OFFSET + t for t in ticks], tag) if ticks else []
        rec["ticks"] = [{"tick": t, "info": "0x" + x} for t, x in zip(ticks, vals)]
    if int(ext, 16) == int(args.mev, 16):
        rec["mev_state"] = "0x" + sload(rpc, args.mev, [int(pid, 16)], tag)[0]
    dec = []
    for t in (t0, t1):
        try:
            dec.append(int(rpc.eth_call(t, SEL_DECIMALS, tag), 16))
        except Exception as e:  # откат или не токен — записываем текст
            dec.append(str(e))
    rec["decimals"] = dec
    quotes = []
    for is_token1, d in ((False, dec[0]), (True, dec[1])):
        if not isinstance(d, int):
            continue
        for s in args.sizes.split(","):
            amount = int(float(s) * 10 ** d)
            q = {"is_token1": is_token1, "amount": str(amount)}
            try:
                q["out"] = rpc.eth_call(args.router, quote_data(t0, t1, config, is_token1, amount), tag)
            except Exception as e:
                q["error"] = str(e)
            quotes.append(q)
    rec["quotes"] = quotes
    return rec


def main():
    ap = argparse.ArgumentParser(description="Запись фактов узла Arbitrum для Ekubo (решение 447)")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--gt", action="store_true", help="записать пулы GeckoTerminal ekubo-v3-arbitrum и взять их PoolId")
    ap.add_argument("--gt-pages", type=int, default=10)
    ap.add_argument("--pool", action="append", default=[], help="PoolId (0x…, 32 байта); можно несколько")
    ap.add_argument("--words", type=int, default=4, help="слов битмапа по каждую сторону от слова текущего тика")
    ap.add_argument("--sizes", default="0.001,0.01,0.1,1,10,100,1000", help="суммы входа в единицах токена")
    ap.add_argument("--log-blocks", type=int, default=2000, help="сколько блоков логов ядра до B")
    ap.add_argument("--core", default=CORE)
    ap.add_argument("--key-index", default=KEY_INDEX)
    ap.add_argument("--mev", default=MEV_CAPTURE)
    ap.add_argument("--router", default=ROUTER)
    ap.add_argument("--out", default=None, help="папка вывода (по умолчанию tests/data/arbitrum_ekubo_<дата>)")
    args = ap.parse_args()

    rpc = Rpc(args.rpc)
    chain = rpc.call("eth_chainId", [])
    head = rpc.call("eth_getBlockByNumber", ["latest", False])
    b, tag = int(head["number"], 16), head["number"]
    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_ekubo_" + day)
    os.makedirs(out, exist_ok=True)
    files = {}

    meta = {"chainId": chain, "block": b, "block_hex": tag, "timestamp": int(head["timestamp"], 16),
            "core": args.core, "key_index": args.key_index, "mev_capture": args.mev, "router": args.router,
            "words": args.words, "sizes": args.sizes, "log_blocks": args.log_blocks}
    files["meta.json"] = meta

    ids = list(args.pool)
    if args.gt:
        pages = gt_pages(args.gt_pages)
        files["gt_pools.json"] = pages
        for p in pages:
            if p["status"] == 200:
                for item in json.loads(p["body"]).get("data", []):
                    addr = item.get("attributes", {}).get("address", "")
                    if addr.startswith("0x") and len(addr) == 66:
                        ids.append(addr)
    pools = []
    for pid in dict.fromkeys(i.lower() for i in ids):
        try:
            pools.append(record_pool(rpc, pid, args, tag))
        except Exception as e:
            pools.append({"pool_id": pid, "error": str(e)})
    files["pools.json"] = pools

    frm = hex(max(0, b - args.log_blocks))
    files["core_logs.json"] = {"from": frm, "to": tag,
                               "logs": rpc.call("eth_getLogs", [{"address": args.core, "fromBlock": frm, "toBlock": tag}])}

    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Ekubo (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/ekubo.py (решение 447): только чтение, без ключей и подписей.\n")
        f.write("Блок B = %d (%s), время %d, chainId %s. Пулов: %d. Узел: %s.\n\n" % (
            b, tag, meta["timestamp"], chain, len(pools), args.rpc.split("@")[-1]))
        f.write("Файлы — шапка скрипта. Ядро %s, PoolKeyIndex %s, MEVCapture %s, MEVCaptureRouter %s.\n\n" % (
            args.core, args.key_index, args.mev, args.router))
        f.write("## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
