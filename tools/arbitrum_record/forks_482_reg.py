"""Файл: tools/arbitrum_record/forks_482_reg.py
Что делает: решение 482 (поручение координатора) — часть записи forks_482.py для источников пулов без индексатора:
  1. Swapline V1 (вид lb_v21): перечисление фабрики getNumberOfLBPairs() / getLBPairAtIndex(i) — эталон; по каждой
     паре getTokenX(), getTokenY(), getFactory() пары и вопрос бота getAllLBPairs(tokenX, tokenY) (factory_ask.rs Q::Lb).
  2. Metric V1 (вид metric_omm): вопросы бота metric_list.rs — nextPoolIdx(), idxToPool(1..n − 1), getImmutables()
     пулов; ответы проверки verify_metric.rs — isPool(пул) фабрики; eth_getCode первого пула (есть ли в коде topic0
     событий пула abi_metric.rs topics() — они записаны на BSC).
  3. Логи событий создания (подписка created_log.rs) фабрик обеих DEX за последние --reg-log-blocks блоков
     (recnode.logs_split; окна нет — пусто, это не ошибка): LBPairCreated, PoolCreated (Metric).
  Всё состояние (п. 1–2) — одним телом recnode.fresh на свежем блоке после логов (решение 460).
Кто вызывает: tools/arbitrum_record/forks_482.py (record).
От чего зависит: recnode.py (fresh, logs_split), curve_plain_pools.py (Batch, call), v4live_rpc.py (sel, w, ad, word,
  topic), curve_crypto_rpc.py (keccak256).
Ошибки: сетевые и StaleState после трёх попыток — наружу (forks_482.py пишет причину в reg.json).

Подписи (Sourcify 42161, 20.11.2026): LBFactory Swapline 0xEE06…9cEa (exact_match = lfj-gg/joe-v2 v2.1.0) —
  getNumberOfLBPairs(), getLBPairAtIndex(uint256), getAllLBPairs(address,address) (LBFactory.sol L265), событие
  LBPairCreated(address,address,uint256,address,uint256) (ILBFactory.sol); MetricOmmPoolFactory 0x2a53…8f32 (match) —
  nextPoolIdx() L101, idxToPool(uint256) L104, isPool(address) L198, событие PoolCreated (IMetricOmmPoolFactory.sol,
  подпись ниже — ExtensionOrders = 6 × uint256).
"""

import recnode
from curve_plain_pools import call
from v4live_rpc import ad, sel, topic, w, word

SWAPLINE = "0xEE0616a2DEAa5331e2047Bc61E0b588195A49cEa"
METRIC = "0x2a53833cc95548cf52c7b159110e22D3a9018f32"
LB_CREATED = "LBPairCreated(address,address,uint256,address,uint256)"
METRIC_CREATED = ("PoolCreated(address,address,address,uint256,address,address,address,address[],"
                  "(uint256,uint256,uint256,uint256,uint256,uint256),uint256,uint256,uint256,uint256,uint24,uint24,"
                  "uint24,uint24,address,int24,uint256[],uint256[])")


def _logs(b, addr, sig, head, blocks, say):
    """Логи события sig контракта addr за последние blocks блоков (окно пополам при таймауте)."""
    flt = {"address": addr, "topics": [topic(sig)]}

    def fetch(x, y):
        got = b.one("eth_getLogs", [dict(flt, fromBlock=hex(x), toBlock=hex(y))])
        if "result" not in got:
            raise RuntimeError(str(got.get("error")))  # таймаут узла — logs_split делит окно
        return got["result"]

    return recnode.logs_split(fetch, max(0, head - blocks), head, 10_000, say)


def _lb(b, tag):
    """Swapline: перечисление и вопросы бота на блоке tag."""
    n = b.run([call(SWAPLINE, "0x" + sel("getNumberOfLBPairs()"), tag)])[0]
    cnt = word(n["result"], 0) if n.get("result") else 0
    pairs = b.run([call(SWAPLINE, "0x" + sel("getLBPairAtIndex(uint256)") + w(i), tag) for i in range(cnt)])
    out = []
    for g in pairs:
        if not g.get("result"):
            continue
        p = "0x" + g["result"][-40:]
        t = b.run([call(p, "0x" + sel(s), tag) for s in ("getTokenX()", "getTokenY()", "getFactory()")])
        rec = {"pair": p, "getTokenX": t[0], "getTokenY": t[1], "getFactory": t[2]}
        if t[0].get("result") and t[1].get("result"):
            d = "0x" + sel("getAllLBPairs(address,address)") + ad("0x" + t[0]["result"][-40:]) \
                + ad("0x" + t[1]["result"][-40:])
            rec["getAllLBPairs"] = b.run([call(SWAPLINE, d, tag)])[0]
        out.append(rec)
    return {"getNumberOfLBPairs": n, "pairs": out}


def _metric(b, tag):
    """Metric: вопросы metric_list.rs и verify_metric.rs на блоке tag."""
    n = b.run([call(METRIC, "0x" + sel("nextPoolIdx()"), tag)])[0]
    cnt = word(n["result"], 0) if n.get("result") else 0
    idx = b.run([call(METRIC, "0x" + sel("idxToPool(uint256)") + w(i), tag) for i in range(1, cnt)])
    pools = []
    for i, g in zip(range(1, cnt), idx):
        rec = {"idx": i, "idxToPool": g}
        if g.get("result"):
            p = "0x" + g["result"][-40:]
            got = b.run([call(METRIC, "0x" + sel("isPool(address)") + ad(p), tag),
                         call(p, "0x" + sel("getImmutables()"), tag)])
            rec.update(pool=p, isPool=got[0], getImmutables=got[1])
        pools.append(rec)
    code = None
    first = next((r["pool"] for r in pools if r.get("pool")), None)
    if first:
        c = b.one("eth_getCode", [first, tag])
        code = {"pool": first, "code": c.get("result"), "error": c.get("error")}
    return {"nextPoolIdx": n, "pools": pools, "code": code}


def record(b, log_blocks, say):
    """Запись источников пулов. Принимает: Batch, сколько блоков логов, печать. Возвращает: словарь для reg.json."""
    head = int(b.one("eth_blockNumber", [])["result"], 16)
    logs = {}
    for name, addr, sig in (("swapline", SWAPLINE, LB_CREATED), ("metric", METRIC, METRIC_CREATED)):
        try:
            logs[name] = {"from": head - log_blocks, "to": head, "logs": _logs(b, addr, sig, head, log_blocks, say)}
        except Exception as e:  # noqa: BLE001 — причина в запись, состояние пишется дальше
            logs[name] = {"error": str(e)[:300]}

    def body(h):
        tag = h["number"]
        return {"block": int(tag, 16), "swapline": _lb(b, tag), "metric": _metric(b, tag)}

    st = recnode.fresh(lambda: b.one("eth_getBlockByNumber", ["latest", False])["result"], body, say=say)
    st.update(logs=logs, swapline_factory=SWAPLINE, metric_factory=METRIC,
              topic_lb_created=topic(LB_CREATED), topic_metric_created=topic(METRIC_CREATED))
    return st
