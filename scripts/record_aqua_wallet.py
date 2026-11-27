"""Изолированный taker Aqua с собственным USDC: две суммы, два порядка оплаты, обе стороны."""
import argparse
import json
import gzip
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


def measure_order(url, book, cash, stock, sizes, fund, compiled=None, mode="original", wrapped=WETH):
    at = book["parent"]
    hdr = rpc(url, "eth_getBlockByHash", [at["blockHash"], False])
    assert rpc(url, "eth_getCode", [OWNER, at]) == "0x"
    order = (book["maker"], int(book["traits"]), bytes.fromhex(book["data"][2:]))
    assert "0x" + keccak(encode([ORDER], [order])).hex() == book["order_hash"]
    end = int(hdr["timestamp"], 16) + 900
    env = {OWNER: {"balance": hex(10**20)}}
    guard = None
    endpoint = ROUTER
    if compiled:
        from aqua_guard_replay import Guard
        guard = Guard(url,at,compiled,wrapped,order,mode)
        endpoint = guard.endpoint
        fund = guard.prefix + fund
        env = guard.env
    rows = []
    for qty in sizes:
        for flags in (0x41, 0x61):
            prep = fund + [approve(cash, endpoint, qty)]
            for src, dst in [(cash, stock), (stock, cash)]:
                now_qty = qty if src == cash else buy
                if src == stock:
                    prep += [approve(stock, endpoint, now_qty)]
                def wire(name, limit, expiry=end):
                    if guard:
                        action=guard.wire if name=="swap" else guard.quote
                        return action(src,dst,now_qty,limit,expiry,flags)
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
                if trial[3]["status"] != "0x1":
                    row["refusal"] = trial[2:4]
                    assert [v["returnData"] for v in trial[:2]] == [v["returnData"] for v in trial[-2:]]
                    break
                paid = int(trial[0]["returnData"],16)-int(trial[-2]["returnData"],16)
                got = int(trial[-1]["returnData"],16)-int(trial[1]["returnData"],16)
                assert paid == now_qty and got > 0
                amt = None
                if trial[2]["status"] == "0x1":
                    if guard:
                        amt,=decode(["uint256"],bytes.fromhex(trial[2]["returnData"][2:]))
                    else:
                        quoted, amt, h = decode(["uint256","uint256","bytes32"], bytes.fromhex(trial[2]["returnData"][2:]))
                        assert quoted == paid and "0x"+h.hex() == book["order_hash"]
                elif not guard:
                    raise ValueError("original swap succeeded despite a rejected original quote")

                controls = run([wire("swap", got+1), wire("swap", got, 1), wire("swap", got)])
                assert [c["status"] for c in controls[2:5]] == ["0x0","0x0","0x1"]
                logs = controls[4]["logs"]
                if guard:
                    guard.evidence(logs,src,dst,paid,got,flags)
                    before = guard.marks(src,dst)
                    extra = [guard.wire(src,dst,now_qty,1,end,flags,unknown=True)] if mode=="v5" else []
                    check = simulate_block(url,at,env,prep+before+extra+[wire("swap",got)]+before)[len(prep):]
                    if extra and check[len(before)]["status"]!="0x0":
                        raise ValueError("unknown V5 order key accepted")
                    if not all(v["status"]=="0x1" for v in check[:len(before)]+check[-len(before)-1:]):
                        raise ValueError("protected payment or balance read failed")
                    if any(int(v["returnData"],16) for v in check[:len(before)]+check[-len(before):]):
                        raise ValueError("unexpected leftover or approval")

                assert token_flow(logs,src,OWNER) == -paid and token_flow(logs,dst,OWNER) == got
                assert [v["returnData"] for v in controls[-2:]] == [v["returnData"] for v in trial[-2:]]
                row.update(paid=paid, net=got, quote=amt, gas=int(controls[4]["gasUsed"],16),
                           rejects=[v.get("error") for v in controls[2:4]], logs=logs)
                if src == cash:
                    buy = got
                    prep += [wire("swap", got)]
    return dict(selector=at, order=book["order_hash"], wallet=OWNER, router=ROUTER, mode=mode, protected=guard.manifest if guard else None,rows=rows)


