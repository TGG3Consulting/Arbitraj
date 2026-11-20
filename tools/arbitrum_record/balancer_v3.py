#!/usr/bin/env python3
# Файл: tools/arbitrum_record/balancer_v3.py
# Что делает: решения 441, 452 (T433-F2) — запись ответов своего узла Arbitrum для пулов Balancer V3 на ОДНОМ блоке (только
#   чтение, eth_call): все вызовы, которые бот может сделать к пулу (хранилище, isPoolFromFactory каждой фабрики из
#   списков, виды пулов — весовой, стейблсвап, Gyro 2-CLP / E-CLP, ReClamm, LBP, LBP с фиксированной ценой, QuantAMM;
#   крючок StableSurge / MEV Capture; прекомпайлы ArbSys / ArbOwnerPublic) — тест берёт их по (адрес, данные) в порядке
#   бота; и ответы Router V3 querySwapSingleTokenExactIn / ExactOut на несколько сумм по каждой паре токенов пула
#   (eth_call БЕЗ from — Vault.quote требует tx.origin = 0). Ключей и подписей нет.
# Кто вызывает: Тигран вручную (README ниже); тест — tests/arbitrum_balancer_v3.rs (сверяет расчёт бота с роутером).
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459). Адрес узла и все адреса контрактов — аргументы (официальные значения —
#   docs/FIRST_RUN.md, «DEX Balancer V3 на Arbitrum»: balancer/balancer-deployments@b55dc2b8 output/arbitrum.json,
#   QuantAMMProtocol/deployment@e7867165).
#
# Пример (списки — как в docs/FIRST_RUN.md; каждый --list — одна настройка):
#   python3 tools/arbitrum_record/balancer_v3.py --rpc http://127.0.0.1:8547 \
#     --vault 0xbA1333333333a1BA1108E8412f11850A5C319bA9 --router 0xEAedc32a51c510d35ebC11088fD5fF2b47aACF2E \
#     --list dex.bv3_weighted_factories=0xD961E30156C2E0D0d925A0De45f931CB7815e970 \
#     --list dex.bv3_weighted_min_factories=0x83bf399FA3DC49Af8fb5c34031a50c7C93F56129 \
#     --list dex.bv3_stable_factories=0xEB1eeaBF0126d813589C3D2CfeFFE410D9aE3863 \
#     --list dex.bv3_stable_open_factories=0x44d33798dddCdAbc93Fe6a40C80588033Dc502d3,0x86e67E115f96DF37239E0479441303De0de7bc2b,0x201efd508c8DfE9DE1a13c2452863A78CB2a86Cc \
#     --list dex.bv3_stable_guard_factories=0x1FC7F1F84CFE61a04224AC8D3F87f56214FeC08c,0x8A8B9f35765899B3a0291700141470D79EA2eA88 \
#     --list dex.bv3_gyro_2clp_factories=0x65A22Ec32c37835Ad5E77Eb6f7452Ac59E113a9F,0x64b7dfE8a3972639712EFA124D180DC3Ff0f5bcF \
#     --list dex.bv3_gyro_eclp_factories=0x88ED12A90142fDBFe2a28f7d5b48927254C7e760,0xe31715e75207acC8bfadd96902FF522058928479 \
#     --list dex.bv3_reclamm_factories=0xa1D0791a41318c775707C56eAe247AF81a05322C,0x355bD33F0033066BB3DE396a6d069be57353AD95 \
#     --list dex.bv3_reclamm_v3_factories=0x3ccD78683efFffdDc1A16f5553C896ac6D3ab7FF \
#     --list dex.bv3_lbp_v1_factories=0x4BB42f71CAB7Bd13e9f958dA4351B9fa2d3A42FF \
#     --list dex.bv3_lbp_v2_factories=0x8D217CB74f675B46cC2767C8cF0aAB53BE1c4818 \
#     --list dex.bv3_lbp_v3_factories=0xF9309a99836b5F07a2440c177C049b0f0A9A2c33 \
#     --list dex.bv3_lbp_v4_factories=0x6b7DEd11F15D6AB9959c29cEd3Cd101D69A14c35 \
#     --list dex.bv3_fixed_lbp_factories=0x7f246E7Bab4CdC8C7AB41EaDA8290009a5b26E0D \
#     --list dex.bv3_quantamm_factories=0x62B9eC6A5BBEBe4F5C5f46C8A8880df857004295 \
#     --list dex.bv3_stable_surge_hooks=0x0Fa0f9990D7969a7aE6f9961d663E4A201Ed6417,0x7c1b7A97BfAcD39975dE53e989A16c7BC4C78275 \
#     --list dex.bv3_mev_capture_hooks=0x5B42eC6D40f7B7965BE5308c70e2603c0281C1E9 --gt-pages 5
#   (пулы — с GeckoTerminal dex balancer-v3-arbitrum, страниц --gt-pages; или явно --pools a,b,c)

import argparse
import datetime
import hashlib
import json
import os
import sys

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests

# --- keccak256 (Keccak-f[1600], padding 0x01 — как в Ethereum; нужен только для селекторов функций) ---
_RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
]
_ROT = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
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
        x, y = i % 5, i // 5
        out += a[x][y].to_bytes(8, "little")
    return out


