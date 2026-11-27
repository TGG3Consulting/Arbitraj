"""Snapshot the Puzzle Ranges factory, vault, hook and all pool balances on a canonical block."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_bitswap_ethereum import BLOCK, read_batch
from record_ethereum_x7 import ROOT_HASH


MANIFEST = "https://ranges.finance/deployments/1"
AT = {"blockHash": BLOCK, "requireCanonical": True}
ENDPOINT = "https://eth-mainnet.public.blastapi.io"
DYNAMIC = "(" + ",".join(["uint256[]"] * 5 + ["uint256"] * 2 + ["bool"] * 6) + ")"
IMMUTABLE = "(address[],uint256[],uint256[])"
DYNAMIC_KEYS = ("raw", "live", "virtual", "leverage", "rates", "fee", "supply",
                "registered", "initialized", "paused", "recovery", "vault_paused", "hook_stopped")


def manifest(name):
    reply = requests.get(f"{MANIFEST}/{name}.json", timeout=20)
    reply.raise_for_status()
    data = reply.json()
    if data["chainId"] != 1:
        raise ValueError("Ranges manifest chain")
    return data


def record(endpoint):
    core, stack = manifest("balancer-core"), manifest("range")
    roles = {k: core[k].lower() for k in ("router", "vault", "weth", "permit2")}
    roles.update({k: stack[k].lower() for k in ("factory", "hook")})
    if roles["vault"] != stack["vault"].lower():
        raise ValueError("Ranges vault mismatch")
    chain, header, *codes = read_batch(endpoint, [("eth_chainId", []), ("eth_getBlockByHash", [BLOCK, False]),
        *(("eth_getCode", [address, AT]) for address in roles.values())])
    if chain != "0x1" or header["stateRoot"] != ROOT_HASH or header["hash"] != BLOCK or "0x" in codes:
        raise ValueError("chain, state root or missing role")
    c = Capture(dict(endpoint=endpoint, selector=AT))
    factory, vault, router = (roles[k] for k in ("factory", "vault", "router"))
    for contract in (factory, router):
        if c.one(contract, "getVault()", returns=["address"])[0] != vault:
            raise ValueError("contract belongs to a different vault")
    pools, = c.one(factory, "getPools()", returns=["address[]"])
    count, = c.one(factory, "getPoolCount()", returns=["uint256"])
    if len(set(pools)) != len(pools) or len(pools) != count or not count:
        raise ValueError("registry incomplete")
    records = []
    for start in range(0, count, 6):
        group = pools[start:start + 6]
        calls = [(pool, sig, [], []) for pool in group for sig in (
            "getRangePoolImmutableData()", "getRangePoolDynamicData()", "getHook()", "getVault()")]
        states = posts(c, calls)
        bindings = posts(c, [(a, k, ["address"], [pool]) for pool in group for a, k in (
            (factory, "isPoolFromFactory(address)"), (vault, "isPoolRegistered(address)"),
            (vault, "getPoolTokens(address)"), (vault, "getHooksConfig(address)"))])
        for i, pool in enumerate(group):
            imm, dyn, hook, own_vault = states[4*i:4*i+4]
            member, registered, assets, hook_config = bindings[4*i:4*i+4]
            tokens, scales, weights = decode([IMMUTABLE], imm)[0]
            state = dict(zip(DYNAMIC_KEYS, decode([DYNAMIC], dyn)[0], strict=True))
            flags = decode(["bool"]*10 + ["address"], hook_config)
            if (not decode(["bool"], member)[0] or not decode(["bool"], registered)[0]
                    or decode(["address[]"], assets)[0] != tokens or decode(["address"], own_vault)[0] != vault
                    or decode(["address"], hook)[0] != flags[-1] or flags[-1] != roles["hook"]
                    or not all(len(a) == len(tokens) for a in (scales, weights, *(state[k] for k in DYNAMIC_KEYS[:5])))):
                raise ValueError(f"mismatched pool bindings or dimensions: {pool}")
            records.append(dict(pool=pool, tokens=tokens, scales=scales, weights=weights, hooks=flags[:-1], **state))
    return dict(chain_id=1, height=int(header["number"], 16), selector=AT, state_root=ROOT_HASH,
                manifests={k: f"{MANIFEST}/{k}.json" for k in ("range", "balancer-core")},
                roles=roles, runtime={k:"0x"+keccak(hexstr=v).hex() for k,v in zip(roles,codes,strict=True)},
                pools=records)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path)
    parser.add_argument("--endpoint", default=ENDPOINT)
    args = parser.parse_args()
    result = record(args.endpoint)
    data = json.dumps(result, separators=(",", ":")).encode()
    args.out.write_bytes(gzip.compress(data, mtime=0) if args.out.suffix == ".gz" else data)
    print("Registered markets:", len(result["pools"]))
