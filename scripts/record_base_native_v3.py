"""Read the native Base periphery from Sourcify and replay quotes, payments and limits at one canonical block."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata, rpc
from record_solidly_v3 import HOLDERS

WETH = "0x4200000000000000000000000000000000000006"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
MARKETS = {
    "9mm-v3-base": (
        "0x7b72c4002ea7c276dd717b96b20f4956c5c904e7", "0x4647ee5955f9d58f76cd86756bfb2eee543fd2b8",
        "0xa07d063b595168e081b51280ada5fc8e11cde52b", "0xf26600e17728f41adfb73d986e3deaf6df29f1c4", WETH),
    "throne-v3-base": (
        "0xe8839bf8175812691c6578c0fc80e721bc3e00fb", "0x56fcc093d4fa30e1eea14d9ea963b9f90b29de04",
        "0x48e285bc3690415c3b6220c3f833ed9c303a4f46", "0xf2e99648bf3b788690b6091b7ae19b507220c80b", WETH),
    "omni-exchange-v3-base": (
        "0xd6ab0566e7e60b67c50ac73ddff4e3ddcb829ec2", "0x1f4c9e2ed56a76d6297e3217bb5c2b87e5937fc0",
        "0x34c97b800876d9ca4658561f00512fa7abdaf315", "0x134d3c90a4a4c712cf0b548fe02003c9d934baba", USDC),
}
ORDER = "(address,address,uint24,address,uint256,uint256,uint256,uint160)"
QUOTE = "(address,address,uint256,uint24,uint160)"


def verified(address, chain_code):
    url = f"https://sourcify.dev/server/v2/contract/8453/{address}"
    reply = requests.get(url, params={"fields": "abi,deployment,compilation,runtimeBytecode.onchainBytecode"}, timeout=25)
    reply.raise_for_status()
    bundle = reply.json()
    if bundle["runtimeMatch"] not in ("match", "exact_match") or bundle["runtimeBytecode"]["onchainBytecode"].lower() != chain_code.lower():
        raise ValueError(f"source no longer matches canonical runtime of {address}")
    methods = [f["name"] for f in bundle["abi"] if f.get("type") == "function"]
    return dict(address=address, deployment=bundle["deployment"], source=url, compiler=bundle["compilation"]["compilerVersion"],
                name=bundle["compilation"]["fullyQualifiedName"], runtime="0x"+keccak(bytes.fromhex(chain_code[2:])).hex(),
                callbacks=[n for n in methods if n.endswith("SwapCallback")])


def collect(snapshot, endpoint):
    selector = snapshot["selector"]
    header = rpc(endpoint, "eth_getBlockByHash", [selector["blockHash"], False])
    if int(rpc(endpoint, "eth_chainId", []), 16) != 8453 or int(header["number"], 16) != snapshot["block"]:
        raise ValueError("wrong Base snapshot")
    def view(target, signature, rtype, args=(), vals=()):
        data = calldata(signature, args, vals)
        raw = rpc(endpoint, "eth_call", [{"to": target, "data": data}, selector])
        return decode([rtype], bytes.fromhex(raw[2:]))[0]
    def simulate(calls):
        stage, = rpc(endpoint, "eth_simulateV1", [{"blockStateCalls": [{"calls": calls}], "validation": False}, selector])
        if stage["parentHash"] != selector["blockHash"] or len(stage["calls"]) != len(calls):
            raise ValueError("mismatched simulation frame")
        return stage["calls"]

    markets = {}
    for name, (factory, pool, router, quoter, cash) in MARKETS.items():
        tokens = [view(pool, f"token{i}()", "address") for i in (0, 1)]
        fee = view(pool, "fee()", "uint24")
        if view(factory, "getPool(address,address,uint24)", "address", ["address","address","uint24"], [*tokens,fee]) != pool:
            raise ValueError("pool not registered")
        parts = {}
        for label, target in (("router", router), ("quoter", quoter)):
            parts[label] = verified(target, rpc(endpoint, "eth_getCode", [target, selector]))
            details = parts[label]
            details.update({key:view(target,key+"()","address") for key in ("factory", "deployer", "WETH9")})
            if details["factory"] != factory or details["WETH9"] != WETH:
                raise ValueError("periphery points to a different venue")
        if parts["router"]["callbacks"] != parts["quoter"]["callbacks"] or parts["router"]["deployer"] != parts["quoter"]["deployer"]:
            raise ValueError("router/quoter path mismatch")
        holder = HOLDERS["base"][cash]
        other = tokens[1-tokens.index(cash)]
        market = dict(factory=factory, pool=pool, tokens=tokens, fee=fee, owner=holder, **parts, cases=[])
        markets[name] = market
        deadline = int(header["timestamp"],16)+120
        quote_method = "quoteExactInputSingleV3" if name == "throne-v3-base" else "quoteExactInputSingle"
        def tx(to, data):
            return {"from":holder, "to":to, "gas":hex(2_000_000), "data":data}
        def balance(token):
            return tx(token,calldata("balanceOf(address)",["address"],[holder]))
        def approve(token, x):
            return tx(token,calldata("approve(address,uint256)",["address","uint256"],[router,x]))
        def price(a, b, x):
            return tx(quoter,calldata(quote_method+"("+QUOTE+")",[QUOTE],[(a,b,x,fee,0)]))
        def fill(a, b, x, minout, end):
            return tx(router,calldata("exactInputSingle("+ORDER+")",[ORDER],[(a,b,fee,holder,end,x,minout,0)]))
        def output(reply):
            if reply["status"] != "0x1":
                return None
            return int.from_bytes(bytes.fromhex(reply["returnData"][2:])[:32],"big")
        def leg(prefix,a,b,amount):
            prepared = prefix+[balance(a),balance(b),approve(a,0),approve(a,amount)]
            replies = simulate(prepared+[price(a,b,amount)])
            row = dict(input=a, output=b, requested=str(amount))
            q = output(replies[-1])
            if not q:
                return {**row, "quote_error":replies[-1].get("error", "zero output")}, [], 0
            order = fill(a,b,amount,q,deadline)
            controls = [fill(a,b,amount,q+1,deadline),fill(a,b,amount,q,0)]
            after = simulate(prepared+controls+[order,balance(a),balance(b)])
            actual = after[-3]
            row.update(quoted=str(q), minimum_plus_one=after[-5].get("error"), expired=after[-4].get("error"))
            if not row["minimum_plus_one"] or not row["expired"]:
                raise ValueError("limit passed on identical state")
            if actual["status"] != "0x1":
                return {**row,"execution_error":actual["error"]}, [], 0
            n=len(prefix)
            used=output(after[n])-output(after[-2])
            received=output(after[-1])-output(after[n+1])
            if used < 0 or used > amount or received < 0:
                raise ValueError("unexpected balances")
            row.update(spent=str(used),received=str(received),reported=str(output(actual)),gas=int(actual["gasUsed"],16))
            return row,prepared+[order],received
        for amount in ((10**12,10**14,10**16) if cash==WETH else (10_000,1_000_000,10_000_000)):
            buy,prefix,received=leg([],cash,other,amount)
            case={"buy":buy}
            if received:
                case["sell"],_,_=leg(prefix,other,cash,received)
            market["cases"].append(case)
    return dict(chain_id=8453,selector=selector,block=snapshot["block"],state_root=header["stateRoot"],markets=markets)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    snapshot=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    args.output.write_text(json.dumps(collect(snapshot,snapshot["endpoint"]),indent=2)+"\n",encoding="utf8",newline="\n")
