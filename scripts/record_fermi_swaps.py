"""Read-only eth_simulateV1 on the canonical parent of a published Ethereum Fermi slot."""
from record_base_slipstream_executor import calldata, call, OWNER
from record_fermi_ethereum import ACTIVE, ENGINE, WETH, USDT, UNIV2, ARGS, QUOTE, SWAP, rpc, block, selector, digest


def simulation(frame):
    target = frame["target"]
    parent = block(target - 1)
    tag = selector(parent)
    timestamp = 1606824023 + 12 * frame["slot"]
    if int(parent["timestamp"], 16) >= timestamp or rpc("eth_getCode", [OWNER, tag]) != "0x":
        raise ValueError("invalid parent or occupied simulated wallet")
    engine = rpc("eth_call", [{"to": ACTIVE, "data": calldata("fermi()")}, tag])
    if "0x" + engine[-40:] != ENGINE:
        raise ValueError("unexpected oracle")
    balance = calldata("balanceOf(address)", ["address"], [OWNER])
    approve = lambda spender, qty: calldata("approve(address,uint256)", ["address","uint256"], [spender, qty])

    def simulate(items, with_frame, seconds, following=False):
        overrides = {OWNER: {"balance": hex(10**18)}}
        if with_frame:
            overrides[ENGINE] = {"stateDiff": frame["stateDiff"]}
        body = {"blockStateCalls": [{"blockOverrides": {"time": hex(seconds)},
                                     "stateOverrides": overrides, "calls": [item for _, item in items]}]}
        if following:
            first = body["blockStateCalls"][0]
            first["calls"] = []
            first["blockOverrides"]["time"] = hex(timestamp)
            body["blockStateCalls"].append({"blockOverrides": {"time": hex(seconds)},
                                            "calls": [item for _, item in items]})
        b = rpc("eth_simulateV1", [body, tag])[-1]
        if int(b["number"], 16) != target + int(following) or int(b["timestamp"], 16) != seconds:
            raise ValueError("wrong simulated block")
        return dict(zip((name for name, _ in items), b["calls"], strict=True))

    cases = []
    for source, dest, qty in [(WETH, USDT, 10**15), (WETH, USDT, 10**16), (USDT, WETH, 10**6), (USDT, WETH, 10**7)]:
        args = [source, dest, qty]
        quote = ("quote", call(ACTIVE, calldata(QUOTE, ARGS, args)))
        funded = [("wrap", call(WETH, calldata("deposit()"), 10**16))]
        if source == USDT:
            funded.extend([("fund_approve", call(WETH, approve(UNIV2, 10**16))),
                           ("fund_input", call(UNIV2, calldata("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
                            ["uint256","uint256","address[]","address","uint256"], [10**16, qty, [WETH,USDT], OWNER, timestamp])))])
        funded.extend([("approve", call(source, approve(ACTIVE, qty))), ("before_in", call(source, balance)),
                       ("before_out", call(dest, balance)), quote])
        q = simulate([quote], True, timestamp)["quote"]
        if q["status"] != "0x1":
            raise ValueError(("target-slot quote", q.get("error")))
        quoted_in = int(q["returnData"][2:66], 16)
        minimum = int(q["returnData"][66:], 16)
        if quoted_in != qty or not minimum:
            raise ValueError("bad quote amounts")

        def swap(min_out):
            payload = calldata(SWAP, ARGS + ["uint256","address"], args + [min_out, OWNER])
            items = funded + [("swap", call(ACTIVE, payload)), ("after_in", call(source, balance)),
                              ("after_out", call(dest, balance))]
            results = simulate(items, True, timestamp)
            return payload, results

        data, ok = swap(minimum)
        if any(row["status"] != "0x1" for row in ok.values()):
            raise ValueError(("expected fill", [(k, v.get("error")) for k,v in ok.items() if v["status"] != "0x1"]))
        num = lambda row, key: int(row[key]["returnData"], 16)
        if num(ok, "after_out")-num(ok, "before_out") != minimum or num(ok, "before_in")-num(ok, "after_in") != qty:
            raise ValueError("wallet deltas do not match")
        if not any(x["address"] == ACTIVE and x["topics"][0] == digest(b"Swapped(address,address,address,uint256,uint256,address)")
                   for x in ok["swap"]["logs"]):
            raise ValueError("missing exchange event")
        _, failed = swap(minimum + 1)
        if failed["swap"]["status"] != "0x0" or any(num(failed, "before_"+t) != num(failed, "after_"+t) for t in ["in","out"]):
            raise ValueError("minOut+1 did not roll back")
        conditions = {}
        for k, use_frame, time in [("without_update", False, timestamp), ("next_slot", True, timestamp+12)]:
            conditions[k] = simulate([quote], use_frame, time, following=k == "next_slot")["quote"]
        cases.append(dict(token_in=source, token_out=dest, amount=str(qty), minimum=str(minimum), swap_data=data,
                          results=ok, min_plus_one=failed["swap"], conditions=conditions))
    return dict(parent=parent, timestamp=timestamp, wallet_funding="WETH.deposit; USDT via independent Uniswap V2",
                cases=cases)
