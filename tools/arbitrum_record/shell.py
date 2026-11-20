#!/usr/bin/env python3
# Файл: tools/arbitrum_record/shell.py
# Что делает: решение 486 — запись СВОЕГО узла Arbitrum One для тестов вида shell_proteus (Shell Protocol Proteus):
#   на свежем блоке B (recnode.fresh — узел хранит состояние ~128 блоков) по каждому пулу — те же вызовы, что читает
#   бот (pools/load_shell.rs calls1: ocean, xToken, yToken, implementation, poolFrozen; calls2: balanceOfBatch и
#   unwrapFeeDivisor учёта, getSlopes / getAs / getBs / getKs / BASE_FEE реализации, decimals() токенов сторон) и
#   лесенка квотера: swapGivenInputAmount(xBal, yBal, вход, сторона) реализации — та же функция, что зовёт прокси пула
#   при обмене (LiquidityPoolProxy.sol L49-L61), — на долях баланса входа (--fractions) в обе стороны. Стороны пула
#   (id в учёте) сопоставляются с токенами-кандидатами (--tokens) по keccak256(токен, 0); id «Ether» — нативная монета
#   учёта (сторона ETH, в паре бота — обёртка). Только eth_call / eth_getBlockByNumber / eth_chainId, без ключей и
#   подписей. Пишет tests/data/arbitrum_shell_<дата>/ (meta.json, pools.json, README.md с SHA-256). Тест —
#   tests/arbitrum_shell.rs (#[ignore] до записи).
# Кто вызывает: Тигран (record_all.py → record_steps.py) или вручную.
# От чего зависит: Python 3.8+ (стандартная библиотека), stdhttp.py (через curve_plain_pools.Batch), recnode.py
#   (fresh), curve_crypto_rpc.py (keccak256, selector, word, addr_word), curve_plain_pools.py (Batch, call, key).
# Настройки: нет. Адреса — аргументы; официальные — docs.shellprotocol.io/start-here/reference-library/
#   contract-addresses (docs/FIRST_RUN.md, раздел решения 486).
#
# Запуск: python3 tools/arbitrum_record/shell.py --node http://УЗЕЛ:8547 --pools 0x…,0x… --tokens 0x…,0x…

import argparse
import datetime
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import recnode  # noqa: E402
from curve_crypto_rpc import addr_word, keccak256, selector, word  # noqa: E402
from curve_plain_pools import Batch, call, key  # noqa: E402

# Метка нативной монеты учёта: hexadecimal(ascii("Ether")) (Ocean.sol aaa0ccf L197).
ETHER = "0x0000000000000000000000000000004574686572"
CALLS1 = ["ocean()", "xToken()", "yToken()", "implementation()", "poolFrozen()"]
IMPL = ["getSlopes()", "getAs()", "getBs()", "getKs()", "BASE_FEE()"]
SWAP = "swapGivenInputAmount(uint256,uint256,uint256,uint8)"


def say(text):
    print(text, file=sys.stderr, flush=True)


def ocean_id(token):
    """keccak256(abi.encodePacked(token, uint256(0))) — id токена в учёте (OceanERC1155.sol L307-L313)."""
    return int.from_bytes(keccak256(bytes.fromhex(token[2:].rjust(40, "0")) + bytes(32)), "big")


def num(x):
    """Ответ eth_call (одно слово) → int; откат — None."""
    r = x.get("result")
    return int(r, 16) if r and r != "0x" else None


def batch_data(pool, x_id, y_id):
    """calldata balanceOfBatch([pool, pool], [x, y]) — как calls2 бота."""
    return (selector("balanceOfBatch(address[],uint256[])") + word(0x40) + word(0xa0) + word(2)
            + addr_word(pool) + addr_word(pool) + word(2) + word(x_id) + word(y_id))


