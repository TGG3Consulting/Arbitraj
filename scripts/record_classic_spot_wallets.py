"""Replay the configured Sushi and Pancake classic routers and V5 at the same canonical reserves."""
import argparse
import json
from decimal import Decimal
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata, rpc
from record_v2_chain_pools import NETWORKS, measure, view

PROFILES = {
    "base": ("sushiswap-v2-base", "pancakeswap-v2-base"),
    "ethereum": ("sushiswap", "pancakeswap_ethereum"),
}
USDC = {"base": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
        "ethereum": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"}


def capture(network):
    chain = json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())[network]
    profiles = json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    endpoint, wrapped = NETWORKS[network]
    tag = {"blockHash": chain["hash"], "requireCanonical": True}

    def address(target, method, args=()):
        payload = calldata(method, ["address"] * len(args), list(args))
        return decode(["address"], view(endpoint, tag, target, payload))[0].lower()

    result = {}
    for key in PROFILES[network]:
        profile, = [p for p in profiles if p["id"] == key and p["chain_id"] == chain["chain"]]
        settings = profile["settings"]
        factory, router = (settings[k].lower() for k in ("dex.factory", "dex.router"))
        if address(router, "factory()") != factory or address(router, "WETH()") != wrapped:
            raise ValueError(f"{key}: router factory or wrapped currency mismatch")
        pool = address(factory, "getPair(address,address)", (wrapped, USDC[network]))
        tokens = [address(pool, method) for method in ("token0()", "token1()")]
        if set(tokens) != {wrapped, USDC[network]} or address(pool, "factory()") != factory:
            raise ValueError(f"{key}: pair membership mismatch")
        fee = Decimal(settings["dex.v2_fee_pct"]) * 10000
        if settings["dex.v2_fee_source"] != "fixed" or fee != int(fee):
            raise ValueError(f"{key}: incompatible configured fee")
        code = bytes.fromhex(rpc(endpoint, "eth_getCode", [pool, tag])[2:])
        row = {"dex": key, "network": network, "pool": pool, "factory": factory,
               "tokens": tokens, "runtime_hash": "0x"+keccak(code).hex(),
               "verify_pair_boundary": True, "compare_native_router": router,
               "fee_fraction": [1_000_000, int(fee)]}
        result[key] = measure(row, chain, endpoint)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("network", choices=PROFILES)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(capture(args.network), sort_keys=True) + "\n")
