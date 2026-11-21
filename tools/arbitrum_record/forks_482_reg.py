"""Файл: tools/arbitrum_record/forks_482_reg.py
Что делает: решение 482 (поручение координатора) — часть записи forks_482.py для источников пулов без индексатора:
  1. Swapline V1 (вид lb_v21): перечисление фабрики getNumberOfLBPairs() / getLBPairAtIndex(i) — эталон; по каждой
     паре getTokenX(), getTokenY(), getFactory() пары и вопрос бота getAllLBPairs(tokenX, tokenY) (factory_ask.rs Q::Lb).
  2. Metric V1 (вид metric_omm): вопросы бота metric_list.rs — nextPoolIdx(), idxToPool(1..n − 1), getImmutables()
     пулов; ответы проверки verify_metric.rs — isPool(пул) фабрики; eth_getCode первого пула (есть ли в коде topic0
     событий пула abi_metric.rs topics() — они записаны на BSC).
  3. Логи событий создания (подписка created_log.rs) фабрик обеих DEX за последние --reg-log-blocks блоков
     (recnode.logs_split; окна нет — пусто, это не ошибка): LBPairCreated, PoolCreated (Metric).
  Всё состояние (п. 1–2) — одним телом recnode.fresh на свежем блоке после логов (решение 460), четырьмя пачками
  JSON-RPC (ревью t251; _state).
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


def _state(b, tag):
    """Swapline и Metric на блоке tag — пачками по фазам (ревью t251): B1 — счётчики фабрик; B2 — getLBPairAtIndex и
    idxToPool всех номеров; B3 — getTokenX / getTokenY / getFactory пар, isPool / getImmutables пулов; B4 —
    getAllLBPairs пар и eth_getCode первого пула Metric. Возвращает: (swapline, metric)."""
    n_lb, n_mt = b.run([call(SWAPLINE, "0x" + sel("getNumberOfLBPairs()"), tag),
                        call(METRIC, "0x" + sel("nextPoolIdx()"), tag)])
    c_lb = word(n_lb["result"], 0) if n_lb.get("result") else 0
    c_mt = word(n_mt["result"], 0) if n_mt.get("result") else 0
    got = b.run([call(SWAPLINE, "0x" + sel("getLBPairAtIndex(uint256)") + w(i), tag) for i in range(c_lb)]
                + [call(METRIC, "0x" + sel("idxToPool(uint256)") + w(i), tag) for i in range(1, c_mt)])
    pairs = ["0x" + g["result"][-40:] for g in got[:c_lb] if g.get("result")]
    pools = [{"idx": i, "idxToPool": g} for i, g in zip(range(1, c_mt), got[c_lb:])]
    for r in pools:
        if r["idxToPool"].get("result"):
            r["pool"] = "0x" + r["idxToPool"]["result"][-40:]
    known = [r for r in pools if r.get("pool")]
    reqs = []
    for p in pairs:
        reqs += [call(p, "0x" + sel(x), tag) for x in ("getTokenX()", "getTokenY()", "getFactory()")]
    for r in known:
        reqs += [call(METRIC, "0x" + sel("isPool(address)") + ad(r["pool"]), tag),
                 call(r["pool"], "0x" + sel("getImmutables()"), tag)]
    got = b.run(reqs)
    lb = []
    for k, p in enumerate(pairs):
        t = got[3 * k:3 * k + 3]
        lb.append({"pair": p, "getTokenX": t[0], "getTokenY": t[1], "getFactory": t[2]})
    for k, r in enumerate(known):
        r["isPool"], r["getImmutables"] = got[3 * len(pairs) + 2 * k:3 * len(pairs) + 2 * k + 2]
    reqs, ask = [], []
    for rec in lb:
        if rec["getTokenX"].get("result") and rec["getTokenY"].get("result"):
            d = "0x" + sel("getAllLBPairs(address,address)") + ad("0x" + rec["getTokenX"]["result"][-40:]) \
                + ad("0x" + rec["getTokenY"]["result"][-40:])
            reqs.append(call(SWAPLINE, d, tag))
            ask.append(rec)
    first = known[0]["pool"] if known else None
    if first:
        reqs.append(("eth_getCode", [first, tag]))
    got = b.run(reqs)
    for rec, g in zip(ask, got):
        rec["getAllLBPairs"] = g
    code = {"pool": first, "code": got[-1].get("result"), "error": got[-1].get("error")} if first else None
    return {"getNumberOfLBPairs": n_lb, "pairs": lb}, {"nextPoolIdx": n_mt, "pools": pools, "code": code}


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
        lb, mt = _state(b, tag)
        return {"block": int(tag, 16), "swapline": lb, "metric": mt}

    st = recnode.fresh(lambda: b.one("eth_getBlockByNumber", ["latest", False])["result"], body, say=say)
    st.update(logs=logs, swapline_factory=SWAPLINE, metric_factory=METRIC,
              topic_lb_created=topic(LB_CREATED), topic_metric_created=topic(METRIC_CREATED))
    return st
