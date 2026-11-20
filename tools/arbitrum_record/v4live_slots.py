"""Файл: tools/arbitrum_record/v4live_slots.py
Что делает: решение 459 (T457-02) — слоты баланса и разрешения ERC-20 для подмены состояния в симуляции исполнителя v4
  (как у записи решения 400: кошелёк W получает вход подменой баланса, исполнителю X дано разрешение 2^256−1). Слот
  ищется ОДНИМ eth_call: все кандидаты подменяются разными числами, ответ balanceOf / allowance называет слот (пустые
  кандидаты — хеши mapping, на остальное хранилище не влияют). Кандидаты: mapping Solidity в слотах 0…60, Vyper
  (keccak(слот . ключ)) 0…20, ERC-7201 OpenZeppelin v5 (ERC20Storage). Найденный слот проверяется вторым eth_call.
Кто вызывает: tools/arbitrum_record/exec_v4_live.py.
От чего зависит: v4live_rpc.py.
Ошибки: нет исключений — слот не найден → None и текст причины.
"""

from v4live_rpc import MAXU, ad, mapslot, mapslot_raw, sel, w, word
from curve_crypto_rpc import keccak256

# ERC-7201 «openzeppelin.storage.ERC20» (OpenZeppelin Contracts v5 ERC20Upgradeable, ERC20StorageLocation):
# _balances — смещение 0, _allowances — смещение 1.
OZ7201 = 0x52c63247e1f47db19d5ce0460030c497f067ca4cebf71ba98eeadabe20bace00
TAG_SHIFT = 100


def _balance_cands(owner):
    out = [("sol", i, mapslot(ad(owner), i)) for i in range(61)]
    out += [("vyper", i, "0x" + keccak256(bytes.fromhex(w(i) + ad(owner))).hex()) for i in range(21)]
    out.append(("oz7201", 0, mapslot_raw(ad(owner), "0x" + w(OZ7201))))
    return out


def _allow_cands(owner, spender):
    out = []
    for i in range(61):
        inner = keccak256(bytes.fromhex(ad(owner) + w(i))).hex()
        out.append(("sol", i, "0x" + keccak256(bytes.fromhex(ad(spender) + inner)).hex()))
    inner = keccak256(bytes.fromhex(ad(owner) + w(OZ7201 + 1))).hex()
    out.append(("oz7201", 1, "0x" + keccak256(bytes.fromhex(ad(spender) + inner)).hex()))
    return out


def _find(node, tag, token, cands, data):
    diff = {s: "0x" + w((k + 1) << TAG_SHIFT) for k, (_, _, s) in enumerate(cands)}
    res, err = node.call(token, data, tag, overrides={token: {"stateDiff": diff}})
    if res is None or len(res) < 66:
        return None, f"eth_call с подменой: {err or res}"
    v = word(res, 0) & ((1 << 255) - 1)  # FiatToken v2.2: старший бит слова баланса — флаг чёрного списка
    k = (v >> TAG_SHIFT) - 1
    if v & ((1 << TAG_SHIFT) - 1) or not 0 <= k < len(cands):
        return None, f"ответ {hex(v)} не совпал ни с одним кандидатом"
    return cands[k], None


def find(node, tag, token, owner, spender):
    """Слоты баланса owner и разрешения owner → spender у token. Возвращает: ({balance, allowance}, None) или
    (None, причина). Ошибок-исключений нет."""
    b, e = _find(node, tag, token, _balance_cands(owner), "0x" + sel("balanceOf(address)") + ad(owner))
    if b is None:
        return None, "баланс: " + e
    a, e = _find(node, tag, token, _allow_cands(owner, spender),
                 "0x" + sel("allowance(address,address)") + ad(owner) + ad(spender))
    if a is None:
        return None, "разрешение: " + e
    return {"balance": {"kind": b[0], "index": b[1], "slot": b[2]},
            "allowance": {"kind": a[0], "index": a[1], "slot": a[2]}}, None


def balance_of(node, tag, token, who):
    """balanceOf или None."""
    r, _ = node.call(token, "0x" + sel("balanceOf(address)") + ad(who), tag)
    return word(r, 0) if r and len(r) >= 66 else None


def overrides(node, tag, token_slots, owner, spender, fund):
    """Подмена: у каждого токена разрешение owner → spender = 2^256−1; у токенов из fund ({токен: сумма}) баланс owner =
    текущий + сумма. Проверка — eth_call balanceOf / allowance с подменой. Возвращает: (stateOverrides, None) или
    (None, причина)."""
    ov = {}
    for tok, s in token_slots.items():
        d = ov.setdefault(tok, {"stateDiff": {}})["stateDiff"]
        d[s["allowance"]["slot"]] = "0x" + w(MAXU)
        if tok in fund:
            cur = balance_of(node, tag, tok, owner)
            if cur is None:
                return None, f"{tok}: balanceOf не ответил"
            d[s["balance"]["slot"]] = "0x" + w(cur + fund[tok])
    for tok in token_slots:
        r, _ = node.call(tok, "0x" + sel("allowance(address,address)") + ad(owner) + ad(spender), tag, overrides=ov)
        if not r or len(r) < 66 or word(r, 0) != MAXU:
            return None, f"{tok}: разрешение после подмены {r}"
        if tok in fund:
            r, _ = node.call(tok, "0x" + sel("balanceOf(address)") + ad(owner), tag, overrides=ov)
            if not r or len(r) < 66 or (word(r, 0) & ((1 << 255) - 1)) < fund[tok]:
                return None, f"{tok}: баланс после подмены {r}"
    return ov, None
