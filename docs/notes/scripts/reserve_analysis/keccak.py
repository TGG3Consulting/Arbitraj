"""Файл: docs/notes/scripts/reserve_analysis/keccak.py
Что делает: keccak256 на чистом Python (в stdlib есть только SHA3-256 — другое дополнение, не keccak) и topic0 событий
пулов, посчитанные из ТЕХ ЖЕ сигнатур, что в коде бота (alloy::sol! считает SIGNATURE_HASH = keccak256(сигнатура)).
Самопроверка: посчитанные topic0 сверяются с topic0 из записанных ответов узла в tests/data (бот их получал от узла).
Кто вызывает: pools.py (TOPICS, SEL_TOKEN0/1), run.py (self_check).
От чего зависит: стандартная библиотека.

Сигнатуры событий (канонические типы ABI) — из кода бота:
- V2 Sync(uint112,uint112) — src/venues/bsc_dex/pools/abi_v2v3.rs, IV2Pair (Uniswap v2-core IUniswapV2Pair.sol, 6a9e7c97);
- PancakeSwap V3 Swap (9 полей) — abi_v2v3.rs IPancakeV3Pool (IPancakeV3PoolEvents.sol#L74-L84);
- Uniswap V3 Swap (7 полей) — abi_v2v3.rs IUniswapV3Pool (IUniswapV3PoolEvents.sol#L72-L80); Slipstream — тот же
  (events_v3.rs: «Slipstream — форк Uni V3, тот же topic0»);
- Infinity CL Swap — src/venues/bsc_dex/pools/abi_v4.rs IInfinityCl; Uniswap V4 Swap — abi_v4.rs IUniswapV4
  (PoolId = bytes32 indexed id).
"""

_ROT = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
_M = (1 << 64) - 1


def _rc():
    """24 константы раунда Keccak-f[1600] из LFSR x^8+x^6+x^5+x^4+1 (FIPS 202, алгоритм 5). Ошибок нет."""
    out = []
    r = 1
    bits = []
    for _ in range(255):
        bits.append(r & 1)
        r <<= 1
        if r & 0x100:
            r ^= 0x171
    for i in range(24):
        c = 0
        for j in range(7):
            if bits[(j + 7 * i) % 255]:
                c |= 1 << ((1 << j) - 1)
        out.append(c)
    return out


_RC = _rc()


def _rol(v, n):
    n %= 64
    return ((v << n) | (v >> (64 - n))) & _M


def _f(a):
    for rc in _RC:
        c = [a[x][0] ^ a[x][1] ^ a[x][2] ^ a[x][3] ^ a[x][4] for x in range(5)]
        d = [c[(x - 1) % 5] ^ _rol(c[(x + 1) % 5], 1) for x in range(5)]
        a = [[a[x][y] ^ d[x] for y in range(5)] for x in range(5)]
        b = [[0] * 5 for _ in range(5)]
        for x in range(5):
            for y in range(5):
                b[y][(2 * x + 3 * y) % 5] = _rol(a[x][y], _ROT[x][y])
        a = [[b[x][y] ^ ((~b[(x + 1) % 5][y]) & b[(x + 2) % 5][y]) for y in range(5)] for x in range(5)]
        a[0][0] ^= rc
    return a


def keccak256(data):
    """Принимает: bytes. Возвращает: 32 байта keccak256 (дополнение 0x01…0x80, rate 136). Ошибок нет."""
    rate = 136
    msg = bytearray(data)
    msg.append(0x01)
    while len(msg) % rate:
        msg.append(0)
    msg[-1] |= 0x80
    a = [[0] * 5 for _ in range(5)]
    for off in range(0, len(msg), rate):
        block = msg[off:off + rate]
        for i in range(rate // 8):
            x, y = i % 5, i // 5
            a[x][y] ^= int.from_bytes(block[8 * i:8 * i + 8], "little")
        a = _f(a)
    out = b"".join(a[i % 5][i // 5].to_bytes(8, "little") for i in range(4))
    return out


def topic(sig):
    """topic0 события: '0x' + keccak256(сигнатура). Ошибок нет."""
    return "0x" + keccak256(sig.encode()).hex()


def selector(sig):
    """Селектор функции: первые 4 байта keccak256(сигнатура), '0x…'. Ошибок нет."""
    return "0x" + keccak256(sig.encode())[:4].hex()


SIG = {
    "v2_sync": "Sync(uint112,uint112)",
    "v3_swap_pancake": "Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)",
    "v3_swap_uniswap": "Swap(address,address,int256,int256,uint160,uint128,int24)",
    "cl_swap_infinity": "Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24,uint16)",
    "cl_swap_uniswap_v4": "Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)",
}
TOPICS = {k: topic(v) for k, v in SIG.items()}
SEL_TOKEN0 = selector("token0()")
SEL_TOKEN1 = selector("token1()")

# topic0, которые бот реально получал от узла (записанные ответы в репозитории) — для самопроверки keccak.
RECORDED = {
    "v2_sync": ("0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1",
                "tests/data/receipts_2026-10-25/responses.json"),
    "v3_swap_pancake": ("0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83",
                        "tests/data/indexers_2026-10-24/arb_node_responses_2_2026-10-24.json"),
    "v3_swap_uniswap": ("0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67",
                        "tests/data/indexers_2026-10-24/arb_node_responses_2_2026-10-24.json"),
    "cl_swap_infinity": ("0x04206ad2b7c0f463bff3dd4f33c5735b0f2957a351e4f79763a4fa9e775dd237",
                         "tests/data/node_calls_2_2026-10-24.json"),
    "cl_swap_uniswap_v4": ("0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f",
                           "tests/data/ws_logs_2026-10-26/responses_mgr.json"),
}


def self_check():
    """Сверяет посчитанные topic0 с записанными. Возвращает: список расхождений (пусто — всё сошлось). Ошибок нет."""
    bad = []
    for k, (want, src) in RECORDED.items():
        if TOPICS[k] != want:
            bad.append("%s: посчитано %s, записано %s (%s)" % (k, TOPICS[k], want, src))
    return bad