def phase(b, pools, tokens, fractions, head):
    """Фаза состояния на блоке head. Возвращает: (head, recs). StaleState — наружу (recnode.fresh — заново)."""
    tag = head["number"]
    recs = []
    by_id = {ocean_id(t): (t, False) for t in tokens}
    by_id[ocean_id(ETHER)] = (ETHER, True)
    first = [call(p, selector(s), tag) for p in pools for s in CALLS1]
    got = b.run(first)
    for n, p in enumerate(pools):
        ans = {key(p, selector(s)): got[n * len(CALLS1) + k] for k, s in enumerate(CALLS1)}
        rec = {"pool": p, "answers": ans, "quotes": []}
        recs.append(rec)
        v = [num(ans[key(p, selector(s))]) for s in CALLS1]
        if None in v[:4]:
            rec["skip"] = "пул откатил поле"
            continue
        ocean, x_id, y_id, imp = "0x" + format(v[0], "040x"), v[1], v[2], "0x" + format(v[3], "040x")
        rec.update({"ocean": ocean, "implementation": imp, "x_id": hex(x_id), "y_id": hex(y_id)})
        sides = [by_id.get(x_id), by_id.get(y_id)]
        rec["sides"] = [{"token": s[0], "native": s[1]} if s else None for s in sides]
        reqs = [(ocean, batch_data(p, x_id, y_id)), (ocean, selector("unwrapFeeDivisor()"))]
        reqs += [(imp, selector(s)) for s in IMPL]
        reqs += [(s[0], selector("decimals()")) for s in sides if s and not s[1]]
        out = b.run([call(to, d, tag) for to, d in reqs])
        for (to, d), x in zip(reqs, out):
            ans[key(to, d)] = x
        bal = out[0].get("result")
        if not bal or None in sides:
            rec["skip"] = "балансы не прочитаны или сторона не опознана"
            continue
        w = bal[2:]
        xb, yb = int(w[128:192], 16), int(w[192:256], 16)
        qreqs, qkeys = [], []
        for side, b_in in ((0, xb), (1, yb)):
            for f in fractions.split(","):
                amt = int(b_in * float(f))
                if amt <= 0:
                    continue
                d = selector(SWAP) + word(xb) + word(yb) + word(amt) + word(side)
                qreqs.append(call(imp, d, tag))
                qkeys.append((side, amt, d))
        for (side, amt, d), x in zip(qkeys, b.run(qreqs)):
            rec["quotes"].append({"side": side, "amount": str(amt), "data": d, **x})
    return head, recs


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum для тестов Shell Protocol Proteus (решение 486)")
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--pools", required=True, help="адреса пулов через запятую (docs.shellprotocol.io)")
    ap.add_argument("--tokens", required=True, help="ERC-20 кандидаты сторон через запятую")
    ap.add_argument("--fractions", default="0.0001,0.001,0.01,0.05,0.2,0.5", help="доли баланса входа")
    ap.add_argument("--deadline", type=int, default=900, help="потолок времени скрипта, секунд")
    a = ap.parse_args()
    dl = recnode.Deadline(a.deadline)
    b = Batch(a.node)
    pools = [p.strip().lower() for p in a.pools.split(",") if p.strip()]
    tokens = [t.strip().lower() for t in a.tokens.split(",") if t.strip()]
    chain = b.one("eth_chainId", []).get("result")

    def head():
        dl.check("блок для фазы состояния")
        return b.one("eth_getBlockByNumber", ["latest", False])["result"]

    blk, recs = recnode.fresh(head, lambda h: phase(b, pools, tokens, a.fractions, h), say=say)
    day = datetime.date.today().isoformat()
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "tests", "data",
                                         f"arbitrum_shell_{day}"))
    os.makedirs(root, exist_ok=True)
    written = []

    def save(name, obj):
        with open(os.path.join(root, name), "w") as f:
            json.dump(obj, f, indent=1, sort_keys=True)
        written.append(name)

    save("meta.json", {"chainId": chain, "number": int(blk["number"], 16), "hash": blk["hash"],
                       "timestamp": int(blk["timestamp"], 16), "pools": pools, "tokens": tokens,
                       "fractions": a.fractions})
    save("pools.json", recs)
    for r in recs:
        say(f"{r['pool']}: квот {len(r['quotes'])}" + (f", пропуск: {r['skip']}" if r.get("skip") else ""))
    lines = ["# Shell Protocol Proteus, Arbitrum One — запись своего узла (решение 486)", "",
             f"Записано tools/arbitrum_record/shell.py {day}: блок {int(blk['number'], 16)}, хеш {blk['hash']}, "
             f"chainId {chain}. Ответы eth_call — как есть (result или error), ключ «адрес|calldata».",
             "Квоты — swapGivenInputAmount реализации на балансах блока. Ключей и подписей нет.", "", "SHA-256:", ""]
    for n in sorted(written):
        with open(os.path.join(root, n), "rb") as f:
            lines.append(f"- `{hashlib.sha256(f.read()).hexdigest()}  {n}`")
    with open(os.path.join(root, "README.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("записано в", root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
