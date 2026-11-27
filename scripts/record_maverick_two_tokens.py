"""Сверяет проверенные исходники токенов и код на блоке реестра Maverick V2."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

import requests
from eth_utils import keccak
from record_base_slipstream_executor import rpc


TOKENS = {
    "base": {
        "DEGEN": ("0x4ed4e862860bed51a9570b96d89af5e1b0efefed", "contracts/DegenToken.sol:DegenToken",
                  "ERC20Pausable and ERC20Votes delegate _update with the unchanged value; pause reverts."),
        "VIRTUAL": ("0x0b3e328455c4059eeb9e3f84b5543f74e24e7e1b", "contracts/OptimismMintableERC20.sol:OptimismMintableERC20",
                    "Local transfer and transferFrom inherit ERC20; OFT cross-chain debits and bridge mint/burn are separate entries."),
    },
    "ethereum": {
        "SKY": ("0x56072c95faa701256059aa122697b133aded9279", "src/Sky.sol:Sky",
                "Both local transfers add and subtract value exactly; mint and burn are separate entries."),
        "wstETH": ("0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0", "WstETH.sol:WstETH",
                   "The ERC20 moves exact wrapper shares. No conversion to stETH is called by the trade."),
    },
}


def collect(data, cache):
    rows = []
    for network, tokens in TOKENS.items():
        anchor = json.loads(gzip.decompress((data / f"maverick_v2_{network}_directory.json.gz").read_bytes()))
        chain, at = anchor["chain_id"], anchor["selector"]
        endpoint = "https://mainnet.base.org" if network == "base" else "https://eth.drpc.org"
        if int(rpc(endpoint, "eth_chainId", []), 16) != chain:
            raise ValueError("Wrong chain")
        for name, (address, expected, transfers) in tokens.items():
            url = f"https://sourcify.dev/server/v2/contract/{chain}/{address}?fields=all"
            saved = cache / (name.lower() + ".json") if cache else None
            if saved and saved.exists():
                record = json.loads(saved.read_bytes())
            else:
                response = requests.get(url, timeout=35)
                response.raise_for_status()
                record = response.json()
            code = bytes.fromhex(rpc(endpoint, "eth_getCode", [address, at])[2:])
            primary = record["compilation"]["fullyQualifiedName"]
            if (not code or record["address"].lower() != address or int(record["chainId"]) != chain
                    or record["runtimeMatch"] not in ("match", "exact_match") or primary != expected
                    or code.hex() != record["runtimeBytecode"]["onchainBytecode"].removeprefix("0x")):
                raise ValueError(f"Source or runtime mismatch for {name}")
            source = record["sources"][primary.split(":")[0]]["content"].encode()
            rows.append(dict(network=network, chain=chain, selector=at, address=address, symbol=name, source=url,
                             primary=primary, runtime_match=record["runtimeMatch"], code_bytes=len(code),
                             keccak256="0x" + keccak(code).hex(), source_sha256=hashlib.sha256(source).hexdigest(),
                             deployment=record["deployment"]["transactionHash"], transfer_rule=transfers))
    return rows


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output", type=Path)
    ap.add_argument("--cache", type=Path)
    args = ap.parse_args()
    data = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
    args.output.write_bytes((json.dumps(collect(data, args.cache), indent=2) + "\n").encode())
