#!/usr/bin/env python3
"""Файл: tools/bsc_record/thena_fusion.py
Что делает: запись ответов своего узла BSC для вида пула thena_fusion (THENA Fusion; ревью t235, решение 461, T461-06):
  доказывает (или опровергает) режим цели шага Row (math/swap_algebra.rs NextTick::Row — строка СЫРОГО битмапа) на
  живых пулах. Только чтение (eth_call, eth_getCode, web3_sha3), транзакций, ключей и подписей нет.
  Каждый пул — на СВОЁМ свежем блоке (голова на начало пула), всё состояние и котировки пула — пачками JSON-RPC на
  этом блоке сразу (узел с короткой историей состояния: долгих операций перед блоком нет; GeckoTerminal — до блоков).
Что пишет в tests/data/bsc_thena_fusion_<дата UTC>/:
  meta.json — chainId, web3_clientVersion, аргументы; factory.json — poolDeployer() фабрики, код деплоера (keccak),
  CREATE2 каждого пула по хешам кода пула (--init-hash: AlgebraV1.9 main и AlgebraV1 master, PoolAddress.sol);
  pool_<адрес>.json — блок; token0/token1, poolByPair, globalState, liquidity, tickSpacing, timepoints(timepointIndex),
  код пула (eth_getCode) и его keccak; слова tickTable вокруг тика в ДВУХ толкованиях: СЫРОЕ (слово = tick >> 8, как
  читает бот, load_algebra.rs) — tickTable / ticks, и СЖАТОЕ (слово = floor(tick / tickSpacing) >> 8, как TickTable.sol
  AlgebraV1) — tickTable_compressed / ticks_compressed; ticks() по каждому установленному биту; balanceOf пула;
  котировки на суммах --bps (доля баланса входа пула, 1/10000) в обе стороны — виртуальный квотер бота (обмен самого
  пула в eth_call, код contracts/out/VirtualQuoter.bin-runtime подменой по адресу — как бот) и официальный Quoter
  THENA (quoteExactInputSingle(address,address,uint256,uint160) — AlgebraV1 master periphery lens/Quoter.sol), каждая
  дважды: на блоке и с blockOverrides.time = время последней точки оракула (обмен не пересчитывает комиссию);
  sourcify.json — ответы Sourcify (сеть 56) по деплоеру, фабрике, квотеру и первому пулу (после всех блоков);
  geckoterminal_pools.json — список пулов GeckoTerminal; README.md — сводка фактов и SHA-256 файлов.
  Решение 462 (t237, повторная запись): GeckoTerminal id «thena-fusion» (запись 20.11: «thena-fusion-bsc» не тот),
  страницы --gecko-pages (по 20 пулов), сжатых слов --words (8) и сырых --raw-words (1) в каждую сторону; данные
  оракула для динамической комиссии (_getNewFee — первый своп в блоке): dataStorageOperator() пула, feeConfig()
  оператора, timepoints(index − 2 … index + 1) пула, у оператора от имени пула (eth_call from = пул, onlyPool) —
  getFee / getAverages (uint32 time, int24 tick, uint16 index, uint128 liquidity) и getTimepoints(time, [0, WINDOW])
  при time = время блока + 0, 1, 3 с; котировки ещё и с blockOverrides.time = время блока + 1 и + 3 (at_plus_1,
  at_plus_3) — комиссия, которую получит своп следующего блока.
  Решение 463 (t237, повтор записи для порта комиссии следующего блока, T463-02): слово хранилища пула slot 3
  (eth_getStorageAt; раскладка PoolState.sol исходника THENA, Sourcify 56: slot 0 totalFeeGrowth0Token, 1
  totalFeeGrowth1Token, 2 globalState (160+24+16+16+16+16+8 бит), 3 = liquidity (младшие 128 бит) |
  volumePerLiquidityInBlock (старшие 128); PoolImmutables — только immutable) — storage_slot3; точки оракула ещё
  index + 2 и 0 (timepoints_around); начало окна для бота — getTimepoints(время блока + 1, [WINDOW + 1, WINDOW]) от имени
  пула (oracle.starts: начало окна на время блока + 0 и + 1 — тот же вызов, что делает бот).
  В конце — zip папки, путь печатается последней строкой.
Адреса (официальная таблица контрактов THENA — docs.thena.fi/thena/official-links «List of smart contracts»):
  AlgebraFactory 0x306F06C147f064A010530292A1EB6737c3e378e4, Quoter 0xeA68020D6A9532EeC42D4dB0f92B83580c39b2cA.
  Id DEX GeckoTerminal — thena-fusion (ответ GeckoTerminal записи 20.11, tests/data/bsc_thena_fusion_2026-11-20).
Запуск (из корня репозитория): python3 tools/bsc_record/thena_fusion.py --rpc http://127.0.0.1:8545
  Решение 487: сеть — параметрами (--chain-id, --name, --gecko-network, --pair-token, --heads, --quoter ""), общий ход
  записи — thena_fusion_run.py; Zyberswap V3 Arbitrum — tools/arbitrum_record/zyberswap_v3.py с этими же функциями.
Кто вызывает: Тигран; tools/arbitrum_record/zyberswap_v3.py (args, pool_record). От чего зависит: стандартная
  библиотека, tools/arbitrum_record/stdhttp.py, rec_rpc.py и thena_fusion_run.py рядом.
"""
import argparse
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(ROOT, "tools", "arbitrum_record"))
sys.path.insert(0, HERE)
from rec_rpc import addr, int24, word, words  # noqa: E402

