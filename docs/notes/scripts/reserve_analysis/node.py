"""Файл: docs/notes/scripts/reserve_analysis/node.py
Что делает: только-чтение JSON-RPC узла BSC (geth): eth_blockNumber, eth_getBlockByNumber (без транзакций), eth_getLogs
(окнами, выровненными по сетке блоков), eth_call (token0()/token1() пула). Предел частоты, повторы, кэш ответов в файле
(возобновление после обрыва), поиск блока по времени по временным меткам блоков (не по фиксированному времени блока).
Кто вызывает: pools.py, run.py.
От чего зависит: common.py, keccak.py; стандартная библиотека.

Никаких eth_sendRawTransaction и прочих меняющих методов: список ALLOWED, иное — ошибка до запроса.
Временная метка блока — поле timestamp заголовка (секунды): блок «на момент t» = последний блок с timestamp ≤ t
(точность — секунда, у BSC это ~2 блока: 450 мс/блок по данным бота, docs/notes/transit-times-2026-11-09.md п.1).
"""

import bisect
import json
import os
import time
import urllib.error
import urllib.request
from decimal import Decimal

from common import log
from keccak import SEL_TOKEN0, SEL_TOKEN1

ALLOWED = {"eth_blockNumber", "eth_getBlockByNumber", "eth_getLogs", "eth_call"}


