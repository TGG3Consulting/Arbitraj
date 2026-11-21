"""Файл: tools/arbitrum_record/gt.py
Что делает: решение 460 — темп запросов к GeckoTerminal (api.geckoterminal.com) и повтор при HTTP 429 для всех
  скриптов записи узла Arbitrum (одно место: stdhttp.get зовёт paced для адресов GeckoTerminal).
  Лимит — официальная документация (проверено 20.11.2026):
    * docs.coingecko.com/docs/keyless-public-api (раздел о лимитах публичного API без ключа): «GeckoTerminal
      endpoints: ~5 calls/min (dynamic, varies by server load)»; «If you hit a 429 Too Many Requests, back off
      exponentially before retrying»; запросы считаются по IP (общий пул), заголовок Retry-After там не описан;
    * www.geckoterminal.com/dex-api: «… increase rate limits by 16X, from 30 calls/min to 500 calls/min».
  Документы расходятся (5 и 30 в минуту) — берётся меньший: пауза между запросами 60 / 5 = 12 с (ARB_GT_INTERVAL —
  другое значение, секунды). Пауза держится и между скриптами одного запуска record_all.py: время последнего запроса —
  в файле ARB_GT_STAMP (его задаёт record_all.py; без него — только внутри процесса).
  HTTP 429: ждать Retry-After (если ответ его дал, секунды), иначе 15 / 30 / 60 / 60 / 60 с; всего до 6 попыток;
  после шестой — ответ 429 как есть (скрипт падает с причиной в сводке).
  Прогон 20.11.2026: balancer_v3.py получил 429 на ?page=4 (страницы подряд без паузы).
Кто вызывает: tools/arbitrum_record/stdhttp.py (Session.get).
От чего зависит: стандартная библиотека (os, time).
Ошибки: исключения функции запроса — как есть.
"""

import os
import sys
import time
import urllib.parse

HOST = "api.geckoterminal.com"
# 60 с / 5 запросов (docs.coingecko.com/docs/keyless-public-api) — меньший из двух официальных лимитов, см. шапку.
INTERVAL = 12.0
# Паузы перед повторами после 429 (без Retry-After): 5 повторов → всего 6 попыток.
BACKOFF = (15, 30, 60, 60, 60)
_last = [0.0]


def is_gt(url):
    """Адрес — GeckoTerminal? Ошибок нет."""
    return (urllib.parse.urlsplit(url).hostname or "").lower() == HOST


def _interval():
    """Пауза между запросами, с (ARB_GT_INTERVAL или INTERVAL). Ошибок нет: нечисло — INTERVAL."""
    try:
        return float(os.environ.get("ARB_GT_INTERVAL", INTERVAL))
    except ValueError:
        return INTERVAL


def _wait_turn():
    """Ждать, пока с последнего запроса (этого процесса или файла ARB_GT_STAMP) не пройдёт пауза; отметить запрос.
    Ошибок нет (файл не читается / не пишется — только своя отметка)."""
    stamp = os.environ.get("ARB_GT_STAMP")
    last = _last[0]
    if stamp:
        try:
            with open(stamp, encoding="ascii") as fh:
                last = max(last, float(fh.read().strip() or 0))
        except (OSError, ValueError):
            pass
    delay = last + _interval() - time.time()
    if delay > 0:
        time.sleep(delay)
    _last[0] = time.time()
    if stamp:
        try:
            with open(stamp, "w", encoding="ascii") as fh:
                fh.write(repr(_last[0]))
        except OSError:
            pass


def _retry_after(r):
    """Retry-After ответа в секундах (число) или None. Ошибок нет."""
    v = (getattr(r, "headers", None) or {}).get("retry-after")
    try:
        return max(0.0, float(v)) if v is not None else None
    except ValueError:
        return None


def paced(send):
    """Запрос к GeckoTerminal с паузой и повтором при 429. Принимает: send() → Response. Возвращает: Response (после
    6 попыток с 429 — последний ответ 429). Ошибки: исключения send как есть."""
    for k in range(len(BACKOFF) + 1):
        _wait_turn()
        r = send()
        if r.status_code != 429 or k == len(BACKOFF):
            return r
        pause = _retry_after(r)
        pause = BACKOFF[k] if pause is None else pause
        print(f"GeckoTerminal 429 ({r.url}): попытка {k + 1} из {len(BACKOFF) + 1}, пауза {pause:.0f} с",
              file=sys.stderr, flush=True)
        time.sleep(pause)
    return r
