"""Файл: tools/arbitrum_record/ramses_calls.py
Что делает: решение 483 — вызовы узла для записи пулов Ramses (ramses.py): кодирование eth_call, разбор слов, слова
  tickBitmap и ticks() вокруг тика, проба обмена swap пула через eth_call с переопределением кода вызывающего.
  Проба обмена: у адреса PROBE на время eth_call — код 0x366000600037366000fd (CALLDATASIZE PUSH1 0 PUSH1 0
  CALLDATACOPY CALLDATASIZE PUSH1 0 REVERT — откат с данными своего вызова). eth_call от PROBE к пулу:
  swap(PROBE, zeroForOne, +сумма, предел, "") — пул считает обмен своим кодом и зовёт обратный вызов у PROBE
  (RamsesV2Pool.sol L1238 / L1256 ramsesV2SwapCallback; RamsesV3Pool.sol L662 / L671 uniswapV3SwapCallback), тот
  откатывается со своими аргументами (amount0, amount1), пул пробрасывает откат (вызов высокого уровня Solidity) —
  в ответе узла дельты обмена. В сеть ничего не уходит (eth_call, третий параметр — переопределение состояния:
  geth.ethereum.org/docs/interacting-with-geth/rpc/objects#state-override-set).
Кто вызывает: tools/arbitrum_record/ramses.py.
От чего зависит: camelot_v2.py (keccak256: sel, topic), stdhttp.py (через Rpc вызывающего).
Ошибки: см. функции.
"""

from camelot_v2 import sel

PROBE = "0x00000000000000000000000000000000000ab0e1"
PROBE_CODE = "0x366000600037366000fd"
MIN_LIMIT = 4295128739 + 1  # TickMath.MIN_SQRT_RATIO + 1 (v3-core TickMath.sol L14)
MAX_LIMIT = 1461446703485210103287273052203988822378723970342 - 1  # MAX_SQRT_RATIO − 1 (L16)
SWAP = sel("swap(address,bool,int256,uint160,bytes)")


def word(v):
    """Целое → 32-байтное слово hex (без 0x); отрицательное — дополнительный код. Ошибок нет."""
    return format(v % (1 << 256), "064x")


def addr(a):
    """Адрес → слово. Ошибок нет."""
    return word(int(a, 16))


def w(res, i=0, signed=False):
    """Слово i ответа {"result": hex} целым; None — откат или короткий ответ. Ошибок нет."""
    h = (res or {}).get("result") or ""
    h = h[2:] if h.startswith("0x") else h
    if len(h) < 64 * (i + 1):
        return None
    v = int(h[64 * i:64 * (i + 1)], 16)
    return v - (1 << 256) if signed and v >= 1 << 255 else v


def a_of(res):
    """Адрес из первого слова ответа; None — нет. Ошибок нет."""
    v = w(res)
    return None if v is None else "0x" + format(v, "040x")


def call(rpc, to, sig, args, tag):
    """eth_call метода sig с готовыми словами args (строка hex). Возвращает: {"result"} или {"error"}. Ошибки: узел."""
    return rpc.call(to, sel(sig)[2:] + args, tag)


def bitmap_and_ticks(rpc, pool, tick, spacing, words, ticks_sig, tag):
    """Слова tickBitmap [w0 − words, w0 + words] и ticks() установленных тиков. Возвращает: {"words": [{word, raw}],
    "ticks": [{tick, raw}]}. Ошибок нет (откат — в записи)."""
    comp = tick // spacing  # TickBitmap.position: compressed = tick / tickSpacing, округление к −∞
    w0 = comp >> 8
    out = {"words": [], "ticks": []}
    for wp in range(w0 - words, w0 + words + 1):
        res = call(rpc, pool, "tickBitmap(int16)", word(wp), tag)
        out["words"].append({"word": wp, "raw": res})
        bits = w(res)
        if not bits:
            continue
        for b in range(256):
            if bits >> b & 1:
                t = (wp * 256 + b) * spacing
                out["ticks"].append({"tick": t, "raw": call(rpc, pool, ticks_sig, word(t), tag)})
    return out


def probe(rpc, pool, zero_for_one, amount, tag):
    """Проба обмена (шапка): точный вход amount. Возвращает: {"amount0", "amount1"} (строки, со знаком) или
    {"error": …}. Ошибок нет: ответ без отката с данными — в записи как есть."""
    limit = MIN_LIMIT if zero_for_one else MAX_LIMIT
    data = SWAP + addr(PROBE) + word(1 if zero_for_one else 0) + word(amount) + word(limit) + word(0xa0) + word(0)
    body = rpc.raw("eth_call", [{"from": PROBE, "to": pool, "data": data}, tag, {PROBE: {"code": PROBE_CODE}}])
    err = body.get("error") or {}
    raw = err.get("data") if isinstance(err, dict) else None
    if not isinstance(raw, str) or len(raw) < 2 + 8 + 128:
        return {"error": err or body.get("result")}
    h = raw[2 + 8:]
    a0, a1 = int(h[:64], 16), int(h[64:128], 16)
    a0 = a0 - (1 << 256) if a0 >= 1 << 255 else a0
    a1 = a1 - (1 << 256) if a1 >= 1 << 255 else a1
    return {"selector": raw[:10], "amount0": str(a0), "amount1": str(a1)}


def balance(rpc, token, holder, tag):
    """balanceOf(holder) токена; None — откат. Ошибок нет."""
    return w(call(rpc, token, "balanceOf(address)", addr(holder), tag))
