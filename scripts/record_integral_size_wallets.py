"""Replay historical Integral SIZE wallet fills and compare them with the current public queue."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from record_base_slipstream_executor import ROOT, calldata, rpc
from record_ethereum_x7 import ENDPOINTS
from record_integral_size_ethereum import topic, SELL, BUY
from record_solidly_v3 import HOLDERS

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
HISTORIC = (25705000, "0x6f967dfd05d1c8620b1f46a71abd412f23fd04fac402686a42bc31e69953ba2e",
            "0x231d7a18f1867b4ceecb35d3b98a402f1b31911821b04f1fc77a5cb7d45c8075")
ORDER = "(address,address,uint256,uint256,bool,address,uint32)"
DELAY_ORDER = "(address,address,uint256,uint256,bool,address,uint256,uint32)"


def capture(endpoint, directory):
    proxy = directory["roles"]["relayerProxy"]["address"]
    delay = directory["roles"]["delay"]["address"]
    weth, usdt = [directory["historical"][k] for k in ("input", "output")]

    def tx(payer, to, signature, types=(), values=(), value=0):
        return dict(to=to, data=calldata(signature, types, values), gas=hex(700000), value=hex(value), **{"from": payer})

    def anchor(height, block_hash, root):
        head = rpc(endpoint, "eth_getBlockByHash", [block_hash, False])
        if head["stateRoot"] != root or int(head["number"], 16) != height:
            raise ValueError("wrong canonical state")
        return dict(blockHash=block_hash, requireCanonical=True), int(head["timestamp"], 16) + 12

    def run(tag, ts, calls):
        frame = dict(calls=calls, blockOverrides=dict(time=hex(ts)))
        block, = rpc(endpoint, "eth_simulateV1", [dict(blockStateCalls=[frame], validation=False), tag])
        if block["parentHash"] != tag["blockHash"] or len(block["calls"]) != len(calls):
            raise ValueError("wrong simulated block or response count")
        return block["calls"]

    def uint(result):
        if result["status"] != "0x1":
            raise ValueError("reverted read: " + str(result.get("error"))[:150])
        return int(result["returnData"], 16)

    def rejected(result, reason):
        if result["status"] != "0x0" or result.get("logs"):
            raise ValueError("an invalid order changed state")
        raw = bytes.fromhex(result["returnData"][2:])
        raw = raw or bytes.fromhex((result.get("error") or {}).get("data", "0x")[2:])
        if raw[:4].hex() != "08c379a0" or decode(["string"], raw[4:])[0] != reason:
            raise ValueError(f"expected {reason}, got {str(result)[:300]}")

    old_tag, old_ts = anchor(*HISTORIC)
    tag, ts = anchor(directory["height"], directory["selector"]["blockHash"], directory["state_root"])
    cases, disabled = [], []
    for x, y, amounts in [(weth, usdt, [3*10**18, 6*10**18]), (usdt, weth, [7000*10**6, 11000*10**6])]:
        payer = HOLDERS["ethereum"][x]
        if any(rpc(endpoint, "eth_getCode", [payer, frame]) != "0x" for frame in [tag, old_tag]):
            raise ValueError("payer is not the expected EOA")
        approval = lambda spender, n: tx(payer, x, "approve(address,uint256)", ["address", "uint256"], [spender, n])
        balance = lambda asset, who: tx(payer, asset, "balanceOf(address)", ["address"], [who])
        balances = [balance(x, payer), balance(y, payer), balance(x, delay), balance(y, proxy)]
        for amount in amounts:
            sell_quote = tx(payer, proxy, "quoteSell(address,address,uint256)", ["address","address","uint256"], [x,y,amount])
            sell_out = uint(run(old_tag, old_ts, [sell_quote])[0])
            buy_quote = tx(payer, proxy, "quoteBuy(address,address,uint256)", ["address","address","uint256"], [x,y,sell_out])
            buy_in = uint(run(old_tag, old_ts, [buy_quote])[0])
            for side, quoted, paid in [("sell", sell_quote, amount), ("buy", buy_quote, buy_in)]:
                def order(max_in, min_out, deadline):
                    return tx(payer, proxy, side+ "("+ORDER+")", [ORDER], [(x,y,max_in,min_out,False,payer,deadline)])
                valid = order(paid, sell_out, old_ts+300)
                strict = order(paid-(side=="buy"), sell_out+(side=="sell"), old_ts+300)
                before = [approval(proxy, 0), approval(proxy, paid), *balances, quoted]
                calls = [*before, strict, order(paid, sell_out, old_ts-1), valid, *balances]
                results = run(old_tag, old_ts, calls)
                i = len(before)
                rejected(results[i], "TR37" if side=="sell" else "TR08")
                rejected(results[i+1], "OS04")
                fill = results[i+2]
                oid = uint(fill)
                start = [uint(v) for v in results[2:6]]
                end = [uint(v) for v in results[-4:]]
                if uint(results[i-1]) != (sell_out if side=="sell" else paid) or [b-a for a,b in zip(start,end)] != [-paid,sell_out,paid,-sell_out]:
                    raise ValueError("nominal quote and physical balances disagree")
                events = [l for l in fill["logs"] if l["address"].lower()==proxy and l["topics"][0]==topic(SELL if side=="sell" else BUY)]
                if len(events)!=1 or int(events[0]["topics"][3],16)!=oid:
                    raise ValueError("order ID not tied to the payment")
                cases.append(dict(input=x,output=y,payer=payer,side=side,paid=str(paid),net=str(sell_out),
                                  gas=int(fill["gasUsed"],16),order=oid,calls=calls,results=results))
                if amount == amounts[0]:
                    current = [*balances, order(paid,sell_out,ts+300), *balances]
                    rs = run(tag, ts, current)
                    rejected(rs[4], "TR5A")
                    if [uint(v) for v in rs[:4]] != [uint(v) for v in rs[-4:]]:
                        raise ValueError("disabled fill moved funds")
                    disabled.append(dict(calls=current,results=rs))
    payer = HOLDERS["ethereum"][usdt]
    quantity = 1000*10**6
    gas_limit = 500000
    get_price = tx(payer, delay, "gasPrice()")
    gas_price = uint(run(tag, ts, [get_price])[0])
    order_nonce = tx(payer, delay, "newestOrderId()")
    balances = [tx(payer, asset, "balanceOf(address)", ["address"], [who])
                for asset, who in [(usdt,payer),(weth,payer),(usdt,delay)]]
    approve = lambda n: tx(payer, usdt, "approve(address,uint256)", ["address","uint256"], [delay,n])
    submit = tx(payer,delay,"sell("+DELAY_ORDER+")",[DELAY_ORDER],[(usdt,weth,quantity,1,False,payer,gas_limit,ts+300)],gas_price*gas_limit)
    calls = [approve(0),approve(quantity),get_price,order_nonce,*balances,submit,order_nonce,*balances]
    rs = run(tag, ts, calls)
    vals = list(map(uint,rs[2:]))
    price, initial, before_in,before_out,before_escrow, submitted,final,after_in,after_out,after_escrow = vals
    if (price,initial+1,submitted,before_in-after_in,after_out-before_out,after_escrow-before_escrow)!=(gas_price,final,final,quantity,0,quantity):
        raise ValueError("queue result does not match escrow and unchanged terminal balance")
    return dict(chain_id=1,historical=dict(height=HISTORIC[0],selector=old_tag,state_root=HISTORIC[2],cases=cases),
                current=dict(selector=tag,state_root=directory["state_root"],rejections=disabled,
                             queued=dict(order=final,paid=str(quantity),received="0",prepaid_wei=str(gas_price*gas_limit),calls=calls,results=rs)))


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    directory=json.loads(gzip.decompress((DATA/"integral_size_ethereum.json.gz").read_bytes()))
    failures=[]
    for endpoint in ENDPOINTS:
        try:
            report=capture(endpoint,directory)
            break
        except Exception as err:
            failures.append((endpoint,str(err)[:400]))
    else:
        raise RuntimeError(failures)
    args.output.write_text(json.dumps(report,separators=(",",":")),encoding="utf8")
