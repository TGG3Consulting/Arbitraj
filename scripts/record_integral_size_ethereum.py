"""Prove the Integral SIZE atomic/queued distinction from the complete Ethereum registry and canonical payments."""

import argparse
import gzip
import json
from pathlib import Path
import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT, calldata
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH

COMMIT = "91d8803b1732dfe675e25754332f1078945dcc89"
MANIFEST = (
    f"https://raw.githubusercontent.com/IntegralHQ/Integral-SIZE-Smart-Contracts/{COMMIT}/deployments.json"
)
SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
SELL = "Sell(address,address,address,uint256,uint256,uint256,bool,uint256,address,address,uint256)"
BUY = SELL.replace("Sell(", "Buy(")
ZERO = "0x" + "00" * 20
MC = "0xca11bde05977b3631167028862be2a173976ca11"


def topic(s):
    return "0x" + keccak(text=s).hex()


def address(b):
    return decode(["address"], b)[0]


def ctrl(endpoint):
    snap = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    http = requests.Session()
    c = Capture(dict(snap, endpoint=endpoint))
    tag = c.selector

    def rpc(m, p):
        if m not in {
            "eth_chainId",
            "eth_getBlockByNumber",
            "eth_getBlockByHash",
            "eth_getCode",
            "eth_getStorageAt",
            "eth_getBalance",
            "eth_getLogs",
            "eth_getTransactionReceipt",
        }:
            raise ValueError(m)
        errors = []
        for url in dict.fromkeys([endpoint, *ENDPOINTS]):
            try:
                res = http.post(url, json=dict(jsonrpc="2.0", id=1, method=m, params=p), timeout=18)
                res.raise_for_status()
                reply = res.json()
                if reply.get("result") is None:
                    raise ValueError(str(reply.get("error"))[:140])
                return reply["result"]
            except (requests.RequestException, ValueError) as e:
                errors.append(str(e)[:160])
        raise ValueError((m, errors))

    def head(n):
        return rpc("eth_getBlockByNumber", [hex(n), False])

    m = requests.get(MANIFEST, timeout=12)
    m.raise_for_status()
    manifest = m.json()["mainnet"]
    roles = {k: manifest[k]["address"].lower() for k in ("factory", "delay", "relayer", "relayerProxy")}
    factory, delay, impl, proxy = roles.values()
    h = head(snap["block"])
    if rpc("eth_chainId", []) != "0x1" or h["hash"] != tag["blockHash"] or h["stateRoot"] != ROOT_HASH:
        raise ValueError("foreign head")
    codes = {}
    for k, a in roles.items():
        wire = bytes.fromhex(rpc("eth_getCode", [a, tag])[2:])
        codes[k] = dict(address=a, bytes=len(wire), keccak256="0x" + keccak(wire).hex())
        if not wire:
            raise ValueError("undeployed role")
        source = "https://sourcify.dev/server/v2/contract/1/" + a
        reply = http.get(source, params={"fields": "compilation,runtimeBytecode.onchainBytecode"}, timeout=20)
        reply.raise_for_status()
        verified = reply.json()
        names = {
            "relayerProxy": "Proxy",
            "delay": "TwapDelay",
            "factory": "TwapFactory",
            "relayer": "TwapRelayer",
        }
        if (
            verified["compilation"]["name"] != names[k]
            or bytes.fromhex(verified["runtimeBytecode"]["onchainBytecode"][2:]) != wire
        ):
            raise ValueError("verified source does not match pinned bytecode")
        codes[k].update(source=source, contract=names[k], match=verified["match"])

    bound = posts(
        c,
        [(proxy, k, [], []) for k in ("factory()", "delay()", "implementation()", "weth()")]
        + [(delay, "factory()", [], []), (delay, "relayer()", [], [])],
    )
    if [address(x) for x in bound] != [
        factory,
        delay,
        impl,
        manifest["tokens"]["weth"]["address"].lower(),
        factory,
        proxy,
    ] or address(bytes.fromhex(rpc("eth_getStorageAt", [proxy, SLOT, tag])[2:])) != impl:
        raise ValueError("wrong proxy/factory/delay")
    (count,) = c.one(factory, "allPairsLength()", returns=["uint256"])
    pairs = [
        address(b) for b in posts(c, [(factory, "allPairs(uint256)", ["uint256"], [i]) for i in range(count)])
    ]
    if len(set(pairs)) != count:
        raise ValueError("duplicate factory pair")
    vals = posts(
        c,
        [
            (a, f, [], [])
            for a in pairs
            for f in ("factory()", "token0()", "token1()", "oracle()", "trader()")
        ],
    )
    flags = posts(
        c,
        [
            (dest, fn, ["address"], [p])
            for p in pairs
            for dest, fn in [
                (proxy, "isPairEnabled(address)"),
                (delay, "getBuyDisabled(address)"),
                (delay, "getSellDisabled(address)"),
            ]
        ],
    )
    markets = []
    checks = []
    tokens = set()
    for i, p in enumerate(pairs):
        f, x, y, oracle, trader = map(address, vals[i * 5 : i * 5 + 5])
        enabled, buy_disabled, sell_disabled = [decode(["bool"], b)[0] for b in flags[i * 3 : i * 3 + 3]]
        if f != factory or x >= y or oracle == ZERO:
            raise ValueError("invalid pair")
        markets.append(
            dict(
                pair=p,
                tokens=[x, y],
                oracle=oracle,
                trader=trader,
                atomic_enabled=enabled,
                delayed_buy_enabled=not buy_disabled,
                delayed_sell_enabled=not sell_disabled,
            )
        )
        checks.extend(
            [(factory, "getPair(address,address)", ["address", "address"], ts) for ts in [(x, y), (y, x)]]
        )
        tokens.update([x, y])
    rev = posts(c, checks)
    if any(address(b) != pairs[k // 2] for k, b in enumerate(rev)):
        raise ValueError("factory reverse lookup")
    physical = posts(
        c, [(asset, "balanceOf(address)", ["address"], [v["pair"]]) for v in markets for asset in v["tokens"]]
    )
    for i, v in enumerate(markets):
        v["pool_balances"] = [str(int.from_bytes(b, "big")) for b in physical[i * 2 : i * 2 + 2]]
    assets = sorted(tokens)
    read = posts(c, [(a, "balanceOf(address)", ["address"], [proxy]) for a in assets])
    inventory = dict(zip(assets, map(lambda b: str(int.from_bytes(b, "big")), read), strict=True))
    reverts = []
    for part in range(0, len(markets), 6):
        funcs = [
            (
                v["pair"],
                s,
                t,
                calldata(s + "(address,address,uint256)", ["address", "address", "uint256"], [*t, 1]),
            )
            for v in markets[part : part + 6]
            for t in (v["tokens"], v["tokens"][::-1])
            for s in ("quoteSell", "quoteBuy")
        ]
        (records,) = c.one(
            MC,
            "aggregate3((address,bool,bytes)[])",
            ["(address,bool,bytes)[]"],
            [[(proxy, True, bytes.fromhex(data[2:])) for _, _, _, data in funcs]],
            ["(bool,bytes)[]"],
        )
        for (a, s, t, data), (ok, raw) in zip(funcs, records, strict=True):
            reason = (
                decode(["string"], raw[4:])[0]
                if not ok and raw[:4] == keccak(text="Error(string)")[:4]
                else None
            )
            if not next(v["atomic_enabled"] for v in markets if v["pair"] == a) and (ok or reason != "TR5A"):
                raise ValueError("expected the explicit disabled-pair rejection")
            reverts.append(
                dict(
                    pair=a, method=s, tokens=t, data=data, success=ok, result="0x" + raw.hex(), reason=reason
                )
            )

    # Calls are made by Multicall3, a public caller distinct from the factory and Delay.
    direct_data = bytes.fromhex(
        calldata(
            "swap(uint256,uint256,address,bytes)", ["uint256", "uint256", "address", "bytes"], [1, 0, MC, b""]
        )[2:]
    )
    (direct,) = c.one(
        MC,
        "aggregate3((address,bool,bytes)[])",
        ["(address,bool,bytes)[]"],
        [[(p, True, direct_data) for p in pairs]],
        ["(bool,bytes)[]"],
    )
    for market, (ok, raw) in zip(markets, direct, strict=True):
        if ok or raw[:4] != keccak(text="Error(string)")[:4] or decode(["string"], raw[4:])[0] != "TP0C":
            raise ValueError("direct pair trading role changed")
        market["direct_check"] = dict(
            caller=MC, data="0x" + direct_data.hex(), success=ok, result="0x" + raw.hex()
        )

    def logs(addr, begin, end, topics):
        return rpc("eth_getLogs", [dict(address=addr, fromBlock=hex(begin), toBlock=hex(end), topics=topics)])

    # Scan a short retirement window separately; the direct getters above cover every pair on H.
    changes = logs(proxy, 25805000, 25805400, [topic("PairEnabledSet(address,bool)")])
    for l in changes:
        if l.get("removed") or l["blockHash"] != head(int(l["blockNumber"], 16))["hash"]:
            raise ValueError("stale enablement event")
    old = logs(proxy, 25805000, 25805253, [[topic(SELL), topic(BUY)]])
    if not old:
        raise ValueError("missing historical fill")
    old = max(old, key=lambda l: (int(l["blockNumber"], 16), int(l["logIndex"], 16)))
    n = int(old["blockNumber"], 16)
    st = head(n)
    if st["hash"] != old["blockHash"]:
        raise ValueError("orphaned historical fill")
    tx = rpc("eth_getTransactionReceipt", [old["transactionHash"]])
    data = decode(
        ["address", "address", "uint256", "uint256", "uint256", "bool", "uint256", "address"],
        bytes.fromhex(old["data"][2:]),
    )
    if tx["blockHash"] != st["hash"] or tx["status"] != "0x1" or data[-1] != delay:
        raise ValueError("unconfirmed swap")
    event_order = int(old["topics"][3], 16)
    recipient = "0x" + old["topics"][2][-40:]
    cash_in, cash_out = data[:2]
    units_out = data[3] if old["topics"][0] == topic(SELL) else data[4]
    sent = [
        l
        for l in tx["logs"]
        if l["address"] == cash_out
        and len(l["topics"]) == 3
        and l["topics"][0] == topic("Transfer(address,address,uint256)")
        and "0x" + l["topics"][1][-40:] == proxy
        and "0x" + l["topics"][2][-40:] == recipient
        and int(l["data"], 16) == units_out
    ]
    if len(sent) != 1:
        raise ValueError("unlinked immediate ERC20 credit")

    def outcomes(begin, end, oid=None):
        topics = [topic("OrderExecuted(uint256,bool,bytes,uint256,uint256)")]
        topics += [oid] if oid else []
        return logs(delay, begin, end, topics)

    completed = outcomes(n, n + 300, old["topics"][3])
    assert len(completed) == 1
    later = completed[0]
    lh = head(int(later["blockNumber"], 16))
    if later["blockHash"] != lh["hash"] or later["removed"] or int(later["topics"][2], 16) != 1:
        raise ValueError("unconfirmed downstream settlement")
    report = dict(
        chain_id=1,
        height=snap["block"],
        selector=tag,
        state_root=ROOT_HASH,
        manifest=MANIFEST,
        roles=codes,
        markets=markets,
        inventory=inventory,
        proxy_native_balance=str(int(rpc("eth_getBalance", [proxy, tag]), 16)),
        atomic_checks=reverts,
        closure_events=changes,
        delay_seconds=c.one(delay, "delay()", returns=["uint256"])[0],
        newest=c.one(delay, "newestOrderId()", returns=["uint256"])[0],
        processed=c.one(delay, "lastProcessedOrderId()", returns=["uint256"])[0],
        historical=dict(
            order=event_order,
            swap=old,
            payment=sent[0],
            paid_raw=str(data[2]),
            received_raw=str(units_out),
            input=cash_in,
            output=cash_out,
            settlement=later,
            lag_seconds=int(lh["timestamp"], 16) - int(st["timestamp"], 16),
            parent_hash=st["parentHash"],
            block=n,
            parent_state_root=head(n - 1)["stateRoot"],
        ),
    )
    if head(snap["block"])["hash"] != tag["blockHash"]:
        raise ValueError("orphaned head")
    return report


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output", type=Path)
    ap.add_argument("--endpoint", default="https://mainnet.gateway.tenderly.co")
    cfg = ap.parse_args()
    result = ctrl(cfg.endpoint)
    cfg.output.write_bytes(gzip.compress(json.dumps(result, separators=(",", ":")).encode(), mtime=0))
