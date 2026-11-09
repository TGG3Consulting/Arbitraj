"""Файл: docs/notes/scripts/reserve_analysis/common.py
Что делает: общие помощники разбора «запас против движения цены» (решение 240): Decimal из JSON бота, время RFC 3339,
медиана и квантиль, журнал прогресса, запись JSON/CSV атомарно.
Кто вызывает: все модули пакета (run.py, records.py, pools.py, node.py, reserve.py, report.py).
От чего зависит: только стандартная библиотека Python 3.8+.

Числа бота: rust_decimal с serde-with-str (Cargo.toml) — Decimal приходит СТРОКОЙ («0.0123»); целые настройки — числом.
"""

import csv
import json
import os
import re
import sys
import time
from decimal import Decimal, getcontext

getcontext().prec = 60

_LOG_PATH = None


def set_log(path):
    """Принимает: путь файла журнала прогресса. Возвращает: ничего. Ошибок нет."""
    global _LOG_PATH
    _LOG_PATH = path


def log(msg, quiet=False):
    """Пишет строку с временем UTC в журнал прогресса и (если не quiet) в stderr. quiet — подробности (каждый запрос
    к узлу, каждая сделка/строка): только в файл, чтобы зависание было видно по последней строке. Ошибок нет."""
    line = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()) + " " + msg
    if not quiet:
        print(line, file=sys.stderr, flush=True)
    if _LOG_PATH:
        try:
            with open(_LOG_PATH, "a", encoding="utf-8") as f:
                f.write(line + "\n")
        except OSError:
            pass


def D(x):
    """Decimal из строки/числа JSON бота. None/'' → None. Ошибки: InvalidOperation — не число."""
    if x is None or x == "":
        return None
    if isinstance(x, Decimal):
        return x
    if isinstance(x, bool):
        raise ValueError("bool не число")
    return Decimal(str(x))


_TS = re.compile(r"^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)$")


def ts(s):
    """Время RFC 3339 (chrono пишет до наносекунд и «Z») → секунды Unix (Decimal). None → None.
    Ошибки: ValueError — не RFC 3339."""
    if s is None:
        return None
    m = _TS.match(s.strip())
    if not m:
        raise ValueError("не RFC 3339: %r" % s)
    y, mo, d, h, mi, se, frac, tz = m.groups()
    import calendar
    base = calendar.timegm((int(y), int(mo), int(d), int(h), int(mi), int(se), 0, 0, 0))
    v = Decimal(base)
    if frac:
        v += Decimal("0." + frac)
    if tz != "Z":
        sign = 1 if tz[0] == "+" else -1
        v -= sign * (int(tz[1:3]) * 3600 + int(tz[4:6]) * 60)
    return v


def iso(sec):
    """Секунды Unix → «YYYY-MM-DDTHH:MM:SSZ». None → ''. Ошибок нет."""
    if sec is None:
        return ""
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(int(sec)))


def quantile(vals, q):
    """Квантиль q (0–1) с линейной интерполяцией по отсортированным Decimal. Пусто → None. Ошибок нет."""
    v = sorted(x for x in vals if x is not None)
    if not v:
        return None
    if len(v) == 1:
        return v[0]
    pos = Decimal(str(q)) * (len(v) - 1)
    lo = int(pos)
    hi = min(lo + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (pos - lo)


def median(vals):
    """Медиана Decimal. Пусто → None. Ошибок нет."""
    return quantile(vals, 0.5)


def rnd(x, dp=6):
    """Округление Decimal для вывода (строка). None → None. Ошибок нет."""
    if x is None:
        return None
    return str(x.quantize(Decimal(1).scaleb(-dp)))


def fmt(x):
    """Decimal для вывода: не больше 10 знаков после точки, без экспоненты и лишних нулей. Ошибок нет."""
    if abs(x) >= Decimal("1e15"):
        return format(x, "f")
    q = x.quantize(Decimal("1e-10"))
    s = format(q, "f")
    if "." in s:
        s = s.rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def _default(o):
    if isinstance(o, Decimal):
        return fmt(o)
    if isinstance(o, set):
        return sorted(o)
    raise TypeError("не сериализуется: %r" % type(o))


def dump_json(path, obj):
    """Атомарная запись JSON (Decimal → строка). Ошибки: OSError."""
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False, indent=1, default=_default)
    os.replace(tmp, path)


def load_json(path):
    """JSON с Decimal вместо float. Ошибки: OSError, ValueError."""
    with open(path, encoding="utf-8") as f:
        return json.load(f, parse_float=Decimal)


def write_csv(path, rows, cols):
    """CSV с заголовком cols; значения Decimal — строкой, None — пусто. Ошибки: OSError."""
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        w = csv.writer(f)
        w.writerow(cols)
        for r in rows:
            w.writerow(["" if r.get(c) is None else (fmt(r[c]) if isinstance(r[c], Decimal) else str(r[c]))
                        for c in cols])
    os.replace(tmp, path)
