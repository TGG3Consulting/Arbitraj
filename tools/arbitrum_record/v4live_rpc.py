"""Файл: tools/arbitrum_record/v4live_rpc.py
Что делает: решение 459 (T457-02) — общее для записи приёмки исполнителя v4 на живых пулах Arbitrum (exec_v4_live.py):
  JSON-RPC узла с журналом всех запросов (raw_rpc.json записи — как у записи решения 400), eth_call с подменой
  состояния, eth_simulateV1 (несколько вызовов подряд в одном состоянии — (б) после (а), квотер в той же симуляции;
  логи вызова — кредит события Swap), слова ABI, кодирование execute исполнителя, разбор отката.
Кто вызывает: tools/arbitrum_record/exec_v4_live.py, v4live_slots.py, v4live_mgr.py, v4live_old.py.
От чего зависит: curve_crypto_rpc.py (keccak256), stdhttp.py (HTTP, стандартная библиотека).
Ошибки: сетевые — исключения как есть; ответ узла с error — возвращается как есть (запись, не исключение).
"""

import json
import threading
import time

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests
from curve_crypto_rpc import keccak256

MAXU = 2**256 - 1
ZERO = "0x" + "0" * 40


def sel(sig):
    """Селектор: первые 4 байта keccak256 подписи (hex без 0x)."""
    return keccak256(sig.encode()).hex()[:8]


def topic(sig):
    """topic0 события: keccak256 подписи (0x…)."""
    return "0x" + keccak256(sig.encode()).hex()


def w(x):
    """Слово ABI целого (дополнение до 2^256 для отрицательных)."""
    return (x & MAXU).to_bytes(32, "big").hex()


def ad(a):
    """Слово ABI адреса."""
    return a[2:].lower().rjust(64, "0")


def word(h, i):
    """i-е слово ответа (hex 0x…) как целое без знака."""
    return int(h[2 + 64 * i:2 + 64 * (i + 1)], 16)


def sword(h, i):
    """i-е слово как целое со знаком (int256)."""
    v = word(h, i)
    return v - (1 << 256) if v >> 255 else v


def adr(h, i):
    """i-е слово как адрес (0x… нижний регистр)."""
    return "0x" + h[2 + 64 * i + 24:2 + 64 * (i + 1)]


def mapslot(key_word, i):
    """Слот элемента mapping Solidity: keccak256(ключ . слот i). key_word — 64 hex без 0x."""
    return "0x" + keccak256(bytes.fromhex(key_word + w(i))).hex()


def mapslot_raw(key_word, base_hex):
    """Слот mapping с базой-хешем (ERC-7201): keccak256(ключ . база)."""
    return "0x" + keccak256(bytes.fromhex(key_word + base_hex[2:].rjust(64, "0"))).hex()


class Node:
    """JSON-RPC узла; log — все запросы и ответы (raw_rpc.json)."""

    def __init__(self, url):
        self.url, self.n, self.log, self.lock = url, 0, [], threading.Lock()

    def rpc(self, method, params, timeout=120):
        """Ответ узла как есть ({result} или {error}). Ошибки: сетевые, HTTP ≥ 400 (HTTPError)."""
        self.n += 1
        t = time.time()
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=timeout)
        r.raise_for_status()
        j = r.json()
        with self.lock:
            rec = {"t": round(t, 3), "method": method, "params": params}
            rec.update({"result": j["result"]} if "result" in j else {"error": j.get("error")})
            self.log.append(rec)
        return j

    def ok(self, method, params):
        """result или исключение RuntimeError с ответом узла."""
        j = self.rpc(method, params)
        if "result" not in j:
            raise RuntimeError(f"{method}: {json.dumps(j.get('error'))[:300]}")
        return j["result"]

    def call(self, to, data, tag, frm=None, overrides=None):
        """eth_call: (result, None) или (None, текст ошибки). from не задан — tx.origin = 0 (Vault V3 quote)."""
        o = {"to": to, "input": data}
        if frm:
            o["from"] = frm
        params = [o, tag] + ([overrides] if overrides else [])
        j = self.rpc("eth_call", params)
        if "result" in j:
            return j["result"], None
        return None, json.dumps(j.get("error"))[:400]

    def simulate(self, tag, overrides, calls):
        """eth_simulateV1 одного блока поверх tag (validation false). Возвращает: (список вызовов, None) или
        (None, ошибка узла)."""
        st = {"stateOverrides": overrides, "calls": calls}
        j = self.rpc("eth_simulateV1", [{"blockStateCalls": [st], "validation": False}, tag], timeout=300)
        if "result" not in j:
            return None, j.get("error")
        return j["result"][0]["calls"], None


EXEC_SIG = "execute(address,uint256,address,uint256,uint256,(uint8,address,address,address,uint16,bytes)[])"


def _bytes(b):
    raw = bytes.fromhex(b[2:]) if isinstance(b, str) else b
    return w(len(raw)) + raw.hex() + "00" * ((-len(raw)) % 32)


def _step(s):
    head = w(s["kind"]) + ad(s["pool"]) + ad(s["tokenIn"]) + ad(s["tokenOut"]) + w(s["shareBps"]) + w(6 * 32)
    return head + _bytes(s["data"])


def enc_execute(tin, amt, tout, minout, deadline, steps):
    """Данные execute исполнителя (contracts/ArbExecutor.sol, execute; Step — kind, pool, tokenIn, tokenOut, shareBps,
    data). Ошибок нет."""
    bodies = [_step(s) for s in steps]
    offs, cur = [], 32 * len(steps)
    for b in bodies:
        offs.append(cur)
        cur += len(b) // 2
    arr = w(len(steps)) + "".join(w(o) for o in offs) + "".join(bodies)
    head = ad(tin) + w(amt) + ad(tout) + w(minout) + w(deadline) + w(6 * 32)
    return "0x" + sel(EXEC_SIG) + head + arr


def decode_revert(rd, err=None):
    """Текст отката: Error(строка), Panic(код), селектор и данные; иначе сообщение узла."""
    if rd and rd.startswith("0x08c379a0") and len(rd) >= 138:
        try:
            n = int(rd[2 + 8 + 64:2 + 8 + 128], 16)
            return "Error(" + bytes.fromhex(rd[2 + 8 + 128:2 + 8 + 128 + 2 * n]).decode("utf8", "replace") + ")"
        except ValueError:
            pass
    if rd and rd.startswith("0x4e487b71"):
        return "Panic(" + str(int(rd[10:74] or "0", 16)) + ")"
    if rd and rd != "0x":
        return "sel " + rd[:10] + " data " + rd[:200]
    return (err or {}).get("message", "") if isinstance(err, dict) else ""


def parse_call(c):
    """Вызов eth_simulateV1 → status (1/0), returnData, gasUsed, logs, revert, error."""
    st = 1 if c.get("status") == "0x1" else 0
    rd = c.get("returnData", "0x")
    return dict(status=st, returnData=rd, gasUsed=int(c.get("gasUsed", "0x0"), 16), logs=c.get("logs", []),
                revert="" if st else decode_revert(rd, c.get("error")), error=c.get("error"))


def uint_of(c):
    """Первое слово успешного вызова eth_simulateV1 или None."""
    rd = c.get("returnData", "0x")
    return word(rd, 0) if c.get("status") == "0x1" and len(rd) >= 66 else None
