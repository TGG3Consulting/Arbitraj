"""Снимок фабрики Fraxswap V2 и её TWAMM на одном каноническом блоке Ethereum."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, calldata, rpc
from record_community_v3_ethereum import posts

FACTORY = "0x43ec799eadd63848443e2347c49f5f52e8fe0f6f"
UINT = "uint256"


def capture(endpoint, selected=()):
    source = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c = Capture(dict(source, endpoint=endpoint))
    head = rpc(endpoint, "eth_getBlockByHash", [c.selector["blockHash"], False])
    if int(rpc(endpoint, "eth_chainId", []), 16) != 1 or head["hash"] != c.selector["blockHash"] or int(head["number"], 16) != source["block"]:
        raise ValueError("wrong chain or head")
    timestamp = int(head["timestamp"], 16)
    count, = c.one(FACTORY, "allPairsLength()", returns=[UINT])
    listings = posts(c, [(FACTORY, "allPairs(uint256)", [UINT], [i]) for i in range(count)])
    pools = [decode(["address"], value)[0] for value in listings]
    if len(set(pools)) != count:
        raise ValueError("duplicate registry entry")
    read = iter(posts(c, [op for pool in pools for op in [
        (pool, "factory()", [], []), (pool, "token0()", [], []), (pool, "token1()", [], []),
        (pool, "getTwammReserves()", [], []), (pool, "getTwammState()", [], []),
        (pool, "fee()", [], []), (pool, "newSwapsPaused()", [], [])]]))
    markets = []
    for pool in pools:
        owner, token0, token1 = [decode(["address"], next(read))[0] for _ in range(3)]
        if owner != FACTORY or token0 >= token1:
            raise ValueError("wrong factory or tokens")
        reserves = decode([UINT] * 6, next(read))
        state = decode([UINT] * 6, next(read))
        retained, = decode([UINT], next(read))
        paused, = decode(["bool"], next(read))
        if retained + reserves[5] != 10000:
            raise ValueError("inconsistent fee fraction")
        markets.append(dict(pool=pool, tokens=[token0, token1], twamm_reserves=list(map(str,reserves)),
                            twamm_state=list(map(str,state)), retained_bps=retained,
                            new_swaps_paused=paused))
    refs = iter(posts(c, [op for m in markets for op in [
        (FACTORY, "getPair(address,address)", ["address"] * 2, m["tokens"]),
        (FACTORY, "getPair(address,address)", ["address"] * 2, list(reversed(m["tokens"])))]]))
    balance_data = [(token, True, bytes.fromhex(calldata("balanceOf(address)", ["address"], [m["pool"]])[2:]))
                    for m in markets for token in m["tokens"]]
    multi = "0xca11bde05977b3631167028862be2a173976ca11"
    balances, = c.one(multi, "aggregate3((address,bool,bytes)[])", ["(address,bool,bytes)[]"],
                      [balance_data], ["(bool,bytes)[]"])
    balances = iter(balances)
    for m in markets:
        if any(decode(["address"], next(refs))[0] != m["pool"] for _ in range(2)):
            raise ValueError("unregistered pair")
        m["balances"] = [str(decode([UINT], data)[0]) if ok and len(data) == 32 else None
                         for ok, data in (next(balances), next(balances))]
    for m in markets:
        if m["pool"] in selected:
            for t in [timestamp, timestamp + 12]:
                out, = c.batch([(m["pool"], "getReserveAfterTwamm(uint256)", [UINT], [t])])
                evidence = c.calls[-1]
                evidence["decoded"] = list(map(str,decode([UINT]*5, out))) if out else None
                m.setdefault("projection", {})[str(t)] = evidence
    router = "0xc14d550632db8592d1243edc8b95b0ad06703867"
    bound, = c.one(router, "factory()", returns=["address"])
    if bound != FACTORY:
        raise ValueError("official router belongs to a different factory")
    contracts = {}
    for address in (router, FACTORY):
        code = bytes.fromhex(rpc(endpoint, "eth_getCode", [address, c.selector])[2:])
        if not code:
            raise ValueError("missing deployment")
        contracts[address] = dict(bytes=len(code), keccak256="0x"+keccak(code).hex())
    return dict(chain=1, selector=c.selector, height=source["block"], state_root=head["stateRoot"],
                timestamp=timestamp, factory=FACTORY, router=router, contracts=contracts,
                markets=markets, endpoint=endpoint)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output", type=Path)
    ap.add_argument("--endpoint", default="https://eth.drpc.org")
    ap.add_argument("--project", action="append", default=[])
    args = ap.parse_args()
    result = capture(args.endpoint, args.project)
    args.output.write_text(json.dumps(result, indent=2)+"\n", encoding="utf-8")