def measure(network, url, compiled=None):
    folder = ROOT / "tests/data/first_run_dex_2026-11-26"
    if network == "base":
        assert rpc(url,"eth_chainId",[]) == "0x2105"
        book=json.loads((folder/"aqua_base_public_order.json").read_bytes())
        seed=10**16
        fund=[tx(WETH,"deposit()",value=seed), approve(WETH,FUND,seed),
              tx(FUND,"exactInputSingle("+FUND_ARGS+")",[FUND_ARGS],[(WETH,BASE_CASH,500,OWNER,seed,1800000,0)])]
        params=(url,book,BASE_CASH,book["source"]["tokenOut"],[900000,1800000],fund)
        if compiled:
            return dict(selector=book["parent"],strategies=[measure_order(*params,compiled,mode,WETH) for mode in ("wallet","v5")])
        return measure_order(*params)

    from record_aktionariat_wallet import FUND as ROUTER_V3, SWAP, WETH as ETH_WETH
    from record_balancer_markets import Capture
    assert rpc(url,"eth_chainId",[]) == "0x1"
    day=json.loads(gzip.decompress((folder/"aqua_ethereum_day.json.gz").read_bytes()))
    end=day["timestamp"]+900
    cash_bridge = "0x111111111117dc0aa78b770fa6a738034120c302"
    uni_factory="0x1f98431c8ad98523631ae4a59f267346ea31f984"
    reader=Capture(dict(selector=day["selector"],endpoint=url))
    fees=[]
    for fee in [500,3000,10000]:
        pool,=reader.one(uni_factory,"getPool(address,address,uint24)",["address","address","uint24"],[ETH_WETH,cash_bridge,fee],["address"])
        if int(pool,16):
            liquid,=reader.one(pool,"liquidity()",returns=["uint128"])
            if liquid:fees.append((liquid,fee))
    paid=[]; checked=set()
    for row in day["directions"]:
        h=row["order"]; outcome=row.get("quote")
        if not isinstance(outcome,str) or h in checked:continue
        pair=(row["tokenIn"],row["tokenOut"])
        if ETH_WETH in pair: cash=ETH_WETH
        elif cash_bridge in pair: cash=cash_bridge
        else:continue
        checked.add(h)
        stock=next(t for t in pair if t!=cash)
        book=dict(parent=day["selector"],order_hash=h,**day["orders"][h])
        seed=10**16
        quantities=[10**14,2*10**14] if cash==ETH_WETH else [10**17,2*10**17]
        fund=[tx(ETH_WETH,"deposit()",value=seed)]
        if cash!=ETH_WETH:
            for _,fee in sorted(fees,reverse=True):
                trial=fund+[approve(ETH_WETH,ROUTER_V3,seed),tx(ROUTER_V3,"exactInputSingle("+SWAP+")",[SWAP],[(ETH_WETH,cash,fee,OWNER,end,seed,max(quantities),0)])]
                probe=simulate_block(url,day["selector"],{OWNER:{"balance":hex(10**20)}},trial)
                if all(v["status"]=="0x1" for v in probe):
                    fund=trial
                    break
            else:
                paid.append(dict(order=h,funding="no funded WETH bridge to "+cash));continue
        for mode in (("wallet","v5") if compiled else ("original",)):
            paid.append(measure_order(url,book,cash,stock,quantities,fund,compiled,mode,ETH_WETH))
    return dict(selector=day["selector"],strategies=paid)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("network", choices=("base","ethereum"))
    parser.add_argument("output", type=Path)
    parser.add_argument("--compiled", type=Path)
    args = parser.parse_args()
    endpoints = (["https://mainnet.base.org","https://base.gateway.tenderly.co"] if args.network=="base"
                 else ["https://rpc.mevblocker.io","https://eth.drpc.org"])
    import requests
    for endpoint in endpoints:
        try:
            output=measure(args.network,endpoint,json.loads(args.compiled.read_bytes()) if args.compiled else None)
            break
        except (requests.RequestException, ValueError, RuntimeError) as error:
            last=error
    else:
        raise RuntimeError("no complete archive response") from last
    args.output.write_text(json.dumps(output, separators=(",",":")), encoding="utf8")
