#!/usr/bin/env python3
"""Запись ответов своего узла Arbitrum One для решения 450 (задачи T448-01…05).

Что делает: на ОДНОМ блоке B читает (решение 460 п. 9: сначала поиск пулов без скана истории — sub_dex_find.py:
перечисление фабрик с конца списка, GeckoTerminal, реестр по парам; событие создания — только узким окном вокруг
блока создания пула; потом свежий блок B и всё состояние на нём — узел с короткой историей состояния, решение 460):
  1) KyberSwap Classic (T448-04): пулы фабрик DMM / KSFactory / KSFactory Legacy — хвост allPools(i) и пулы
     GeckoTerminal kyberswap_classic_arbitrum со своей factory(); событие создания (все три подписи PoolCreated) —
     окном вокруг pool_created_at GeckoTerminal; у пулов — factory(), token0(), token1(), ampBps(), getTradeInfo(),
     реестр фабрики getPools(token0, token1) и isPool, ответы роутера своей фабрики getAmountsOut(сумма, [пул], [вход,
     выход]) лесенкой сумм в обе стороны (доли реального резерва входа). Пул с событием — в "pools" (тест читает
     p["log"]), без события — в "pools_no_log" с причиной;
  2) Balancer V2 (T448-03): пулы GeckoTerminal balancer_arbitrum, фабрика — та из --balancer-factory, чей
     isPoolFromFactory = 1; у пулов — getVault, getPoolId, isPoolFromFactory у ВСЕХ фабрик списка, Vault.getPool /
     getPoolTokens, getSwapFeePercentage, getPausedState пула и Vault, getNormalizedWeights или
     getAmplificationParameter, decimals токенов и ответы Vault.queryBatchSwap GIVEN_IN (одиночный обмен token_i →
     token_j лесенкой долей баланса входа);
  3) iZiSwap (T448-05): fee2pointDelta(fee) фабрики по --izi-fee, пулы реестра pool(x, y, fee) по парам токенов
     (--izi-token и токены пулов сети GeckoTerminal), у пулов — tokenX, tokenY, fee, factory и ответ реестра
     pool(tokenX, tokenY, fee) в обоих порядках токенов;
  4) Universal Router PancakeSwap v3 (T448-01): eth_getCode роутера — длина и keccak256 (сверка с Sourcify).
Потолок времени --time-limit (решение 460 п. 9, по умолчанию 600 с): вышел во время поиска — поиск останавливается, состояние
найденного пишется, причина — в block.json и README записи, код выхода 3 (последняя строка — причина).
Пишет JSON-файлы и README (блок, время, хеш блока, SHA-256 файлов) в tests/data/arbitrum_sub_dex_<дата>/. Только чтение
(eth_blockNumber, eth_getBlockByNumber, eth_getLogs, eth_call, eth_getCode) и GeckoTerminal — ключей, подписей и
транзакций нет.

Запуск (Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459)), адреса — docs/notes/arbitrum/dex-settings-arbitrum.md (официальные источники):
    python3 tools/arbitrum_record/sub_dex.py --node http://АДРЕС_УЗЛА:8547 --max-pools 20 \\
        --kyber DMM:0xD9bfE9979e9CA4b2fe84bA5d4Cf963bBcB376974:0xEaE47c5D99f7B31165a7f0c5f7E0D6afA25CFd55 \\
        --kyber KS:0x1c758aF0688502e49140230F6b0EBd376d429be5:0x5649B4DD00780e99Bab7Abb4A3d581Ea1aEB23D0 \\
        --kyber KS_LEGACY:0x51E8D106C646cA58Caf32A47812e95887C071a62:0xC3E2aED41ECdFB1ad41ED20D45377Da98D5489dD \\
        --vault 0xBA12222222228d8Ba445958a75a0704d566BF2C8 \\
        --balancer-factory w1:v1:0x7dFdEF5f355096603419239CE743BfaF1120312B \\
        --balancer-factory w2t:v1:0xCF0a32Bbef8F064969F21f7e02328FB577382018 \\
        --balancer-factory s1:v1:0x2433477A10FC5d31B9513C638F19eE85CaED53Fd \\
        --balancer-factory wv2:v2:0x8df6EfEc5547e31B0eb7d1291B511FF8a2bf987c \\
        --balancer-factory sv2:v2:0xEF44D6786b2b4d544b7850Fe67CE6381626Bf2D6 \\
        --balancer-factory wv3:v2:0xf1665E19bc105BE4EDD3739F88315cC699cc5b65 \\
        --balancer-factory wv4:v2:0xc7E5ED1054A24Ef31D827E6F86caA58B3Bc168d7 \\
        --izi-factory 0xCFD8A067e1fa03474e79Be646c5f6b6A27847399 --izi-fee 100 --izi-fee 400 --izi-fee 2000 \\
        --izi-fee 10000 --pancake-ur 0xFE6508f0015C778Bdcc1fB5465bA5ebE224C9912
--max-pools — сколько пулов каждой фабрики (у KyberSwap Classic — до --max-pools из GeckoTerminal и до --max-pools
с конца allPools). Запись 20.11.2026: за 2 000 000 блоков до головы событий создания не было (пулов 0); затем поиск
назад окнами (решение 460) за 37 мин прошёл ~144 млн блоков из ~526 млн — заменён (решение 460 п. 9).
"""

