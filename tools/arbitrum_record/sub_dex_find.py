"""Файл: tools/arbitrum_record/sub_dex_find.py
Что делает: решение 460 п. 9 — пулы для sub_dex.py БЕЗ скана истории событий. Прогон 20.11.2026 у Тиграна (Nitro,
  state-scheme=path): поиск событий создания назад окнами eth_getLogs за 37 мин прошёл ~144 млн блоков из ~526 млн —
  на каждом окне таймаут узла и деление. Теперь:
  1. KyberSwap Classic — перечисление фабрики с конца списка: allPoolsLength() и allPools(i) (eth_call на latest;
     KyberNetwork/dmm-smart-contracts@e557b57 DMMFactory.sol L20 `address[] public override allPools`, L90-L92
     allPoolsLength; KSFactory.sol L24, L127-L129; KSFactory Legacy (событие с uint16 feeBps) — тот же репозиторий
     до 195e3e9: KSFactory.sol L23, L125-L127) и пулы GeckoTerminal kyberswap_classic_arbitrum, чья factory() — фабрика.
  2. Balancer V2 — перечисления у фабрик нет (только isPoolFromFactory) — пулы GeckoTerminal balancer_arbitrum; своя
     фабрика — isPoolFromFactory(pool) = 1 у фабрики списка (latest). Адрес GeckoTerminal — адрес пула (20 байт) или
     poolId (32 байта: старшие 20 байт — адрес пула, balancer-v2-monorepo pkg/vault/contracts/PoolRegistry.sol
     L116-L139 _toPoolId / _getPoolAddress) — тогда getPoolId() пула должен совпасть.
  3. iZiSwap — перечисления нет (izumiFinance/iZiSwap-core@3fe21eb iZiSwapFactory.sol L24: mapping pool(x, y, fee)),
     в GeckoTerminal DEX нет (tests/data/arbitrum_node_2026-11-19/geckoterminal/dexes_page1.json — 84 DEX сети) —
     реестр pool(x, y, fee) по парам токенов: --izi-token и токены пулов сети GeckoTerminal /networks/arbitrum/pools;
     пул ≠ 0 и factory() пула (iZiSwapPool.sol L50) = фабрика.
  4. Событие создания нужно только тесту KyberSwap Classic (tests/arbitrum_sub_dex_live.rs: kyber_classic_created_and_math
     читает p["log"] каждого пула; balancer_v2_versions_match_vault и iziswap_factory_registry событий не читают) — узким
     окном eth_getLogs вокруг блока создания пула: время — pool_created_at GeckoTerminal, блок — бинпоиск по времени
     заголовков eth_getBlockByNumber (recnode.block_at_time). Заголовки — не состояние: «The Nitro node databases have
     the raw data of all blocks, including pre-Nitro blocks» (docs.arbitrum.io/run-arbitrum-node/more-types/
     run-classic-node); geth хранит заголовки, тела и квитанции старых блоков во freezer отдельно от состояния
     (geth.ethereum.org/docs/fundamentals/databases). Узел не отдал заголовок — причина в записи пула, не зависание.
  Потолок времени (recnode.Deadline) проверяется между запросами; вышел — TimeUp, найденное остаётся в res.
Кто вызывает: tools/arbitrum_record/sub_dex.py.
От чего зависит: recnode.py (Deadline, TimeUp, logs_split, block_at_time), stdhttp.py (GeckoTerminal — темп gt.py).
Ошибки: TimeUp — потолок времени (ловит run); ошибки узла — как есть.
"""

import datetime
import json

import recnode
import stdhttp as requests

GT = "https://api.geckoterminal.com/api/v2/networks/arbitrum"
MIN_CHUNK = 1000  # наименьшая часть eth_getLogs при делении по таймауту (решение 460), блоков
ZERO = "0x" + "0" * 40


def _u(v):
    return format(v % (1 << 256), "064x")


def _a(a):
    return a.lower().replace("0x", "").rjust(64, "0")


def _addr(r):
    """Адрес из ответа eth_call ({"ok": hex}) или None."""
    h = r.get("ok") or ""
    return ("0x" + h[-40:]).lower() if len(h) >= 66 else None


def _int(r):
    h = r.get("ok") or ""
    return int(h[2:66], 16) if len(h) >= 66 else None


def run(fn, res, *args):
    """Шаг поиска с потолком: fn(res, *args) заполняет res; TimeUp — res["stopped"] = причина. Возвращает res."""
    try:
        fn(res, *args)
    except recnode.TimeUp as e:
        res["stopped"] = str(e)
    return res


def iso_ts(s):
    """«2021-07-15T07:20:16Z» (pool_created_at GeckoTerminal) → секунды UTC или None."""
    try:
        return int(datetime.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ")
                   .replace(tzinfo=datetime.timezone.utc).timestamp())
    except (TypeError, ValueError):
        return None


