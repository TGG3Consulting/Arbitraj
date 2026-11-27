"""Карточка Aqua из канонических ролей и фактического CREATE того же прогона на GitHub."""
import argparse
import hashlib
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from aqua_evidence import ORDER, REGISTRY, ROUTER
from record_base_slipstream_executor import rpc


def read_roles(url, selector):
    roles = {}
    for name, address in (("engine", ROUTER), ("registry", REGISTRY)):
        code = bytes.fromhex(rpc(url, "eth_getCode", [address, selector])[2:])
        if not code:
            raise ValueError(f"no code for Aqua {name}")
        roles[name] = dict(address=address, bytes=len(code), keccak256="0x" + keccak(code).hex())
    return roles


def card(network, replay):
    selector = replay["selector"]
    roles = replay["roles"]
    if not selector["requireCanonical"]:
        raise ValueError("missing canonical Aqua block")
    result = None
    by_order = {}
    for strategy in replay["strategies"]:
        if "protected" not in strategy:
            continue
        m = strategy["protected"]
        if strategy["selector"] != selector or any(
            roles[role]["address"] != m[role] for role in ("engine", "registry")
        ):
            raise ValueError("Aqua CREATE and role evidence differ")
        creation = bytes.fromhex(m["creation"][2:])
        args = encode(["address", "address", "bytes32"], [
            m["engine"], m["registry"], bytes.fromhex(m["v5_hash"][2:]),
        ])
        if not creation.endswith(args):
            raise ValueError("Aqua CREATE arguments differ from the measured roles")
        current = dict(
            kind="aqua_swapvm", chainId={"base": 8453, "ethereum": 1}[network],
            sourceBlock=selector["blockHash"], roles=roles, v5Code=m["v5_hash"],
            data=m["creation"], gas=m["creation_gas"],
            creationSha256=hashlib.sha256(creation).hexdigest(),
            runtimeSha256=m["runtime_sha256"], runtimeKeccak=m["code_hash"],
        )
        if result is not None and result != current:
            raise ValueError("Aqua runtime or creation differs across strategies")
        result = current

        paid = [r for r in strategy["rows"] if r.get("paid", 0) > 0 and r.get("net", 0) > 0]
        if not paid:
            continue
        h = strategy["order"]
        data = bytes.fromhex(m["cache_data"][2:])
        if data[:4] != keccak(text=f"cache({ORDER})")[:4] or "0x" + keccak(data[4:]).hex() != h:
            raise ValueError("Aqua cache calldata differs from the settled order")
        (order,) = decode([ORDER], data[4:])
        tokens = sorted({r[key] for r in paid for key in ("input", "output")})
        if len(tokens) != 2:
            raise ValueError("expected a single paid token pair")
        entry = dict(order=h, maker=order[0], tokens=tokens, data=m["cache_data"], gas=m["cache_gas"])
        modes = by_order.setdefault(h, {})
        mode = strategy["mode"]
        if mode not in ("wallet", "v5") or mode in modes:
            raise ValueError("unexpected or repeated Aqua execution mode")
        modes[mode] = entry

    if result is None or not by_order:
        raise ValueError("no protected Aqua payouts")
    cache = []
    for modes in by_order.values():
        if set(modes) != {"wallet", "v5"} or modes["wallet"] != modes["v5"]:
            raise ValueError("wallet and V5 do not confirm the same cache entry")
        cache.append(modes["wallet"])
    return dict(result, cache=cache)


def write_card(network, replay, path):
    data = json.dumps(card(network, replay), indent=2) + "\n"
    path.write_bytes(data.encode("utf8"))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("network", choices=("base", "ethereum"))
    parser.add_argument("capture", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    write_card(args.network, json.loads(args.capture.read_bytes()), args.output)
