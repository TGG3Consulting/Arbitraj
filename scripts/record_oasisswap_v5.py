"""Replay OasisSwap through V5 on the same pool states and wallet as its official router."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
import requests

from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, STEP, call, calldata, rpc, simulate_block
from record_fraxswap_swaps import bound_runtime
from record_oasisswap_base import FACTORY, FUND, ROUTER, WETH

ADAPTER = "0x" + keccak(text="isolated OasisSwap V5 Base adapter")[-20:].hex()


def tx(to, signature, types=(), args=(), value=0):
    return call(to, calldata(signature, types, args), value)


def balance(token, who):
    return tx(token, "balanceOf(address)", ["address"], [who])


def allowance(token, payer, spender):
    return tx(token, "allowance(address,address)", ["address", "address"], [payer, spender])


def word(value):
    return "0x" + format(value, "064x")


def scalar(reply):
    if reply["status"] != "0x1":
        raise ValueError(f"balance or fee read failed: {reply.get('error')}")
    return decode(["uint256"], bytes.fromhex(reply["returnData"][2:]))[0]


def replay(directory, artifact, endpoint):
    selector = directory["selector"]
    head = rpc(endpoint, "eth_getBlockByHash", [selector["blockHash"], False])
    if rpc(endpoint, "eth_chainId", []) != "0x2105" or head["stateRoot"] != directory["state_root"]:
        raise ValueError("archive network or state root differs")
    deadline = int(head["timestamp"], 16) + 120
    runtime = bound_runtime(artifact, FACTORY, endpoint, selector)
    v5 = RUNTIME.read_text().strip().removeprefix("0x")
    overrides = {
        OWNER: {"balance": hex(50 * 10 ** 18)},
        EXEC: {"code": "0x" + v5, "stateDiff": {word(0):word(int(OWNER, 16)), word(1):word(int(WETH, 16))}},
        ADAPTER: {"code":"0x" + runtime},
    }
    pools = {p["pool"]:p for p in directory["pools"]}

    def leg(prefix, pool, a, b, amount):
        approve = tx(a, "approve(address,uint256)", ["address","uint256"], [EXEC,amount])
        prefix = prefix + [approve]
        reads = [balance(a, OWNER), balance(b, OWNER), allowance(a, OWNER, EXEC),
                 balance(a, EXEC), balance(b, EXEC), balance(a, ADAPTER), balance(b, ADAPTER),
                 allowance(a, EXEC, ADAPTER)]
        quote = tx(ROUTER, "getAmountsOut(uint256,address[],bool,address)",
                   ["uint256","address[]","bool","address"], [amount,[a,b],True,OWNER])
        controls = [tx(pool, sig) for sig in ["fee()", "feeCache0()", "feeCache1()"]]
        controls.append(tx(FACTORY, "getRebate(address)", ["address"], [OWNER]))

        def attempt(floor):
            trade = tx(EXEC, "execute(address,uint256,address,uint256,uint256," + STEP + "[])",
                       ["address","uint256","address","uint256","uint256",STEP + "[]"],
                       [a,amount,b,floor,deadline,[(29,ADAPTER,a,b,10000,b"")]])
            calls = prefix + controls + [quote] + reads + [trade] + reads
            result = simulate_block(endpoint, selector, overrides, calls)
            i = len(prefix)
            if any(x["status"] != "0x1" for x in result[:i]):
                raise ValueError("funding, accepted previous leg or approval failed")
            fee, cache0, cache1 = [scalar(x) for x in result[i:i+3]]
            estimator = result[i+3]
            quote_result = result[i+4]
            start = i+5
            before = [scalar(x) for x in result[start:start+len(reads)]]
            fill = result[start+len(reads)]
            after = [scalar(x) for x in result[start+len(reads)+1:]]
            if fill["status"] == "0x0" and before != after:
                raise ValueError("wallet, allowance or module changed after revert")
            if any(after[3:]) or any(before[3:]):
                raise ValueError("unexpected executor or adapter balance/allowance")
            prediction = None
            if quote_result["status"] == "0x1":
                (quoted,) = decode(["uint256[]"],bytes.fromhex(quote_result["returnData"][2:]))
                if len(quoted) != 2 or quoted[0] != amount:
                    raise ValueError("native quote returned the wrong path")
                prediction = str(quoted[1])
            out = dict(floor=str(floor), status=fill["status"], error=fill.get("error"),
                       spent=str(before[0]-after[0]), received=str(after[1]-before[1]),
                       gas=int(fill["gasUsed"],16), approval_gas=int(result[i-1]["gasUsed"],16),
                       fee=fee, rebate=scalar(estimator) if estimator["status"]=="0x1" else estimator.get("error"),
                       fee_cache=[str(cache0), str(cache1)], native_quote=prediction)
            return out, trade

        first, _ = attempt(1)
        if first["status"] != "0x1":
            return {"minimum_one":first}, None
        output = int(first["received"])
        if output < 1 or first["spent"] != str(amount):
            raise ValueError("V5 did not transfer the requested input or deliver the measured output")
        accepted, order = attempt(output)
        rejected, _ = attempt(output + 1)
        if (accepted["status"], accepted["received"], accepted["spent"], rejected["status"]) != (
                "0x1", str(output), str(amount), "0x0"):
            raise ValueError("V5 wallet minimum boundary differs from the measured payout")
        return {"accepted":accepted,"strict":rejected}, prefix + [order]

    cases=[]
    for sample in directory["samples"]:
        pool = sample["pool"]
        asset = sample["settlement"]
        token, = [t for t in pools[pool]["tokens"] if t != asset]
        for size in sorted({int(r["size"]) for r in sample["cases"]}):
            if asset == WETH:
                fund = tx(WETH, "deposit()", value=size)
            else:
                fund = tx(FUND, "swapETHForExactTokens(uint256,address[],address,uint256)",
                          ["uint256","address[]","address","uint256"],[size,[WETH,asset],OWNER,deadline],10**18)
            buy, prefix = leg([fund],pool,asset,token,size)
            case = dict(pool=pool,asset=asset,token=token,size=str(size),buy=buy)
            if prefix:
                case["sell"], _ = leg(prefix,pool,token,asset,int(buy["accepted"]["received"]))
            cases.append(case)
    if not cases or not any("accepted" in row.get("sell",{}) for row in cases):
        raise ValueError(f"no complete V5 round trip: {str(cases)[:800]}")
    return dict(selector=selector,state_root=head["stateRoot"],height=directory["height"],
                payer=OWNER,module_runtime_hash="0x"+keccak(bytes.fromhex(runtime)).hex(),
                v5_runtime_hash="0x"+keccak(bytes.fromhex(v5)).hex(),factory=FACTORY,cases=cases)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ("directory","artifact","output"):
        parser.add_argument(name,type=Path)
    args=parser.parse_args()
    directory=json.loads(args.directory.read_bytes())
    artifact=json.loads(args.artifact.read_bytes())
    for endpoint in ("https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"):
        try:
            result=replay(directory,artifact,endpoint)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")
            break
        except (requests.RequestException, ValueError, RuntimeError, KeyError) as error:
            print(endpoint,str(error)[:900],flush=True)
    else:
        raise ValueError("unable to complete the canonical OasisSwap adapter replay")
