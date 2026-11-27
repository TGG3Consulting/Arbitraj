"""Replay the Rust allocator's exact order vectors and compare wallet results including gas."""
import argparse
import json
from pathlib import Path

import requests

from record_balancer_markets import Capture
from record_base_slipstream_executor import rpc
from record_carbon_ethereum import ACTION, BLOCK, CTRL, ENDPOINTS, NATIVE, ROOT_HASH, settle
from record_ethereum_v2_factories import WETH, FUNDING


def gas_in_currency(c, currency, gas_wei):
    if currency == NATIVE:
        return gas_wei
    # How many stablecoin units would the wallet have to spend to replace this ETH?
    amounts, = c.one(FUNDING, "getAmountsIn(uint256,address[])", ["uint256", "address[]"],
                     [gas_wei, [currency, WETH]], ["uint256[]"])
    return amounts[0]


def replay(url, data):
    selector = {"blockHash": BLOCK, "requireCanonical": True}
    c = Capture({"selector": selector, "endpoint": url})
    header = rpc(url, "eth_getBlockByHash", [BLOCK, False])
    if (rpc(url, "eth_chainId", []) != "0x1" or selector != data["selector"]
            or header["stateRoot"] != ROOT_HASH or data["state_root"] != ROOT_HASH):
        raise ValueError("unexpected archive state")
    end = int(header["timestamp"], 16) + 240
    base_fee = int(header["baseFeePerGas"], 16)
    rows = []
    for case in data["cases"]:
        source, target, mode = case["source"], case["target"], case["by_target"]
        currency = source if mode else target
        sig = ("calculateTradeSourceAmount" if mode else "calculateTradeTargetAmount") + "(address,address," + ACTION + ")"
        choices = []
        proposals = list(case["alternatives"])
        reference = case.get("reference_actions")
        if reference is None and "reference_id" in case:
            reference = [[case["reference_id"], case["amount"]]]
        if reference is not None:
            original = {"actions": reference,
                        "input": case["reference_input"], "output": case["reference_output"]}
            if not any(p["actions"] == original["actions"] for p in proposals):
                proposals.append(original)
        for p in proposals:
            actions = [(int(i), int(n)) for i, n in p["actions"]]
            spent, received = int(p["input"]), int(p["output"])
            other, = c.one(CTRL, sig, ["address", "address", ACTION], [source, target, actions], ["uint128"])
            if other != (spent if mode else received) or sum(n for _, n in actions) != int(case["amount"]):
                raise ValueError(("controller/planner mismatch", source, mode, actions))
            filled = settle(c, source, target, actions, spent, received, mode, end)
            gas = filled["gas"] + filled["approval_gas"]
            net = {}
            for tip in (0, 10**9):
                replacement = gas_in_currency(c, currency, gas * (base_fee + tip))
                # Maximise this signed value in both exact-input and exact-output modes.
                net[str(tip)] = str((-spent if mode else received) - replacement)
            choices.append(dict(plan=p, gas=filled["gas"], approval_gas=filled["approval_gas"],
                                net_raw=net, limit_revert=filled["strict"], expiry_revert=filled["expired"]))
        winners = {str(tip): max(range(len(choices)), key=lambda i: int(choices[i]["net_raw"][str(tip)]))
                   for tip in (0, 10**9)}
        rows.append(dict(source=source, target=target, amount=case["amount"], by_target=mode,
                         comparison_currency=currency, choices=choices, best_index=winners))
    return dict(selector=selector, state_root=ROOT_HASH, base_fee_per_gas=str(base_fee),
                priority_fee_scenarios_wei=["0", "1000000000"], cases=rows)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    data = json.loads(args.input.read_text(encoding="utf8"))
    errors = []
    for endpoint in ENDPOINTS:
        try:
            result = replay(endpoint, data)
        except (ValueError, RuntimeError, KeyError, requests.RequestException) as exc:
            errors.append((endpoint, str(exc)[:400]))
            continue
        args.output.write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf8")
        break
    else:
        raise RuntimeError(errors)
