"""Канонические роли и физические токены архивных PCSX котировок. Только чтение блокчейна."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
from record_gladius_fills import Reader, addr, call_data, canonical, hashed, runtime
from record_pancakex_history import NETS, PCS

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
B20_FACTORY = "0xb20f000000000000000000000000000000000000"
POLICY = "0x8453000000000000000000000000000000000002"


def profile(net):
    chain, height, urls, _, _ = NETS[net]
    snapshot = json.loads(gzip.decompress((DATA / f"pancakex_{net}_history.json.gz").read_bytes()))
    quotes = json.loads(gzip.decompress((DATA / "pancakex_unsigned_quotes.json.gz").read_bytes()))
    reader = Reader(urls)
    block = reader.rpc("eth_getBlockByNumber", [hex(height), False])
    at = canonical(block)
    assert block["hash"] == snapshot["head"]["hash"]
    roles = snapshot["roles"][PCS]
    contracts = [f"{chain} router {PCS} {roles['runtime']}",
                 f"{chain} implementation {roles['implementation']} {roles['implementation_runtime']}"]
    for kind, name in [("permit", "permit2()"), ("v1", "exclusiveDutchReactor()"), ("v3", "v3DutchReactor()")]:
        role = roles["facets_and_permit"][name]
        contracts.append(f"{chain} {kind} {role['address']} {role['runtime']}")
    if net == "ethereum":
        direct = "0x35db01d1425685789dcc9228d47c7a5c049388d8"
        assert addr(reader.view(direct, call_data("permit2()"), at)) == roles["facets_and_permit"]["permit2()"]["address"]
        contracts.append(f"{chain} standalone-v1 {direct} {runtime(reader, direct, at)}")
    permit = roles["facets_and_permit"]["permit2()"]["address"]
    domain = reader.view(permit, call_data("DOMAIN_SEPARATOR()"), at)
    assert domain == hashed(encode(["bytes32", "bytes32", "uint256", "address"],
                                   [keccak(text="EIP712Domain(string name,uint256 chainId,address verifyingContract)"),
                                    keccak(text="Permit2"), chain, permit]))
    subject = next(x for x in quotes if x["status"] == 200 and x["body"]["tokenInChainId"] == chain)
    remote = subject["response"]["message"]["bestOrder"]["order"]
    owner, order_hash = remote["orderInfo"]["swapper"], remote["orderHash"]
    cancelled = reader.view(PCS, call_data("cancelledOrders(address,bytes32)", ["address", "bytes32"],
                                          [owner, bytes.fromhex(order_hash[2:])]), at)
    coins = {r["body"][k] for r in quotes if r["status"] == 200 and r["body"]["tokenInChainId"] == chain
             for k in ["tokenIn", "tokenOut"]}
    if chain == 1:
        coins.add("0xdac17f958d2ee523a2206206994597c13d831ec7")
    tokens = []
    for token in sorted(coins):
        code = runtime(reader, token, at)
        if token.startswith("0xb200000000"):
            assert code == hashed(bytes([0xef]))
            tokens.append(f"{chain} b20 {token} {code} {B20_FACTORY} {POLICY}")
        elif token.startswith(("0x833589", "0xa0b869")):
            def slot(name):
                key = "0x" + keccak(text=f"org.zeppelinos.proxy.{name}").hex()
                return addr(reader.rpc("eth_getStorageAt", [token, key, at]))
            impl = slot("implementation")
            tokens.append(f"{chain} fiat {token} {code} {impl} {runtime(reader, impl, at)} {slot('admin')}")
        else:
            kind = "tether-net" if chain == 1 and token == "0xdac17f958d2ee523a2206206994597c13d831ec7" else "exact"
            tokens.append(f"{chain} {kind} {token} {code}")
    return {"block": height, "selector": at, "contracts": contracts, "tokens": tokens, "permit_domain": domain, "cancellation_read": {"owner": owner, "order": order_hash, "cancelled": int(cancelled, 16)}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True, type=Path)
    path = parser.parse_args().out
    path.write_text(json.dumps({n: profile(n) for n in NETS}, ensure_ascii=False, indent=2) + "\n", encoding="utf8", newline="\n")
