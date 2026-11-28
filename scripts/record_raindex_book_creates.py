"""Записывает канонический Base runtime и результат изолированного CREATE для книг Rain.

Входы: опубликованный RaindexWallet.json из GitHub Actions, архив официального manifest и остатков.
Результат: gzip для проверки карточек без RPC. Выплаты по ордерам записываются отдельно.
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak
from raindex_wire import SELECTORS
from record_raindex_base_balances import push4
from record_raindex_quotes import DATA, PAYER

SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"


def capture(artifact):
    """Читает сеть и исполняет только eth_call. Несовпадение сети, manifest или ABI останавливает запись."""
    with gzip.open(DATA / "raindex_base_deployment_balances.json.gz") as stream:
        evidence = json.load(stream)
    compiled = json.loads(artifact.read_bytes())
    session = requests.Session()
    manifest = session.get(evidence["source"], timeout=20)
    manifest.raise_for_status()
    if hashlib.sha256(manifest.content).hexdigest() != evidence["source_sha256"]:
        raise ValueError("Изменился Rain manifest")
    official = {m["address"].lower(): m for m in manifest.json() if m["network"] == "base"}
    if set(official) != {m["book"] for m in evidence["books"]}:
        raise ValueError("Неполный состав размещений")
    at = evidence["selector"]
    urls = [json.loads((DATA.parent / "base_ethereum_networks.json").read_bytes())["base"]["url"],
            "https://base.gateway.tenderly.co", "https://base-rpc.publicnode.com"]

    def rpc(method, args):
        for endpoint in urls:
            try:
                r = session.post(endpoint, json={"jsonrpc": "2.0", "id": 1, "method": method, "params": args}, timeout=20)
                r.raise_for_status()
                return r.json()["result"]
            except (requests.RequestException, ValueError, KeyError) as err:
                failure = err
        raise RuntimeError(f"{method}: нет архивного ответа") from failure

    def header(n):
        block = rpc("eth_getBlockByNumber", [hex(n), False])
        if int(block["number"], 16) != n:
            raise ValueError("Неверная высота заголовка")
        return {"blockHash": block["hash"], "requireCanonical": True}

    if int(rpc("eth_chainId", []), 16) != 8453 or header(evidence["height"]) != at:
        raise ValueError("Неверная сеть или высота")
    codes, entries = {}, []
    for row in evidence["books"]:
        book, version, block = row["book"], row["version"], row["start_block"]
        if (official[book]["startBlock"], official[book]["interfaceVersion"]) != (block, version):
            raise ValueError(book + " не совпал с реестром")
        code = rpc("eth_getCode", [book, at])
        raw = bytes.fromhex(code[2:])
        digest = "0x" + keccak(raw).hex()
        if digest != row["code_hash"] or SELECTORS[version] not in push4(raw):
            raise ValueError(book + " другой runtime/селектор")
        codes[digest] = code
        # startBlock является официальной точкой чтения событий. Отдельно проверяем появление кода.
        birth = header(block)
        before = header(block - 1)
        old = rpc("eth_getCode", [book, before])
        born = rpc("eth_getCode", [book, birth])
        if old != "0x" or born != code:
            raise ValueError(f"{book} — расхождение фактического размещения с Блоком {block}")
        implementation = rpc("eth_getStorageAt", [book, SLOT, at])
        if int(implementation, 16) != 0:
            raise ValueError(book + " implementation не пуст")
        selector = bytes.fromhex(SELECTORS[version])
        args = encode(["address", "bytes32", "bytes4"], [book, bytes.fromhex(digest[2:]), selector])
        init = bytes.fromhex(compiled["creation"]) + args
        runtime = rpc("eth_call", [{"from": PAYER, "data": "0x" + init.hex()}, at])
        if not runtime or runtime == "0x":
            raise ValueError(book + " пустой ответ CREATE")
        entries.append(dict(book=book, version=version, startBlock=block, birth=birth["blockHash"],
                            beforeBirth=before["blockHash"], beforeCode=old, birthCodeHash=digest,
                            bookHash=digest, implementation=implementation, selector="0x" + selector.hex(),
                            initSha256=hashlib.sha256(init).hexdigest(), runtime=runtime))
    return dict(chainId=8453, height=evidence["height"], selector=at, manifestSha256=evidence["source_sha256"],
                fromAddress=PAYER, codes=codes, entries=entries)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiler", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_bytes(gzip.compress(json.dumps(capture(args.compiler), separators=(",", ":")).encode(), mtime=0))
