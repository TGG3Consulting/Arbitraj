"""Record direct Metaquoter and ArbExecutor V5 exchanges on one published block per network.

Run from record_metadex_slipstream.py. Only public eth_simulateV1, eth_getCode and block reads.
Swap state comes from the canonical parent. Only the isolated owner and executor are overridden.
"""
import hashlib

from record_base_slipstream_executor import RUNTIME, rpc, make_calls, call, calldata, OWNER, EXEC


def run_swaps(e, pools, weth, usdc, router, header, step):
    runtime = bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))
    url, tag = e["endpoint"], e["selector"]
    for address in (OWNER, EXEC):
        if rpc(url, "eth_getCode", [address, tag]) != "0x":
            raise ValueError("simulation address occupied")
    word = lambda n: "0x" + n.to_bytes(32, "big").hex()
    overrides = {OWNER: {"balance": hex(10**18)}, EXEC: {"code": "0x" + runtime.hex(),
        "stateDiff": {word(0): word(int(OWNER, 16)), word(1): word(int(weth, 16))}}}
    timestamp = int(header["timestamp"], 16)
    cases = []

    def simulate(items, seconds):
        req = {"blockStateCalls": [dict(stateOverrides=overrides, blockOverrides={"time": hex(timestamp + seconds)},
                             calls=[c for _, c in items])]}
        block = rpc(url, "eth_simulateV1", [req, tag])[0]
        if int(block["number"], 16) != int(header["number"], 16) + 1 or int(block["timestamp"], 16) != timestamp + seconds:
            raise ValueError("simulated number/time mismatch")
        return {name: result for (name, _), result in zip(items, block["calls"], strict=True)}

    def number(row):
        return int(row["returnData"][2:66], 16)

    for p in pools:
        if int(p["liquidity"]) == 0:
            continue
        usdc = p["stable"]
        for forward in (True, False):
            for i, amount in enumerate(([10**13, 10**15] if forward else [10**4, 10**6])):
                token = weth if forward else usdc
                seconds = step * (1 + i)
                def scenario(minimum):
                    calls = make_calls(p, weth, usdc, router, amount, forward, timestamp + 120, minimum)
                    q = calldata("quoteExactInputSingleV3(address,address,uint256,uint160)",
                        ["address", "address", "uint256", "uint160"], [p["pool"], token, amount, 0])
                    return [(name, call(p["quoter"], q) if name == "quote" else req) for name,req in calls]
                initial = simulate(scenario(0), seconds)
                if any(r["status"] != "0x1" for r in initial.values()):
                    raise ValueError([(k, v.get("error")) for k, v in initial.items() if v["status"] != "0x1"])
                minimum = number(initial["quote"])
                calls = scenario(minimum)
                results = simulate(calls, seconds)
                if any(r["status"] != "0x1" for r in results.values()):
                    raise ValueError("exact minimum failed")
                incoming = number(results["owner_out_after"]) - number(results["owner_out_before"])
                outgoing = number(results["owner_in_before"]) - number(results["owner_in_after"])
                if not minimum or minimum != incoming or amount != outgoing or number(results["execute"]) != incoming:
                    raise ValueError(("wallet delta", minimum, incoming, amount, outgoing))
                if number(results["executor_input"]) or number(results["executor_output"]):
                    raise ValueError("residue on executor")
                failed = simulate(scenario(minimum + 1), seconds)
                if failed["execute"]["status"] != "0x0" or number(failed["owner_in_before"]) != number(failed["owner_in_after"]) or number(failed["owner_out_before"]) != number(failed["owner_out_after"]):
                    raise ValueError("excessive minimum did not revert atomically")
                cases.append(dict(pool=p["pool"], forward=forward, amount=str(amount), min_out=str(minimum),
                    ahead_seconds=seconds, calldata=dict(calls), results=results, rejected=failed["execute"],
                    rejected_calldata=dict(scenario(minimum+1))["execute"]["data"]))
                print(e["chain_id"],p["spacing"],forward,amount,incoming,flush=True)
    return dict(owner=OWNER, executor=EXEC, runtime_sha256=hashlib.sha256(runtime).hexdigest(),
        funding_router=router, cases=cases)
