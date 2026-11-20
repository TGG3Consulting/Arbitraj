"""Файл: tools/arbitrum_record/kyber_elastic_ticks.py
Что делает: решение 462 — модуль kyber_elastic_v1.py (файлы ≤ ~200 строк): JSON-RPC узла, слова ABI и запись одного пула
  KyberSwap Elastic (v1 или v2 — квотер, читатель тиков и фабрика своей версии): состояние, getPool фабрики,
  getTicksInRange(pool, MIN_TICK, 0) как у бота (pools/load_elastic.rs) и тики частями по связному списку, ticks(t),
  decimals, QuoterV2.quoteExactInputSingle в обе стороны.
  Чтение частями: TicksFeesReader.getTicksInRange(pool, start, length > 0) (ks-elastic-sc 4ab08c0a
  contracts/periphery/TicksFeesReader.sol L37-L67) — массив из length тиков от start по initializedTicks; после MAX_TICK
  цикл обрывается (остаток массива — нули). Следующая часть начинается с последнего тика предыдущей.
  Селекторы (keccak256 сигнатур): getPoolState() 217ac237, getLiquidityState() ab612f2b, swapFeeUnits() c79a590e,
  tickDistance() 48626a8c, token0() 0dfe1681, token1() d21220a7, factory() c45a0155, ticks(int24) f30dba93,
  getTicksInRange(address,int24,uint32) 6bea4865, getPool(address,address,uint24) 1698ee82,
  quoteExactInputSingle((address,address,uint256,uint24,uint160)) c6a5026a, decimals() 313ce567.
Кто вызывает: tools/arbitrum_record/kyber_elastic_v1.py.
От чего зависит: stdhttp.py.
Ошибки: см. функции; StaleState (stdhttp, BaseException) не перехватывается — его повторяет recnode.fresh.
"""

import json

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests

MIN_TICK = -887272
MAX_TICK = 887272
MOD = 1 << 256


class Rpc:
    """JSON-RPC узла: один запрос — один вызов; ошибка узла — исключение с текстом."""

    def __init__(self, url):
        self.url = url
        self.n = 0

    def call(self, method, params):
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=120)
        r.raise_for_status()
        body = r.json()
        if "error" in body:
            raise RuntimeError(json.dumps(body["error"]))
        return body["result"]

    def eth_call(self, to, data, tag):
        return self.call("eth_call", [{"to": to, "input": "0x" + data}, tag])


def word(v):
    """Число (в т.ч. отрицательное — дополнение до 2^256) → 32 байта hex без 0x."""
    return format(v % MOD, "064x")


def addr_word(a):
    return word(int(a, 16))


def try_call(rpc, to, data, tag):
    """eth_call → {"ok": hex} или {"error": текст} (откат пишется, а не роняет запись)."""
    try:
        return {"ok": rpc.eth_call(to, data, tag)}
    except Exception as e:  # noqa: BLE001
        return {"error": str(e)}


def int24_list(hexdata):
    """Разбор int24[] из ответа ABI: смещение, длина, слова."""
    raw = hexdata[2:]
    n = int(raw[64:128], 16)
    out = []
    for i in range(n):
        v = int(raw[128 + 64 * i:192 + 64 * i], 16)
        out.append(v - MOD if v >> 255 else v)
    return out


def upto_max(ticks):
    """Тики до MAX_TICK включительно (после него — нули незаполненного массива). Ошибок нет."""
    return ticks[:ticks.index(MAX_TICK) + 1] if MAX_TICK in ticks else ticks


def ticks_by_chunks(rpc, reader, pool, chunk, tag):
    """Все инициализированные тики частями по chunk. Возвращает: (ответы частей как есть, тики). Откат части — в ответах,
    чтение останавливается."""
    parts, ticks, start = [], [], MIN_TICK
    while True:
        r = try_call(rpc, reader, "6bea4865" + addr_word(pool) + word(start) + word(chunk), tag)
        parts.append(dict(r, start=start, length=chunk))
        if "ok" not in r:
            return parts, ticks
        got = upto_max(int24_list(r["ok"]))
        ticks += got if not ticks else got[1:]  # первый тик части = последний тик прошлой
        if not got or got[-1] == MAX_TICK or len(got) < chunk:
            return parts, ticks
        start = got[-1]


def record_pool(rpc, pool, ver, args, tag):
    """Один пул: ver — {"factory", "quoter", "reader"} его версии. Возвращает запись. Ошибки: ответ не по ABI —
    исключение (вызывающий пишет пул с error)."""
    rec = {"pool": pool, "quoter": ver["quoter"], "reader": ver["reader"]}
    for name, sel in [("token0", "0dfe1681"), ("token1", "d21220a7"), ("factory", "c45a0155"),
                      ("swapFeeUnits", "c79a590e"), ("tickDistance", "48626a8c"), ("getPoolState", "217ac237"),
                      ("getLiquidityState", "ab612f2b")]:
        rec[name] = try_call(rpc, pool, sel, tag)
    t0 = "0x" + rec["token0"]["ok"][-40:]
    t1 = "0x" + rec["token1"]["ok"][-40:]
    fee = int(rec["swapFeeUnits"]["ok"], 16)
    rec["factory_getPool"] = try_call(rpc, ver["factory"], "1698ee82" + addr_word(t0) + addr_word(t1) + word(fee), tag)
    rec["decimals"] = {t: try_call(rpc, t, "313ce567", tag) for t in (t0, t1)}
    # Как бот (load_elastic.rs): getTicksInRange(pool, MIN_TICK, 0) — ответ или ошибка узла как есть.
    rng = try_call(rpc, ver["reader"], "6bea4865" + addr_word(pool) + word(MIN_TICK) + word(0), tag)
    rec["getTicksInRange"] = rng
    parts, chunked = ticks_by_chunks(rpc, ver["reader"], pool, args.tick_chunk, tag)
    rec["ticks_chunks"] = parts
    whole = upto_max(int24_list(rng["ok"])) if "ok" in rng else []
    rec["ticks_chunks_match"] = (whole == chunked) if whole else None
    rec["ticks"] = {str(t): try_call(rpc, pool, "f30dba93" + word(t), tag) for t in (whole or chunked)}
    quotes = []
    for a, b in ((t0, t1), (t1, t0)):
        dec = int(rec["decimals"][a]["ok"], 16)
        for s in args.sizes.split(","):
            amount = int(float(s) * 10 ** dec)
            data = "c6a5026a" + addr_word(a) + addr_word(b) + word(amount) + word(fee) + word(0)
            q = {"tokenIn": a, "tokenOut": b, "amountIn": str(amount)}
            q.update(try_call(rpc, ver["quoter"], data, tag))
            quotes.append(q)
    rec["quotes"] = quotes
    return rec