def gt_pages(path, pages, dl, out):
    """Страницы GET {GT}/{path}?page=N как есть — в список out ({"page", "status", "body"}); не 200 — стоп.
    Ошибки: TimeUp."""
    for p in range(1, pages + 1):
        dl.check(f"GeckoTerminal {path} страница {p}")
        r = requests.get(f"{GT}/{path}", params={"page": p}, headers={"Accept": "application/json"}, timeout=60)
        out.append({"page": p, "status": r.status_code, "body": r.text})
        if r.status_code != 200:
            break


def gt_rows(pages):
    """Пулы из страниц: [{"address", "created_at_raw", "created_at", "base", "quote"}]. Ошибок нет."""
    rows = []
    for pg in pages:
        try:
            data = json.loads(pg["body"]).get("data") or []
        except (ValueError, AttributeError):
            continue
        for it in data:
            at, rel = it.get("attributes") or {}, it.get("relationships") or {}

            def tok(k):
                return ((((rel.get(k) or {}).get("data") or {}).get("id")) or "").split("_")[-1].lower()
            rows.append({"address": (at.get("address") or "").lower(), "created_at_raw": at.get("pool_created_at"),
                         "created_at": iso_ts(at.get("pool_created_at")), "base": tok("base_token"),
                         "quote": tok("quote_token")})
    return rows


class Stamps:
    """Время заголовков блоков узла с кешем (для block_at_time)."""

    def __init__(self, node, dl):
        self.node, self.dl, self.cache = node, dl, {}

    def __call__(self, n):
        """Время заголовка блока n, с. Ошибки: RuntimeError — узел не отдал заголовок (причина — ответ узла)."""
        if n not in self.cache:
            j = self.node.rpc("eth_getBlockByNumber", [hex(n), False], timeout=self.dl.http(120))
            if j.get("error") or not j.get("result"):
                raise RuntimeError(f"узел не отдал заголовок блока {n}: {j.get('error') or 'result = null'}")
            self.cache[n] = int(j["result"]["timestamp"], 16)
        return self.cache[n]


def created_log(node, factory, topics0, pool, t0, t1, ts, head, a, dl, stamps, say):
    """Событие создания пула узким окном вокруг времени ts: окна ±m с для m из --created-margin по очереди, фильтр
    address = фабрика, topics = [подписи, token0, token1], совпадение — пул в первом слове данных. Возвращает:
    {"log" (или None), "windows": [[lo, hi, m]], "why"}. Ошибки: TimeUp; RuntimeError узла — в "why"."""
    res = {"log": None, "windows": [], "why": ""}
    try:
        for m in a.created_margin:
            lo = recnode.block_at_time(stamps, 0, head, ts - m, dl)
            hi = min(head, recnode.block_at_time(stamps, 0, head, ts + m + 1, dl) - 1)
            res["windows"].append([lo, hi, m])
            say(f"событие создания {pool}: блоки {lo}..{hi} (±{m} с от {ts})")

            def one(x, y):
                j = node.rpc("eth_getLogs", [{"address": factory, "topics": [topics0, "0x" + _a(t0), "0x" + _a(t1)],
                                              "fromBlock": hex(x), "toBlock": hex(y)}], timeout=dl.http(120))
                if "error" in j:
                    raise RuntimeError(f"eth_getLogs {x}..{y}: {j['error']}")
                return j["result"]
            for lg in recnode.logs_split(one, lo, hi, MIN_CHUNK, say, dl):
                if ("0x" + lg["data"][26:66]).lower() == pool:
                    res["log"] = lg
                    return res
        res["why"] = "в окнах событие создания не найдено"
    except RuntimeError as e:
        res["why"] = str(e)
    return res


