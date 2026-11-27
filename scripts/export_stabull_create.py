"""Выпускает неподписанный CREATE Stabull для того же кода и фабрики, которыми воспроизведены выплаты."""
import argparse
import gzip
import json
from hashlib import sha256
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak


def manifest(compiled, paid, directory):
    role = directory["factory"]
    factory = role["address"]
    module = paid["module"]
    if directory["chain"] not in (1,8453) or paid["chain"] != directory["chain"] or paid["selector"] != directory["anchor"] or module["factory"] != factory:
        raise ValueError("Неверная сеть или состояние фабрики")
    modes = {case["mode"] for row in paid["records"] for case in row["cases"]
             if case.get("buy", {}).get("stage") == "paid" and case.get("sell", {}).get("stage") == "paid"}
    if modes != {"module", "v5"}:
        raise ValueError("Нет встречных выплат модуля и V5")
    [references] = compiled["immutableReferences"].values()
    if not references:
        raise ValueError("Отсутствует immutable factory")
    args = encode(["address"], [factory])
    runtime = bytearray.fromhex(compiled["runtime"])
    for ref in references:
        i = ref["start"]
        if ref["length"] != 32 or i < 0 or i+32 > len(runtime) or any(runtime[i:i+32]):
            raise ValueError("Некорректная ссылка на factory")
        runtime[i:i+32] = args
    if "0x" + keccak(bytes(runtime)).hex() != module["runtime"]:
        raise ValueError("Сборка отличается от оплаченной в архиве")
    creation = bytes.fromhex(compiled["creation"]) + args
    return {"chainId": paid["chain"], "kind": "stabull", "market": factory, "factoryHash": role["runtime"],
            "runtimeKeccak": module["runtime"], "runtimeSha256": sha256(runtime).hexdigest(),
            "creationSha256": sha256(creation).hexdigest(), "data": "0x" + creation.hex(),
            "archiveGas": module["deployment_gas"], "compiler": compiled["solc"], "blockHash": paid["selector"]["blockHash"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("compiled", "paid", "output"):
        parser.add_argument(key, type=Path)
    parser.add_argument("--network",choices=["base","ethereum"],default="base")
    args = parser.parse_args()
    data = Path(__file__).resolve().parents[1] / f"tests/data/first_run_dex_2026-11-26/stabull_{args.network}_directory.json.gz"
    result = manifest(json.loads(args.compiled.read_bytes()), json.loads(args.paid.read_bytes()), json.loads(gzip.decompress(data.read_bytes())))
    args.output.write_bytes((json.dumps(result, separators=(",", ":")) + "\n").encode())
