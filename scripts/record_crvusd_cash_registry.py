"""Read the immutable USDT/crvUSD clone and the factory that registered it at the paid archive head."""
import argparse
import gzip
import json
from pathlib import Path

from eth_utils import keccak

from fork_fee_rpc import Reader
from record_llama_base_cl import CanonicalCalls
from record_yield_basis_usdt import BRIDGE
from record_ethereum_v2_factories import STABLES

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
FACTORY = "0x4f8846ae9380b90d2e71d5e3d042dff3e7ebb40d"
OLD_FACTORY = "0xb9fc157394af804a3578134a6585c0dc9cc990d4"
CRVUSD = "0xf939e0a03fb07f59a73314e73794be0e57ac1b4e"
UINT = "uint256"


def read():
    head = json.loads((DATA / "ethereum.json").read_bytes())
    wallets = json.loads(gzip.decompress((DATA / "llamma_ethereum_wallets.json.gz").read_bytes()))
    reader = Reader("ethereum", head)
    cap = CanonicalCalls(reader)
    if wallets["selector"] != reader.tag or wallets["bridge"] != BRIDGE:
        raise ValueError("different payment root")
    def one(target, sig, args=(), values=(), ret=UINT):
        return cap.one(target, sig, list(args), list(values), [ret])[0]
    coins = [one(BRIDGE, "coins(uint256)", [UINT], [i], "address") for i in range(2)]
    if coins != [STABLES[1], CRVUSD] or one(BRIDGE, "factory()", ret="address") != FACTORY:
        raise ValueError("unexpected pool registry or currencies")
    for pair in [coins, coins[::-1]]:
        if one(FACTORY, "find_pool_for_coins(address,address)", ["address"] * 2, pair, "address") != BRIDGE:
            raise ValueError("wrong factory pair")
    implementation = one(FACTORY, "get_implementation_address(address)", ["address"], [BRIDGE], "address")
    if int(one(OLD_FACTORY, "get_implementation_address(address)", ["address"], [BRIDGE], "address"), 16):
        raise ValueError("duplicate registration")
    contracts = {}
    for addr in (FACTORY, BRIDGE, implementation):
        code = reader.one("eth_getCode", [addr, reader.tag])
        if code == "0x" or (addr == BRIDGE and code != "0x363d3d373d3d3d363d73" + implementation[2:] + "5af43d82803e903d91602b57fd5bf3"):
            raise ValueError("immutable pool implementation mismatch")
        contracts[addr] = {"bytes": (len(code) - 2) // 2, "keccak256": "0x" + keccak(hexstr=code).hex()}
    quotes = []
    for row in wallets["usdt"]:
        i = 0 if row["source"] == coins[0] else 1
        dx = int(row["intermediateSpent"] if i else row["spent"])
        dy = one(BRIDGE, "get_dy(int128,int128,uint256)", ["int128", "int128", UINT], [i, 1-i, dx])
        if dy != int(row["received"] if i else row["intermediateReceived"]):
            raise ValueError("payment and canonical bridge quote differ")
        quotes.append(dict(i=i, j=1-i, dx=str(dx), dy=str(dy)))
    return dict(selector=reader.tag, factory=FACTORY, pool=BRIDGE, implementation=implementation,
                contracts=contracts, coins=coins, count=one(FACTORY, "pool_count()"),
                a=str(one(BRIDGE, "A_precise()")), fee=str(one(BRIDGE, "fee()")),
                balances=[str(one(BRIDGE, "balances(uint256)", [UINT], [i])) for i in range(2)],
                decimals=[one(coin, "decimals()") for coin in coins], quotes=quotes)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(read(), indent=2) + "\n", encoding="utf-8")

