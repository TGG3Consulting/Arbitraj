"""Материализация карточки Motoswap в CI. На архивном хеше Ethereum выполняются
только eth_call конструкторов и чтение исходного кода ролей. Обмена и отправки нет.
"""
import gzip
import json
import sys
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from fork_fee_rpc import Reader
from verify_wallet_creation import executable, sha


def verify(compiled, archive, cards):
    """Сверяет прежние CREATE с платёжными архивами и обе программы executePath без metadata."""
    archives = [json.loads(gzip.decompress((archive / name).read_bytes())) for name in
                ("motoswap_ethereum_wallets.json.gz", "motoswap_v2_ethereum_wallets.json.gz")]
    head = json.loads((archive / "ethereum.json").read_bytes())
    reader = Reader("ethereum", head)
    assert all(row["selector"] == reader.tag and row["root"] == reader.header["stateRoot"] for row in archives)
    assert all(card["chainId"] == head["chain_id"] == 1 and card["sourceBlock"] == reader.tag["blockHash"] for card in cards)
    single, previous = cards
    assert single["roleRows"] == previous["roleRows"]
    roles = {words[0]: words[1:] for words in map(str.split, previous["roleRows"])}
    arguments = [roles[name][0] for name in ("factory", "fee_router")]
    args = encode(["address", "address"], arguments)

    # Тела ролей в том же блоке нужны и для автономной проверки полного пути deployed() в UI.
    pins = dict(pair for role in roles.values() for pair in zip(role[::2], role[1::2], strict=True))
    bodies = reader.many([("eth_getCode", [address, reader.tag]) for address in pins])
    code = dict(zip(pins, bodies, strict=True))
    assert all("0x" + keccak(hexstr=body).hex() == pins[address] for address, body in code.items())

    creation = bytes.fromhex(compiled["creation"])
    assert executable(creation) == executable(bytes.fromhex(previous["data"][2:])[:-len(args)])
    owner = archives[0]["owner"]
    for card, row in zip(cards, archives, strict=True):
        original = row["manifest"]
        assert owner == row["owner"] and original["chain"] == 1
        assert [original["factory"], original["fee_router"]] == arguments
        assert card["data"] == "0x" + original["creation"] + args.hex()
        assert card["creationSha256"] == original["creation_sha256"] == sha(bytes.fromhex(card["data"][2:]))
        assert card["runtimeKeccak"] == original["runtime"]

    current_init = creation + args
    raw = reader.many([("eth_call", [{"from": owner, "data": data}, reader.tag])
                       for data in [single["data"], previous["data"], "0x" + current_init.hex()]])
    runtimes = [bytes.fromhex(body[2:]) for body in raw]
    for card, runtime in zip(cards, runtimes[:2], strict=True):
        assert sha(runtime) == card["runtimeSha256"] and "0x" + keccak(runtime).hex() == card["runtimeKeccak"]
    runtime = runtimes[2]
    assert executable(runtime) == executable(runtimes[1])
    normalized = bytearray(runtime)
    words = set()
    for refs in compiled["immutableReferences"].values():
        group = set()
        for ref in refs:
            start = ref["start"]
            assert ref["length"] == 32
            group.add(bytes(normalized[start:start + 32]))
            normalized[start:start + 32] = bytes(32)
        assert len(group) == 1
        words.update(group)
    assert normalized.hex() == compiled["runtime"] and words == {encode(["address"], [a]) for a in arguments}
    card = dict(previous, data="0x" + current_init.hex(), creationSha256=sha(current_init),
                runtimeSha256=sha(runtime), runtimeKeccak="0x" + keccak(runtime).hex())
    assert len({c["runtimeKeccak"] for c in [*cards, card]}) == 3
    verification = dict(sourceBlock=reader.tag["blockHash"], chain=1, solc=compiled["solc"],
                        identicalInitBytes=len(executable(creation)), identicalRuntimeBytes=len(executable(runtime)),
                        runtimeProgramSha256=sha(executable(runtime)))
    return card, dict(verification=verification, roles=code, owner=owner,
                      singleRuntime=raw[0], previousRuntime=raw[1], currentRuntime=raw[2])


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    archive = root / "tests/data/first_run_dex_2026-11-26"
    cards = [json.loads((root / f"ui/src/assets/motoswap-ethereum-{version}.json").read_bytes())
             for version in ("single", "previous")]
    card, evidence = verify(json.loads(Path(sys.argv[1]).read_bytes()), archive, cards)
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    (out / "motoswap-ethereum.json").write_text(json.dumps(card, separators=(",", ":")) + "\n")
    (out / "motoswap_creation_v3.json.gz").write_bytes(gzip.compress(json.dumps(evidence, separators=(",", ":")).encode(), mtime=0))