def kyber(res, node, head, a, dl, sel, topics0, say):
    """KyberSwap Classic: res[spec] = {"length", "pools": [{"pool", "index"|"gt", "log_search"?}]}, res["gt"] —
    страницы GeckoTerminal. Пулы фабрики: из GeckoTerminal (своя factory(), до --max-pools) и хвост allPools (до
    --max-pools). Ошибки: TimeUp."""
    res["gt"] = []
    for spec in a.kyber:
        res[spec] = {"length": None, "pools": []}
    by_fac = {s.split(":")[1].lower(): s for s in a.kyber}
    if a.gt_pages:
        gt_pages("dexes/kyberswap_classic_arbitrum/pools", a.gt_pages, dl, res["gt"])
    for row in gt_rows(res["gt"]):
        if len(row["address"]) != 42:
            continue
        dl.check("factory() пулов GeckoTerminal")
        spec = by_fac.get(_addr(node.call(row["address"], sel("factory()"), "latest")) or "")
        if spec and len(res[spec]["pools"]) < a.max_pools:
            res[spec]["pools"].append({"pool": row["address"], "gt": row})
    for spec in a.kyber:
        factory = spec.split(":")[1]
        n = _int(node.call(factory, sel("allPoolsLength()"), "latest"))
        res[spec]["length"] = n
        have = {p["pool"] for p in res[spec]["pools"]}
        for i in (range(n - 1, max(-1, n - 1 - a.max_pools), -1) if n else ()):
            dl.check(f"allPools({i}) {factory}")
            p = _addr(node.call(factory, sel("allPools(uint256)") + _u(i), "latest"))
            if p and p != ZERO and p not in have:
                have.add(p)
                res[spec]["pools"].append({"pool": p, "index": i})
    stamps = Stamps(node, dl)
    for spec in a.kyber:
        for p in res[spec]["pools"]:
            ts = (p.get("gt") or {}).get("created_at")
            if ts is None:
                p["log_search"] = {"log": None, "why": "нет pool_created_at GeckoTerminal — время создания неизвестно"}
                continue
            t0 = _addr(node.call(p["pool"], sel("token0()"), "latest"))
            t1 = _addr(node.call(p["pool"], sel("token1()"), "latest"))
            if not (t0 and t1):
                p["log_search"] = {"log": None, "why": "token0()/token1() пула не ответили"}
                continue
            p["log_search"] = {"log": None, "why": "потолок времени вышел до поиска"}  # останется, если TimeUp
            p["log_search"] = created_log(node, spec.split(":")[1], topics0, p["pool"], t0, t1, ts, head, a, dl,
                                          stamps, say)


def balancer(res, node, a, dl, sel):
    """Balancer V2: res[spec] = {"pools": [адрес]}, res["skipped"] — пулы GeckoTerminal не из списка (с причиной),
    res["gt"] — страницы. Ошибки: TimeUp."""
    res["gt"], res["skipped"] = [], []
    facs = [s.split(":") for s in a.balancer_factory]
    for spec in a.balancer_factory:
        res[spec] = {"pools": []}
    if a.gt_pages:
        gt_pages("dexes/balancer_arbitrum/pools", a.gt_pages, dl, res["gt"])
    for row in gt_rows(res["gt"]):
        dl.check("проверка пулов Balancer GeckoTerminal")
        x = row["address"]
        pid, pool = (x, "0x" + x[2:42]) if len(x) == 66 else (None, x) if len(x) == 42 else (None, None)
        if not pool:
            res["skipped"].append({"address": x, "why": "адрес не 20 и не 32 байта"})
            continue
        got = node.call(pool, sel("getPoolId()"), "latest").get("ok")
        if pid and (got or "").lower() != pid:
            res["skipped"].append({"address": x, "why": f"getPoolId() пула = {got}"})
            continue
        own = [s for s, (_, _, f) in zip(a.balancer_factory, facs)
               if _int(node.call(f, sel("isPoolFromFactory(address)") + _a(pool), "latest")) == 1]
        if not own:
            res["skipped"].append({"address": x, "why": "isPoolFromFactory = 0 у всех фабрик списка"})
        elif len(res[own[0]]["pools"]) < a.max_pools and pool not in res[own[0]]["pools"]:
            res[own[0]]["pools"].append(pool)


def izi(res, node, a, dl, sel):
    """iZiSwap: res["pools"] — адреса пулов реестра pool(x, y, fee), чья factory() = фабрика; res["tokens"],
    res["gt"]. Ошибки: TimeUp."""
    res["gt"], res["pools"], res["asked"] = [], [], 0
    tokens = [t.lower() for t in a.izi_token]
    if a.izi_gt_pages:
        gt_pages("pools", a.izi_gt_pages, dl, res["gt"])
    for row in gt_rows(res["gt"]):
        for t in (row["base"], row["quote"]):
            if len(t) == 42 and t not in tokens and len(tokens) < a.izi_tokens + len(a.izi_token):
                tokens.append(t)
    res["tokens"] = tokens
    f = a.izi_factory.lower()
    for i in range(len(tokens)):
        for j in range(i + 1, len(tokens)):
            for fee in a.izi_fee:
                if len(res["pools"]) >= a.max_pools:
                    return
                dl.check("реестр pool(x, y, fee) iZiSwap")
                res["asked"] += 1
                p = _addr(node.call(f, sel("pool(address,address,uint24)") + _a(tokens[i]) + _a(tokens[j]) + _u(fee),
                                    "latest"))
                if p and p != ZERO and p not in res["pools"] and \
                        _addr(node.call(p, sel("factory()"), "latest")) == f:
                    res["pools"].append(p)
