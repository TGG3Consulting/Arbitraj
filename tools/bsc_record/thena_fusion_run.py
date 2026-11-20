"""Файл: tools/bsc_record/thena_fusion_run.py
Что делает: решение 487 — общий ход записи пулов кода AlgebraV1 (THENA Fusion BSC, Zyberswap V3 Arbitrum): пулы
  (GeckoTerminal сети --gecko-network и/или poolByPair фабрики по всем парам монет --pair-token и/или --pool), проверка
  eth_chainId = --chain-id ДО чтения пулов, каждый пул — thena_fusion.pool_record на своём свежем блоке, времена
  последних --heads блоков (heads.json: номер, время — для вопроса «следующий блок в ту же / следующую секунду», T487-02;
  заголовки старых блоков узел отдаёт без состояния), фабрика / деплоер / CREATE2, Sourcify сети --chain-id, README.md
  и zip. Только чтение. Вынесено из thena_fusion.py (main) без изменения хода записи BSC.
Кто вызывает: tools/bsc_record/thena_fusion.py (main), tools/arbitrum_record/zyberswap_v3.py.
От чего зависит: thena_fusion.py (SIGS, pool_record), rec_rpc.py, tools/arbitrum_record/stdhttp.py.
"""
import datetime
import hashlib
import json
import os
import sys
import zipfile

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests
from rec_rpc import Node, addr, word, words
from thena_fusion import SIGS, pool_record


def gecko(o):
    """Пулы GeckoTerminal (страницы по 20). Возвращает: (url, ответ). Без --gecko-pools / --gecko-dex — пусто."""
    u = f"https://api.geckoterminal.com/api/v2/networks/{o.gecko_network}/dexes/{o.gecko_dex}/pools?page="
    g = {"data": [], "pages": []}
    if not (o.gecko_pools and o.gecko_dex):
        return u, g
    for page in range(1, o.gecko_pages + 1):
        r = requests.get(u + str(page), timeout=60, headers={"Accept": "application/json"}).json()
        g["pages"].append(r)
        g["data"] += r.get("data", [])
        if not r.get("data"):
            break
    return u, g


def by_pairs(n, s, o):
    """poolByPair(a, b) фабрики по всем парам --pair-token (на голове). Возвращает: (ответы, ненулевые пулы)."""
    toks = [t.lower() for t in o.pair_token]
    calls, keys = [], []
    for i, a in enumerate(toks):
        for b in toks[i + 1:]:
            calls.append(("eth_call", [{"to": o.factory, "data": s["poolByPair(address,address)"] + word(a) + word(b)},
                                       "latest"]))
            keys.append(f"{a}/{b}")
    got = n.batch(calls, o.batch) if calls else []
    pools = []
    for j in got:
        if "result" in j and int(j["result"], 16):
            pools.append(addr(words(j["result"])[0]))
    return dict(zip(keys, got)), pools


def heads(n, o):
    """Номер и время последних --heads блоков (заголовки). Возвращает: список {number, timestamp}."""
    if o.heads <= 0:
        return []
    top = int(n.ok("eth_blockNumber", []), 16)
    calls = [("eth_getBlockByNumber", [hex(b), False]) for b in range(top - o.heads + 1, top + 1)]
    out = []
    for j in n.batch(calls, o.batch):
        b = n.must(j, "eth_getBlockByNumber")["result"]
        out.append({"number": b["number"], "timestamp": b["timestamp"]})
    return out


def run(o):
    """Запись целиком (см. шапку). Ошибки: выход с причиной — узел не той сети, пулов нет."""
    n = Node(o.rpc)
    s = {k: n.sel(k) for k in SIGS}
    chain = n.ok("eth_chainId", [])
    if int(chain, 16) != o.chain_id:
        sys.exit(f"узел не той сети: chainId {int(chain, 16)}, ждём {o.chain_id}")
    u, g = gecko(o)
    pairs, found = by_pairs(n, s, o)
    pools = [p.lower() for p in o.pool] + [d["attributes"]["address"].lower() for d in g["data"][:o.gecko_pools]]
    pools = list(dict.fromkeys(pools + found))
    if not pools:
        sys.exit("нет пулов: GeckoTerminal и poolByPair пусты, --pool не задан")
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    out = os.path.join(o.out, f"{o.name}_{now}")
    os.makedirs(out, exist_ok=True)
    files = {}

    def save(name, obj):
        files[name] = True
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            json.dump(obj, f, indent=1, ensure_ascii=False)

    save("geckoterminal_pools.json", {"url": u, "response": g})
    save("meta.json", {"chainId": chain, "client": n.call("web3_clientVersion", []),
                       "args": {k: v for k, v in vars(o).items() if k != "rpc"}, "poolByPair": pairs})
    vq_addr = "0x" + n.sha3("0x" + b"arb.virtual_quoter".hex())[-40:]
    vq_code = "0x" + open(o.vq_runtime).read().strip().replace("0x", "")
    recs = []
    for p in pools:
        rec = pool_record(n, s, o, p, vq_addr, vq_code)
        save(f"pool_{p}.json", rec)
        recs.append(rec)
        print(f"пул {p}: блок {int(rec['block']['number'], 16)}, тиков {len(rec['ticks'])} / "
              f"{len(rec['ticks_compressed'])}, котировок {len(rec['quotes'])}")
    if o.heads > 0:
        save("heads.json", heads(n, o))
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
    named = [("deployer", deployer), ("factory", o.factory), ("pool", pools[0])]
    if o.quoter:
        named.append(("quoter", o.quoter))
    for name, a in named:
        url = f"https://sourcify.dev/server/v2/contract/{o.chain_id}/{a}?fields=compilation"
        try:
            r = requests.get(url, timeout=60, headers={"Accept": "application/json"})
            src[name] = {"url": url, "status": r.status_code, "body": r.text[:20000]}
        except OSError as e:  # сеть до Sourcify — не повод терять запись узла
            src[name] = {"url": url, "error": str(e)}
    save("sourcify.json", src)
    write_readme(out, files, fac, recs, o.chain_id)
    zpath = out + ".zip"
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(os.listdir(out)):
            z.write(os.path.join(out, f), os.path.join(os.path.basename(out), f))
    print(f"Архив для отправки: {zpath}")


def write_readme(out, files, fac, recs, chain_id):
    """README.md: факты записи (без выводов) и SHA-256 файлов."""
    lines = [f"# Пулы кода AlgebraV1 — запись своего узла ({os.path.basename(out)})", "",
             "Записано tools/bsc_record/thena_fusion_run.py (только чтение). Состав файлов — шапка thena_fusion.py.", "",
             f"- chainId: {chain_id}", f"- фабрика {fac['factory']}, poolDeployer {fac['poolDeployer']}, "
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
