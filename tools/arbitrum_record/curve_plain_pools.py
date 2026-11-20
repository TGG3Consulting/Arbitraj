"""Файл: tools/arbitrum_record/curve_plain_pools.py
Что делает: решение 472 — общее для записи Curve StableSwap 2021 (curve_plain.py): JSON-RPC пакетом, адреса
  (официальные), список пулов фабрики Curve Sidechain/L2 по сверенным реализациям (plain и мета), вызовы бота по
  (адрес, calldata) — те же, что строят pools/load_curve_plain.rs calls() и load_curve_meta.rs calls(), — и сверочные
  геттеры рампы A. Разбито из curve_plain.py (файлы ≤ ~200 строк).
Кто вызывает: tools/arbitrum_record/curve_plain.py.
От чего зависит: stdhttp.py, curve_crypto_rpc.py (selector, word, addr_word).
Адреса:
  2pool 0x7f90122BF0700F9E7e1F688fe926940E8839F353 — curvefi/curve-js ab3f1c2 src/constants/pools/arbitrum.ts L15-L20;
  фабрика 0xb17b674D9c5CB2e441F8e196a2f048A81355d031 — network_constants.ts L157 (ALIASES_ARBITRUM.factory);
  реализации — src/constants/factory/stable.ts L158-L183 (implementationABIDictArbitrum): plain — Plain2/3/4 Basic,
  Balances, ETH, Optimized, Plain2BasicEMA, Plain2ETHEMA; мета — MetaUSD, MetaUSDBalances, MetaFraxUSD,
  MetaFraxUSDBalances (MetaBTCRen — нет: LP базового пула renBTC не сам пул).
"""

import stdhttp as requests  # noqa: E402  решение 459
from curve_crypto_rpc import addr_word, selector, word

POOL_2 = "0x7f90122BF0700F9E7e1F688fe926940E8839F353"
FACTORY = "0xb17b674D9c5CB2e441F8e196a2f048A81355d031"
PLAIN = ["0x54e8A25d0Ac0E4945b697C80b8372445FEA17A62", "0x73Ec37618683C274D0bBf5f5726aA856B2BDAB81",
         "0xD68970e266cE1A015953897C7055a5E0bC657Af8", "0x7DA64233Fefb352f8F501B357c018158ED8aA455",
         "0x6F9fb833501f46CBE6f6A4b6Cf32C834E5A5e8C5", "0x0100fBf414071977B19fC38e6fc7c32FE444F5C9",
         "0xe381C25de995d62b453aF8B931aAc84fcCaa7A62", "0xc379bA7b8e1c6C48D64e1cf9dD602C97c9fD0F40",
         "0xAAe75FAebCae43b9d541Fd875622BE48D9B4f5D0", "0x8866414733F22295b7563f9C5299715D2D76CAf4",
         "0x8d53E5De033078367Ad91527c53abfd1Eb6bfa86", "0x2ac56cEBc2D27c9bB51a11773355E44371Eb88D3",
         "0x89287c32c2CAC1C76227F6d300B2DBbab6b75C08", "0x06e3C4da96fd076b97b7ca3Ae23527314b6140dF"]
META = ["0x09672362833d8f703D5395ef3252D4Bfa51c15ca", "0xBE175115BF33E12348ff77CcfEE4726866A0Fbd5",
        "0x8DEb66a4A40E370355bEe35f12E55Fe9c755d686", "0x3edE9b145F82e9e46C03f8A8F67B77aEE847b632"]
MAX_COINS = 8  # abi_curve.rs MAX_COINS (curve-pool-registry Registry.vy)
NATIVE = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
CHECK = ["A()", "admin_fee()", "initial_A()", "future_A()", "initial_A_time()", "future_A_time()"]


class Batch:
    """JSON-RPC пакетом: список (метод, параметры) → список {"result"} / {"error"} по порядку."""

    def __init__(self, url):
        self.url = url

    def run(self, reqs, step=500):
        out = []
        for s in range(0, len(reqs), step):
            part = reqs[s:s + step]
            body = [{"jsonrpc": "2.0", "id": s + k, "method": m, "params": p} for k, (m, p) in enumerate(part)]
            r = requests.post(self.url, json=body, timeout=120)
            r.raise_for_status()
            got = {x["id"]: x for x in r.json()}
            for k in range(len(part)):
                x = got.get(s + k, {"error": "нет ответа"})
                out.append({"result": x["result"]} if "result" in x else {"error": x.get("error")})
        return out

    def one(self, method, params):
        return self.run([(method, params)])[0]


def call(to, data, tag):
    return ("eth_call", [{"to": to, "data": data}, tag])


def key(to, data):
    """Ключ ответа в записи: «адрес|calldata» (нижний регистр) — тест ищет ответ на тот же вызов бота."""
    return to.lower() + "|" + data.lower()


def bot_calls(pool, base=None):
    """(адрес, calldata) вызовов бота: load_curve_plain.rs calls (coins(k) / balances(k) k < 8, A_precise(), fee(),
    ma_exp_time()), у метапула — get_virtual_price() базового пула (load_curve_meta.rs); и сверочные геттеры."""
    out = []
    for k in range(MAX_COINS):
        out.append((pool, selector("coins(uint256)") + word(k)))
        out.append((pool, selector("balances(uint256)") + word(k)))
    for sig in ["A_precise()", "fee()", "ma_exp_time()"] + CHECK:
        out.append((pool, selector(sig)))
    if base:
        out.append((base, selector("get_virtual_price()")))
    return out


def factory_pools(b, max_pools):
    """Пулы фабрики на сверенных реализациях, с конца pool_list: [(пул, реализация, вид, базовый пул)], число пулов
    фабрики. Вид — plain / meta; у метапула базовый пул — get_base_pool фабрики."""
    n = b.one(*call(FACTORY, selector("pool_count()"), "latest"))
    count = int(n["result"], 16) if n.get("result") else 0
    idx = list(range(count - 1, -1, -1))
    lst = b.run([call(FACTORY, selector("pool_list(uint256)") + word(i), "latest") for i in idx])
    pools = ["0x" + x["result"][-40:] for x in lst if x.get("result")]
    imps = b.run([call(FACTORY, selector("get_implementation_address(address)") + addr_word(p), "latest")
                  for p in pools])
    plain, meta = [a.lower() for a in PLAIN], [a.lower() for a in META]
    picked = []
    for p, x in zip(pools, imps):
        imp = ("0x" + x["result"][-40:]).lower() if x.get("result") else None
        if imp in plain or imp in meta:
            picked.append((p, imp, "plain" if imp in plain else "meta"))
        if len(picked) >= max_pools:
            break
    bases = b.run([call(FACTORY, selector("get_base_pool(address)") + addr_word(p), "latest")
                   for p, _, k in picked if k == "meta"])
    it = iter(bases)
    out = []
    for p, imp, k in picked:
        base = None
        if k == "meta":
            x = next(it)
            base = "0x" + x["result"][-40:] if x.get("result") else None
        out.append((p, imp, k, base))
    return count, out
