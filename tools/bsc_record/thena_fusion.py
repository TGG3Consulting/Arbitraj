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
  В конце — zip папки, путь печатается последней строкой.
Адреса (официальная таблица контрактов THENA — docs.thena.fi/thena/official-links «List of smart contracts»):
  AlgebraFactory 0x306F06C147f064A010530292A1EB6737c3e378e4, Quoter 0xeA68020D6A9532EeC42D4dB0f92B83580c39b2cA.
  Id DEX GeckoTerminal — thena-fusion-bsc (tests/bsc_dex_algebra_family.rs).
Запуск (из корня репозитория): python3 tools/bsc_record/thena_fusion.py --rpc http://127.0.0.1:8545
Кто вызывает: Тигран. От чего зависит: стандартная библиотека, tools/arbitrum_record/stdhttp.py, rec_rpc.py рядом.
"""
import argparse
import datetime
import hashlib
import json
import os
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(ROOT, "tools", "arbitrum_record"))
sys.path.insert(0, HERE)
import stdhttp as requests  # noqa: E402 — решение 459: стандартная библиотека вместо пакета requests
from rec_rpc import Node, addr, int24, word, words  # noqa: E402

SIGS = ["poolDeployer()", "poolByPair(address,address)", "token0()", "token1()", "globalState()", "liquidity()",
        "tickSpacing()", "timepoints(uint256)", "tickTable(int16)", "ticks(int24)", "balanceOf(address)",
        "quoteExactInputSingle(address,address,uint256,uint160)", "quoteExactIn(address,bool,uint256)"]
MAX_TICK = 887272


def args():
    a = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    a.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла BSC")
    a.add_argument("--factory", default="0x306F06C147f064A010530292A1EB6737c3e378e4", help="AlgebraFactory THENA")
    a.add_argument("--quoter", default="0xeA68020D6A9532EeC42D4dB0f92B83580c39b2cA", help="Quoter THENA")
    a.add_argument("--gecko-dex", default="thena-fusion-bsc", help="id DEX GeckoTerminal")
    a.add_argument("--gecko-pools", type=int, default=8, help="сколько пулов взять с первой страницы GeckoTerminal")
    a.add_argument("--pool", action="append", default=[], help="адрес пула (дополнительно; можно несколько)")
    a.add_argument("--init-hash", action="append", default=[], help="POOL_INIT_CODE_HASH версии (несколько)")
    a.add_argument("--words", type=int, default=2, help="слов tickTable в каждую сторону от слова тика")
    a.add_argument("--bps", type=int, action="append", default=[], help="сумма = баланс входа × bps / 10000")
    a.add_argument("--batch", type=int, default=100, help="вызовов в одной пачке JSON-RPC")
    a.add_argument("--vq-runtime", default=os.path.join(ROOT, "contracts", "out", "VirtualQuoter.bin-runtime"))
    a.add_argument("--out", default=os.path.join(ROOT, "tests", "data"), help="папка tests/data")
    o = a.parse_args()
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
    raw_w = range((tick >> 8) - o.words, (tick >> 8) + o.words + 1)
    cmp_w = range(((tick // ts) >> 8) - o.words, ((tick // ts) >> 8) + o.words + 1) if ts > 0 else range(0)
    second = [call(p, s["timepoints(uint256)"] + word(tp_index)),
              call(o.factory, s["poolByPair(address,address)"] + word(t0) + word(t1)),
              call(t0, s["balanceOf(address)"] + word(p)), call(t1, s["balanceOf(address)"] + word(p))]
    second += [call(p, s["tickTable(int16)"] + word(w)) for w in list(raw_w) + list(cmp_w)]
    got = n.batch(second, o.batch)
    rec["timepoints"], rec["poolByPair"] = n.must(got[0], "timepoints"), got[1]
    bal = {t0: int(n.must(got[2], "balanceOf")["result"], 16), t1: int(n.must(got[3], "balanceOf")["result"], 16)}
    rec["balances"] = {t: hex(v) for t, v in bal.items()}
    rows = got[4:]
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
            for at, over in (("at_block", None), ("at_last_timepoint", {"time": hex(last_ts)})):
                vq = [{vq_addr: {"code": vq_code}}] + ([over] if over else [])
                third.append(call(vq_addr, s["quoteExactIn(address,bool,uint256)"] + word(p) + word(int(zfo))
                                  + word(amt), *vq))
                qd = s["quoteExactInputSingle(address,address,uint256,uint160)"] + word(tin) + word(tout) + word(amt)
                third.append(call(o.quoter, qd + word(0), *([{}, over] if over else [])))
                plan.append((zfo, bps, amt, at))
    got = n.batch(third, o.batch)
    for (field, t), j in zip(keys, got):
        rec[field][str(t)] = j
    rec["quotes"], q = [], {}
    for i, (zfo, bps, amt, at) in enumerate(plan):
        v, qt = got[len(keys) + 2 * i], got[len(keys) + 2 * i + 1]
        e = q.setdefault((zfo, bps), {"zero_for_one": zfo, "bps": bps, "amount_in": hex(amt)})
        e[at] = {"virtual": v, "quoter": qt}
    rec["quotes"] = list(q.values())
    rec["last_timepoint_ts"] = last_ts
    rec["code"], rec["code_keccak"] = code, n.sha3(code)
    return rec


def main():
    o = args()
    n = Node(o.rpc)
    s = {k: n.sel(k) for k in SIGS}
    u = f"https://api.geckoterminal.com/api/v2/networks/bsc/dexes/{o.gecko_dex}/pools?page=1"
    g = requests.get(u, timeout=60, headers={"Accept": "application/json"}).json() if o.gecko_pools else {"data": []}
    pools = [p.lower() for p in o.pool] + [d["attributes"]["address"].lower() for d in g["data"][:o.gecko_pools]]
    pools = list(dict.fromkeys(pools))
    if not pools:
        sys.exit("нет пулов: GeckoTerminal пуст и --pool не задан")
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    out = os.path.join(o.out, f"bsc_thena_fusion_{now}")
    os.makedirs(out, exist_ok=True)
    files = {}

    def save(name, obj):
        files[name] = True
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            json.dump(obj, f, indent=1, ensure_ascii=False)

    save("geckoterminal_pools.json", {"url": u, "response": g})
    chain = n.ok("eth_chainId", [])
    save("meta.json", {"chainId": chain, "client": n.call("web3_clientVersion", []),
                       "args": {k: v for k, v in vars(o).items() if k != "rpc"}})
    if int(chain, 16) != 56:
        sys.exit(f"узел не BSC: chainId {int(chain, 16)}")
    vq_addr = "0x" + n.sha3("0x" + b"arb.virtual_quoter".hex())[-40:]
    vq_code = "0x" + open(o.vq_runtime).read().strip().replace("0x", "")
    recs = []
    for p in pools:
        rec = pool_record(n, s, o, p, vq_addr, vq_code)
        save(f"pool_{p}.json", rec)
        recs.append(rec)
        print(f"пул {p}: блок {int(rec['block']['number'], 16)}, тиков {len(rec['ticks'])} / "
              f"{len(rec['ticks_compressed'])}, котировок {len(rec['quotes'])}")
    # После всех блоков (состояние не нужно): фабрика, деплоер, CREATE2, Sourcify.
    dep_j = n.call("eth_call", [{"to": o.factory, "data": s["poolDeployer()"]}, "latest"])
    deployer = addr(words(n.must(dep_j, "poolDeployer")["result"])[0])
    dep_code = n.ok("eth_getCode", [deployer, "latest"])
    fac = {"factory": o.factory, "poolDeployer": deployer, "deployer_code_keccak": n.sha3(dep_code),
           "init_hashes": o.init_hash, "pools": {}}
    for r in recs:
        salt = n.sha3("0x" + word(r["token0"]) + word(r["token1"]))
        fac["pools"][r["pool"]] = {h: "0x" + n.sha3("0xff" + deployer[2:] + salt[2:] + h[2:])[-40:]
                                   for h in o.init_hash}
    save("factory.json", fac)
    src = {}
    for name, a in (("deployer", deployer), ("factory", o.factory), ("quoter", o.quoter), ("pool", pools[0])):
        url = f"https://sourcify.dev/server/v2/contract/56/{a}?fields=compilation"
        try:
            r = requests.get(url, timeout=60, headers={"Accept": "application/json"})
            src[name] = {"url": url, "status": r.status_code, "body": r.text[:20000]}
        except OSError as e:  # сеть до Sourcify — не повод терять запись узла
            src[name] = {"url": url, "error": str(e)}
    save("sourcify.json", src)
    write_readme(out, files, fac, recs)
    zpath = out + ".zip"
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(os.listdir(out)):
            z.write(os.path.join(out, f), os.path.join(os.path.basename(out), f))
    print(f"Архив для отправки: {zpath}")


def write_readme(out, files, fac, recs):
    """README.md: факты записи (без выводов) и SHA-256 файлов."""
    lines = [f"# THENA Fusion — запись своего узла BSC ({os.path.basename(out)})", "",
             "Записано tools/bsc_record/thena_fusion.py (только чтение). Состав файлов — шапка скрипта.", "",
             "- chainId: 56", f"- фабрика {fac['factory']}, poolDeployer {fac['poolDeployer']}, "
             f"keccak кода деплоера {fac['deployer_code_keccak']}", ""]
    for r in recs:
        bits = lambda f: sum(1 for j in r[f].values() if "result" in j)  # noqa: E731
        live = lambda f: sum(1 for j in r[f].values()  # noqa: E731
                             if "result" in j and int(words(j["result"])[0], 16) > 0)
        hits = [h for h, a in fac["pools"][r["pool"]].items() if a == r["pool"]]
        lines.append(f"- пул {r['pool']}: блок {int(r['block']['number'], 16)}, tickSpacing {r['tick_spacing']}, "
                     f"keccak кода {r['code_keccak']}, CREATE2 по хешу {hits or 'ни одного'}; "
                     f"сырое толкование: битов {bits('ticks')}, из них liquidityTotal > 0 — {live('ticks')}; "
                     f"сжатое: битов {bits('ticks_compressed')}, liquidityTotal > 0 — {live('ticks_compressed')}")
    lines += ["", "## SHA-256 файлов"]
    for name in sorted(files):
        lines.append(hashlib.sha256(open(os.path.join(out, name), "rb").read()).hexdigest() + "  " + name)
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
