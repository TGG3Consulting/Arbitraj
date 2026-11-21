"""Файл: docs/notes/scripts/reserve_analysis/botapi.py
Что делает: чтение REST бота — ТОЛЬКО GET (ничего не пишет в бота), с кэшем на диске и возобновлением листания журнала.
Кто вызывает: run.py.
От чего зависит: common.py; стандартная библиотека (urllib).

Маршруты и форматы — по коду бота (src/api/server.rs, journal.rs, settings.rs, core_api.rs, windows.rs):
- GET /api/journal?limit=N[&before=ID] — записи новые первыми (storage/journal.rs list: id < before ORDER BY id DESC);
  поля id, at, level, kind, venue, message, data, repeats, last_at (Entry).
- GET /api/deals — все сделки (core/deal.rs Deal), GET /api/settings — [{name, scope{kind,id}, value}],
  GET /api/settings/history?limit=N[&name=…], GET /api/transits, GET /api/windows?limit=N&offset=M[&coin=…].
- Заголовок Host обязан совпасть с --listen бота (src/api/guard.rs allowed) — иначе 403.
"""

import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from decimal import Decimal

from common import load_json, dump_json, log

ALLOWED = ("/api/journal", "/api/deals", "/api/settings", "/api/settings/history", "/api/transits", "/api/windows")


class Bot:
    """Клиент REST бота: base — «http://127.0.0.1:8787», host — значение заголовка Host, cache — каталог кэша."""

    def __init__(self, base, host, cache, timeout=120):
        self.base = base.rstrip("/")
        self.host = host
        self.cache = cache
        self.timeout = timeout
        os.makedirs(cache, exist_ok=True)

    def get(self, path, params=None):
        """GET с повтором при сбое сети (5 попыток). Принимает: путь из ALLOWED, параметры.
        Возвращает: JSON (float → Decimal). Ошибки: RuntimeError — путь не разрешён или 5 сбоев подряд."""
        if path not in ALLOWED:
            raise RuntimeError("путь %s не в списке только-чтение" % path)
        url = self.base + path + ("?" + urllib.parse.urlencode(params) if params else "")
        last = None
        for i in range(5):
            req = urllib.request.Request(url, method="GET", headers={"Host": self.host, "Accept": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=self.timeout) as r:
                    return json.loads(r.read().decode("utf-8"), parse_float=Decimal)
            except urllib.error.HTTPError as e:
                body = e.read().decode("utf-8", "replace")[:300]
                last = "HTTP %s: %s" % (e.code, body)
                if e.code in (400, 403, 404):
                    break
            except (urllib.error.URLError, OSError, ValueError) as e:
                last = repr(e)
            time.sleep(2 * (i + 1))
        raise RuntimeError("GET %s: %s" % (url, last))

    def cached(self, name, path, params=None, refresh=False):
        """Ответ из кэша (файл name.json) или GET и запись в кэш. Ошибки: как у get."""
        f = os.path.join(self.cache, name + ".json")
        if os.path.exists(f) and not refresh:
            return load_json(f)
        v = self.get(path, params)
        dump_json(f, v)
        return v

    def journal_kind(self, kind, page, max_pages, since_s, refresh=False):
        """Листает журнал назад страницами по page, сохраняет только записи вида kind (jsonl) и состояние листания —
        обрыв продолжается с той же страницы. Стоп: пустая страница, max_pages (0 — без предела) или запись старше
        since_s (секунды Unix; None — без предела). Возвращает: (список записей, сводка листания). Ошибки: как у get."""
        from common import ts
        state_f = os.path.join(self.cache, "journal_state.json")
        rows_f = os.path.join(self.cache, "journal_%s.jsonl" % kind.replace(".", "_"))
        if refresh:
            for f in (state_f, rows_f):
                if os.path.exists(f):
                    os.remove(f)
        st = load_json(state_f) if os.path.exists(state_f) else {"before": None, "pages": 0, "seen": 0, "done": False,
                                                                   "newest_id": None, "oldest_id": None,
                                                                   "oldest_at": None, "newest_at": None}
        while not st["done"]:
            params = {"limit": page}
            if st["before"] is not None:
                params["before"] = st["before"]
            batch = self.get("/api/journal", params)
            if not batch:
                st["done"] = True
                break
            keep = [e for e in batch if e.get("kind") == kind]
            with open(rows_f, "a", encoding="utf-8") as f:
                for e in keep:
                    f.write(json.dumps(e, ensure_ascii=False, default=str) + "\n")
            st["pages"] += 1
            st["seen"] += len(batch)
            if st["newest_id"] is None:
                st["newest_id"], st["newest_at"] = batch[0]["id"], batch[0]["at"]
            st["oldest_id"], st["oldest_at"] = batch[-1]["id"], batch[-1]["at"]
            st["before"] = batch[-1]["id"]
            log("журнал: страница %d, записей %d (до id %s, %s), %s: +%d"
                % (st["pages"], len(batch), batch[-1]["id"], batch[-1]["at"], kind, len(keep)))
            if (max_pages and st["pages"] >= max_pages) or (since_s is not None and ts(batch[-1]["at"]) < since_s):
                st["done"] = True
            dump_json(state_f, st)
        dump_json(state_f, st)
        rows, ids = [], set()
        if os.path.exists(rows_f):
            with open(rows_f, encoding="utf-8") as f:
                for line in f:
                    e = json.loads(line, parse_float=Decimal)
                    if e["id"] not in ids:
                        ids.add(e["id"])
                        rows.append(e)
        return rows, st

    def windows(self, page=200, refresh=False):
        """Все окна (список WindowInfo, без кадров) страницами по page. Ошибки: как у get."""
        f = os.path.join(self.cache, "windows.json")
        if os.path.exists(f) and not refresh:
            return load_json(f)
        out, off = [], 0
        while True:
            r = self.get("/api/windows", {"limit": page, "offset": off})
            ws = r.get("windows") or []
            out.extend(ws)
            off += len(ws)
            if not ws or off >= int(r.get("total") or 0):
                break
        dump_json(f, out)
        return out
