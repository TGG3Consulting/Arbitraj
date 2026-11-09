"""Файл: docs/notes/scripts/reserve_analysis/pools.py
Что делает: относительное изменение цены маршрута DEX между блоком котировки и целевым блоком — по событиям пулов
с узла: r = (выход за единицу входа в целевом блоке) / (то же в блоке котировки), по каждому хопу в направлении
обмена, произведение по хопам доли, взвешенное по expected_out долей маршрута.
Кто вызывает: run.py.
От чего зависит: node.py (Pruned — блок старше истории узла), keccak.py (TOPICS), common.py.

Факты кода бота, на которых стоит разбор:
- маршрут котировки — core/types/cost.rs DexRoute {mode, legs[{share, hops[{family, kind, pool}], expected_out}], block}:
  block — «блок, по состоянию которого посчитан маршрут»; PoolRef — {"Contract": адрес} | {"Singleton": {manager, id}} |
  {"Member": …} (core/types/pool.rs);
- V2 (PoolKind V2 = Proto::V2 у всех вариантов dex.kind, pools/families.rs spec): Sync(uint112 reserve0, uint112 reserve1)
  после каждого mint/burn/swap/sync — новые резервы целиком (pools/events_v2.rs, UniswapV2Pair.sol#L73-L86);
- V3 (PancakeV3, UniswapV3, Slipstream — families.rs spec): Swap со значениями после обмена, sqrtPriceX96 — 3-е слово
  данных (indexed sender, recipient — в topics; pools/events_v3.rs, abi_v2v3.rs); Mint/Burn цену не меняют;
- CL в синглтоне (Infinity CL, Uniswap V4): Swap(bytes32 indexed id, address indexed sender, int128, int128, uint160
  sqrtPriceX96, …) у менеджера — 3-е слово данных (abi_v4.rs); topic1 = PoolId;
- порядок токенов: token0 < token1 по адресу «во всех видах пулов» (pools/discover.rs:100 sorted); у пула-контракта
  token0()/token1() читаются eth_call (ответ узла — факт);
- цена sqrtPriceX96: (sqrtPriceX96 / 2^96)^2 = token1 за token0 (формула V3; в отношении двух цен масштаб сокращается).
Остальные виды (Bin, Pmm, Solidly, StableSwap, OraclePmm, TickBin, CL-контракт Algebra/Elastic) — «вид пула не
поддержан разбором»: их событие у бота есть, но разбор цены в этом скрипте не написан (не гадаем).
"""

from decimal import Decimal

from common import D
from keccak import TOPICS
from node import Pruned

SUPPORTED = "V2, V3 (PancakeSwap/Uniswap/Slipstream), CL в синглтоне (Infinity CL, Uniswap V4)"


class Unsupported(Exception):
    """Эпизод/сделку нельзя посчитать: причина текстом (вид пула, нет базы, нет токенов…)."""


def hop_spec(hop):
    """Хоп маршрута → (вид разбора, адрес для getLogs, topics фильтра, PoolId или None).
    Ошибки: Unsupported — вид не поддержан."""
    kind = hop.get("kind")
    pool = hop.get("pool") or {}
    if "Contract" in pool:
        addr = str(pool["Contract"]).lower()
        if kind == "V2":
            return "v2", addr, [[TOPICS["v2_sync"]]], None
        if kind == "V3":
            return "sq", addr, [[TOPICS["v3_swap_pancake"], TOPICS["v3_swap_uniswap"]]], None
        raise Unsupported("вид пула %s (контракт, %s) не поддержан разбором" % (kind, hop.get("family")))
    if "Singleton" in pool and kind == "Cl":
        s = pool["Singleton"]
        pid = str(s["id"]).lower()
        return "sq", str(s["manager"]).lower(), [[TOPICS["cl_swap_infinity"], TOPICS["cl_swap_uniswap_v4"]], [pid]], pid
    raise Unsupported("вид пула %s (%s, %s) не поддержан разбором" % (kind, list(pool)[:1], hop.get("family")))


def _word(data, i):
    h = data[2 + 64 * i: 2 + 64 * (i + 1)]
    if len(h) != 64:
        raise Unsupported("данные события короче %d слов" % (i + 1))
    return int(h, 16)


def state_of(tag, log):
    """Сжатый лог (блок, logIndex, topic1, data) → цена token1 за token0 (Decimal). Ошибки: Unsupported — ноль."""
    data = log[3]
    if tag == "v2":
        r0, r1 = _word(data, 0), _word(data, 1)
        if r0 == 0 or r1 == 0:
            raise Unsupported("резерв пула 0")
        return Decimal(r1) / Decimal(r0)
    s = Decimal(_word(data, 2))
    if s == 0:
        raise Unsupported("sqrtPriceX96 = 0")
    return s * s  # масштаб 2^192 сокращается в отношении