def sel(sig: str) -> str:
    return keccak256(sig.encode()).hex()[:8]


def word_addr(a: str) -> str:
    return a.lower().replace("0x", "").rjust(64, "0")


def word_int(n: int) -> str:
    return format(n, "064x")


# --- JSON-RPC ---
class Node:
    def __init__(self, url):
        self.url, self.n = url, 0

    def rpc(self, method, params):
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}, timeout=60)
        r.raise_for_status()
        return r.json()

    def call(self, to, data, block):
        """eth_call без from (tx.origin = 0). Возвращает запись: to, data, result или error (ответ узла как есть)."""
        j = self.rpc("eth_call", [{"to": to, "input": "0x" + data}, block])
        rec = {"to": to, "data": "0x" + data}
        if "error" in j:
            rec["error"] = j["error"]
        else:
            rec["result"] = j["result"]
        return rec


def words(hexstr):
    h = hexstr[2:] if hexstr.startswith("0x") else hexstr
    return [int(h[i:i + 64], 16) for i in range(0, len(h), 64)]


def dyn_array(ws, head_index):
    """Разбор uint256[] / address[] по смещению в голове ответа (ABI)."""
    off = ws[head_index] // 32
    n = ws[off]
    return ws[off + 1:off + 1 + n]


def gt_pools(pages):
    out = []
    for p in range(1, pages + 1):
        url = f"https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/balancer-v3-arbitrum/pools?page={p}"
        r = requests.get(url, headers={"Accept": "application/json"}, timeout=60)
        r.raise_for_status()
        out.append(r.json())
    return out


# Решение 452: вызовы, которые бот может сделать к пулу (все виды и крючки) — тест берёт нужные по (адрес, данные).
POOL_VIEWS = ["getNormalizedWeights()", "getMinTokenBalances()", "getAmplificationParameter()",
              "getGyro2CLPPoolImmutableData()", "getECLPParams()", "getLBPoolImmutableData()", "isSwapEnabled()",
              "getTokenIndices()", "isProjectTokenSwapInBlocked()", "getReserveTokenVirtualBalance()",
              "getProjectTokenRate()", "getQuantAMMWeightedPoolImmutableData()", "computeCurrentVirtualBalances()",
              "getLastVirtualBalances()", "getCenterednessMargin()", "getQuantAMMWeightedPoolDynamicData()"]
VAULT_POOL = ["getHooksConfig(address)", "getPoolTokenInfo(address)", "getCurrentLiveBalances(address)",
              "getPoolTokenRates(address)", "getPoolConfig(address)", "isPoolPaused(address)"]
HOOK_POOL = ["getMaxSurgeFeePercentage(address)", "getSurgeThresholdPercentage(address)",
             "getPoolMevTaxMultiplier(address)", "getPoolMevTaxThreshold(address)"]
