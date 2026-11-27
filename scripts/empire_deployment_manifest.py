"""Формирует неподписанный CREATE Empire из проверенных результатов архива и байткода Solidity."""
import argparse
import hashlib
import json
from pathlib import Path
from eth_abi import encode
from eth_utils import keccak


def manifest(archive, compiled, profile):
    factory = archive["factory"]
    attested = archive["module"]
    role = encode(["address"], [factory])
    deploy = bytes.fromhex(compiled["creation"]) + role
    code = bytearray.fromhex(compiled["runtime"])
    [fields] = compiled["immutableReferences"].values()
    if not fields or attested["factory"] != factory or profile["dex.factory"] != factory:
        raise ValueError("wrong factory")
    for entry in fields:
        start, length = entry["start"], entry["length"]
        if length != len(role):
            raise ValueError("unexpected immutable width")
        code[start:start+length] = role
    runtime = "0x" + keccak(code).hex()
    if runtime != attested["runtime"] or runtime not in profile["dex.module_runtime_hashes"]:
        raise ValueError("unexpected bound runtime")
    return dict(kind="empire_module", chainId=1, factory=factory, factoryHash=profile["dex.factory_runtime_hash"],
                sourceBlock=archive["selector"]["blockHash"], data="0x"+deploy.hex(), runtimeKeccak=runtime,
                creationSha256=hashlib.sha256(deploy).hexdigest(), runtimeSha256=hashlib.sha256(code).hexdigest())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("archive","compiled","output"):
        parser.add_argument(name,type=Path)
    args = parser.parse_args()
    profiles = json.loads(Path("config/first_run_dex.json").read_text("utf8"))
    settings = next(row["settings"] for row in profiles if row["id"]=="empire-module-ethereum")
    result = manifest(json.loads(args.archive.read_bytes()),json.loads(args.compiled.read_bytes()),settings)
    args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
