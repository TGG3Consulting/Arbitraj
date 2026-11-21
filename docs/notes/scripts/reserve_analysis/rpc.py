"""Файл: docs/notes/scripts/reserve_analysis/rpc.py
Что делает: транспорт JSON-RPC к узлу BSC только на чтение — предел частоты (по монотонным часам), общий срок на запрос
(сторож: запрос идёт в отдельном потоке, ожидание — join с пределом; зависшее чтение не держит разбор), повторы при
сбое внутри общего срока, запись в журнал прогресса ПЕРЕД каждым запросом (метод и параметры кратко) и после медленного.
Кто вызывает: node.py (свой узел и, если задан --archive-url, архивный узел — только для разбора истории).
От чего зависит: common.py (log); стандартная библиотека.

Почему сторож: urlopen(timeout) ограничивает каждую операцию сокета, но не запрос целиком (6 попыток × 60 с + паузы у
прежней версии — до ~6.5 мин на запрос без строки в журнале); первый запуск на сервере стоял без CPU на сделках, чьи
блоки старше истории узла (docs/notes/reserve-analysis-2026-11-09.md, раздел 8).
"""

import json
import threading
import time
import urllib.error
import urllib.request

from common import log


class NodeTimeout(RuntimeError):
    """Узел не ответил за общий срок запроса (deadline)."""


class RpcError(RuntimeError):
    """Узел ответил ошибкой JSON-RPC (поле error) — текст ответа как есть."""


def brief(method, params):
    """Кратко параметры для журнала: окно getLogs, номер блока, адрес. Ошибок нет."""
    try:
        if method == "eth_getLogs":
            f = params[0]
            a, b = int(f["fromBlock"], 16), int(f["toBlock"], 16)
            return "%s блоки %d–%d" % (f.get("address"), a, b)
        if method == "eth_getBlockByNumber":
            return "блок %d" % int(params[0], 16)
        if method == "eth_call":
            return "%s %s" % (params[0].get("to"), params[0].get("data"))
    except (KeyError, ValueError, TypeError, IndexError, AttributeError):
        pass
    return json.dumps(params)[:160]


class Rpc:
    """Клиент одного узла. label — «узел»/«архив» (для журнала), allowed — разрешённые методы, rps — запросов в секунду,
    attempt_s — предел одной попытки, deadline_s — общий срок запроса со всеми повторами."""

    def __init__(self, url, label, allowed, rps, attempt_s, deadline_s):
        self.url, self.label, self.allowed = url, label, set(allowed)
        self.gap = 1.0 / rps
        self.next_t = 0.0
        self.attempt_s, self.deadline_s = attempt_s, deadline_s
        self.calls = 0
        self.timeouts = 0
        self.slow = 0

    def _pace(self):
        """Предел частоты по монотонным часам (перевод системных часов не даёт долгого сна). Ошибок нет."""
        now = time.monotonic()
        wait = min(self.next_t - now, self.gap)
        if wait > 0:
            time.sleep(wait)
        self.next_t = max(time.monotonic(), self.next_t) + self.gap

    def _attempt(self, body, limit):
        """Одна попытка в отдельном потоке; ждём не дольше limit. Возвращает: тело ответа (bytes).
        Ошибки: NodeTimeout — не уложилась; иначе исключение потока (URLError, OSError…)."""
        box = {}

        def work():
            try:
                req = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
                with urllib.request.urlopen(req, timeout=limit) as r:
                    box["data"] = r.read()
            except BaseException as e:  # noqa: B902 — передаём в основной поток как есть
                box["err"] = e

        t = threading.Thread(target=work, daemon=True)
        t.start()
        t.join(limit)
        if t.is_alive():
            raise NodeTimeout("попытка дольше %.0f с" % limit)
        if "err" in box:
            raise box["err"]
        return box["data"]

    def call(self, method, params, retries=6):
        """Запрос: не больше retries попыток и не дольше deadline_s всего. Возвращает: result ответа.
        Ошибки: RuntimeError — метод запрещён; NodeTimeout — «<label> не ответил за N с»; RpcError — ответ с error
        (после повторов); RuntimeError — сбой сети/разбора после повторов."""
        if method not in self.allowed:
            raise RuntimeError("метод %s запрещён для %s (только чтение)" % (method, self.label))
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        what = "%s %s" % (method, brief(method, params))
        t0 = time.monotonic()
        last, last_rpc = None, None
        for i in range(retries):
            left = self.deadline_s - (time.monotonic() - t0)
            if left <= 0:
                break
            self._pace()
            self.calls += 1
            log("%s → %s (попытка %d)" % (self.label, what, i + 1), quiet=True)
            ts = time.monotonic()
            try:
                resp = json.loads(self._attempt(body, min(self.attempt_s, left)).decode())
                took = time.monotonic() - ts
                if took > 5:
                    self.slow += 1
                    log("%s ← %s: ответ за %.1f с" % (self.label, what, took), quiet=True)
                if "error" in resp:
                    last = last_rpc = "ошибка JSON-RPC: %s" % json.dumps(resp["error"], ensure_ascii=False)[:300]
                else:
                    return resp.get("result")
            except NodeTimeout as e:
                last = str(e)
            except (urllib.error.URLError, OSError, ValueError) as e:
                last = repr(e)[:300]
            log("%s ← %s: %s" % (self.label, what, last), quiet=True)
            pause = min(1.5 * (i + 1), self.deadline_s - (time.monotonic() - t0))
            if i + 1 < retries and pause > 0:
                time.sleep(pause)
        spent = time.monotonic() - t0
        if last_rpc is not None and last == last_rpc:
            raise RpcError("%s: %s" % (what, last_rpc))
        if spent >= self.deadline_s or (last or "").startswith("попытка дольше"):
            self.timeouts += 1
            log("%s: %s — срок %.0f с вышел, пропуск (последнее: %s)" % (self.label, what, self.deadline_s, last))
            raise NodeTimeout("%s не ответил за %.0f с (%s)" % (self.label, self.deadline_s, method))
        raise RuntimeError("%s: %s" % (what, last))

    def stats(self):
        """Сводка запросов. Ошибок нет."""
        return {"calls": self.calls, "timeouts": self.timeouts, "slow_over_5s": self.slow}
