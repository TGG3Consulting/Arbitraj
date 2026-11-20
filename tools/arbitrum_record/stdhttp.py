"""Файл: tools/arbitrum_record/stdhttp.py
Что делает: решение 459 (T456-03) — замена пакета requests на стандартную библиотеку Python 3 для скриптов записи узла
  Arbitrum: Тигран запускает их на Windows и на Linux без pip. Та же малая часть интерфейса requests, что используют
  скрипты: post(url, json=, timeout=, headers=), get(url, params=, headers=, timeout=), Session().post / .get, ответ —
  status_code, text, content, json(), raise_for_status(). Узел — постоянное соединение http.client на хост (как
  Session у requests: сотни тысяч eth_call без нового TCP на каждый); при прокси из окружения (HTTP(S)_PROXY) и для
  https — urllib (он сам берёт прокси окружения).
  Решение 460: (1) ответ узла «historical state … is not available» (узел с короткой историей состояния — Nitro,
  state-scheme=path) — исключение StaleState сразу здесь, в одном месте для всех скриптов: такой ответ не может
  попасть в запись как «ошибка вызова»; (2) GeckoTerminal (api.geckoterminal.com) — темп и повтор при 429 (gt.py).
Кто вызывает: tools/arbitrum_record/*.py (`import stdhttp as requests`).
От чего зависит: только стандартная библиотека (http.client, urllib, json), gt.py (темп GeckoTerminal).
Ошибки: HTTPError — raise_for_status при коде ≥ 400; StaleState — узел уже не хранит состояние запрошенного блока;
  сетевые — исключения http.client / urllib как есть (OSError).
"""

import http.client
import socket
import json as _json
import urllib.error
import urllib.parse
import urllib.request

import gt

# Решение 460: текст ошибки узла без состояния блока — факт журнала lb_v22 прогона 20.11.2026 (Nitro, path):
# {'code': -32000, 'message': 'historical state <корень> is not available'}.
STALE_MARKS = (b"historical state", b"is not available")


class HTTPError(Exception):
    """Ответ с кодом ≥ 400 (как requests.HTTPError)."""


class StaleState(BaseException):
    """Решение 460: узел ответил «historical state … is not available» — состояния блока записи у него уже нет.
    Наследник BaseException, а не Exception: скрипты, которые пишут ошибку вызова в запись (`except Exception`), его не
    перехватывают — запись не получает «ошибку» вместо данных; ловит только recnode.fresh (новый блок) или выход."""


class Response:
    """Ответ: код, тело байтами и текстом, заголовки (словарь, имена в нижнем регистре), разбор JSON."""

    def __init__(self, status, body, url, headers=None):
        self.status_code, self.content, self.url = status, body, url
        self.headers = {k.lower(): v for k, v in (headers or {}).items()}

    @property
    def text(self):
        return self.content.decode("utf-8", "replace")

    def json(self):
        """Тело как JSON. Ошибки: ValueError — не JSON."""
        return _json.loads(self.content.decode("utf-8"))

    def raise_for_status(self):
        """Ошибки: HTTPError — код ≥ 400 (текст ответа — в сообщении, первые 300 знаков)."""
        if self.status_code >= 400:
            raise HTTPError(f"HTTP {self.status_code} {self.url}: {self.text[:300]}")


def _proxied(scheme, host):
    """Есть ли прокси окружения для схемы и хоста (тогда — urllib). Ошибок нет."""
    proxies = urllib.request.getproxies()
    return scheme in proxies and not urllib.request.proxy_bypass(host)


class Session:
    """Постоянные соединения по хосту (http без прокси); https и прокси — через urllib."""

    def __init__(self):
        self._conn = {}

    def _send(self, method, url, body, headers, timeout):
        u = urllib.parse.urlsplit(url)
        if u.scheme != "http" or _proxied(u.scheme, u.hostname or ""):
            return _urllib(method, url, body, headers, timeout)
        key = (u.hostname, u.port or 80)
        path = (u.path or "/") + (("?" + u.query) if u.query else "")
        for attempt in (0, 1):
            conn = self._conn.get(key)
            if conn is None:
                conn = http.client.HTTPConnection(key[0], key[1], timeout=timeout)
                self._conn[key] = conn
            # Ревью t233, п. 3: таймаут — на каждый запрос (у открытого соединения — и у его сокета).
            conn.timeout = timeout
            if conn.sock is not None:
                conn.sock.settimeout(timeout)
            try:
                conn.request(method, path, body=body, headers=headers)
                r = conn.getresponse()
                data = r.read()
                return Response(r.status, data, url, dict(r.getheaders()))
            except (socket.timeout, TimeoutError):
                # Ревью t233, п. 4: таймаут чтения — без повтора, исключение сразу.
                conn.close()
                self._conn.pop(key, None)
                raise
            except (ConnectionError, http.client.RemoteDisconnected, http.client.CannotSendRequest,
                    http.client.BadStatusLine):
                # Узел закрыл простаивающее соединение (разрыв, BrokenPipe — подкласс ConnectionError) — одно новое
                # соединение, затем ошибка как есть.
                conn.close()
                self._conn.pop(key, None)
                if attempt:
                    raise
        raise OSError("недостижимо")

    def post(self, url, json=None, data=None, headers=None, timeout=60):
        """POST; json= — тело JSON. Возвращает Response. Ошибки: сетевые (OSError); StaleState — в ответе узла
        «historical state … is not available» (в т.ч. в одном из ответов пачки)."""
        h = dict(headers or {})
        body = data
        if json is not None:
            body = _json.dumps(json).encode("utf-8")
            h.setdefault("Content-Type", "application/json")
        r = self._send("POST", url, body, h, timeout)
        if all(m in r.content for m in STALE_MARKS):
            at = r.content.find(STALE_MARKS[0])
            raise StaleState(r.content[at:at + 200].decode("utf-8", "replace"))
        return r

    def get(self, url, params=None, headers=None, timeout=60):
        """GET; params= — строка запроса. GeckoTerminal — через gt.paced (темп, повтор при 429). Возвращает Response.
        Ошибки: сетевые (OSError)."""
        if params:
            url += ("&" if "?" in url else "?") + urllib.parse.urlencode(params)
        send = lambda: self._send("GET", url, None, dict(headers or {}), timeout)  # noqa: E731
        if gt.is_gt(url):
            return gt.paced(send)
        return send()


def _urllib(method, url, body, headers, timeout):
    """Запрос через urllib (https, прокси окружения). Код ≥ 400 — Response, не исключение (как requests)."""
    h = dict(headers)
    h.setdefault("User-Agent", "arbitrum-record/1.0")
    req = urllib.request.Request(url, data=body, headers=h, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return Response(r.status, r.read(), url, dict(r.headers.items()))
    except urllib.error.HTTPError as e:
        return Response(e.code, e.read() or b"", url, dict(e.headers.items()) if e.headers else {})


_default = Session()


def post(url, **kw):
    """Как requests.post (общая сессия модуля)."""
    return _default.post(url, **kw)


def get(url, **kw):
    """Как requests.get (общая сессия модуля)."""
    return _default.get(url, **kw)
