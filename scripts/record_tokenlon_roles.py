"""Record the settlement contracts, transfer implementations and wiring at one canonical hash."""
import json
from pathlib import Path

from eth_utils import keccak
from record_gladius_fills import Reader, RPCS, addr, call_data, runtime

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "tests/data/first_run_dex_2026-11-26"


def main():
    source = json.loads((DATA / "tokenlon_ethereum.json").read_text())
    at = source["selector"]
    r = Reader(RPCS["ethereum"])
    official = source["roles"]
    pmm = official["pmm"]["address"]
    reads = {}

    def linked(address, method):
        result = r.view(address, call_data(method), at)
        if len(result) != 66 or int(result[2:26], 16) != 0 or int(result, 16) == 0:
            raise ValueError((address, method, result))
        reads[f"{address}.{method}"] = result
        return addr(result)

    spender = linked(pmm, "spender()")
    proxy = linked(pmm, "userProxy()")
    exchange = linked(pmm, "zeroExchange()")
    assert proxy == official["user_proxy"]["address"]
    assert exchange == official["zero_exchange"]["address"]
    wrapped = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
    roles = dict(user_proxy=proxy, proxy_impl=official["user_proxy_impl"]["address"], pmm=pmm,
                 zero_exchange=exchange, spender=spender,
                 allowance_target=linked(spender, "allowanceTarget()"), weth=wrapped)
    assets = {"tether-net": "0xdac17f958d2ee523a2206206994597c13d831ec7",
              "fiat-usdc": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
              "exact-wbtc": "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599",
              "exact-uni": "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984",
              "wrapped-native": wrapped}
    chain = int(r.rpc("eth_chainId", []), 16)
    assert chain == source["chain"]
    def row(name, address):
        return f"{chain} {name} {address} {runtime(r, address, at)}"

    role_rows = [row(k, v) for k, v in roles.items()]
    asset_rows = []
    for name, address in assets.items():
        fields = row(name, address)
        if name == "fiat-usdc":
            slot = "0x" + keccak(text="org.zeppelinos.proxy.implementation").hex()
            value = r.rpc("eth_getStorageAt", [address, slot, at])
            impl = addr(value)
            fields += f" {impl} {runtime(r, impl, at)}"
            reads[f"{address}.{slot}"] = value
        asset_rows.append(fields)
    record = dict(chain=chain, block=source["block"], selector=at, roles=role_rows,
                  assets=asset_rows, reads=reads)
    (DATA / "tokenlon_ethereum_roles.json").write_bytes((json.dumps(record, indent=2) + "\n").encode())


if __name__ == "__main__":
    main()
