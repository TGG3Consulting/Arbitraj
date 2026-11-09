"""Файл: docs/notes/scripts/reserve_analysis/node.py
Что делает: только-чтение узла BSC (geth): голова, самый ранний доступный блок (бинарный поиск по eth_getBlockByNumber),
метки блоков и блок по времени, eth_getLogs окнами по сетке блоков, eth_call token0()/token1(). Кэш ответов в файле
(возобновление после обрыва). Блоки старше самого раннего доступного — причина Pruned, без запроса к узлу; если задан
--archive-url — эти блоки и логи читаются у архивного узла (только разбор истории, не торговля).
Кто вызывает: pools.py, dexmove.py, run.py.
От чего зависит: rpc.py (транспорт, сторож срока), earliest.py (earliest), common.py, keccak.py; стандартная
библиотека.

Метка блока — поле timestamp заголовка (секунды): блок «на момент t» = последний блок с timestamp ≤ t.
Кэш: ответ null на eth_getBlockByNumber не кэшируется (история узла сдвигается); такие строки старого кэша
отбрасываются при загрузке. Ответы старше safety блоков от головы неизменны — кэш прошлых запусков используется и
ниже earliest (use_old; --ignore-cache-below-earliest — нет; счётчик cache_logs_below_earliest_used). Без кэша: логи
окна, частично старше earliest, запрашиваются с fromBlock = earliest (другой ключ), если нужная часть не старше
earliest; иначе — Pruned. Ответы архива — ключи с приставкой «arch ».
"""

import bisect
import json
import os
from decimal import Decimal

from common import log, iso
from keccak import SEL_TOKEN0, SEL_TOKEN1
from earliest import EarliestMixin
from rpc import Rpc

LOCAL = ("eth_blockNumber", "eth_getBlockByNumber", "eth_getLogs", "eth_call")
ARCHIVE = ("eth_getBlockByNumber", "eth_getLogs", "eth_call")


class Pruned(Exception):
    """Нужный блок старше самого раннего доступного узлу, архив не задан."""


