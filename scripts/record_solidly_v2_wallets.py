"""Compare actual V5 and Solidly Ethereum Router01 wallet transfers on both curves at one head."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from record_aero_basic_base import HOLDERS, RUNTIME, round_trip
from record_base_classic_wallets import capture
from record_solidly_v3 import ETH_WETH
from record_base_slipstream_executor import rpc
from record_ethereum_x7 import ENDPOINTS
from record_lista_ethereum import calldata


def direct(endpoint, directory, m, case, deadline):
    router, owner = directory["router"], case["owner"]
    a,b=m["token_in"],m["token_out"]
    amt,bought,sold=map(int,[case["amount"],case["bought"],case["sold"]])
    def call(target,signature,types,args):
        return dict(to=target, data=calldata(signature,types,args), **{"from":owner,"gas":"0x160000"})
    def balance(token):
        return call(token,"balanceOf(address)",["address"],[owner])
    def approve(token,n):
        return call(token,"approve(address,uint256)",["address","uint256"],[router,n])
    def swap(x,y,n,minimum):
        return call(router,"swapExactTokensForTokensSimple(uint256,uint256,address,address,bool,address,uint256)",
                    ["uint256","uint256","address","address","bool","address","uint256"],[n,minimum,x,y,m["stable"],owner,deadline])
    entries=[("start_a",balance(a)),("start_b",balance(b)),("approve_in",approve(a,amt)),
             ("above_buy",swap(a,b,amt,bought+1)),("buy",swap(a,b,amt,bought)),("after_buy_a",balance(a)),("after_buy_b",balance(b)),
             ("approve_out",approve(b,bought)),("above_sell",swap(b,a,bought,sold+1)),("sell",swap(b,a,bought,sold)),
             ("end_a",balance(a)),("end_b",balance(b))]
    calls=[v for _,v in entries]
    frame,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls}],"validation":False},directory["selector"]])
    if frame["parentHash"]!=directory["selector"]["blockHash"]:
        raise ValueError("router simulation on wrong parent")
    results=dict(zip((k for k,_ in entries),frame["calls"],strict=True))
    for k,v in results.items():
        if v["status"]!=("0x0" if k.startswith("above_") else "0x1"):
            raise ValueError((m["pool"],k,v.get("error")))
    val=lambda k:int(results[k]["returnData"],16)
    if (val("start_a")-val("after_buy_a"),val("after_buy_b")-val("start_b"),val("after_buy_b")-val("end_b"),val("end_a")-val("after_buy_a"))!=(amt,bought,bought,sold):
        raise ValueError((m["pool"],"router wallet debit/credit"))
    for key,expected in [("buy",(amt,bought)),("sell",(bought,sold))]:
        amounts,=decode(["uint256[]"],bytes.fromhex(results[key]["returnData"][2:]))
        if amounts!=expected: raise ValueError((key,"wrong nominal route"))
    return dict(calls=calls,results=results)


def record(endpoint,directory):
    result=capture(directory,endpoint)
    if {m["stable"] for m in result["markets"]}!={True,False}:
        raise ValueError(("missing executable curve",result["rejected"][:4]))
    block=rpc(endpoint,"eth_getBlockByHash",[directory["selector"]["blockHash"],False])
    deadline = int(block["timestamp"],16)+120
    runtime = RUNTIME.read_text().strip().removeprefix("0x")
    token_file=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26/ethereum_exact_transfers.json"
    audit=json.loads(token_file.read_bytes())
    if audit["selector"]!=directory["selector"] or audit["state_root"]!=directory["state_root"]:
        raise ValueError("token evidence on wrong state")
    accepted={row["address"] for row in audit["tokens"].values()}
    for market in directory["markets"]:
        if (set(market["tokens"])<=accepted and ETH_WETH in market["tokens"]
                and market["balances"]==market["reserves"] and all(map(int,market["reserves"]))):
            i=market["tokens"].index(ETH_WETH)
            owned=HOLDERS["ethereum"][ETH_WETH]
            other=market["tokens"][1-i]
            first_qty=min(10**15,int(market["reserves"][i])//100)
            case=round_trip(dict(endpoint=endpoint,selector=directory["selector"]),market["pool"],ETH_WETH,
                            other,first_qty,deadline,runtime,kind=3,owner=owned,wrapped=ETH_WETH)
            result["markets"].append(dict(pool=market["pool"],init=directory["init"],stable=market["stable"],
                token_in=ETH_WETH,token_out=other,cases=[case],exact_transfer=True))
    for m in result["markets"]:
        market = next(o for o in directory["markets"] if o["pool"]==m["pool"])
        i = market["tokens"].index(m["token_in"])
        scale, reserve = int(market["scales"][i]), int(market["reserves"][i])
        for qty in sorted({min(scale*10,reserve//100), min(scale*100,reserve//10)}):
            if qty and all(int(o["amount"]) != qty for o in m["cases"]):
                m["cases"].append(round_trip(dict(endpoint=endpoint,selector=directory["selector"]),m["pool"],
                    m["token_in"],m["token_out"],qty,deadline,runtime,kind=3,owner=m["cases"][0]["owner"],wrapped=ETH_WETH))
        for case in m["cases"]:
            case["router"]=direct(endpoint,directory,m,case,deadline)
    return result


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory",type=Path); parser.add_argument("output",type=Path); args=parser.parse_args()
    directory=json.loads(gzip.decompress(args.directory.read_bytes()))
    reasons=[]
    for endpoint in ENDPOINTS:
        try:
            result=record(endpoint,directory)
            break
        except (OSError,KeyError,RuntimeError,ValueError) as error: reasons.append((endpoint,str(error)[:650]))
    else: raise RuntimeError(reasons)
    args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