class HopTrack:
    """События одного пула около блока котировки: база (последнее событие ≤ b0) и события (b0, b_max]."""

    def __init__(self, node, spec, b0, b_max, lookback):
        self.tag, addr, topics, pid = spec
        self.node = node
        ch = node.chunk
        evs = []
        for k in range((b0 + 1) // ch, b_max // ch + 1):
            # Нужны ВСЕ события после котировки: окно старше истории узла (и не в кэше) — Pruned, не обрезка.
            got = node.chunk_logs(addr, topics, k, need_from=b0 + 1)
            evs.extend(e for e in got if b0 < e[0] <= b_max and (pid is None or e[2] == pid))
        self.after = sorted(evs)
        self.base = None
        self.base_err = None
        if self.after:
            k = b0 // ch
            stop = max(0, b0 - lookback)
            while k >= 0 and k * ch + ch - 1 >= stop:
                try:
                    got = node.chunk_logs(addr, topics, k)
                except Pruned:
                    # История узла кончилась раньше, чем нашлась база: цена в блоке котировки неизвестна.
                    self.base_err = "нет события пула от earliest узла до блока котировки (earliest = %d)" % node.earliest
                    break
                cand = [e for e in got if e[0] <= b0 and (pid is None or e[2] == pid)]
                if cand:
                    self.base = max(cand)
                    break
                k -= 1
            if self.base is None and self.base_err is None:
                self.base_err = "нет события пула не раньше %d блоков до блока котировки — цена в блоке котировки неизвестна" % lookback

    def ratio(self, b1, token_in_is_0):
        """Отношение цен «выход за вход» в блоке b1 к блоку b0. Нет событий в (b0, b1] — ровно 1 (пул не менялся).
        Ошибки: Unsupported — нет базы."""
        upto = [e for e in self.after if e[0] <= b1]
        if not upto:
            return Decimal(1), 0
        if self.base is None:
            raise Unsupported(self.base_err)
        p0, p1 = state_of(self.tag, self.base), state_of(self.tag, upto[-1])
        r = p1 / p0
        return (r if token_in_is_0 else 1 / r), len(upto)


def route_ratio(node, quote, start, end, b0, targets, lookback):
    """Изменение цены маршрута котировки DEX quote от блока b0 до каждого блока из targets.
    start/end — адреса токена входа и выхода маршрута (строчные). Возвращает: ({b1: r}, {b1: событий}, видов хопов).
    Ошибки: Unsupported (вид пула, токены, нет базы)."""
    route = quote.get("route") or {}
    legs = route.get("legs") or []
    if not legs:
        raise Unsupported("в котировке нет маршрута")
    b_max = max(targets)
    num = {b: Decimal(0) for b in targets}
    cnt = {b: 0 for b in targets}
    den = Decimal(0)
    kinds = set()
    for leg in legs:
        w = D(leg.get("expected_out")) or Decimal(0)
        if w <= 0:
            w = D(leg.get("share")) or Decimal(0)
        tok = start
        r_leg = {b: Decimal(1) for b in targets}
        hops = leg.get("hops") or []
        for i, hop in enumerate(hops):
            spec = hop_spec(hop)
            kinds.add("%s:%s" % (hop.get("kind"), hop.get("family")))
            if spec[3] is None:
                t0, t1 = node.tokens(spec[1])
            else:
                if i != len(hops) - 1 or not end:
                    raise Unsupported("пул-синглтон в середине доли — токены хопа неизвестны")
                t0, t1 = sorted([tok, end])
            if tok not in (t0, t1):
                raise Unsupported("токен %s не из пула %s (%s/%s)" % (tok, spec[1], t0, t1))
            out = t1 if tok == t0 else t0
            tr = HopTrack(node, spec, b0, b_max, lookback)
            for b in targets:
                r, n = tr.ratio(b, tok == t0)
                r_leg[b] *= r
                cnt[b] += n
            tok = out
        if end and tok != end:
            raise Unsupported("доля маршрута кончается на %s, ожидался %s" % (tok, end))
        for b in targets:
            num[b] += w * r_leg[b]
        den += w
    if den <= 0:
        raise Unsupported("веса долей маршрута нулевые")
    return {b: num[b] / den for b in targets}, cnt, sorted(kinds)


def ends(quote, coin, side):
    """Токены входа и выхода маршрута по шагам затрат котировки (Cost.step from_token/to_token — quote/steps.rs).
    side: 'sell' — монета → USDT, 'buy' — USDT → монета. Возвращает: (start, end). Ошибки: Unsupported."""
    steps = [c.get("step") for c in (quote.get("costs") or []) if c.get("step")]
    if not coin:
        raise Unsupported("у монеты нет адреса BSC")
    coin = coin.lower()
    if not steps:
        raise Unsupported("в котировке нет шагов маршрута с адресами токенов")
    first = min(steps, key=lambda s: s["index"])
    last = max(steps, key=lambda s: s["index"])
    a = (first.get("from_token") or "").lower() or None
    b = (last.get("to_token") or "").lower() or None
    if side == "sell":
        if a and a != coin:
            raise Unsupported("первый шаг продажи не из монеты (%s)" % a)
        return coin, b
    if b and b != coin:
        raise Unsupported("последний шаг покупки не в монету (%s)" % b)
    return a, coin