class Node(EarliestMixin):
    """Клиент узла. url — bsc.node_http_url, rps — предел запросов в секунду (и у архива), cache_path — jsonl кэша,
    chunk — окно getLogs в блоках, attempt_s/deadline_s — срок попытки/запроса, archive_url — архивный узел или None,
    refresh_s — как часто перепроверять earliest, safety — сколько последних блоков не кэшировать."""

    def __init__(self, url, rps, cache_path, chunk, attempt_s, deadline_s, archive_url=None, refresh_s=600,
                 use_old=True, safety=64):
        self.rpc = Rpc(url, "узел", LOCAL, rps, attempt_s, deadline_s)
        self.arch = Rpc(archive_url, "архив", ARCHIVE, rps, attempt_s, deadline_s) if archive_url else None
        self.chunk, self.safety, self.refresh_s, self.use_old = chunk, safety, refresh_s, use_old
        self.hits, self.dropped_null, self.old_hits = 0, 0, 0
        self.cache = {}
        if os.path.exists(cache_path):
            with open(cache_path, encoding="utf-8") as f:
                for line in f:
                    try:
                        k, v = json.loads(line)
                    except ValueError:
                        continue  # оборванная последняя строка — повторим запрос
                    if v is None and "eth_getBlockByNumber" in k:
                        self.dropped_null += 1  # null прежней версии (история могла быть другой)
                        self.cache.pop(k, None)
                        continue
                    self.cache[k] = v
        self.cf = open(cache_path, "a", encoding="utf-8")
        self.ts_blocks, self.ts_vals, self.ts_map = [], [], {}
        self.head = self.head_ts = None
        self.earliest = self.earliest_ts = None
        self.earliest_at = 0.0
        self.earliest_log = []

    def _store(self, key, v):
        self.cache[key] = v
        self.cf.write(json.dumps([key, v]) + "\n")
        self.cf.flush()

    def call(self, method, params, cacheable, archive=False):
        """Запрос с кэшем (cacheable — ответ не может измениться; null не кэшируется). archive — к архивному узлу.
        Ошибки: NodeTimeout, RpcError, RuntimeError (rpc.py)."""
        key = ("arch " if archive else "") + method + " " + json.dumps(params, sort_keys=True)
        if cacheable and key in self.cache:
            self.hits += 1
            return self.cache[key]
        v = (self.arch if archive else self.rpc).call(method, params)
        if cacheable and v is not None:
            self._store(key, v)
        return v

    def refresh_head(self):
        """Номер и метка головы. Ошибки: как у call; RuntimeError — нет блока головы."""
        self.head = int(self.call("eth_blockNumber", [], False), 16)
        b = self.call("eth_getBlockByNumber", [hex(self.head), False], False)
        if b is None:
            raise RuntimeError("узел не отдал блок головы %d" % self.head)
        self.head_ts = int(b["timestamp"], 16)
        self._remember(self.head, self.head_ts)
        return self.head

    def _remember(self, n, t):
        if n not in self.ts_map:
            self.ts_map[n] = t
            i = bisect.bisect_left(self.ts_blocks, n)
            self.ts_blocks.insert(i, n)
            self.ts_vals.insert(i, t)

    def load_cached_ts(self):
        """После find_earliest: метки блоков из кэша (заголовки неизменны — кэшируются только старше safety от
        головы) — в карту времени; у своего узла ниже earliest — только если use_old. Ошибок нет."""
        for k, v in self.cache.items():
            arch = k.startswith("arch ")
            m = k[5:] if arch else k
            if not m.startswith("eth_getBlockByNumber ") or not isinstance(v, dict) or "timestamp" not in v:
                continue
            n = int(json.loads(m[len("eth_getBlockByNumber "):])[0], 16)
            if arch or n >= self.earliest or self.use_old:
                self._remember(n, int(v["timestamp"], 16))

    def block_ts(self, n):
        """Метка блока n (секунды). Ошибки: Pruned; RuntimeError — блока нет; NodeTimeout."""
        if n in self.ts_map:
            return self.ts_map[n]
        if n < self.earliest:
            if not self.arch:
                raise Pruned(self.pruned())
            b = self.call("eth_getBlockByNumber", [hex(n), False], True, archive=True)
            if b is None:
                raise RuntimeError("архив не отдал блок %d" % n)
        else:
            b = self.call("eth_getBlockByNumber", [hex(n), False], n <= self.head - self.safety)
            if b is None:
                self.refresh_head()
                self.find_earliest(self.earliest)  # история сдвинулась во время разбора
                if n < self.earliest:
                    return self.block_ts(n)
                raise RuntimeError("узел не отдал блок %d (earliest = %d, голова %d)" % (n, self.earliest, self.head))
        t = int(b["timestamp"], 16)
        self._remember(n, t)
        return t

    def block_at(self, t):
        """Последний блок с timestamp ≤ t (секунды). None — t не раньше метки головы. Ошибки: Pruned, RuntimeError."""
        self._fresh()
        t = int(Decimal(t))  # floor для положительных
        if t >= self.head_ts:
            return None
        # t раньше earliest: без архива ответ есть, только если нужные метки уже в кэше прошлых запусков (use_old);
        # первая же недостающая — Pruned из block_ts.
        i = bisect.bisect_right(self.ts_vals, t)
        lo = self.ts_blocks[i - 1] if i > 0 else None
        hi = self.ts_blocks[i]  # голова известна и её метка > t, значит i < len
        if lo is None:  # t раньше всех известных меток
            if not self.arch:
                raise Pruned(self.pruned())
            step = 1024
            lo = hi - step
            while self.block_ts(lo) > t:
                hi, step = lo, step * 2
                lo = max(0, hi - step)
        while hi - lo > 1:
            tl, th = self.ts_map[lo], self.ts_map[hi]
            if th > tl and hi - lo > 8:
                mid = min(max(lo + (hi - lo) * (t - tl) // (th - tl), lo + 1), hi - 1)
            else:
                mid = (lo + hi) // 2
            if self.block_ts(mid) <= t:
                lo = mid
            else:
                hi = mid
        return lo

    def chunk_logs(self, address, topics, k, need_from=None):
        """Логи окна k ([k·chunk, (k+1)·chunk − 1], обрезано по голове и по earliest) — сжатые: (блок, logIndex,
        topic1, data). need_from — с какого блока окно нужно целиком (None — годится и обрезанное по earliest).
        Кэшируется, если окно целиком старше safety блоков от головы. Ошибки: Pruned, как у call."""
        self._fresh()
        a = k * self.chunk
        b = min((k + 1) * self.chunk - 1, self.head)
        if a > b:
            return []
        # Кэш прошлых запусков — сначала (логи окна старше safety неизменны); ниже earliest — только если use_old.
        for pre in ("logs ", "arch logs "):
            full = pre + json.dumps({"address": address, "fromBlock": hex(a), "toBlock": hex(b), "topics": topics},
                                    sort_keys=True)
            if full in self.cache and (a >= self.earliest or self.use_old or pre != "logs "):
                self.hits += 1
                if a < self.earliest and pre == "logs ":
                    self.old_hits += 1
                return self.cache[full]
        arch = False
        if b < self.earliest or (a < self.earliest and self.arch):
            if not self.arch:
                raise Pruned(self.pruned())
            arch = True
        elif a < self.earliest:
            if need_from is not None and need_from < self.earliest:
                raise Pruned(self.pruned())  # нужная часть окна старше истории — обрезать нельзя
            a = self.earliest
        flt = {"address": address, "fromBlock": hex(a), "toBlock": hex(b), "topics": topics}
        key = ("arch " if arch else "") + "logs " + json.dumps(flt, sort_keys=True)
        if key in self.cache:
            self.hits += 1
            return self.cache[key]
        raw = (self.arch if arch else self.rpc).call("eth_getLogs", [flt]) or []
        out = []
        for e in raw:
            if e.get("removed"):
                continue
            tp = e.get("topics") or []
            out.append([int(e["blockNumber"], 16), int(e["logIndex"], 16), tp[1] if len(tp) > 1 else None, e["data"]])
        out.sort()
        if b <= self.head - self.safety:
            self._store(key, out)
        return out

    def tokens(self, pool):
        """token0(), token1() пула — eth_call на голове своего узла. Возвращает: (t0, t1) строчными.
        Ошибки: как у call; ValueError — пустой ответ (у контракта нет функции)."""
        out = []
        for sel in (SEL_TOKEN0, SEL_TOKEN1):
            r = self.call("eth_call", [{"to": pool, "data": sel}, "latest"], True)
            if not r or len(r) < 66:
                raise ValueError("eth_call %s у %s: пустой ответ" % (sel, pool))
            out.append("0x" + r[-40:].lower())
        return out[0], out[1]

    def stats(self):
        """Сводка. Ошибок нет."""
        s = {"node": self.rpc.stats(), "cache_hits": self.hits, "cache_entries": len(self.cache),
             "cache_null_blocks_dropped": self.dropped_null, "cache_logs_below_earliest_used": self.old_hits,
             "head": self.head, "head_at": iso(self.head_ts),
             "earliest": self.earliest, "earliest_at": iso(self.earliest_ts), "earliest_probe_errors": self.earliest_log[:5]}
        if self.arch:
            s["archive"] = self.arch.stats()
        return s

    def close(self):
        self.cf.close()
        log("узел: %s" % self.stats())
