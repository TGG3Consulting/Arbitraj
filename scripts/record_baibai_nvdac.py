"""Проверяет исполнение второй пары BaiBai по адресу из Fill, фактическим плательщикам и сроку публикации."""
from record_baibai import (BOOK, ENTRY, FOLDER, HOLDERS, USDC, Capture, abi, available, measure, read, write)

COIN = "0xb20000000000000000000078ee7ce2fe4908108c"
SMALL_HOLDER = "0x2a1b16a4df05fe96afedd50dd9aa6df2f515deeb"
HOLDER = "0xa7408f2c7baeeb34a1ff5da06c3ab3b1acc65639"


def capture():
    source = read(FOLDER / "baibai_base.json")
    c = Capture(available(source))
    volume = read(FOLDER / "baibai_volume_base.json")
    if volume["entry"] != ENTRY or not any(r["base"] == COIN for r in volume["markets"]):
        raise ValueError("У Entry нет событий этой пары")
    views = {}
    for target, method, types, args in ((BOOK, "validUntil", ["address"], [COIN]), (BOOK, "pair", ["address"], [COIN]),
            (BOOK, "ttl", [], []), (COIN, "decimals", [], []), (COIN, "symbol", [], [])):
        views[method] = c.one("eth_call", [{"to": target, "data": abi(method, types, args)}, c.tag])
    decimals = int(views["decimals"], 16)
    if decimals != 8:
        raise ValueError("Изменились единицы базового токена")
    state = bytes.fromhex(views["pair"][2:])
    ttl = int(views["ttl"], 16)
    ends = int(views["validUntil"], 16)
    if ttl:
        ends = min(ends, int.from_bytes(state[64:96]) + ttl)
    sizes = [(USDC, 10**6, HOLDERS["base"][USDC]), (USDC, 10**7, HOLDERS["base"][USDC]),
             (COIN, 1000, SMALL_HOLDER), (COIN, 10**6, HOLDER)]
    cases = [measure(c, token, n, ends, coin=COIN, owner=owner) for token, n, owner in sizes]
    return dict(chain_id=8453, selector=c.tag, endpoint=c.url, entry=ENTRY, base=COIN, views=views, decimals=decimals,
                issuer="https://www.base.org/assets", token_controls="https://www.base.org/blog/b20-tokenized-stocks-on-base",
                expiration=ends, cases=cases)


if __name__ == "__main__":
    write(FOLDER / "baibai_nvdac_base.json", capture())
