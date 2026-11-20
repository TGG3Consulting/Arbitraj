"""Файл: tools/arbitrum_record/curve_sim.py
Что делает: решение 472 (T472-02, доделка) — запись-симуляция exchange пулов Curve: get_dy квотера не доказывает, что
  сам обмен не откатится (tweak_price crypto-пулов, save_p plain-пулов с оракулом), поэтому на ТОМ ЖЕ блоке, что и
  состояние, на суммах лесенки котировок делается eth_call exchange(i, j, dx, 0) от пустого адреса W с подменой
  состояния (как tools/arbitrum_record/exec_v4_live.py, решение 459): баланс монеты входа у W = текущий + dx,
  разрешение W → пул = 2^256 − 1. Слоты баланса и разрешения ищутся ОДИН раз на токен (v4live_slots._find — одним
  eth_call по кандидатам mapping Solidity / Vyper / ERC-7201; для разрешения — ещё кандидаты вложенного HashMap
  Vyper: keccak(keccak(слот . владелец) . получатель)), слот разрешения другому пулу считается по найденному виду и
  номеру. Все симуляции и проверки подмены (balanceOf / allowance с подменой) — ОДНИМ пакетом JSON-RPC: узел хранит
  состояние ~128 блоков. Вход в нативном ETH (метка 0xEeee… plain-пула) — баланс W подменой и value = dx. Ответ —
  выход пула или откат. Только чтение (eth_call): транзакций, ключей и подписей нет.
Кто вызывает: tools/arbitrum_record/curve_plain.py, curve_crypto.py, curve_ren_wsteth.py, basin.py (решение 485).
От чего зависит: v4live_rpc.py (Node, sel, w, ad, word, decode_revert), v4live_slots.py (_find, кандидаты),
  curve_crypto_rpc.py (keccak256), stdhttp.py.
Ошибки: исключений нет (кроме сетевых) — слот не найден / откат — запись с полем error.
"""

import stdhttp as requests  # решение 459: стандартная библиотека
from curve_crypto_rpc import keccak256
from v4live_rpc import MAXU, Node, ad, decode_revert, sel, w, word
import v4live_slots as slots

NATIVE = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
# Пустой отправитель: адрес из хеша метки (ключа нет); пустоту проверяет empty().
SENDER = "0x" + keccak256(b"arb.curve_sim.sender").hex()[-40:]


def _allow_slot(kind, index, owner, spender):
    """Слот разрешения owner → spender по виду и номеру слота mapping."""
    if kind == "vyper":
        inner = keccak256(bytes.fromhex(w(index) + ad(owner))).hex()
        return "0x" + keccak256(bytes.fromhex(inner + ad(spender))).hex()
    base = slots.OZ7201 + 1 if kind == "oz7201" else index
    inner = keccak256(bytes.fromhex(ad(owner) + w(base))).hex()
    return "0x" + keccak256(bytes.fromhex(ad(spender) + inner)).hex()


def _allow_cands(owner, spender):
    out = [("sol", i, _allow_slot("sol", i, owner, spender)) for i in range(61)]
    out += [("vyper", i, _allow_slot("vyper", i, owner, spender)) for i in range(21)]
    out.append(("oz7201", 1, _allow_slot("oz7201", 1, owner, spender)))
    return out


