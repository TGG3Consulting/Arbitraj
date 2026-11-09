"""Файл: docs/notes/scripts/reserve_analysis/earliest.py
Что делает: самый ранний блок, который отдаёт свой узел BSC (earliest), — бинарный поиск по eth_getBlockByNumber,
перепроверка раз в refresh_s (история узла с обрезкой сдвигается вместе с головой), текст причины «блок старше
самого раннего доступного узлу». Примесь к Node (node.py): использует его rpc, call, head, refresh_head, _remember.
Кто вызывает: node.py (Node наследует), run.py и nodeinfo.py (find_earliest).
От чего зависит: rpc.py (RpcError), common.py.

Почему: первый запуск на сервере — узел вернул null на eth_getBlockByNumber(132009995) при голове 132611083
(summary.json, not_computed_reasons); строки и сделки старше этой границы не считаются, а не «цена не менялась».
"""

import time

from common import log, iso
from rpc import RpcError


class EarliestMixin:
    """earliest, earliest_ts — блок и его метка; earliest_log — ошибки JSON-RPC при поиске (ответ узла как есть)."""

    def _probe(self, n):
        """Блок n у своего узла (без кэша): заголовок или None (null или ошибка JSON-RPC — текст в earliest_log).
        Ошибки: NodeTimeout, RuntimeError — сеть."""
        try:
            return self.rpc.call("eth_getBlockByNumber", [hex(n), False], retries=2)
        except RpcError as e:
            self.earliest_log.append("блок %d: %s" % (n, str(e)[:200]))
            return None

    def find_earliest(self, lo=1):
        """Самый ранний блок, который узел отдаёт: бинарный поиск в [lo, голова] (доступность считается сплошной от
        earliest до головы). Генезис (0) не проверяется — он у geth есть всегда. Возвращает: номер.
        Ошибки: NodeTimeout, RuntimeError."""
        b = self._probe(lo)
        if b is None:
            hi = self.head
            while hi - lo > 1:
                mid = (lo + hi) // 2
                if self._probe(mid) is None:
                    lo = mid
                else:
                    hi = mid
            lo = hi
            b = self.call("eth_getBlockByNumber", [hex(lo), False], False)
        if b is None:
            raise RuntimeError("узел не отдал блок %d при поиске earliest" % lo)
        old = self.earliest
        self.earliest, self.earliest_ts, self.earliest_at = lo, int(b["timestamp"], 16), time.monotonic()
        self._remember(lo, self.earliest_ts)
        if old != lo:
            log("узел: самый ранний доступный блок %d (%s), голова %s" % (lo, iso(self.earliest_ts), self.head))
        return lo

    def _fresh(self):
        """earliest старше refresh_s — перепроверка. Ошибки: как у call."""
        if time.monotonic() - self.earliest_at > self.refresh_s:
            self.refresh_head()
            self.find_earliest(self.earliest)

    def pruned(self):
        """Текст причины для счётчиков (число — earliest на момент проверки). Ошибок нет."""
        return "блок старше самого раннего доступного узлу (earliest = %d)" % self.earliest
