"""Файл: tools/bsc_record/rec_rpc.py
Что делает: JSON-RPC своего узла для скриптов записи BSC (ревью t235): одиночный вызов, пачка вызовов (JSON-RPC batch —
  один HTTP-запрос на --batch вызовов, ответы по id), keccak256 и селекторы через web3_sha3 узла (числа в скрипт не
  зашиваются), разбор слов ABI.
Кто вызывает: tools/bsc_record/thena_fusion.py.
От чего зависит: tools/arbitrum_record/stdhttp.py (HTTP стандартной библиотеки; путь добавляет вызывающий).
Ошибки: RuntimeError — ответ узла с ошибкой там, где она недопустима (must / ok); сетевые — OSError как есть.
"""
import stdhttp as requests

MASK = (1 << 256) - 1


class Node:
    """JSON-RPC узла по HTTP."""

    def __init__(self, url):
        self.url, self.n = url, 0

    def call(self, method, params):
        """Один вызов. Возвращает ответ целиком (result или error)."""
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=120)
        r.raise_for_status()
        return r.json()

    def batch(self, calls, size):
        """Пачки по size вызовов [(метод, параметры)]. Возвращает ответы в порядке вызовов."""
        out = []
        for i in range(0, len(calls), size):
            part, first = calls[i:i + size], self.n + 1
            body = [{"jsonrpc": "2.0", "id": first + k, "method": m, "params": p} for k, (m, p) in enumerate(part)]
            self.n += len(part)
            r = requests.post(self.url, json=body, timeout=120)
            r.raise_for_status()
            got = r.json()
            if not isinstance(got, list):
                raise RuntimeError(f"пачка JSON-RPC: узел ответил не списком: {str(got)[:300]}")
            by_id = {j.get("id"): j for j in got}
            out += [by_id.get(first + k, {"error": "нет ответа в пачке"}) for k in range(len(part))]
        return out

    def ok(self, method, params):
        """Вызов, ошибка недопустима. Возвращает result."""
        return self.must(self.call(method, params), method)["result"]

    @staticmethod
    def must(j, what):
        """Ответ без ошибки (иначе RuntimeError с текстом узла). Возвращает ответ."""
        if "error" in j:
            raise RuntimeError(f"{what}: {j['error']}")
        return j

    def sha3(self, data_hex):
        """keccak256 байтов data_hex (0x…) узлом."""
        return self.ok("web3_sha3", [data_hex])

    def sel(self, sig):
        """Селектор функции по тексту сигнатуры."""
        return self.sha3("0x" + sig.encode().hex())[:10]


def word(v):
    """Слово ABI (64 hex без 0x): целое (отрицательное — дополнение до 2^256) или адрес."""
    if isinstance(v, int):
        return hex(v & MASK)[2:].rjust(64, "0")
    return v.lower().replace("0x", "").rjust(64, "0")


def words(raw):
    """Слова ответа eth_call (0x…) списком строк по 64 hex."""
    raw = raw[2:]
    return [raw[i:i + 64] for i in range(0, len(raw), 64)]


def int24(w):
    """int24 из слова (младшие 3 байта со знаком)."""
    v = int(w, 16) & 0xFFFFFF
    return v - (1 << 24) if v >= 1 << 23 else v


def addr(w):
    """Адрес из слова."""
    return "0x" + w[-40:]
