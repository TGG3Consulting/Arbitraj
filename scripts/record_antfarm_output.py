"""Compare replenishment from the same trade's proceeds with and without a Vault loan."""

import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT, calldata, rpc, simulate_block
from record_antfarm_wallets import ATF, ROUTER, MAX, PARAM, WETH, ENDPOINTS

VAULT = "0xba12222222228d8ba445958a75a0704d566bf2c8"
MODULE = "0x" + keccak(text="Antfarm output-funded standalone archive")[-20:].hex()
PAYER = "0xf977814e90da44bfa03b6295a0616a897441acec"
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
ORDER = "(address,address,uint16,uint16,bool,bool,uint256,uint256,uint256,uint256,uint256,uint256)"
EVENT = "0x" + keccak(text="Settled(address,address,address,uint256,uint256,uint256,uint256,uint256)").hex()
DATA = ROOT / "tests/data/first_run_dex_2026-11-26"


def basis(d, m, x, qty):
    i = m["tokens"].index(x)
    ri, ro = (int(m["reserves"][k]) for k in (i, 1 - i))
    if ri == 0 or ro == 0 or int(m["supply"]) <= 1000:
        return 0, 0
    out = qty * ro // (ri + qty)
    oracle = d["oracles"].get(m["oracle"])
    if not oracle or oracle["first_update"]:
        return out, 0
    amount = qty if oracle["asset"] == x else out
    return out, (amount * m["fee"] // 1000) * int(oracle["rate_q112"]) >> 112


def possible(d, m, x, qty):
    out, fee = basis(d, m, x, qty)
    y = next(t for t in m["tokens"] if t != x)
    candidates = []
    for row in d["pairs"]:
        if row["tokens"] != [ATF, y] or row["fee"] >= 1000 or int(row["supply"]) <= 1000 or fee < 1000:
            continue
        atf, cash = map(int, row["reserves"])
        raw = fee * 1000 // (1000 - row["fee"])
        if not cash or not 0 < raw < atf:
            continue
        cost = cash * raw // (atf - raw) + 1
        # A fee below the native ATF pair's own minimum reverts even if quoted by the router.
        if fee * row["fee"] // (1000 - row["fee"]) < 1000 or out <= cost:
            continue
        candidates.append(dict(tier=row["fee"], pool=row["pair"], cost=cost))
    return out, fee, sorted(candidates, key=lambda k: k["cost"])


def capture(url, d, runtime):
    tag = d["selector"]
    deadline = d["block_time"] + 300
    head = rpc(url, "eth_getBlockByHash", [tag["blockHash"], False])
    if head["stateRoot"] != d["state_root"] or rpc(url, "eth_chainId", []) != "0x1":
        raise ValueError("wrong archive")
    if any(rpc(url, "eth_getCode", [a, tag]) != "0x" for a in (MODULE, PAYER)):
        raise ValueError("fixture account changed")

    def tx(to, fn, types=(), args=()):
        return {"from": PAYER, "to": to, "data": calldata(fn, types, args), "gas": hex(1300000)}

    def bal(asset, who):
        return tx(asset, "balanceOf(address)", ["address"], [who])

    def grant(asset, spender, amount):
        return tx(asset, "approve(address,uint256)", ["address", "uint256"], [spender, amount])

    def read(r):
        if r["status"] != "0x1":
            raise ValueError("read reverted " + str(r.get("error"))[:150])
        return int(r["returnData"], 16)

    def run(calls):
        # Only the new module is injected. Balances, allowances, oracles, pools and Vault remain canonical.
        return simulate_block(url, tag, {MODULE: {"code": "0x" + runtime}}, calls)

    vaults = sorted({t for p in d["pairs"] for t in p["tokens"]})
    collect = tx(VAULT, "getProtocolFeesCollector()")
    collector = "0x" + run([collect])[0]["returnData"][-40:]
    fees = read(run([tx(collector, "getFlashLoanFeePercentage()")])[0])
    vault_bal = {t: read(r) for t, r in zip(vaults, run([bal(t, VAULT) for t in vaults]), strict=True)}
    rows = []
    for m in d["pairs"]:
        if m["pair_class"] != "external_fee":
            rows.append(dict(pair=m["pair"], tokens=m["tokens"], class_name="fee included in pool"))
            continue
        sides = []
        for i, x in enumerate(m["tokens"]):
            y = m["tokens"][1 - i]
            r = int(m["reserves"][i])
            proposals = []
            for qty in sorted({max(1, r // 10000), max(1, r // 1000)}):
                gross, fee, opts = possible(d, m, x, qty)
                proposals.append(
                    dict(
                        input=str(qty),
                        gross=str(gross),
                        atf=str(fee),
                        options=[
                            dict(
                                tier=o["tier"],
                                pool=o["pool"],
                                cost=str(o["cost"]),
                                vault_has_principal=vault_bal[y] >= o["cost"],
                            )
                            for o in opts
                        ],
                    )
                )
            sides.append(dict(input_token=x, output_token=y, amounts=proposals))
        rows.append(dict(pair=m["pair"], tokens=m["tokens"], class_name="separate fee", sides=sides))
    cases = []
    abi = "execute(" + ORDER + ")"
    # Two separate-token markets whose input has no registered ATF pair. Reserves and payer tokens are actual.
    for pair, qtys in [
        ("0x5a2c88ddc0dd28cf2c1f9b92e3adde599f3321fa", [10**16, 10**17]),
        ("0x196a843cb27f2ac6c7f9e0164886515d88f17372", [10**23, 10**24]),
    ]:
        m = next(p for p in d["pairs"] if p["pair"] == pair)
        x, y = m["tokens"]
        for qty in qtys:
            gross, fee, opts = possible(d, m, x, qty)
            # Avoid changing the primary oracle through an auxiliary 1% funding swap for this comparison.
            eligible = [o for o in opts if o["tier"] != 10 and o["cost"] <= vault_bal[y]]
            if not eligible:
                raise ValueError(("no funded route", pair, qty, gross, fee))
            funding = eligible[0]
            cost = funding["cost"]
            tier = funding["tier"]
            premium = (cost * fees + 10**18 - 1) // 10**18
            for loan in (False, True):
                seed = []
                if not loan:
                    # Acquire only the working amount on a different real ATF/WETH tier; separately charge its gas and WETH.
                    boot = next(p for p in d["pairs"] if p["tokens"] == [ATF, WETH] and p["fee"] == 500)
                    ra, rb = map(int, boot["reserves"])
                    raw = fee * 1000 // 500
                    start_cost = rb * raw // (ra - raw) + 1
                    seed = [
                        grant(WETH, ROUTER, 0),
                        grant(WETH, ROUTER, start_cost),
                        tx(
                            ROUTER,
                            "swapTokensForExactTokens(" + PARAM + ")",
                            [PARAM],
                            [(fee, start_cost, fee, [WETH, ATF], [500], PAYER, deadline)],
                        ),
                        grant(ATF, MODULE, fee),
                    ]
                pre = [*seed, grant(x, MODULE, 0), grant(x, MODULE, qty)]
                expected = gross - cost - (premium if loan else 0)

                def order(minimum, cap=fee, budget=cost, expiry=deadline):
                    return tx(
                        MODULE,
                        abi,
                        [ORDER],
                        [
                            (
                                x,
                                y,
                                m["fee"],
                                tier,
                                False,
                                loan,
                                qty,
                                minimum,
                                cap,
                                budget,
                                premium if loan else 0,
                                expiry,
                            )
                        ],
                    )

                balances = [bal(asset, who) for who in (PAYER, MODULE) for asset in (x, y, ATF)] + [
                    bal(y, VAULT)
                ]
                callback = tx(
                    MODULE,
                    "receiveFlashLoan(address[],uint256[],uint256[],bytes)",
                    ["address[]", "uint256[]", "uint256[]", "bytes"],
                    [[y], [cost], [0], b""],
                )
                controls = [
                    order(expected + 1),
                    order(expected, fee - 1),
                    order(expected, fee, cost - 1),
                    order(expected, expiry=0),
                    callback,
                ]
                exact = order(expected)
                # Keep each frame's cumulative gas below providers' simulated block limits.
                tested = []
                for start in (0, 3):
                    group = controls[start : start + 3]
                    calls = [*pre, *balances, *group, *balances]
                    rs = run(calls)
                    n = len(pre)
                    k = len(balances)
                    offset = n + k
                    if any(v["status"] != "0x1" for v in rs[:offset]) or [
                        r["status"] for r in rs[offset : offset + len(group)]
                    ] != ["0x0"] * len(group):
                        raise ValueError(
                            (
                                "unexpected guard",
                                pair,
                                qty,
                                loan,
                                start,
                                [(r["status"], r.get("error")) for r in rs[offset : offset + len(group)]],
                            )
                        )
                    if list(map(read, rs[n:offset])) != list(map(read, rs[-k:])):
                        raise ValueError("partial rollback")
                    tested.extend(
                        dict(call=call, result=r)
                        for call, r in zip(group, rs[offset : offset + len(group)], strict=True)
                    )
                calls = [*pre, *balances, exact, *balances]
                rs = run(calls)
                n = len(pre)
                k = len(balances)
                result = rs[n + k]
                if result["status"] != "0x1":
                    raise ValueError(
                        ("payment", pair, qty, loan, result.get("error"), result.get("returnData"))
                    )
                before = list(map(read, rs[n : n + k]))
                after = list(map(read, rs[-k:]))
                delta = [b - a for a, b in zip(before, after)]
                if delta != [-qty, expected, 0, 0, 0, 0, premium if loan else 0]:
                    raise ValueError(("wallet delta", delta))
                logs = [
                    l for l in result["logs"] if l["address"].lower() == MODULE and l["topics"][0] == EVENT
                ]
                if len(logs) != 1 or [v[-40:].lower() for v in logs[0]["topics"][1:]] != [
                    a[2:] for a in (PAYER, x, y)
                ]:
                    raise ValueError("unbound payment")
                if decode(["uint256"] * 5, bytes.fromhex(logs[0]["data"][2:])) != (
                    qty,
                    expected,
                    0,
                    cost,
                    premium if loan else 0,
                ):
                    raise ValueError("settled event does not match balances")
                cases.append(
                    dict(
                        pair=pair,
                        loan=loan,
                        qty=str(qty),
                        gross=str(gross),
                        credit=str(expected),
                        atf=str(fee),
                        cost=str(cost),
                        tier=tier,
                        capital_raw="0" if loan else str(fee),
                        capital_weth_cost="0" if loan else str(start_cost),
                        seed_gas=sum(int(r["gasUsed"], 16) for r in rs[: len(seed)]),
                        gas=int(result["gasUsed"], 16),
                        calls=calls,
                        results=rs,
                        controls=tested,
                    )
                )
    return dict(
        height=d["height"],
        selector=tag,
        state_root=d["state_root"],
        payer=PAYER,
        module=MODULE,
        runtime_hash="0x" + keccak(hexstr=runtime).hex(),
        vault=VAULT,
        vault_balances={k: str(v) for k, v in vault_bal.items()},
        fee_percentage=str(fees),
        coverage=rows,
        cases=cases,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("output", type=Path)
    a = parser.parse_args()
    d = json.loads(gzip.decompress((DATA / "antfarm_ethereum_registry.json.gz").read_bytes()))
    runtime = json.loads(a.artifact.read_bytes())["runtime"]
    errors = []
    for ep in ENDPOINTS:
        try:
            report = capture(ep, d, runtime)
            break
        except Exception as err:
            errors.append((ep, str(err)[:500]))
    else:
        raise RuntimeError(errors)
    a.output.write_text(json.dumps(report, separators=(",", ":")), encoding="utf8")
