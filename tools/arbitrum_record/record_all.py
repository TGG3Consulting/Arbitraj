#!/usr/bin/env python3
"""Файл: tools/arbitrum_record/record_all.py
Что делает: решение 459 (T456-03, T457-02) — ОДНА команда записи своего узла Arbitrum: по очереди запускает все скрипты
  записи семейств (record_steps.py) и запись приёмки исполнителя v4 (exec_v4_live.py) с одним адресом узла. Каждый
  скрипт пишет свою папку tests/data/…; падение одного не останавливает остальные. Вопросов в процессе нет. В конце —
  сводка (что записано, что упало и почему) и один zip со всеми записями, журналами и сводкой — путь печатается.
  Только чтение узла: ключей, подписей и транзакций нет; приватных данных скрипт не просит и не печатает (--wallet —
  адрес, не ключ, и необязателен).
Кто вызывает: Тигран, из корня репозитория:
  Windows: py tools\\arbitrum_record\\record_all.py --rpc http://УЗЕЛ:8547
  Linux:   python3 tools/arbitrum_record/record_all.py --rpc http://УЗЕЛ:8547
От чего зависит: Python 3.8+ (только стандартная библиотека; HTTP — stdhttp.py), record_steps.py, скрипты этой папки;
  GeckoTerminal (api.geckoterminal.com) — списки пулов у kyber_elastic_v1, fluid_t1, balancer_v3, algebra_dir_fee, ekubo.
Ошибки: код выхода 0 — все скрипты прошли, 1 — хотя бы один упал (сводка и архив пишутся в обоих случаях).
"""

import argparse
import datetime
import json
import os
import subprocess
import sys
import time
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
DATA = os.path.join(ROOT, "tests", "data")
sys.path.insert(0, HERE)

import record_steps  # noqa: E402
from v4live_rpc import Node  # noqa: E402

# Ядро Ekubo (ekubo.py CORE — broadcast DeployAll.s.sol/42161, docs.ekubo.org).
EKUBO_CORE = "0x00000000000014aA86C5d3c41765bb24e11bd701"


def say(*parts):
    """Печать, не падающая на консоли Windows без UTF-8."""
    text = " ".join(str(p) for p in parts)
    try:
        print(text, flush=True)
    except UnicodeEncodeError:
        print(text.encode("ascii", "replace").decode(), flush=True)


def ekubo_block(rpc, head):
    """Первый блок с кодом ядра Ekubo (поиск делением по eth_getCode). Возвращает: (блок, None) или (None, причина) —
    узел без истории состояния отвечает ошибкой, тогда блок задаётся --ekubo-from-block."""
    node = Node(rpc)

    def has(b):
        j = node.rpc("eth_getCode", [EKUBO_CORE, hex(b)])
        if "result" not in j:
            raise RuntimeError(f"eth_getCode на блоке {b}: {j.get('error')}")
        return j["result"] not in ("0x", "")

    try:
        if not has(head):
            return None, "на голове у адреса ядра нет кода"
        lo, hi = 0, head
        while lo < hi:
            mid = (lo + hi) // 2
            if has(mid):
                hi = mid
            else:
                lo = mid + 1
        return lo, None
    except (RuntimeError, OSError) as e:
        return None, str(e)[:300]


def touched(since):
    """Папки tests/data, где после момента since изменён хотя бы один файл. Ошибок нет."""
    out = []
    for name in sorted(os.listdir(DATA)):
        d = os.path.join(DATA, name)
        if not os.path.isdir(d):
            continue
        for base, _, files in os.walk(d):
            if any(os.path.getmtime(os.path.join(base, f)) >= since for f in files):
                out.append(d)
                break
    return out


