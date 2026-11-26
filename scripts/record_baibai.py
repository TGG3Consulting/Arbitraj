"""Архивные размещения и полное исполнение WETH/USDC BaiBai на одном каноническом блоке Base."""
import hashlib

import requests
from eth_abi import decode
from eth_utils import keccak

from base_archive_endpoint import available
from record_lunarbase import Capture, FOLDER, SLOT, USDC, calldata, read, write
from record_solidly_v3 import HOLDERS

ENTRY = "0x98c1d9e102eb2806d902b13186bdc7892ac4ffba"
BOOK = "0x604d9b9eb1e1571c78661a6c1088427ec9c8c6e5"
VAULT = "0xaac48feb93c5c97e0fb3c7c57e1633922a4acda3"
WETH = "0x4200000000000000000000000000000000000006"
DOC = "https://docs.baibai.cx/takers/"
FILL = "Fill(address,uint64,address,address,uint256,uint256,uint256)"


def abi(name, types=(), args=()):
    return calldata(name + "(" + ",".join(types) + ")", list(types), list(args))


def addresses(c):
    source = {}
    for file in ("deployments.md", "quoting-and-swapping.md"):
        r = requests.get(DOC + file, timeout=20)
        r.raise_for_status()
        if file == "deployments.md" and any(a not in r.text.lower() for a in (ENTRY, BOOK, VAULT, WETH, USDC)):
            raise ValueError("Официальное размещение изменилось")
        source[DOC + file] = hashlib.sha256(r.content).hexdigest()
    deployments = {}
    for label, addr in (("entry", ENTRY), ("book", BOOK), ("custodian", VAULT)):
        word = c.one("eth_getStorageAt", [addr, SLOT, c.tag])
        impl = "0x" + word[-40:]
        deployments[label] = dict(address=addr, code=c.code(addr), implementation_word=word,
                                  implementation=impl, implementation_code=c.code(impl))
    reads = [(ENTRY, "curveBook", BOOK), (ENTRY, "custodian", VAULT), (ENTRY, "quoteToken", USDC),
             (BOOK, "entrypoint", ENTRY), (BOOK, "quoteToken", USDC), (VAULT, "entrypoint", ENTRY),
             (VAULT, "curveBook", BOOK)]
    links = []
    for addr, method, expected in reads:
        tx = {"to": addr, "data": abi(method)}
        result = c.one("eth_call", [tx, c.tag])
        if "0x" + result[-40:] != expected:
            raise ValueError(f"Размещения расходятся: {method}")
        links.append(dict(request=tx, response=result, expected=expected))
    return source, deployments, links


def simulate(c, owner, steps, timestamp=None):
    txs = [{"from": owner, "to": a, "data": data, "gas": hex(1_000_000)} for _, a, data in steps]
    block = {"calls": txs}
    if timestamp is not None:
        block["blockOverrides"] = {"time": hex(timestamp)}
    stage, = c.one("eth_simulateV1", [{"blockStateCalls": [block], "validation": False}, c.tag])
    if stage["parentHash"] != c.tag["blockHash"]:
        raise ValueError("Сменился контрольный блок")
    return dict(timestamp=int(stage["timestamp"], 16), calls=txs,
                results=dict(zip((k for k, _, _ in steps), stage["calls"], strict=True)))