HOOK_GLOBAL = ["isMevTaxEnabled()", "getMaxMevSwapFeePercentage()"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--vault", required=True)
    ap.add_argument("--router", required=True)
    ap.add_argument("--list", action="append", default=[],
                    help="настройка=адрес,адрес — списки dex.bv3_*_factories и dex.bv3_*_hooks (как в docs/FIRST_RUN.md)")
    ap.add_argument("--arb-sys", default="0x0000000000000000000000000000000000000064", help="dex.arb_sys")
    ap.add_argument("--arb-owner-public", default="0x000000000000000000000000000000000000006b",
                    help="dex.arb_owner_public")
    ap.add_argument("--pools", default="", help="адреса пулов через запятую")
    ap.add_argument("--gt-pages", type=int, default=0, help="сколько страниц пулов взять с GeckoTerminal")
    ap.add_argument("--block", default="", help="номер блока (по умолчанию — текущая голова)")
    ap.add_argument("--out", default="", help="папка записи (по умолчанию tests/data/arbitrum_balancer_v3_<дата>)")
    a = ap.parse_args()

    node = Node(a.rpc)
    chain = node.rpc("eth_chainId", [])["result"]
    head = int(a.block) if a.block else int(node.rpc("eth_blockNumber", [])["result"], 16)
    tag = hex(head)
    blk = node.rpc("eth_getBlockByNumber", [tag, False])["result"]
    date = datetime.datetime.utcfromtimestamp(int(blk["timestamp"], 16)).strftime("%Y-%m-%d")
    out = a.out or os.path.join("tests", "data", f"arbitrum_balancer_v3_{date}")
    os.makedirs(out, exist_ok=True)

    lists = {}
    for item in a.list:
        name, _, addrs = item.partition("=")
        lists[name.strip()] = [x.strip() for x in addrs.split(",") if x.strip()]
    factories = [f for n, l in lists.items() if n.endswith("_factories") for f in l]

    pools = [x.strip() for x in a.pools.split(",") if x.strip()]
    if a.gt_pages:
        gt = gt_pools(a.gt_pages)
        with open(os.path.join(out, "geckoterminal_pools.json"), "w") as fh:
            json.dump(gt, fh, indent=1)
        for page in gt:
            for p in page.get("data", []):
                addr = p.get("attributes", {}).get("address")
                if addr and addr not in pools:
                    pools.append(addr)

    meta = {"chain_id": chain, "block": head, "block_hash": blk["hash"], "timestamp": int(blk["timestamp"], 16),
            "vault": a.vault, "router": a.router, "lists": lists, "arb_sys": a.arb_sys,
            "arb_owner_public": a.arb_owner_public, "pools": pools}
    with open(os.path.join(out, "meta.json"), "w") as fh:
        json.dump(meta, fh, indent=1)

    v = a.vault
    for pool in pools:
        pw = word_addr(pool)
        rec = {"pool": pool, "calls": [], "quotes": [], "quotes_out": []}
        add = lambda to, data: rec["calls"].append(node.call(to, data, tag))  # noqa: E731
        for sig in VAULT_POOL:
            add(v, sel(sig) + pw)
        add(v, sel("getMinimumTradeAmount()"))
        add(v, sel("isVaultPaused()"))
        for f in factories:
            add(f, sel("isPoolFromFactory(address)") + pw)
        for sig in POOL_VIEWS:
            add(pool, sel(sig))
        hooks = rec["calls"][0]
        hook = None
        if "result" in hooks:
            hw = words(hooks["result"])
            if len(hw) >= 11 and hw[10] != 0:
                hook = "0x" + format(hw[10], "040x")
        if hook:
            for sig in HOOK_POOL:
                add(hook, sel(sig) + pw)
            for sig in HOOK_GLOBAL:
                add(hook, sel(sig))
            add(a.arb_sys, sel("arbOSVersion()"))
            add(a.arb_owner_public, sel("getCollectTips()"))
        info = rec["calls"][1]
        if "result" in info:
            ws = words(info["result"])
            tokens = ["0x" + format(t, "040x") for t in dyn_array(ws, 0)]
            raw = dyn_array(ws, 2)
            q = sel("querySwapSingleTokenExactIn(address,address,address,uint256,address,bytes)")
            # Ревью 441, п. 3: и точный выход (querySwapSingleTokenExactOut) — сверка обратного расчёта бота (amount_in).
            qo = sel("querySwapSingleTokenExactOut(address,address,address,uint256,address,bytes)")
            for i, ti in enumerate(tokens):
                for j, tj in enumerate(tokens):
                    if i == j:
                        continue
                    for key, sig, base in (("quotes", q, raw[i]), ("quotes_out", qo, raw[j])):
                        for div in (1_000_000, 10_000, 1_000, 100, 10):
                            amt = base // div
                            if amt == 0:
                                continue
                            data = (sig + pw + word_addr(ti) + word_addr(tj) + word_int(amt) + word_addr("0x0")
                                    + word_int(0xC0) + word_int(0))
                            r = node.call(a.router, data, tag)
                            rec[key].append(dict(r, i=i, j=j, token_in=ti, token_out=tj, amount=str(amt)))
        with open(os.path.join(out, f"pool_{pool.lower()}.json"), "w") as fh:
            json.dump(rec, fh, indent=1)
        print(pool, "вызовов:", len(rec["calls"]), "квот:", len(rec["quotes"]) + len(rec["quotes_out"]), file=sys.stderr)

    lines = [f"# Balancer V3 на Arbitrum — запись своего узла (решения 441, 452)", "",
             f"Блок {head}, хеш {blk['hash']}, время {blk['timestamp']} ({date}), chainId {chain}.",
             "Только eth_call (чтение), без from; ключей и подписей нет. Скрипт — tools/arbitrum_record/balancer_v3.py.",
             f"Хранилище {a.vault}, роутер {a.router}; списки фабрик и крючков, прекомпайлы — meta.json. "
             f"Пулов: {len(pools)}.", "",
             "Файлы: meta.json — блок, адреса, списки, пулы; pool_<адрес>.json — calls (все вызовы, которые бот может "
             "сделать к пулу: хранилище, фабрики, виды пулов, крючок, прекомпайлы; to, data, result или error), quotes "
             "(Router V3 querySwapSingleTokenExactIn: i, j, сумма, ответ), quotes_out (querySwapSingleTokenExactOut: "
             "i, j, точный выход, ответ — вход); geckoterminal_pools.json — ответ GeckoTerminal (если --gt-pages).", "",
             "## SHA-256 файлов"]
    for name in sorted(os.listdir(out)):
        if name == "README.md":
            continue
        with open(os.path.join(out, name), "rb") as fh:
            lines.append(f"{hashlib.sha256(fh.read()).hexdigest()}  {name}")
    with open(os.path.join(out, "README.md"), "w") as fh:
        fh.write("\n".join(lines) + "\n")
    print("записано:", out, file=sys.stderr)


if __name__ == "__main__":
    main()
