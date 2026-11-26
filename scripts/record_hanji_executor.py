"""Read-only Base replay of an Actions-built Hanji executor. No signed transactions."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, ROOT, call, calldata, rpc
from record_v2_chain_pools import NETWORKS

ORDER = "(address,address,address,uint256,uint256,uint256,uint72,uint128,uint256,bytes32)"
FIXTURE = ROOT / "tests/data/first_run_dex_2026-11-26/hanji_base.json"


def capture(compiled):
    data = json.loads(FIXTURE.read_bytes())
    url, wrapped = NETWORKS["base"]
    tag = {"blockHash": data["hash"], "requireCanonical": True}
    header = rpc(url, "eth_getBlockByHash", [data["hash"], False])
    deadline = int(header["timestamp"], 16) + 300
    executor = "0x" + keccak(b"\xd6\x94" + bytes.fromhex(OWNER[2:]) + b"\x80")[-20:].hex()
    profiles = json.loads((ROOT / "config/first_run_dex.json").read_bytes())
    router = next(r["settings"]["dex.router"] for r in profiles if r["network"] == "base"
                  and r["settings"]["dex.kind"] == "uniswap_v2")
    deploy = {"from": OWNER, "data": "0x" + compiled["creation"] + encode(["address"], [wrapped]).hex(),
              "gas": hex(4_000_000)}
    def simulate(calls):
        stage = {"stateOverrides": {OWNER: {"balance": hex(10**18), "nonce": "0x0"}}, "calls": calls}
        return rpc(url, "eth_simulateV1", [{"blockStateCalls": [stage]}, tag])[0]["calls"]
    def approve(token, spender, amount):
        return call(token, calldata("approve(address,uint256)", ["address", "uint256"], [spender, amount]))
    def balance(token):
        return call(token, calldata("balanceOf(address)", ["address"], [OWNER]))
    def order(args, sender=OWNER):
        return {**call(executor, calldata("execute("+ORDER+")", [ORDER], [args]), gas=5_000_000), "from": sender}
    def success(row):
        if int(row["status"], 16) != 1:
            raise ValueError(row.get("error", "simulation failed"))
        return bytes.fromhex(row["returnData"][2:])
    recorded = []
    for market in data["markets"]:
        proxy = market["registry"]["orderbookAddress"]
        x = market["registry"]["baseToken"]["contractAddress"]
        y = market["registry"]["quoteToken"]["contractAddress"]
        for case in market["fills"]:
            budget, spent, bought = [case[k] for k in ("budget_y", "debited_y", "received_x")]
            funding = 10**16
            prefix = [deploy, call(wrapped, calldata("deposit()"), funding)]
            if y.lower() != wrapped:
                prefix.extend([approve(wrapped, router, funding), call(router, calldata(
                    "swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
                    ["uint256", "uint256", "address[]", "address", "uint256"],
                    [funding, budget, [wrapped, y], OWNER, deadline]))])
            prefix += [approve(y, executor, budget), balance(x), balance(y)]
            buy_id = keccak(text=f"{data['hash']}:{proxy}:{budget}:buy")
            sell_id = keccak(text=f"{data['hash']}:{proxy}:{budget}:sell")
            # Discovery calls exist only in this pinned, unsigned replay. Firm controls below
            # use observed amounts and an exact price, independently of the generic helper/EOA.
            buy = [proxy, y, x, budget, budget-1, 1, 2**72-1, 2**128-1, deadline, buy_id]
            read_fill = lambda row: decode(["uint256", "uint256", "uint128"], success(row))
            probe = simulate(prefix + [order(buy)])[-1]
            if int(probe["status"], 16) == 0 and probe.get("error",{}).get("data") in ("0x7c329980","0x6838b4aa"):
                recorded.append({"proxy":proxy, "budget_y":budget, "unavailable":"buy", "revert":probe["error"]["data"]})
                continue
            spent, bought, buy_fee = read_fill(probe)
            scale_x, scale_y = market["scale"]
            shares = bought // scale_x
            ask, remainder = divmod(spent // scale_y - buy_fee, shares)
            if remainder:
                raise ValueError("control budget crosses multiple prices, requires marginal price discovery")
            buy[4:8] = [budget-spent, bought, ask, buy_fee]
            entry = prefix + [order(buy), balance(x), balance(y), approve(x, executor, bought)]
            sell = [proxy, x, y, bought, 0, 1, 1, 2**128-1, deadline, sell_id]
            probe = simulate(entry + [order(sell)])[-1]
            if int(probe["status"],16) == 0 and probe.get("error",{}).get("data") in ("0x7c329980","0x6838b4aa"):
                recorded.append({"proxy":proxy,"budget_y":budget,"actual_debit_y":spent,"received_x":bought,
                                 "unavailable":"sell","revert":probe["error"]["data"]})
                continue
            sold, credited, sell_fee = read_fill(probe)
            if sold != bought: raise ValueError("sell did not consume the full position")
            bid, remainder = divmod(credited // scale_y + sell_fee, shares)
            if remainder: raise ValueError("exit spans multiple prices")
            sell[5:8] = [credited, bid, sell_fee]
            full = simulate(entry + [order(sell), balance(x), balance(y)])
            for row in full:
                success(row)
            val = lambda i: int.from_bytes(success(full[i]), "big")
            assert read_fill(full[len(prefix)]) == (spent, bought, buy_fee)
            assert read_fill(full[-3]) == (bought, credited, sell_fee)
            start_x, start_y = val(len(prefix)-2), val(len(prefix)-1)
            assert val(len(prefix)+1) == start_x + bought and val(len(prefix)+2) == start_y-spent
            assert val(-2) == start_x and val(-1) == start_y - spent + credited
            def completed(operation_id, payer=OWNER):
                return call(executor, calldata("completed(address,bytes32)", ["address", "bytes32"], [payer, operation_id]))
            # Prove a retry cannot debit twice even if its original signature was lost.
            repeated = simulate(entry + [completed(buy_id), order(buy), completed(buy_id), balance(x), balance(y)])
            if (int.from_bytes(success(repeated[-5]), "big") != 1 or
                    int(repeated[-4]["status"], 16) != 0 or
                    not repeated[-4].get("error",{}).get("data",repeated[-4]["returnData"])[2:].startswith(keccak(text="AlreadyCompleted(bytes32)")[:4].hex()) or
                    int.from_bytes(success(repeated[-3]), "big") != 1 or
                    int.from_bytes(success(repeated[-2]), "big") != start_x + bought or
                    int.from_bytes(success(repeated[-1]), "big") != start_y - spent):
                raise ValueError("same leg replay did not give the expected duplicate error and balances")
            stranger = "0x" + keccak(text="independent review sender")[-20:].hex()
            denied = simulate(prefix + [order(buy, stranger), completed(buy_id, stranger), completed(buy_id)])
            if (int(denied[-3]["status"], 16) != 0 or int.from_bytes(success(denied[-2]), "big")
                    or int.from_bytes(success(denied[-1]), "big")):
                raise ValueError("unfunded account consumed another payer's operation ID")
            controls = ["duplicate_operation", "unfunded_independent_sender"]
            altered_cases = [("buy_min_out_plus_one", prefix, buy, 5, bought+1),
                             ("sell_min_out_plus_one", entry, sell, 5, credited+1),
                             ("buy_commission_above_cap", prefix, buy, 7, buy_fee-1),
                             ("buy_expired", prefix, buy, 8, deadline-301)]
            if buy[4]:
                altered_cases.append(("unspent_above_cap", prefix, buy, 4, buy[4]-1))
            if case["bid_before"][0] > bid:
                altered_cases.append(("infeasible_previous_bid", entry, sell, 6, case["bid_before"][0]))
            for label, lead, args, idx, value in altered_cases:
                altered = list(args)
                altered[idx] = value
                prior = simulate(lead + [balance(x), balance(y), order(altered), balance(x), balance(y)])
                if int(prior[-3]["status"], 16) or success(prior[-5]) != success(prior[-2]) or success(prior[-4]) != success(prior[-1]):
                    raise ValueError(f"{label}: expected a revert and unchanged owner balances at {proxy}")
                retry = simulate(lead + [order(altered), completed(args[-1]), order(args), completed(args[-1])])
                if (int(retry[-4]["status"],16) != 0 or int.from_bytes(success(retry[-3]), "big") != 0
                        or int.from_bytes(success(retry[-1]), "big") != 1):
                    raise ValueError(f"{label}: retry after confirmed revert failed")
                success(retry[-2])
                controls.append(label)
            recorded.append({"proxy": proxy, "budget_y":budget, "actual_debit_y":spent, "received_x":bought,
                             "sold_x":bought, "actual_credit_y":credited, "eoa_debit_y":case["debited_y"], "eoa_credit_y":case["received_y"],
                             "eoa_received_x":case["received_x"], "contract_ask":ask, "contract_bid":bid,
                             "buy_fee_units":buy_fee, "sell_fee_units":sell_fee, "reverted":controls})
            print(proxy, budget, len(controls), flush=True)
    if len(recorded) != 14 or not any(r.get("reverted") for r in recorded):
        raise ValueError("missing market observations or no positive controls")
    return {"chain_id":8453, "block":data["block"], "hash":data["hash"], "owner":OWNER,
            "executor":executor, "runtime_keccak":"0x"+keccak(bytes.fromhex(compiled["runtime"])).hex(),
            "solc":compiled["solc"], "round_trips":recorded}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("build", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(capture(json.loads(args.build.read_bytes())), indent=2)+"\n", encoding="utf8")