def measure(c, token, amount, expires, coin=WETH, owner=None):
    other = USDC if token == coin else coin
    if owner is None:
        owner = HOLDERS["base"][token]
    addr, number = "address", "uint256"
    balance = lambda account: abi("balanceOf", [addr], [account])
    approve = lambda n: abi("approve", [addr, number], [ENTRY, n])
    quote = ("net", ENTRY, abi("quoteFor", [addr, addr, number, addr], [coin, token, amount, owner]))
    gross = ("gross", BOOK, abi("quote", [addr, addr, number], [coin, token, amount]))
    pair = ("state", BOOK, abi("pair", [addr], [coin]))
    first = simulate(c, owner, [gross, quote, pair])
    def amounts(result, key):
        return decode(["uint256", "uint256"], bytes.fromhex(result[key]["returnData"][2:]))
    net, fee = amounts(first["results"], "net")
    if not net or net + fee != int(first["results"]["gross"]["returnData"], 16):
        raise ValueError("Чистая котировка и комиссия расходятся")
    swap = lambda minimum: abi("swapExactAmountIn", [addr, addr, number, number, addr],
                               [coin, token, amount, minimum, owner])
    def balances(label):
        return [(f"{role}_{axis}_{label}", asset, balance(account)) for role, account in (
            ("wallet", owner), ("vault", VAULT)) for axis, asset in (("in", token), ("out", other))]
    steps = balances("before") + [pair, gross, quote, ("reset", token, approve(0)), ("approve", token, approve(amount)),
             ("above", ENTRY, swap(net + 1)), ("exact", ENTRY, swap(net)),
             ("after_pair", BOOK, pair[2])] + balances("after")
    actual = simulate(c, owner, steps)
    r = actual["results"]
    if any(v["status"] != ("0x0" if k == "above" else "0x1") for k, v in r.items()):
        raise ValueError({k: v.get("error") for k, v in r.items() if v["status"] == "0x0"})
    n = lambda k: int(r[k]["returnData"], 16)
    if not (n("wallet_in_before") - n("wallet_in_after") == n("vault_in_after") - n("vault_in_before") == amount
            and n("wallet_out_after") - n("wallet_out_before") == n("vault_out_before") - n("vault_out_after") == n("exact") == net
            and amounts(r, "net") == (net, fee) and n("gross") == net + fee):
        raise ValueError("Дельты кошелька и входной резерв расходятся с котировкой")
    topic = "0x" + keccak(text=FILL).hex()
    fill, = [x for x in r["exact"]["logs"] if x["address"] == ENTRY and x["topics"][0] == topic]
    state = lambda k: decode(["uint256", "uint256"], bytes.fromhex(r[k]["returnData"][2:130]))
    seq, cursor = state("state")
    if (state("after_pair") != (seq, cursor + 1) or int(fill["topics"][2], 16) != cursor + 1
            or "0x" + fill["topics"][1][-40:] != coin or "0x" + fill["topics"][3][-40:] != owner
            or decode([addr, number, number, number], bytes.fromhex(fill["data"][2:])) != (token, amount, net, fee)):
        raise ValueError("Событие Fill расходится с платежами или курсором")
    stale = simulate(c, owner, [quote, pair, ("approve", token, approve(amount)), ("expired", ENTRY, swap(net))], expires+1)
    sr = stale["results"]
    if (amounts(sr, "net") != (0, 0) or sr["state"]["returnData"] != r["state"]["returnData"]
            or sr["expired"]["status"] != "0x0" or stale["timestamp"] != expires + 1):
        raise ValueError("После истечения цены старое состояние осталось исполнимым")
    return dict(owner=owner, token_in=token, token_out=other, amount=str(amount), out=str(net), fee=str(fee),
                gas=int(r["exact"]["gasUsed"], 16), actual=actual, expired=stale)


def capture():
    c = Capture(available(read(FOLDER / "base.json")))
    sources, deployments, links = addresses(c)
    fields = [(k, abi(k, ["address"], [WETH]) if k in ("validUntil", "pair") else abi(k))
              for k in ("ttl", "validUntil", "pair", "cUnit")]
    views = {k: c.one("eth_call", [{"to": BOOK, "data": data}, c.tag]) for k, data in fields}
    ttl = int(views["ttl"], 16)
    state = bytes.fromhex(views["pair"][2:])
    last_update = int.from_bytes(state[64:96])
    expires = int(views["validUntil"], 16)
    if ttl:
        expires = min(expires, last_update + ttl)
    cases = [measure(c, a, n, expires) for a, sizes in ((USDC, (10**6, 10**7)), (WETH, (10**14, 10**15))) for n in sizes]
    return dict(chain_id=8453, endpoint=c.url, selector=c.tag, source=sources, slot=SLOT, deployments=deployments,
                links=links, views=views, effective_expiry=expires, cases=cases)


if __name__ == "__main__":
    write(FOLDER / "baibai_base.json", capture())
