"""Файл: tools/arbitrum_record/recnode.py
Что делает: решение 460 — общие правила записи с узла, который хранит только недавнее состояние (Nitro,
  state-scheme=path: состояние последних ~128 блоков, ≈ 30 с на Arbitrum; логи и квитанции старых блоков есть).
  1. logs_split — eth_getLogs окна [lo, hi]; узел не успел (таймаут) — окно делится пополам, до min_span блоков.
     Факт прогона 20.11.2026: camelot_v2.py — {'code': -32002, 'message': 'request timed out'} на топик-фильтре без
     address окном 2 000 000 блоков.
  2. fresh — фаза чтения СОСТОЯНИЯ (eth_call, eth_getCode, eth_getStorageAt, eth_getBalance) на свежем блоке B,
     взятом ПОСЛЕ долгих операций (поиск логов, GeckoTerminal); узел ответил «historical state … is not available»
     (stdhttp.StaleState) — фаза повторяется целиком на новом блоке B. Факт прогона 20.11.2026: lb_v22.py —
     eth_getCode на блоке, взятом до 500 с поиска логов.
  3. Решение 460 п. 9: Deadline / TimeUp — потолок времени скрипта (проверяется между запросами; logs_split проверяет его
     перед каждой частью); block_at_time — первый блок со временем заголовка ≥ t бинпоиском по eth_getBlockByNumber
     (заголовки — не состояние: «The Nitro node databases have the raw data of all blocks, including pre-Nitro
     blocks» — docs.arbitrum.io/run-arbitrum-node/more-types/run-classic-node).
Кто вызывает: tools/arbitrum_record/camelot_v2.py (+ camelot_v2_logs.py), lb_v22.py, ekubo_more.py, sub_dex.py
  (+ sub_dex_find.py), balancer_v3.py, kyber_elastic_v1.py, ekubo.py. (algebra_dir_fee.py и exec_v4_live.py — только
  порядок «GeckoTerminal / логи → блок B»: фаза состояния у них короткая, StaleState — выход с причиной.)
От чего зависит: stdhttp.py (StaleState).
Ошибки: см. функции.
"""

import socket
import sys
import time

from stdhttp import StaleState

# Признаки таймаута узла: код -32002 и текст ответа Nitro (факт прогона 20.11.2026), таймаут чтения HTTP.
TIMEOUT_MARKS = ("-32002", "timed out")
# Сколько раз фаза состояния начинается заново на новом блоке.
TRIES = 3


def is_timeout(err):
    """Ошибка — таймаут узла (код -32002 / «timed out») или чтения HTTP? Ошибок нет."""
    if isinstance(err, (socket.timeout, TimeoutError)):
        return True
    text = str(err)
    return any(m in text for m in TIMEOUT_MARKS)


class TimeUp(Exception):
    """Решение 460 п. 9: потолок времени скрипта вышел (текст — что делали в этот момент)."""


class Deadline:
    """Решение 460 п. 9: потолок времени скрипта. Принимает: секунды (0 или None — без потолка). Ошибок нет."""

    def __init__(self, seconds):
        self.seconds = seconds
        self.end = time.time() + seconds if seconds else None

    def left(self):
        """Сколько секунд осталось (без потолка — бесконечность). Ошибок нет."""
        return float("inf") if self.end is None else self.end - time.time()

    def over(self):
        """Потолок вышел? Ошибок нет."""
        return self.left() <= 0

    def check(self, what):
        """Ничего, если время есть. Ошибки: TimeUp — потолок вышел (в тексте — what)."""
        if self.over():
            raise TimeUp(f"потолок времени {self.seconds} с вышел ({what})")

    def http(self, cap):
        """Таймаут одного запроса, с: не больше cap и остатка потолка, не меньше 5 с. Ошибок нет."""
        return cap if self.end is None else max(5.0, min(cap, self.left()))


def block_at_time(stamp, lo, hi, t, deadline=None):
    """Первый блок в [lo, hi] со временем заголовка ≥ t (бинпоиск; время заголовков не убывает). Принимает:
    stamp(n) → время заголовка блока n, с (ошибка узла — исключение с причиной), границы, t (с), потолок (или None).
    Возвращает: блок; у всех блоков время < t — hi + 1. Ошибки: TimeUp; ошибки stamp — как есть."""
    while lo <= hi:
        if deadline:
            deadline.check(f"поиск блока по времени {t}")
        mid = (lo + hi) // 2
        if stamp(mid) >= t:
            hi = mid - 1
        else:
            lo = mid + 1
    return lo


def logs_split(fetch, lo, hi, min_span, say=None, deadline=None):
    """Логи окна [lo, hi] по возрастанию блоков. Принимает: fetch(lo, hi) → список логов (ошибка узла — исключение,
    текст которого содержит ответ узла), границы, наименьшее окно деления, say(текст) — печать (или None), потолок
    времени (решение 460 п. 9, или None). Таймаут — окно пополам, каждая половина отдельно (рекурсивно), пока окно ≥
    min_span. Ошибки: TimeUp — потолок вышел перед очередной частью; таймаут на окне меньше min_span и любая другая
    ошибка fetch — как есть."""
    out, stack = [], [(lo, hi)]
    while stack:
        a, b = stack.pop()
        if deadline:
            deadline.check(f"eth_getLogs {a}..{b}")
        try:
            out.extend(fetch(a, b))
        except Exception as e:  # noqa: BLE001 — классификация ниже; не таймаут — как есть
            if not is_timeout(e) or b - a + 1 < 2 * min_span:
                raise
            mid = (a + b) // 2
            if say:
                say(f"eth_getLogs {a}..{b}: таймаут узла — делю на {a}..{mid} и {mid + 1}..{b}")
            stack.append((mid + 1, b))
            stack.append((a, mid))
    return out


def fresh(head, body, tries=TRIES, say=None):
    """Фаза чтения состояния на свежем блоке. Принимает: head() → блок (номер или объект блока узла), body(блок) →
    результат, число попыток. Возвращает: результат body. Ошибки: StaleState после tries попыток; остальные — как
    есть."""
    for k in range(tries):
        b = head()
        try:
            return body(b)
        except StaleState as e:
            if k + 1 == tries:
                raise
            (say or _err)(f"состояние блока уже удалено узлом ({str(e)[:160]}) — заново на новом блоке "
                          f"(попытка {k + 2} из {tries})")
    raise RuntimeError("недостижимо")


def _err(text):
    print(text, file=sys.stderr, flush=True)
