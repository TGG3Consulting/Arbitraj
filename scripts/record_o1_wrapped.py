"""Измеряет точный расход WETH и чистую выплату на нативных рынках o1 через адаптер и V5."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak
from record_base_launch_hooks import DATA, ENDPOINTS
from record_base_launch_wallets import replay
from record_base_slipstream_executor import rpc
from record_woo_base import WETH, HOLDERS

MODULE = "0x" + keccak(text="O1NativeAdapter Base archive settlement")[-20:].hex()


def capture(endpoint, artifact):
    with gzip.open(DATA / "base_launch_hooks.json.gz") as stream:
        d = json.load(stream)
    at = d["selector"]
    if rpc(endpoint, "eth_getCode", [MODULE, at]) != "0x":
        raise ValueError("тестовый адрес модуля занят")
    hooks = {}
    for row in d["rows"]:
        if row["group"] == "o1-launchpad" and "terms" in row:
            hooks[row["key"][-1]] = len(row["terms"]["config"])
    current, = [a for a, n in hooks.items() if n == 8]
    legacy = sorted(a for a, n in hooks.items() if n == 11)
    roles = [d["manager"], WETH, current, *legacy]
    if len(roles) != 5:
        raise ValueError("изменилось поколение Standard Hook")
    creation = dict(data="0x" + artifact["creation"] + encode(["address"] * 5, roles).hex(),
                    **{"from": HOLDERS[WETH]})
    def build_read(method):
        if method not in ("eth_call", "eth_estimateGas"):
            raise ValueError(method)
        response = requests.post(endpoint, json=dict(jsonrpc="2.0", id=1, method=method, params=[creation, at]), timeout=30)
        response.raise_for_status()
        body = response.json()
        if "result" not in body:
            raise ValueError(str(body.get("error"))[:240])
        return body["result"]

    code = build_read("eth_call")
    concrete = bytes.fromhex(code[2:])
    template = bytes.fromhex(artifact["runtime"])
    blank = bytearray(concrete)
    values = set()
    for refs in artifact["immutableReferences"].values():
        words = {concrete[r["start"]:r["start"] + r["length"]] for r in refs}
        if len(words) != 1 or any(r["length"] != 32 for r in refs):
            raise ValueError("несогласованный immutable")
        values.add(words.pop())
        for r in refs:
            i = r["start"]
            blank[i:i + 32] = template[i:i + 32]
    if bytes(blank) != template or values != {encode(["address"], [a]) for a in roles}:
        raise ValueError("код расходится со сборкой или привязками")
    gas = build_read("eth_estimateGas")
    archive = replay("o1-launchpad", endpoint, d, (MODULE, code))
    archive["module"] = dict(roles=roles, keccak="0x" + keccak(concrete).hex(), runtime=code, deploy_gas=int(gas, 16))
    archive["gas_price"] = rpc(endpoint, "eth_getBlockByHash", [at["blockHash"], False])["baseFeePerGas"]
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    artifact = json.loads(args.artifact.read_bytes())
    for url in ENDPOINTS:
        try:
            result = capture(url, artifact)
            args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf8")
            break
        except (OSError, ValueError) as exc:
            print(url, str(exc)[:300], flush=True)
    else:
        raise ValueError("нет канонического архива")
