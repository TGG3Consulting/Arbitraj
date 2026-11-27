"""Снимает три реестра Curve Spectra и физические IBT/PT на одном каноническом блоке Base."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from record_base_launch_bindings import read
from record_omni_flux_registry import batch, data

BLOCK = 62262756
HASH = "0xdf458884e3b25319497f122bfe50a2dff59b0079421fec1e1a9ac403d3c3e715"
AT = {"blockHash": HASH, "requireCanonical": True}
REGISTRY = "0x786da12e9836a9ff9b7d92e8bac1c849e2ace378"
LEGACY = ["0x7ea3097e2af59ea705398544e0f58eddb7bd1852", "0x04ec86bf37dfd77397bcc2e386cb37f175b4e106"]
DOCS = "https://dev.spectra.finance/docs/technical-reference/deployed-contracts"


def call(address, sig, result, *args):
    types = sig[sig.index("(") + 1:-1]
    return address, data(sig, types.split(",") if types else [], args), result


def many(calls, optional=False):
    out = []
    for start in range(0, len(calls), 60):
        part = calls[start:start + 60]
        for (target, calldata, typ), (ok, raw) in zip(part, batch(AT, [c[:2] for c in part]), strict=True):
            if not ok or not raw:
                if not optional:
                    raise ValueError(f"{target} {calldata.hex()} не вернул ABI")
                value = None
            else:
                value = decode([typ], raw)[0]
            out.append(value)
    return out


def indexed(address, count, item, typ):
    n, = many([call(address, count, "uint256")])
    values = many([call(address, item, typ, i) for i in range(n)])
    if len(set(values)) != n:
        raise ValueError(f"{address}: повтор в реестре")
    return values


def info(rows, queries, optional=False):
    plans = [(r, k, c) for r in rows for k, c in queries(r).items()]
    for (row, key, _), value in zip(plans, many([x[2] for x in plans], optional), strict=True):
        row[key] = value


def topology():
    head = read("eth_getBlockByNumber", [hex(BLOCK), False])
    if head["hash"] != HASH:
        raise ValueError("изменилась каноническая точка")
    factory, beacon, router, util = many([call(REGISTRY, s, "address") for s in
                                         ("getFactory()", "getPTBeacon()", "getRouter()", "getRouterUtil()")])
    back, current = many([call(factory, s, "address") for s in ("getRegistry()", "getCurveFactory()")])
    if back != REGISTRY:
        raise ValueError("factory.registry не совпала")
    pt_set = set(indexed(REGISTRY, "pTCount()", "getPTAt(uint256)", "address"))
    rows = [dict(factory=f, pool=p, kind=k) for k, f in [("legacy", x) for x in LEGACY] + [("sng", current)]
            for p in indexed(f, "pool_count()", "pool_list(uint256)", "address")]
    info(rows, lambda r: {**{f"coin{i}": call(r["pool"], "coins(uint256)", "address", i) for i in range(2)},
                          **{f"balance{i}": call(r["pool"], "balances(uint256)", "uint256", i) for i in range(2)}})
    if len({r["pool"] for r in rows}) != len(rows) or {r["coin1"] for r in rows} != pt_set:
        raise ValueError("состав пулов и реестр PT расходятся")
    info(rows, lambda r: {
        "registered": call(REGISTRY, "isRegisteredPT(address)", "bool", r["coin1"]),
        "ibt": call(r["coin1"], "getIBT()", "address"), "underlying": call(r["coin1"], "underlying()", "address"),
        "yt": call(r["coin1"], "getYT()", "address"), "maturity": call(r["coin1"], "maturity()", "uint256"),
        "paused": call(r["coin1"], "paused()", "bool"),
        "decimals0": call(r["coin0"], "decimals()", "uint8"), "decimals1": call(r["coin1"], "decimals()", "uint8"),
        "factory_coins": call(r["factory"], "get_coins(address)", "address[]" if r["kind"]=="sng" else "address[2]", r["pool"])})
    info([r for r in rows if r["kind"] == "legacy"], lambda r: {"own_factory":call(r["pool"],"factory()","address"),
        "template":call(r["factory"],"pool_implementation()","address")})
    info([r for r in rows if r["kind"] == "sng"], lambda r: {"version":call(r["pool"],"version()","string"),
        "asset_types":call(r["factory"],"get_pool_asset_types(address)","uint8[]",r["pool"]),
        "meta":call(r["factory"],"is_meta(address)","bool",r["pool"]),
        "template":call(r["factory"],"get_implementation_address(address)","address",r["pool"])})
    info(rows, lambda r: {"underlying_decimals": call(r["underlying"], "decimals()", "uint8")})
    for r in rows:
        if (not r["registered"] or r["ibt"] != r["coin0"] or tuple(r["factory_coins"]) != (r["coin0"],r["coin1"])
                or r.get("own_factory",r["factory"]) != r["factory"]):
            raise ValueError(f"не подтверждена связь пула {r['pool']}")
    return dict(chain_id=8453, block=BLOCK, selector=AT, state_root=head["stateRoot"], timestamp=int(head["timestamp"],16),
                sources=[DOCS], registry=REGISTRY, pt_beacon=beacon, factory=factory, router=router, router_util=util, markets=rows)


def quotes(records):
    rows = records["markets"]
    def probes(r):
        ops = {}
        typ = "int128" if r["kind"] == "sng" else "uint256"
        for i in range(2):
            for scale in (100000, 1000):
                amount = max(r[f"balance{i}"] // scale, 1)
                ops[f"quote_{i}_{amount}"] = call(r["pool"], f"get_dy({typ},{typ},uint256)", "uint256", i, 1-i, amount)
        return ops
    info(rows, probes, optional=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("tests/data/first_run_dex_2026-11-26/spectra_base_directory.json.gz"))
    args = parser.parse_args()
    snapshot = topology()
    quotes(snapshot)
    args.output.write_bytes(gzip.compress(json.dumps(snapshot, separators=(",", ":")).encode(), mtime=0))
