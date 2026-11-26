"""Verify both Xchange generations and replay ordinary V5 swap settlement on Ethereum."""
import argparse
import json
import re
from pathlib import Path

import requests
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, rpc
from record_ethereum_v2_factories import digest, directory, raw

MARKETS = {
    "x7-finance": ("0x7de800467afce442019884f51a4a1b9143a34fac", "0x4badcb6ad8352875290cb5e356f1de61450f9609"),
    "x7-finance-ethereum": ("0x8b76c05676d205563ffc1cbd11c0a6e3d83929c5", "0xbd7908e528341bb69006f9d9524f94b65385d891"),
}
ROOT_HASH = "0xbd73f03df514c4d550c21a6d567e25a4190f646acbb94b8e8c2462d7c9b17db7"
ENDPOINTS = ("https://mainnet.gateway.tenderly.co", "https://eth.drpc.org", "https://ethereum-rpc.publicnode.com")


def verified(c, address, name):
    url = "https://sourcify.dev/server/v2/contract/1/" + address
    params = {"fields": "compilation,sources,runtimeBytecode.onchainBytecode,creationBytecode.onchainBytecode"}
    response = requests.get(url, params=params, timeout=24)
    response.raise_for_status()
    record = response.json()
    code = raw(rpc(c.endpoint, "eth_getCode", [address, c.selector]))
    if record["compilation"]["name"] != name or not code or code != raw(record["runtimeBytecode"]["onchainBytecode"]):
        raise ValueError(f"deployed {name} differs from the verified code on the selected block")
    return url, record, code


def verify_factory(c, factory, pair):
    url, _, factory_code = verified(c, factory, "XchangeFactory")
    pair_url, record, pair_code = verified(c, pair, "XchangePair")
    init = raw(record["creationBytecode"]["onchainBytecode"])
    offset = factory_code.find(init)
    if not init or offset < 0 or factory_code.find(init, offset + 1) >= 0:
        raise ValueError("canonical pair creation code is missing or ambiguous in factory runtime")
    source = next(item["content"] for path, item in record["sources"].items() if path.endswith("XchangePair.sol"))
    normalized = re.sub(r"\s+", "", source)
    checks = [
        "functionswap(uintamount0Out,uintamount1Out,addressto,bytescalldatadata)externallock{_swap(amount0Out,amount1Out,to,200,data);}",
        "uintfeeAmount=200;if(feeAmountOverride!=200){",
        "(balances[0]*100000)-(amount0In*feeAmount)",
        "(balances[1]*100000)-(amount1In*feeAmount)",
    ]
    if not all(check in normalized for check in checks):
        raise ValueError("ordinary pair.swap fee or invariant changed")
    return dict(url=url, runtime=digest(factory_code), creation_code_hash=digest(init),
                codecopy=[offset,len(init)], verified_pair=pair_url, pair_runtime=digest(pair_code),
                standard_swap_fee={"entry":"swap(uint256,uint256,address,bytes)","numerator":200,"denominator":100000})


def capture(name, endpoint, snap):
    cap = Capture(dict(snap, endpoint=endpoint))
    head = rpc(endpoint, "eth_getBlockByHash", [cap.selector["blockHash"], False])
    if (int(rpc(endpoint, "eth_chainId", []), 16) != 1 or head["hash"] != cap.selector["blockHash"]
            or int(head["number"],16)!=snap["block"] or head["stateRoot"] != ROOT_HASH):
        raise ValueError("wrong chain or canonical header")
    factory, pair = MARKETS[name]
    proof = verify_factory(cap, factory, pair)
    result = directory(name, cap, config=(factory,100000,200,"Xchange: K"), proof=proof)
    if pair not in {p["pool"] for p in result["pools"]}:
        raise ValueError("indexer sample is absent from factory registry")
    result.update(selector=cap.selector, height=snap["block"], state_root=ROOT_HASH, endpoint=endpoint)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("market", choices=MARKETS)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    snap = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    failures = []
    for endpoint in ENDPOINTS:
        try:
            result = capture(args.market, endpoint, snap)
            args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf8")
            break
        except (requests.RequestException, ValueError, KeyError, RuntimeError) as why:
            failures.append((endpoint, str(why)[:220]))
    else:
        raise RuntimeError(f"could not reproduce the canonical Xchange registry: {failures}")
