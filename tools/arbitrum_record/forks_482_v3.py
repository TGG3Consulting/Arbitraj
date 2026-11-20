"""Файл: tools/arbitrum_record/forks_482_v3.py
Что делает: решение 482 — часть записи forks_482.py для форков Uniswap V3 на Arbitrum (вид v3_fork): Rubicon CLMM и
  Arbitrum Exchange V3 (Arbidex V3). На блоке B записываются: поля пула (factory, token0, token1, fee, tickSpacing,
  slot0, liquidity), реестр фабрики getPool(token0, token1, fee), балансы пула; слова битмапа вокруг текущего тика
  через TickLens DEX (у Rubicon — официальный, у Arbidex V3 своего нет) и TickLens Uniswap V3; ticks(t) самого пула
  по каждому тику ответа TickLens (сверка: TickLens читает этот пул верно); выход обмена — QuoterV2 DEX (если есть) и
  виртуальный квотер бота (contracts/out/VirtualQuoter.bin-runtime подменой кода в eth_call, как
  quote/virtual_quoter.rs) на долях баланса входа, в обе стороны.
Кто вызывает: tools/arbitrum_record/forks_482.py.
От чего зависит: v4live_rpc.py (sel, w, ad, word), curve_plain_pools.py (call), curve_crypto_rpc.py (keccak256).
Ошибок-исключений нет, кроме сетевых и StaleState (stdhttp.py) — их ловит recnode.fresh в forks_482.py.

Подписи (официальные исходники, Sourcify 42161, 20.11.2026): UniswapV3Pool (Rubicon 0xee57…700f, Arbidex V3 0x8378…d592 —
  `match`): slot0(), liquidity(), fee(), tickSpacing(), ticks(int24) — первые два слова liquidityGross, liquidityNet
  (Tick.sol struct Info), tickBitmap(int16); фабрика getPool(address,address,uint24); TickLens (Uniswap v3-periphery
  contracts/lens/TickLens.sol): getPopulatedTicksInWord(address,int16) → (int24,int128,uint128)[]; QuoterV2 Rubicon
  0x2206…284C (`match`, QuoterV2): quoteExactInputSingle((address,address,uint256,uint24,uint160)).
"""

from curve_crypto_rpc import keccak256
from curve_plain_pools import call
from v4live_rpc import ad, sel, w, word

VQ_ADDR = "0x" + keccak256(b"arb.virtual_quoter").hex()[-40:]
FIELDS = ["factory()", "token0()", "token1()", "fee()", "tickSpacing()", "slot0()", "liquidity()"]
LENS_SIG = "getPopulatedTicksInWord(address,int16)"
QV2_SIG = "quoteExactInputSingle((address,address,uint256,uint24,uint160))"
VQ_SIG = "quoteExactIn(address,bool,uint256)"


def i24(v):
    """Слово ABI → int24 (дополнение до двух)."""
    return v - (1 << 256) if v >> 255 else v


def sw(v):
    """Знаковое число → слово ABI (дополнение до двух)."""
    return w(v % (1 << 256))


def state_reqs(dex, pool, tokens, tag):
    """Запросы B1: поля пула, getPool фабрики, balanceOf(pool) обоих токенов. tokens — (token0, token1, fee)."""
    reqs = [call(pool, "0x" + sel(s), tag) for s in FIELDS]
    t0, t1, fee = tokens
    reqs.append(call(dex["factory"], "0x" + sel("getPool(address,address,uint24)") + ad(t0) + ad(t1) + w(fee), tag))
    reqs += [call(t, "0x" + sel("balanceOf(address)") + ad(pool), tag) for t in (t0, t1)]
    return reqs


def parse_state(got):
    """Ответы B1 → словарь сырых ответов (имя → {"result"} / {"error"})."""
    names = FIELDS + ["getPool", "balance0", "balance1"]
    return dict(zip(names, got))


def words_around(st, spread):
    """Слова битмапа: текущее ± spread (как dex.tick_words загрузчика бота). None — нет slot0 / tickSpacing."""
    s0, ts = st["slot0()"].get("result"), st["tickSpacing()"].get("result")
    if not s0 or not ts:
        return None
    tick, spacing = i24(word(s0, 1)), i24(word(ts, 0))
    cur = (tick // spacing) >> 8
    return list(range(cur - spread, cur + spread + 1))


def quote_reqs(dex, pool, tokens, st, fractions, vq_code, tag):
    """Запросы B2: TickLens DEX и Uniswap по словам, QuoterV2 (если есть), виртуальный квотер. Возвращает: (запросы,
    метки) — метка ("lens", имя, слово) или ("q", zeroForOne, amountIn строкой, вид)."""
    reqs, marks = [], []
    t0, t1, fee = tokens
    for name, lens in (("dex", dex.get("tick_lens")), ("uniswap", dex["uniswap_lens"])):
        if not lens:
            continue
        for wd in words_around(st, dex["words"]) or []:
            reqs.append(call(lens, "0x" + sel(LENS_SIG) + ad(pool) + sw(wd), tag))
            marks.append(("lens", name, wd))
    bal = [int(st[k].get("result") or "0x0", 16) for k in ("balance0", "balance1")]
    for zfo in (True, False):
        have = bal[0] if zfo else bal[1]
        tin, tout = (t0, t1) if zfo else (t1, t0)
        for f in fractions:
            amt = max(1, int(have * f))
            if dex.get("quoter"):
                d = "0x" + sel(QV2_SIG) + ad(tin) + ad(tout) + w(amt) + w(fee) + w(0)
                reqs.append(call(dex["quoter"], d, tag))
                marks.append(("q", zfo, str(amt), "quoterV2"))
            d = "0x" + sel(VQ_SIG) + ad(pool) + w(int(zfo)) + w(amt)
            reqs.append(("eth_call", [{"to": VQ_ADDR, "data": d}, tag, {VQ_ADDR: {"code": vq_code}}]))
            marks.append(("q", zfo, str(amt), "virtual"))
    return reqs, marks


def lens_ticks(raw):
    """Тики ответа TickLens (массив кортежей (int24, int128, uint128)). Пусто — откат или пустое слово."""
    if not raw:
        return []
    n = word(raw, 1)
    return [i24(word(raw, 2 + 3 * k)) for k in range(n)]


def ticks_reqs(pool, ticks, tag):
    """Запросы B3: ticks(t) самого пула по тикам ответов TickLens."""
    return [call(pool, "0x" + sel("ticks(int24)") + sw(t), tag) for t in ticks]
