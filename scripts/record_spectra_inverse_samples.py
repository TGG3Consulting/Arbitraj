"""Записывает прямые get_dy Spectra Base внутри интервала и вокруг одного падения на 1 атоме.

Запуск из корня проекта: python scripts/record_spectra_inverse_samples.py OUTPUT_DIR
Источник входов — зарегистрированные пулы и размеры из record_spectra_math.py.
Ответы снимаются с официального Base RPC на каноническом hash из исходного каталога.
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

import requests
from eth_abi import decode
from record_omni_flux_registry import MC, data
from record_spectra_base import AT, BLOCK, LEGACY, call

DATA = Path("tests/data/first_run_dex_2026-11-26")
METHOD = "get_dy(uint256,uint256,uint256)"
POOL = "0xf5650b7dd5216fcc6cfa5c009ffd147ed8254f37"


def read_gz(name):
    with gzip.open(DATA / name) as stream:
        return json.load(stream)


def payload(method, params, idx=1):
    return {"jsonrpc": "2.0", "id": idx, "method": method, "params": params}


def eth_call(item, idx=1):
    address, raw, _ = item
    return payload("eth_call", [{"to": address, "data": "0x" + raw.hex()}, AT], idx)


def get_dy(pool, i, dx):
    return call(pool, METHOD, "uint256", i, 1 - i, dx)


def capture(url, output):
    directory = read_gz("spectra_base_directory.json.gz")
    book = read_gz("spectra_base_math.json.gz")
    with requests.Session() as session:
        def post(body):
            reply = session.post(url, json=body, timeout=30)
            reply.raise_for_status()
            return reply.json()

        def values(items):
            results = []
            for start in range(0, len(items), 12):
                chunk = items[start:start + 12]
                calldata = data("aggregate3((address,bool,bytes)[])", ["(address,bool,bytes)[]"],
                                [[(p, False, raw) for p, raw, _ in chunk]])
                reply = post(eth_call((MC, calldata, None)))
                if "error" in reply:
                    raise ValueError(reply["error"])
                words, = decode(["(bool,bytes)[]"], bytes.fromhex(reply["result"][2:]))
                for (_, _, typ), (ok, word) in zip(chunk, words, strict=True):
                    if not ok:
                        raise ValueError("Внутренний вызов не выполнен")
                    results.append(decode([typ], word)[0])
            return results

        chain = int(post(payload("eth_chainId", []))["result"], 16)
        header = post(payload("eth_getBlockByNumber", [hex(BLOCK), False]))["result"]
        if (chain != 8453 or chain != directory["chain_id"] or int(header["number"], 16) != BLOCK
                or header["hash"] != AT["blockHash"] or header["stateRoot"] != directory["state_root"]
                or directory["selector"] != AT or book["selector"] != AT
                or directory["block"] != BLOCK or book["timestamp"] != int(header["timestamp"], 16)):
            raise ValueError("Сеть, корень или заголовок расходятся с исходным снимком")

        grid = []
        blind = []
        center = None
        for market in book["markets"]:
            if market["kind"] != "legacy":
                continue
            for i in range(2):
                anchors = sorted(int(q["dx"]) for q in market["quotes"]
                                 if q["i"] == i and q["dy"] and int(q["dy"]) > 0)
                if len(anchors) != 2:
                    continue
                lo, hi = anchors
                entropy = hashlib.sha256(bytes.fromhex(AT["blockHash"][2:]) +
                                         bytes.fromhex(market["pool"][2:]) + bytes([i])).digest()
                blind.append((market["pool"], i, lo + int.from_bytes(entropy, "big") % (hi - lo + 1)))
                for n in (1, 3):
                    grid.append((market["pool"], i, lo + (hi - lo) * n // 4))
                if market["pool"] == POOL and i == 1:
                    center = hi
        if center is None or len(grid) != 92:
            raise ValueError("Потеряна исходная область измерений")
        wanted = {p for p, _, _ in grid} | {POOL}
        bindings = []
        for factory in LEGACY:
            group = [m for m in directory["markets"] if m["factory"] == factory]
            for index, market in enumerate(group):
                p = market["pool"]
                if p in wanted:
                    bindings.extend([(call(factory, "pool_list(uint256)", "address", index), p),
                                     (call(p, "factory()", "address"), factory)])
        if len(bindings) != 2 * len(wanted) or values([op for op, _ in bindings]) != [x for _, x in bindings]:
            raise ValueError("Адрес пула не связан с исходной физической фабрикой")
        out = values([get_dy(p, i, x) for p, i, x in grid])
        independent = values([get_dy(p, i, x) for p, i, x in blind])
        if any(y == 0 for y in out + independent):
            raise ValueError("Нулевая промежуточная котировка")
        powers = [1 << i for i in range(11)]
        xs = [center - n for n in reversed(powers)] + [center] + [center + n for n in powers]
        ys = values([get_dy(POOL, 1, x) for x in xs])
        req = eth_call(get_dy(POOL, 1, center + 3))
        response = post(req)
        adjacent = dict(url=url, block=BLOCK, rawInput=str(center + 3), request=req, response=response)
        if not (ys[xs.index(center + 2)] > int(response["result"], 16)):
            raise ValueError("Снижение на соседних входах не воспроизвелось")
    common = dict(selector=AT, block=BLOCK, method=METHOD)
    intermediate = dict(**common, points=[[p, i, str(x), str(y)] for (p, i, x), y in zip(grid, out, strict=True)])
    neighbors = dict(**common, pool=POOL, inputIndex=1, rawInput=str(center), target=str(ys[xs.index(center)]),
                     samples=[[str(x), str(y)] for x, y in zip(xs, ys, strict=True)])
    output.mkdir(parents=True, exist_ok=True)
    for name, record in [("neighbors", neighbors), ("adjacent", adjacent)]:
        (output / f"spectra_base_inverse_{name}.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    control = dict(**common, sampling="lo + sha256(blockHash || pool || uint8(i)) mod (hi-lo+1)",
                   points=[[p, i, str(x), str(y)] for (p, i, x), y in zip(blind, independent, strict=True)])
    for kind, data in [("intermediate", intermediate), ("blind", control)]:
        filename = output / f"spectra_base_inverse_{kind}.json.gz"
        filename.write_bytes(gzip.compress(json.dumps(data, separators=(",", ":")).encode(), mtime=0))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    capture(json.loads(Path("tests/data/base_ethereum_networks.json").read_bytes())["base"]["url"], args.output)