class Node:
    """Клиент узла. url — bsc.node_http_url, rps — предел запросов в секунду, cache_path — jsonl кэша,
    chunk — размер окна getLogs в блоках, safety — сколько последних блоков не кэшировать (могут смениться)."""

    def __init__(self, url, rps, cache_path, chunk, safety=64, timeout=60):
        self.url = url
        self.gap = 1.0 / rps
        self.next_t = 0.0
        self.chunk = chunk
        self.safety = safety
        self.timeout = timeout
        self.calls = 0
        self.hits = 0
        self.cache = {}
        self.cache_path = cache_path
        if os.path.exists(cache_path):
            with open(cache_path, encoding="utf-8") as f:
                for line in f:
                    try:
                        k, v = json.loads(line)
                    except ValueError:
                        continue  # оборванная последняя строка — повторим запрос
                    self.cache[k] = v
        self.cf = open(cache_path, "a", encoding="utf-8")
        self.ts_blocks = []  # отсортированные номера блоков с известной меткой
        self.ts_vals = []  # их метки (по возрастанию блока — не убывают)
        self.ts_map = {}
        self.head = None
        self.head_ts = None

    def _rpc(self, method, params):
        if method not in ALLOWED:
            raise RuntimeError("метод %s запрещён (только чтение)" % method)
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        last = None
        for i in range(6):
            wait = self.next_t - time.time()
            if wait > 0:
                time.sleep(wait)
            self.next_t = max(time.time(), self.next_t) + self.gap
            req = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
            try:
                self.calls += 1
                with urllib.request.urlopen(req, timeout=self.timeout) as r:
                    resp = json.loads(r.read().decode())
                if "error" in resp:
                    last = "ошибка узла: %s" % resp["error"]
                else:
                    return resp.get("result")
            except (urllib.error.URLError, OSError, ValueError) as e:
                last = repr(e)
            time.sleep(1.5 * (i + 1))
        raise RuntimeError("%s %s: %s" % (method, json.dumps(params)[:200], last))

    def call(self, method, params, cacheable):
        """Запрос с кэшем (cacheable — ответ не может измениться). Ошибки: RuntimeError после 6 попыток."""
        key = method + " " + json.dumps(params, sort_keys=True)
        if cacheable and key in self.cache:
            self.hits += 1
            return self.cache[key]
        v = self._rpc(method, params)
        if cacheable:
            self.cache[key] = v
            self.cf.write(json.dumps([key, v]) + "\n")
            self.cf.flush()
        return v

    def refresh_head(self):
        """Номер и метка головы. Ошибки: RuntimeError."""
        self.head = int(self.call("eth_blockNumber", [], False), 16)
        b = self.call("eth_getBlockByNumber", [hex(self.head), False], False)
        self.head_ts = int(b["timestamp"], 16)
        self._remember(self.head, self.head_ts)
        return self.head

    def _remember(self, n, t):
        if n not in self.ts_map:
            self.ts_map[n] = t
            i = bisect.bisect_left(self.ts_blocks, n)
            self.ts_blocks.insert(i, n)
            self.ts_vals.insert(i, t)

    def block_ts(self, n):
        """Метка блока n (секунды). Ошибки: RuntimeError, блока нет (None от узла)."""
        if n in self.ts_map:
            return self.ts_map[n]
        b = self.call("eth_getBlockByNumber", [hex(n), False], n <= self.head - self.safety)
        if b is None:
            raise RuntimeError("узел не отдал блок %d (обрезана история?)" % n)
        t = int(b["timestamp"], 16)
        self._remember(n, t)
        return t

    def block_at(self, t):
        """Последний блок с timestamp ≤ t (t — секунды, Decimal/float). None — t не раньше метки головы (ещё не
        наступило). Ошибки: RuntimeError."""
        t = int(Decimal(t))  # floor для положительных
        if t >= self.head_ts:
            return None
        # Скобка из уже известных блоков: lo — метка ≤ t, hi — метка > t.
        i = bisect.bisect_right(self.ts_vals, t)
        lo = self.ts_blocks[i - 1] if i > 0 else None
        hi = self.ts_blocks[i]  # голова известна и её метка > t, значит i < len
        if lo is None:
            step = 1024
            lo = hi - step
            while self.block_ts(lo) > t:
                hi, step = lo, step * 2
                lo = max(0, hi - step)
        while hi - lo > 1:
            tl, th = self.ts_map[lo], self.ts_map[hi]
            if th > tl and hi - lo > 8:
                guess = lo + (hi - lo) * (t - tl) // (th - tl)
                mid = min(max(guess, lo + 1), hi - 1)
            else:
                mid = (lo + hi) // 2
            if self.block_ts(mid) <= t:
                lo = mid
            else:
                hi = mid
        return lo

    def chunk_logs(self, address, topics, k):
        """Логи окна k ([k·chunk, (k+1)·chunk − 1], обрезано по голове) — сжатые: (блок, logIndex, topic1, data).
        Кэшируется, если окно целиком старше safety блоков от головы. Ошибки: RuntimeError."""
        a = k * self.chunk
        b = min((k + 1) * self.chunk - 1, self.head)
        if a > b:
            return []
        flt = {"address": address, "fromBlock": hex(a), "toBlock": hex(b), "topics": topics}
        key = "logs " + json.dumps(flt, sort_keys=True)
        if key in self.cache:
            self.hits += 1
            return self.cache[key]
        raw = self._rpc("eth_getLogs", [flt]) or []
        out = []
        for e in raw:
            if e.get("removed"):
                continue
            tp = e.get("topics") or []
            out.append([int(e["blockNumber"], 16), int(e["logIndex"], 16), tp[1] if len(tp) > 1 else None, e["data"]])
        out.sort()
        if b <= self.head - self.safety:
            self.cache[key] = out
            self.cf.write(json.dumps([key, out]) + "\n")
            self.cf.flush()
        return out

    def tokens(self, pool):
        """token0(), token1() пула (неизменяемые поля пары V2/V3) — eth_call на голове. Возвращает: (t0, t1) строчными.
        Ошибки: RuntimeError (узел), ValueError (пустой ответ — у контракта нет функции)."""
        out = []
        for sel in (SEL_TOKEN0, SEL_TOKEN1):
            r = self.call("eth_call", [{"to": pool, "data": sel}, "latest"], True)
            if not r or len(r) < 66:
                raise ValueError("eth_call %s у %s: пустой ответ" % (sel, pool))
            out.append("0x" + r[-40:].lower())
        return out[0], out[1]

    def stats(self):
        """Сводка запросов. Ошибок нет."""
        return {"rpc_calls": self.calls, "cache_hits": self.hits, "cache_entries": len(self.cache)}

    def close(self):
        self.cf.close()
        log("узел: запросов %d, из кэша %d" % (self.calls, self.hits))
