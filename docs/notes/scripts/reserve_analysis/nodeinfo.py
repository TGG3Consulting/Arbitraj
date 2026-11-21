"""Файл: docs/notes/scripts/reserve_analysis/nodeinfo.py
Что делает: диагностика узла без разбора (run.py --node-info): голова, самый ранний доступный блок (бинарный поиск по
eth_getBlockByNumber), и для каждого --probe-block — что узел отвечает на eth_getBlockByNumber и на eth_getLogs одного
блока (без адреса: число логов или текст ошибки / «не ответил за N с»). Только чтение; кэш не пишется.
Кто вызывает: run.py.
От чего зависит: node.py, rpc.py, common.py.
"""

from common import iso
from rpc import NodeTimeout, RpcError


def probe(node, n):
    """Ответы узла на блок n. Возвращает: словарь с текстом ответов. Ошибок нет (всё — текстом)."""
    out = {"block": n}
    for name, method, params in (
            ("eth_getBlockByNumber", "eth_getBlockByNumber", [hex(n), False]),
            ("eth_getLogs", "eth_getLogs", [{"fromBlock": hex(n), "toBlock": hex(n)}])):
        try:
            v = node.rpc.call(method, params, retries=1)
            if v is None:
                out[name] = "null"
            elif method == "eth_getLogs":
                out[name] = "логов: %d" % len(v)
            else:
                out[name] = "есть, timestamp %d (%s), hash %s" % (int(v["timestamp"], 16), iso(int(v["timestamp"], 16)),
                                                                 v.get("hash"))
        except (NodeTimeout, RpcError, RuntimeError) as e:
            out[name] = "%s: %s" % (type(e).__name__, str(e)[:300])
    return out


def node_info(node, blocks):
    """Голова, earliest, ответы на блоки blocks и на earliest − 1 / earliest. Ошибки: как у Node (голова)."""
    node.refresh_head()
    node.find_earliest()
    res = {"head": node.head, "head_at": iso(node.head_ts), "earliest": node.earliest,
           "earliest_at": iso(node.earliest_ts), "blocks_kept": node.head - node.earliest + 1,
           "earliest_probe_errors": node.earliest_log[:5], "probes": []}
    seen = set()
    for n in list(blocks) + ([node.earliest - 1] if node.earliest > 1 else []) + [node.earliest]:
        if n not in seen:
            seen.add(n)
            res["probes"].append(probe(node, n))
    return res