SIGS = ["poolDeployer()", "poolByPair(address,address)", "token0()", "token1()", "globalState()", "liquidity()",
        "tickSpacing()", "timepoints(uint256)", "tickTable(int16)", "ticks(int24)", "balanceOf(address)",
        "quoteExactInputSingle(address,address,uint256,uint160)", "quoteExactIn(address,bool,uint256)",
        "dataStorageOperator()", "feeConfig()", "getFee(uint32,int24,uint16,uint128)",
        "getAverages(uint32,int24,uint16,uint128)", "getTimepoints(uint32,uint32[],int24,uint16,uint128)"]
WINDOW = 86400  # DataStorage.WINDOW = 1 days (cryptoalgebra/AlgebraV1 libraries/DataStorage.sol)
AHEAD = (0, 1, 3)  # время блока + секунды: время свопа следующего блока (оракул и комиссия)
MAX_TICK = 887272


def args(defaults=None, doc=None):
    """Аргументы запуска. Решение 487: `defaults` — значения по умолчанию другой сети (tools/arbitrum_record/
    zyberswap_v3.py), `doc` — его описание; без них — THENA Fusion на BSC, как было."""
    a = argparse.ArgumentParser(description=doc or __doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    a.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла")
    a.add_argument("--chain-id", type=int, default=56, help="ожидаемый eth_chainId узла (решение 487)")
    a.add_argument("--name", default="bsc_thena_fusion", help="папка записи tests/data/<name>_<дата UTC>")
    a.add_argument("--gecko-network", default="bsc", help="сеть GeckoTerminal")
    a.add_argument("--pair-token", action="append", default=None,
                   help="монета для поиска пулов poolByPair фабрики по всем парам (решение 487; можно несколько; "
                        "заданные заменяют список по умолчанию)")
    a.add_argument("--heads", type=int, default=0, help="времена последних N блоков узла — heads.json (решение 487)")
    a.add_argument("--factory", default="0x306F06C147f064A010530292A1EB6737c3e378e4", help="AlgebraFactory THENA")
    a.add_argument("--quoter", default="0xeA68020D6A9532EeC42D4dB0f92B83580c39b2cA", help="Quoter THENA")
    a.add_argument("--gecko-dex", default="thena-fusion", help="id DEX GeckoTerminal")
    a.add_argument("--gecko-pages", type=int, default=3, help="страниц GeckoTerminal (по 20 пулов)")
    a.add_argument("--gecko-pools", type=int, default=60, help="сколько пулов взять из страниц GeckoTerminal")
    a.add_argument("--pool", action="append", default=[], help="адрес пула (дополнительно; можно несколько)")
    a.add_argument("--init-hash", action="append", default=[], help="POOL_INIT_CODE_HASH версии (несколько)")
    a.add_argument("--words", type=int, default=8, help="СЖАТЫХ слов tickTable в каждую сторону от слова тика")
    a.add_argument("--raw-words", type=int, default=1, help="СЫРЫХ слов tickTable в каждую сторону (для факта)")
    a.add_argument("--bps", type=int, action="append", default=[], help="сумма = баланс входа × bps / 10000")
    a.add_argument("--batch", type=int, default=100, help="вызовов в одной пачке JSON-RPC")
    a.add_argument("--vq-runtime", default=os.path.join(ROOT, "contracts", "out", "VirtualQuoter.bin-runtime"))
    a.add_argument("--out", default=os.path.join(ROOT, "tests", "data"), help="папка tests/data")
    defaults = dict(defaults or {})
    # Ревью t256: --pair-token заменяет список по умолчанию, а не дописывает к нему (append к default дописал бы).
    pair_default = defaults.pop("pair_token", [])
    if defaults:
        a.set_defaults(**defaults)
    o = a.parse_args()
    o.pair_token = o.pair_token or list(pair_default)
    # Хеши кода пула (PoolAddress.sol POOL_INIT_CODE_HASH): cryptoalgebra/AlgebraV1.9 main и cryptoalgebra/AlgebraV1.
    o.init_hash = o.init_hash or ["0xbce37a54eab2fcd71913a0d40723e04238970e7fc1159bfd58ad5b79531697e7",
                                  "0x6ec6c9c8091d160c0aa74b2b14ba9c1717e95093bd3ac085cee99a49aab294a4"]
    o.bps = o.bps or [1, 10, 100, 1000]
    return o


def pool_record(n, s, o, p, vq_addr, vq_code):
    """Всё по пулу на одном свежем блоке: три пачки (состояние → слова и балансы → тики и котировки)."""
    head = n.ok("eth_getBlockByNumber", ["latest", False])
    tag = head["number"]
    call = lambda to, data, *extra: ("eth_call", [{"to": to, "data": data}, tag] + list(extra))  # noqa: E731
    rec = {"pool": p, "block": {k: head[k] for k in ("number", "hash", "timestamp")}}
    first = ["token0()", "token1()", "globalState()", "liquidity()", "tickSpacing()"]
    got = n.batch([call(p, s[k]) for k in first] + [("eth_getCode", [p, tag])], o.batch)
    for k, j in zip(first, got):
        rec[k] = n.must(j, k)
    code = n.must(got[-1], "eth_getCode")["result"]
    t0, t1 = (addr(words(rec[k]["result"])[0]) for k in ("token0()", "token1()"))
    gs = words(rec["globalState()"]["result"])
    tick, tp_index = int24(gs[1]), int(gs[3], 16)  # globalState AlgebraV1: price, tick, fee, timepointIndex, …
    ts = int24(words(rec["tickSpacing()"]["result"])[0])
    rec.update(token0=t0, token1=t1, tick=tick, tick_spacing=ts)
    raw_w = range((tick >> 8) - o.raw_words, (tick >> 8) + o.raw_words + 1)
    cmp_w = range(((tick // ts) >> 8) - o.words, ((tick // ts) >> 8) + o.words + 1) if ts > 0 else range(0)
    liq = int(words(rec["liquidity()"]["result"])[0], 16)
    second = [call(p, s["timepoints(uint256)"] + word(tp_index)),
              call(o.factory, s["poolByPair(address,address)"] + word(t0) + word(t1)),
              call(t0, s["balanceOf(address)"] + word(p)), call(t1, s["balanceOf(address)"] + word(p))]
    second += [call(p, s["tickTable(int16)"] + word(w)) for w in list(raw_w) + list(cmp_w)]
    # Оракул динамической комиссии (решение 462): оператор пула, его feeConfig, точки оракула вокруг индекса.
    second.append(call(p, s["dataStorageOperator()"]))
    tps = [(tp_index + k) % 65536 for k in (-2, -1, 1, 2)] + [0]
    second += [call(p, s["timepoints(uint256)"] + word(i)) for i in tps]
    second.append(("eth_getStorageAt", [p, "0x3", tag]))  # liquidity | volumePerLiquidityInBlock (решение 463)
    got = n.batch(second, o.batch)
    rec["storage_slot3"] = got[-1]
    k = 4 + len(raw_w) + len(cmp_w)
    op_j = got[k]
    rec["dataStorageOperator"] = op_j
    rec["timepoints_around"] = {str(i): got[k + 1 + j] for j, i in enumerate(tps)}
    head_ts = int(head["timestamp"], 16)
    if "result" in op_j:
        op = addr(words(op_j["result"])[0])
        frm = lambda data: ("eth_call", [{"from": p, "to": op, "data": data}, tag])  # noqa: E731
        args = lambda t: word(t) + word(tick) + word(tp_index) + word(liq)  # noqa: E731
        oc = [("eth_call", [{"to": op, "data": s["feeConfig()"]}, tag])]
        for d in AHEAD:
            t = head_ts + d
            oc.append(frm(s["getFee(uint32,int24,uint16,uint128)"] + args(t)))
            oc.append(frm(s["getAverages(uint32,int24,uint16,uint128)"] + args(t)))
            # getTimepoints(time, uint32[] secondsAgos = [0, WINDOW], tick, index, liquidity): массив — хвост ABI.
            oc.append(frm(s["getTimepoints(uint32,uint32[],int24,uint16,uint128)"] + word(t) + word(5 * 32)
                          + word(tick) + word(tp_index) + word(liq) + word(2) + word(0) + word(WINDOW)))
        # Начало окна на время блока + 0 и + 1 (решение 463): тот же вызов, что делает бот (pools/fusion_fee.rs).
        ts1 = head_ts + 1
        oc.append(frm(s["getTimepoints(uint32,uint32[],int24,uint16,uint128)"] + word(ts1) + word(5 * 32)
                      + word(tick) + word(tp_index) + word(liq) + word(2) + word(WINDOW + 1) + word(WINDOW)))
        og = n.batch(oc, o.batch)
        rec["oracle"] = {"operator": op, "feeConfig": og[0], "by_time": {
            str(head_ts + d): {"getFee": og[1 + 3 * i], "getAverages": og[2 + 3 * i], "getTimepoints": og[3 + 3 * i]}
            for i, d in enumerate(AHEAD)}, "starts": {"time": ts1, "seconds_agos": [WINDOW + 1, WINDOW],
                                                      "getTimepoints": og[-1]}}
    rec["timepoints"], rec["poolByPair"] = n.must(got[0], "timepoints"), got[1]
    bal = {t0: int(n.must(got[2], "balanceOf")["result"], 16), t1: int(n.must(got[3], "balanceOf")["result"], 16)}
    rec["balances"] = {t: hex(v) for t, v in bal.items()}
    rows = got[4:4 + len(raw_w) + len(cmp_w)]
    rec["tickTable"] = {str(w): rows[i] for i, w in enumerate(raw_w)}
    rec["tickTable_compressed"] = {str(w): rows[len(raw_w) + i] for i, w in enumerate(cmp_w)}
    keys = []  # (поле записи, тик)
    for field, table, mult in (("ticks", rec["tickTable"], 1), ("ticks_compressed", rec["tickTable_compressed"], ts)):
        rec[field] = {}
        for w, j in table.items():
            bits = int(n.must(j, "tickTable")["result"], 16)
            for b in range(256):
                t = (int(w) * 256 + b) * mult
                if bits >> b & 1 and -MAX_TICK <= t <= MAX_TICK:
                    keys.append((field, t))
    last_ts = int(words(rec["timepoints"]["result"])[1], 16)
    third = [call(p, s["ticks(int24)"] + word(t)) for _, t in keys]
    plan = []
    for zfo in (True, False):
        tin, tout = (t0, t1) if zfo else (t1, t0)
        for bps in o.bps:
            amt = bal[tin] * bps // 10000
            if amt == 0:
                continue
            for at, over in (("at_block", None), ("at_last_timepoint", {"time": hex(last_ts)}),
                             ("at_plus_1", {"time": hex(head_ts + 1)}), ("at_plus_3", {"time": hex(head_ts + 3)})):
                vq = [{vq_addr: {"code": vq_code}}] + ([over] if over else [])
                third.append(call(vq_addr, s["quoteExactIn(address,bool,uint256)"] + word(p) + word(int(zfo))
                                  + word(amt), *vq))
                qd = s["quoteExactInputSingle(address,address,uint256,uint160)"] + word(tin) + word(tout) + word(amt)
                # Решение 487: квотера может не быть (Zyberswap V3 — --quoter ""): тогда только виртуальный квотер.
                if o.quoter:
                    third.append(call(o.quoter, qd + word(0), *([{}, over] if over else [])))
                plan.append((zfo, bps, amt, at))
    got = n.batch(third, o.batch)
    for (field, t), j in zip(keys, got):
        rec[field][str(t)] = j
    rec["quotes"], q = [], {}
    per = 2 if o.quoter else 1  # ответов на одну котировку
    for i, (zfo, bps, amt, at) in enumerate(plan):
        v = got[len(keys) + per * i]
        qt = got[len(keys) + per * i + 1] if o.quoter else None
        e = q.setdefault((zfo, bps), {"zero_for_one": zfo, "bps": bps, "amount_in": hex(amt)})
        e[at] = {"virtual": v, "quoter": qt}
    rec["quotes"] = list(q.values())
    rec["last_timepoint_ts"] = last_ts
    rec["code"], rec["code_keccak"] = code, n.sha3(code)
    return rec


def main():
    """Запуск THENA Fusion на BSC (общий ход записи — thena_fusion_run.py, решение 487)."""
    from thena_fusion_run import run
    run(args())


if __name__ == "__main__":
    main()