class Sim:
    """Симуляции exchange одного блока tag: add() копит, run() шлёт одним пакетом."""

    def __init__(self, url, tag):
        self.url, self.tag, self.node = url, tag, Node(url)
        self.found, self.bal, self.reqs, self.recs = {}, {}, [], []

    def empty(self):
        """У W нет кода и баланса на блоке (иначе симуляция не от пустого адреса)."""
        code = self.node.rpc("eth_getCode", [SENDER, self.tag]).get("result")
        bal = self.node.rpc("eth_getBalance", [SENDER, self.tag]).get("result") or "0x0"
        return code == "0x" and int(bal, 16) == 0

    def _slots(self, token, pool):
        """(вид, номер) слотов баланса и разрешения токена — один поиск на токен. None и причина — не найден."""
        t = token.lower()
        if t not in self.found:
            b, e = slots._find(self.node, self.tag, token, slots._balance_cands(SENDER),
                               "0x" + sel("balanceOf(address)") + ad(SENDER))
            a, e2 = (None, None)
            if b is not None:
                a, e2 = slots._find(self.node, self.tag, token, _allow_cands(SENDER, pool),
                                    "0x" + sel("allowance(address,address)") + ad(SENDER) + ad(pool))
            self.found[t] = (b, a, e or e2)
        return self.found[t]

    def prefill(self, token, pool):
        """До блока симуляций (на «latest»): слоты токена и текущий баланс W — чтобы после выбора блока не было
        последовательных вызовов (место слота от блока не зависит; W — пустой адрес)."""
        self._slots(token, pool)
        if token.lower() not in self.bal:
            self.bal[token.lower()] = slots.balance_of(self.node, self.tag, token, SENDER) or 0

    def add(self, pool, sig, i, j, dx, token_in, data=None):
        """Копит симуляцию exchange(i, j, dx, 0) пула от W (sig — подпись: int128 или uint256 индексы). Решение 485:
        data — готовые calldata другого обмена (swapFrom скважины Basin, tools/arbitrum_record/basin.py); None —
        exchange(i, j, dx, 0) по sig."""
        rec = {"pool": pool, "i": i, "j": j, "dx": str(dx), "sig": sig}
        data = data or "0x" + sel(sig) + w(i) + w(j) + w(dx) + w(0)
        tx = {"from": SENDER, "to": pool, "input": data}
        if token_in.lower() == NATIVE:
            ov = {SENDER: {"balance": hex(dx + 10**18)}}
            tx["value"] = hex(dx)
            checks = []
        else:
            b, a, why = self._slots(token_in, pool)
            if b is None or a is None:
                rec.update(ok=None, error="слоты не найдены: " + str(why))
                self.recs.append((rec, None))
                return
            if token_in.lower() not in self.bal:
                self.bal[token_in.lower()] = slots.balance_of(self.node, self.tag, token_in, SENDER) or 0
            bal = self.bal[token_in.lower()]
            ov = {token_in: {"stateDiff": {b[2]: "0x" + w(bal + dx),
                                           _allow_slot(a[0], a[1], SENDER, pool): "0x" + w(MAXU)}}}
            q = "0x" + sel("allowance(address,address)") + ad(SENDER) + ad(pool)
            checks = [("eth_call", [{"to": token_in, "input": q}, self.tag, ov]),
                      ("eth_call", [{"to": token_in, "input": "0x" + sel("balanceOf(address)") + ad(SENDER)},
                                    self.tag, ov])]
        start = len(self.reqs)
        self.reqs.extend(checks + [("eth_call", [tx, self.tag, ov])])
        self.recs.append((rec, (start, len(checks), dx)))

    def run(self):
        """Шлёт накопленное одним пакетом. Возвращает: записи {pool, i, j, dx, sig, ok, result | error, revert,
        override_ok}."""
        body = [{"jsonrpc": "2.0", "id": k, "method": m, "params": p} for k, (m, p) in enumerate(self.reqs)]
        got = {}
        if body:
            r = requests.post(self.url, json=body, timeout=300)
            r.raise_for_status()
            got = {x["id"]: x for x in r.json()}
        out = []
        for rec, at in self.recs:
            if at is None:
                out.append(rec)
                continue
            start, n, dx = at
            res = [got.get(start + k, {"error": "нет ответа"}) for k in range(n + 1)]
            ok_over = all("result" in x for x in res[:n])
            if n == 2 and ok_over:
                ok_over = word(res[0]["result"], 0) == MAXU and (word(res[1]["result"], 0) & (MAXU >> 1)) >= dx
            ans = res[n]
            if "result" in ans:
                rec.update(ok=True, result=ans["result"], override_ok=ok_over)
            else:
                err = ans.get("error") or {}
                data = err.get("data") if isinstance(err, dict) else None
                rec.update(ok=False, error=err, revert=decode_revert(data if isinstance(data, str) else None, err),
                           override_ok=ok_over)
            out.append(rec)
        return out