import argparse
import datetime
import hashlib
import json
import os
import sys

import recnode  # решение 460: свежий блок состояния; п. 9: потолок времени
import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests
import sub_dex_find  # решение 460 п. 9: пулы без скана истории

# --- keccak256 (чистый Python: в stdlib только sha3_256 с другим дополнением) ---------------------------------------

_RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
]
_ROT = [
    [0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14],
]
_M = (1 << 64) - 1


def _rol(x, n):
    return ((x << n) | (x >> (64 - n))) & _M if n else x


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


def keccak256(data: bytes) -> bytes:
    rate = 136
    msg = bytearray(data) + b"\x01"
    while len(msg) % rate:
        msg += b"\x00"
    msg[-1] |= 0x80
    a = [[0] * 5 for _ in range(5)]
    for off in range(0, len(msg), rate):
        block = msg[off:off + rate]
        for i in range(rate // 8):
            x, y = i % 5, i // 5
            a[x][y] ^= int.from_bytes(block[8 * i:8 * i + 8], "little")
        a = _f(a)
    out = b""
    for i in range(4):
        out += a[i % 5][i // 5].to_bytes(8, "little")
    return out


def sel(sig: str) -> str:
    """Селектор функции: первые 4 байта keccak256 подписи (hex без 0x)."""
    return keccak256(sig.encode()).hex()[:8]


def topic(sig: str) -> str:
    """topic0 события: keccak256 подписи."""
    return "0x" + keccak256(sig.encode()).hex()


# --- ABI: только нужные здесь типы ---------------------------------------------------------------------------------

def w_uint(v: int) -> str:
    return format(v % (1 << 256), "064x")


def w_addr(a: str) -> str:
    return a.lower().replace("0x", "").rjust(64, "0")


def words(hexdata: str):
    h = hexdata[2:] if hexdata.startswith("0x") else hexdata
    return [h[i:i + 64] for i in range(0, len(h), 64)]


def r_addr(w: str) -> str:
    return "0x" + w[24:]


def r_int(w: str, signed=False) -> int:
    v = int(w, 16)
    if signed and v >= 1 << 255:
        v -= 1 << 256
    return v


def r_dyn_list(hexdata: str, head_index: int):
    """Динамический массив слов: смещение в слове head_index. Возвращает список слов."""
    ws = words(hexdata)
    off = int(ws[head_index], 16) // 32
    n = int(ws[off], 16)
    return ws[off + 1:off + 1 + n]


# --- узел ------------------------------------------------------------------------------------------------------------

class Node:
    def __init__(self, url):
        self.url = url
        self.n = 0

    def rpc(self, method, params, timeout=120):
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=timeout)
        r.raise_for_status()
        return r.json()

    def call(self, to, data, tag):
        """eth_call → {"ok": hex} или {"revert": текст/данные} (откат — тоже ответ)."""
        j = self.rpc("eth_call", [{"to": to, "data": "0x" + data}, tag])
        if "error" in j:
            return {"revert": j["error"]}
        return {"ok": j["result"]}


def ok(r):
    return r.get("ok")


# --- KyberSwap Classic -----------------------------------------------------------------------------------------------

KYBER_CREATED = [
    "PoolCreated(address,address,address,uint32,uint256)",          # DMMFactory.sol L22-L28
    "PoolCreated(address,address,address,uint32,uint24,uint256)",   # KSFactory.sol L26-L33
    "PoolCreated(address,address,address,uint32,uint16,uint256)",   # KSFactory Legacy (Sourcify 42161 0x51E8…)
]


def kyber_pool(node, tag, factory, router, pool, t0, t1, args):
    """Состояние пула KyberSwap Classic на блоке tag и ответы роутера лесенкой сумм. Возвращает словарь пула."""
    p = {"pool": pool, "token0": t0, "token1": t1}
    p["factory"] = node.call(pool, sel("factory()"), tag)
    p["token0_call"] = node.call(pool, sel("token0()"), tag)
    p["token1_call"] = node.call(pool, sel("token1()"), tag)
    p["ampBps"] = node.call(pool, sel("ampBps()"), tag)
    p["getTradeInfo"] = node.call(pool, sel("getTradeInfo()"), tag)
    p["getPools"] = node.call(factory, sel("getPools(address,address)") + w_addr(t0) + w_addr(t1), tag)
    p["isPool"] = node.call(factory, sel("isPool(address,address,address)") + w_addr(t0) + w_addr(t1)
                            + w_addr(pool), tag)
    ti = ok(p["getTradeInfo"])
    quotes = []
    if ti:
        ws = words(ti)
        r0, r1 = r_int(ws[0]), r_int(ws[1])
        for (tin, tout, rin) in ((t0, t1, r0), (t1, t0, r1)):
            for den in args.ladder:
                amt = rin // den
                if amt == 0:
                    continue
                data = (sel("getAmountsOut(uint256,address[],address[])") + w_uint(amt) + w_uint(96)
                        + w_uint(160) + w_uint(1) + w_addr(pool) + w_uint(2) + w_addr(tin) + w_addr(tout))
                quotes.append({"token_in": tin, "token_out": tout, "amount_in": str(amt),
                               "answer": node.call(router, data, tag)})
    p["getAmountsOut"] = quotes
    return p


def kyber(node, tag, found, args):
    """KyberSwap Classic на блоке tag по пулам sub_dex_find.kyber. Пул с событием создания — в "pools" (тест читает
    p["log"]; token0/token1 — из события), без события — в "pools_no_log" (token0/token1 — ответы пула на tag,
    "log_search" — почему события нет). Возвращает словарь kyber.json."""
    out = []
    topics = [topic(s) for s in KYBER_CREATED]
    for spec in args.kyber:
        name, factory, router = spec.split(":")
        got = found.get(spec) or {"length": None, "pools": []}
        pools, no_log = [], []
        for f in got["pools"]:
            lg = (f.get("log_search") or {}).get("log")
            src = {k: f[k] for k in ("index", "gt") if k in f}
            if lg:
                t0, t1 = r_addr(lg["topics"][1][2:]), r_addr(lg["topics"][2][2:])
                p = kyber_pool(node, tag, factory, router, f["pool"], t0, t1, args)
                p.update(log=lg, log_search={k: v for k, v in f["log_search"].items() if k != "log"}, source=src)
                pools.append(p)
                continue
            c0, c1 = ok(node.call(f["pool"], sel("token0()"), tag)), ok(node.call(f["pool"], sel("token1()"), tag))
            if not (c0 and c1 and len(c0) >= 66 and len(c1) >= 66):
                no_log.append({"pool": f["pool"], "source": src, "log_search": f.get("log_search"),
                               "why": "token0()/token1() пула не ответили"})
                continue
            p = kyber_pool(node, tag, factory, router, f["pool"], r_addr(words(c0)[0]), r_addr(words(c1)[0]), args)
            p.update(log_search=f.get("log_search"), source=src)
            no_log.append(p)
        out.append({"name": name, "factory": factory, "router": router, "allPoolsLength": got["length"],
                    "pools": pools, "pools_no_log": no_log})
    return {"created_topics": dict(zip(KYBER_CREATED, topics)), "factories": out,
            "geckoterminal": found.get("gt"), "stopped": found.get("stopped")}


# --- Balancer V2 -----------------------------------------------------------------------------------------------------

def query_batch_swap(pool_id, i, j, amount, assets, me):
    """queryBatchSwap(uint8,(bytes32,uint256,uint256,uint256,bytes)[],address[],(address,bool,address,bool)) —
    IVault.sol (balancer-deployments 20210418-vault build-info): GIVEN_IN = 0, один шаг, userData пусто."""
    sig = "queryBatchSwap(uint8,(bytes32,uint256,uint256,uint256,bytes)[],address[],(address,bool,address,bool))"
    head_len = 32 * (1 + 1 + 1 + 4)
    swaps_off = head_len
    step = w_uint(32) + pool_id[2:] + w_uint(i) + w_uint(j) + w_uint(amount) + w_uint(160) + w_uint(0)
    swaps = w_uint(1) + step
    assets_off = swaps_off + len(swaps) // 2
    assets_enc = w_uint(len(assets)) + "".join(w_addr(a) for a in assets)
    return (sel(sig) + w_uint(0) + w_uint(swaps_off) + w_uint(assets_off) + w_addr(me) + w_uint(0) + w_addr(me)
            + w_uint(0) + swaps + assets_enc)


def balancer(node, tag, found, args):
    """Balancer V2 на блоке tag по пулам sub_dex_find.balancer (решение 460 п. 9). Возвращает словарь balancer.json."""
    facs = [f.split(":") for f in args.balancer_factory]
    me = "0x" + "00" * 19 + "01"
    out = []
    for spec, (name, math, factory) in zip(args.balancer_factory, facs):
        pools = []
        for pool in (found.get(spec) or {}).get("pools", []):
            p = {"pool": pool, "factory_key": name, "math": math}
            p["getVault"] = node.call(pool, sel("getVault()"), tag)
            p["getPoolId"] = node.call(pool, sel("getPoolId()"), tag)
            p["isPoolFromFactory"] = {n: node.call(f, sel("isPoolFromFactory(address)") + w_addr(pool), tag)
                                      for n, _, f in facs}
            pid = ok(p["getPoolId"])
            if not pid:
                pools.append(p)
                continue
            p["getPool"] = node.call(args.vault, sel("getPool(bytes32)") + pid[2:], tag)
            pt = node.call(args.vault, sel("getPoolTokens(bytes32)") + pid[2:], tag)
            p["getPoolTokens"] = pt
            p["getSwapFeePercentage"] = node.call(pool, sel("getSwapFeePercentage()"), tag)
            p["getPausedState"] = node.call(pool, sel("getPausedState()"), tag)
            p["vault_getPausedState"] = node.call(args.vault, sel("getPausedState()"), tag)
            p["getNormalizedWeights"] = node.call(pool, sel("getNormalizedWeights()"), tag)
            p["getAmplificationParameter"] = node.call(pool, sel("getAmplificationParameter()"), tag)
            if not ok(pt):
                pools.append(p)
                continue
            tokens = [r_addr(w) for w in r_dyn_list(ok(pt), 0)]
            bals = [r_int(w) for w in r_dyn_list(ok(pt), 1)]
            p["decimals"] = {t: node.call(t, sel("decimals()"), tag) for t in tokens}
            qs = []
            for i in range(len(tokens)):
                for j in range(len(tokens)):
                    if i == j or tokens[i] == pool or tokens[j] == pool:
                        continue
                    for den in args.ladder:
                        amt = bals[i] // den
                        if amt == 0:
                            continue
                        qs.append({"i": i, "j": j, "amount": str(amt),
                                   "answer": node.call(args.vault, query_batch_swap(pid, i, j, amt, tokens, me), tag)})
            p["queryBatchSwap"] = qs
            pools.append(p)
        out.append({"name": name, "math": math, "factory": factory, "pools": pools})
    return {"vault": args.vault, "factories": out, "skipped": found.get("skipped"), "geckoterminal": found.get("gt"),
            "stopped": found.get("stopped")}


# --- iZiSwap ---------------------------------------------------------------------------------------------------------

def izi(node, tag, found, args):
    """iZiSwap на блоке tag по пулам реестра (sub_dex_find.izi, решение 460 п. 9). Возвращает словарь izi.json."""
    f = args.izi_factory
    deltas = {str(fee): node.call(f, sel("fee2pointDelta(uint24)") + w_uint(fee), tag) for fee in args.izi_fee}
    pools = []
    for pool in found.get("pools", []):
        p = {"pool": pool}
        if pool:
            for g in ("tokenX()", "tokenY()", "fee()", "factory()"):
                p[g] = node.call(pool, sel(g), tag)
            x, y, fee = ok(p["tokenX()"]), ok(p["tokenY()"]), ok(p["fee()"])
            if x and y and fee:
                ax, ay, fv = r_addr(words(x)[0]), r_addr(words(y)[0]), r_int(words(fee)[0])
                p["pool_xy"] = node.call(f, sel("pool(address,address,uint24)") + w_addr(ax) + w_addr(ay) + w_uint(fv), tag)
                p["pool_yx"] = node.call(f, sel("pool(address,address,uint24)") + w_addr(ay) + w_addr(ax) + w_uint(fv), tag)
        pools.append(p)
    return {"factory": f, "fee2pointDelta": deltas, "tokens": found.get("tokens"), "asked": found.get("asked"),
            "geckoterminal": found.get("gt"), "stopped": found.get("stopped"), "pools": pools}


def state_phase(node, head, found, args):
    """Решение 460: всё чтение состояния на одном блоке head (свежем, после поиска событий). Возвращает: (head, блок
    узла, файлы записи без block.json). Ошибки: StaleState (recnode.fresh повторит на новом блоке)."""
    tag = hex(head)
    blk = node.rpc("eth_getBlockByNumber", [tag, False])["result"]
    print("блок состояния B", head, file=sys.stderr, flush=True)
    files = {}
    if found["kyber"] is not None:
        files["kyber.json"] = kyber(node, tag, found["kyber"], args)
    if found["balancer"] is not None:
        files["balancer.json"] = balancer(node, tag, found["balancer"], args)
    if found["izi"] is not None:
        files["izi.json"] = izi(node, tag, found["izi"], args)
    if args.pancake_ur:
        j = node.rpc("eth_getCode", [args.pancake_ur, tag])
        code = j.get("result", "0x")
        files["pancake_ur.json"] = {"address": args.pancake_ur, "code_len": (len(code) - 2) // 2,
                                    "code_keccak": "0x" + keccak256(bytes.fromhex(code[2:])).hex()}
    return head, blk, files


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--max-pools", type=int, required=True,
                    help="пулов на фабрику (KyberSwap Classic: до N из GeckoTerminal и до N с конца allPools)")
    ap.add_argument("--time-limit", type=int, default=600,
                    help="потолок времени поиска пулов, с (решение 460 п. 9; 0 — без потолка): вышел — пишется найденное")
    ap.add_argument("--gt-pages", type=int, default=3,
                    help="страниц пулов GeckoTerminal kyberswap_classic_arbitrum и balancer_arbitrum (0 — не брать)")
    ap.add_argument("--created-margin", type=int, action="append", default=None,
                    help="окно поиска события создания KyberSwap Classic: ± секунд от pool_created_at GeckoTerminal "
                         "(повторять — окна по очереди; по умолчанию 3600 и 86400)")
    ap.add_argument("--ladder", type=int, action="append", default=None,
                    help="делители суммы входа: вход = резерв/баланс входа ÷ делитель (повторять; по умолчанию 10000 1000 100)")
    ap.add_argument("--kyber", action="append", default=[], help="ИМЯ:фабрика:роутер")
    ap.add_argument("--vault", default=None)
    ap.add_argument("--balancer-factory", action="append", default=[], help="ИМЯ:v1|v2:фабрика")
    ap.add_argument("--izi-factory", default=None)
    ap.add_argument("--izi-fee", type=int, action="append", default=[])
    ap.add_argument("--izi-token", action="append", default=[], help="токен для реестра iZiSwap (повторять)")
    ap.add_argument("--izi-tokens", type=int, default=12,
                    help="сколько токенов пулов сети GeckoTerminal взять для пар реестра iZiSwap")
    ap.add_argument("--izi-gt-pages", type=int, default=1, help="страниц /networks/arbitrum/pools GeckoTerminal")
    ap.add_argument("--pancake-ur", default=None)
    args = ap.parse_args()
    args.ladder = args.ladder or [10000, 1000, 100]
    args.created_margin = args.created_margin or [3600, 86400]
    node = Node(args.node)
    if args.balancer_factory and not args.vault:
        sys.exit("--vault обязателен с --balancer-factory")
    say = lambda t: print(t, file=sys.stderr, flush=True)  # noqa: E731
    # Решение 460 п. 9: поиск пулов без скана истории (sub_dex_find.py) под потолком времени; потом свежий блок B и всё
    # состояние на нём (решение 460, recnode.fresh: состояние B удалено узлом — заново на новом блоке).
    dl = recnode.Deadline(args.time_limit)
    b0 = int(node.rpc("eth_blockNumber", [])["result"], 16)
    created = [topic(s) for s in KYBER_CREATED]
    found = {"kyber": sub_dex_find.run(sub_dex_find.kyber, {}, node, b0, args, dl, sel, created, say)
             if args.kyber else None,
             "balancer": sub_dex_find.run(sub_dex_find.balancer, {}, node, args, dl, sel)
             if args.balancer_factory else None,
             "izi": sub_dex_find.run(sub_dex_find.izi, {}, node, args, dl, sel) if args.izi_factory else None}
    stopped = sorted({v["stopped"] for v in found.values() if v and v.get("stopped")})
    for why in stopped:
        say(f"поиск пулов остановлен: {why}")
    head, blk, files = recnode.fresh(lambda: int(node.rpc("eth_blockNumber", [])["result"], 16),
                                     lambda h: state_phase(node, h, found, args))
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    root = os.path.join(os.path.dirname(__file__), "..", "..", "tests", "data", f"arbitrum_sub_dex_{day}")
    os.makedirs(root, exist_ok=True)
    meta = {"block": head, "hash": blk["hash"], "timestamp": int(blk["timestamp"], 16), "search_head": b0,
            "stopped": stopped, "args": vars(args)}
    files["block.json"] = meta
    sums = []
    for name, data in files.items():
        raw = json.dumps(data, indent=1, ensure_ascii=False).encode()
        with open(os.path.join(root, name), "wb") as fh:
            fh.write(raw)
        sums.append(f"| {name} | {hashlib.sha256(raw).hexdigest()} |")
    readme = [
        f"# arbitrum_sub_dex_{day} — запись tools/arbitrum_record/sub_dex.py (решения 450, 460 п. 9)",
        "",
        f"Блок {head} ({blk['hash']}), время блока {meta['timestamp']}. Только чтение (eth_call, eth_getLogs, eth_getCode,",
        "eth_getBlockByNumber) и GeckoTerminal. Пулы — без скана истории (решение 460 п. 9): KyberSwap Classic — хвост allPools и",
        "GeckoTerminal, событие создания — окном вокруг pool_created_at (kyber.json: pools — с событием, pools_no_log — без,",
        "с причиной); Balancer V2 — GeckoTerminal + isPoolFromFactory; iZiSwap — реестр pool(x, y, fee) по парам токенов.",
        ("Поиск пулов остановлен потолком времени: " + "; ".join(stopped) + "." if stopped
         else f"Поиск пулов уложился в потолок {args.time_limit} с."),
        "Тест — tests/arbitrum_sub_dex_live.rs (после записи снять #[ignore]).",
        "",
        "| Файл | SHA-256 |",
        "| --- | --- |",
    ] + sums
    with open(os.path.join(root, "README.md"), "w") as fh:
        fh.write("\n".join(readme) + "\n")
    print(root)
    if stopped:
        say("запись неполная — поиск пулов остановлен: " + "; ".join(stopped))
        sys.exit(3)


if __name__ == "__main__":
    main()