def run(name, script, argv, logs, timeout):
    """Запуск одного скрипта: вывод — в журнал logs/<имя>.log. Возвращает строку сводки. Ошибок-исключений нет."""
    start = time.time()
    env = dict(os.environ, PYTHONUTF8="1", PYTHONIOENCODING="utf-8")
    log = os.path.join(logs, name + ".log")
    with open(log, "w", encoding="utf-8") as fh:
        try:
            p = subprocess.run([sys.executable, os.path.join(HERE, script)] + argv, cwd=ROOT, env=env,
                               stdout=fh, stderr=subprocess.STDOUT, timeout=timeout)
            code, why = p.returncode, ""
        except subprocess.TimeoutExpired:
            code, why = -1, f"превышено время {timeout} с"
    if code and not why:
        with open(log, encoding="utf-8", errors="replace") as fh:
            tail = [ln.strip() for ln in fh.read().splitlines() if ln.strip()]
        why = tail[-1][:300] if tail else f"код выхода {code}"
    dirs = [os.path.relpath(d, ROOT) for d in touched(start)]
    return {"name": name, "ok": code == 0, "code": code, "seconds": round(time.time() - start), "dirs": dirs,
            "why": why, "log": os.path.relpath(log, ROOT)}


def main():
    ap = argparse.ArgumentParser(description="Все записи своего узла Arbitrum одной командой (решение 459)")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum One, например http://127.0.0.1:8547")
    ap.add_argument("--wallet", help="адрес кошелька для записи приёмки v4 (необязательно; адрес, не ключ)")
    ap.add_argument("--ekubo-from-block", type=int, help="блок развёртывания ядра Ekubo (иначе — поиск по узлу)")
    ap.add_argument("--only", action="append", default=[], help="запустить только эти скрипты (имя без .py)")
    ap.add_argument("--skip", action="append", default=[], help="пропустить эти скрипты")
    ap.add_argument("--timeout", type=int, default=None, help="предел времени одного скрипта, с (по умолчанию нет)")
    a = ap.parse_args()
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d_%H%M%S")
    work = os.path.join(ROOT, f"arbitrum_record_{stamp}")
    logs = os.path.join(work, "logs")
    os.makedirs(logs, exist_ok=True)
    head = int(Node(a.rpc).ok("eth_blockNumber", []), 16)
    say(f"Узел отвечает, голова {head}. Журналы: {logs}")
    want = lambda n: (not a.only or n in a.only) and n not in a.skip  # noqa: E731
    ek, ek_why = a.ekubo_from_block, None
    if ek is None and want("ekubo_more"):
        ek, ek_why = ekubo_block(a.rpc, head)
    results = []
    for name, script, argv in record_steps.steps(a.rpc, a.wallet, ek):
        if not want(name):
            continue
        if argv is None:
            results.append({"name": name, "ok": False, "code": None, "seconds": 0, "dirs": [], "log": None,
                            "why": f"не запущен: блок ядра Ekubo не найден ({ek_why}) — задайте --ekubo-from-block"})
            say(f"[--] {name}: {results[-1]['why']}")
            continue
        say(f"[..] {name} …")
        r = run(name, script, argv, logs, a.timeout)
        results.append(r)
        say(f"[{'ок' if r['ok'] else 'СБОЙ'}] {name} за {r['seconds']} с: "
            + (", ".join(r["dirs"]) or "папок нет") + ("" if r["ok"] else f" — {r['why']}"))
    with open(os.path.join(work, "summary.json"), "w", encoding="utf-8") as fh:
        json.dump({"head": head, "rpc_given": True, "results": results}, fh, ensure_ascii=False, indent=1)
    zpath = work + ".zip"
    dirs = sorted({d for r in results for d in r["dirs"]})
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for rel in dirs + [os.path.relpath(work, ROOT)]:
            for base, _, files in os.walk(os.path.join(ROOT, rel)):
                for f in files:
                    full = os.path.join(base, f)
                    z.write(full, os.path.relpath(full, ROOT))
    ok = sum(r["ok"] for r in results)
    say("")
    say(f"Итог: прошло {ok} из {len(results)}.")
    for r in results:
        say(f"  {'ок  ' if r['ok'] else 'СБОЙ'} {r['name']}" + ("" if r["ok"] else f": {r['why']}"))
    say("")
    say(f"Архив для отправки: {zpath}")
    sys.exit(0 if ok == len(results) else 1)


if __name__ == "__main__":
    main()
