"""Изолированный taker Aqua с собственным USDC: две суммы, два порядка оплаты, обе стороны."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from aqua_evidence import INPUTS, ORDER, ROUTER
from record_aktionariat_wallet import approve
from record_base_slipstream_executor import OWNER, ROOT, rpc, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx

BASE_CASH = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
WETH = "0x4200000000000000000000000000000000000006"
FUND = "0x2626664c2603336e57b271c5c0b26f421741e481"
FUND_ARGS = "(address,address,uint24,address,uint256,uint256,uint160)"


def traits(flags, minimum, deadline):
    stops = [32, 32] + [37] * 8
    offsets = sum(v << (16 * i) for i, v in enumerate(stops))
    return offsets.to_bytes(20, "big") + flags.to_bytes(2, "big") + minimum.to_bytes(32, "big") + deadline.to_bytes(5, "big")


def measure(url):
    book = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/aqua_base_public_order.json").read_bytes())
    at = book["parent"]
    hdr = rpc(url, "eth_getBlockByHash", [at["blockHash"], False])
    assert rpc(url, "eth_chainId", []) == "0x2105" and rpc(url, "eth_getCode", [OWNER, at]) == "0x"
    stock = book["source"]["tokenOut"]
    order = (book["maker"], int(book["traits"]), bytes.fromhex(book["data"][2:]))
    assert "0x" + keccak(encode([ORDER], [order])).hex() == book["order_hash"]
    end = int(hdr["timestamp"], 16) + 900
    env = {OWNER: {"balance": hex(10**20)}}
    seed = 10**16
    fund = [tx(WETH, "deposit()", value=seed), approve(WETH, FUND, seed),
            tx(FUND, "exactInputSingle(" + FUND_ARGS + ")", [FUND_ARGS], [(WETH, BASE_CASH, 500, OWNER, seed, 1800000, 0)])]
    rows = []
    for qty in [900000, 1800000]:
        for flags in (0x41, 0x61):
            prep = fund + [approve(BASE_CASH, ROUTER, qty)]
            for src, dst in [(BASE_CASH, stock), (stock, BASE_CASH)]:
                now_qty = qty if src == BASE_CASH else buy
                if src == stock:
                    prep += [approve(stock, ROUTER, now_qty)]
                def wire(name, limit, expiry=end):
                    return tx(ROUTER, name+"("+",".join(INPUTS)+")", INPUTS,
                              [order, src, dst, now_qty, traits(flags, limit, expiry)])
                probes = [balance(src, OWNER), balance(dst, OWNER)]
                def run(calls):
                    raw = simulate_block(url, at, env, prep + probes + calls + probes)
                    assert all(v["status"] == "0x1" for v in raw[:len(prep)+2]+raw[-2:])
                    return raw[len(prep):]
                row = dict(input=src, output=dst, qty=now_qty, flags=flags, seed=qty)
                rows.append(row)
                trial = run([wire("quote", 1), wire("swap", 1)])
                if trial[2]["status"] != "0x1" or trial[3]["status"] != "0x1":
                    row["refusal"] = trial[2:4]
                    assert [v["returnData"] for v in trial[:2]] == [v["returnData"] for v in trial[-2:]]
                    break
                paid = int(trial[0]["returnData"],16)-int(trial[-2]["returnData"],16)
                got = int(trial[-1]["returnData"],16)-int(trial[1]["returnData"],16)
                assert paid == now_qty and got > 0
                quoted, amt, h = decode(["uint256","uint256","bytes32"], bytes.fromhex(trial[2]["returnData"][2:]))
                assert quoted == paid and "0x"+h.hex() == book["order_hash"]
                controls = run([wire("swap", got+1), wire("swap", got, 1), wire("swap", got)])
                assert [c["status"] for c in controls[2:5]] == ["0x0","0x0","0x1"]
                logs = controls[4]["logs"]
                assert token_flow(logs,src,OWNER) == -paid and token_flow(logs,dst,OWNER) == got
                assert [v["returnData"] for v in controls[-2:]] == [v["returnData"] for v in trial[-2:]]
                row.update(paid=paid, net=got, quote=amt, gas=int(controls[4]["gasUsed"],16),
                           rejects=[v.get("error") for v in controls[2:4]], logs=logs)
                if src == BASE_CASH:
                    buy = got
                    prep += [wire("swap", got)]
    return dict(selector=at, order=book["order_hash"], wallet=OWNER, router=ROUTER, rows=rows)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(measure("https://mainnet.base.org"), separators=(",",":")), encoding="utf8")
