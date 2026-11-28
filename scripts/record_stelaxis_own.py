"""Проверяет исходный вход Stelaxis за счёт отдельного кошелька в каноническом ценовом слоте."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_prop_price_slot import frame
from record_prop_venue import Capture, ENDPOINTS, OWNER, USDC, USDT, WETH, rpc, tx

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
UNI = "0x68b3465833fb72a70ecdf485e0e4c7bd8665fc45"
WBTC = "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599"
EVENT = "0x" + keccak(text="Swapped(address,address,address,uint256,uint256,address)").hex()
SWAP = "swapWithExtraFees(address,address,uint256,uint256,uint256,uint256)"
ZERO = "0x" + "00" * 20

def read_amount(row):
    return int(row["returnData"], 16) if row["status"] == "0x1" else None

def capture(endpoint, snapshot):
    receipt = snapshot["receipt"]
    venue = snapshot["venue"]
    height = receipt["height"]
    block_hash = receipt["event"]["blockHash"]
    c = Capture({"endpoint": endpoint, "selector": {"blockHash": block_hash, "requireCanonical": True}})
    source = (receipt["transaction"], snapshot["registry"], "0xe50de8ea")
    parent, block, update, origin = frame(c, rpc, height, block_hash, source)
    c.selector = parent
    if rpc(c, "eth_getCode", [OWNER, parent])["result"] != "0x":
        raise ValueError("Исходный адрес занят")
    deadline = int(block["time"], 16)
    cash = {}
    def funded(inp):
        if inp in cash:
            return cash[inp]
        calls = [tx(WETH, "deposit()", value=10**18)]
        if inp != WETH:
            amount = 2 * 10**17
            calls += [tx(WETH, "approve(address,uint256)", ["address", "uint256"], [UNI,amount]),
                      tx(UNI, "exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))",
                         ["(address,address,uint24,address,uint256,uint256,uint160)"],
                         [(WETH, inp, 3000 if inp == WBTC else 500, OWNER, amount, 1, 0)])]
        cash[inp] = calls
        return calls

    def attempt(inp, out, n, minimum, end, hot):
        approve = tx(inp, "approve(address,uint256)", ["address","uint256"], [venue, n])
        bal = lambda token: tx(token, "balanceOf(address)", ["address"], [OWNER])
        view = tx(venue, "quote(address,address,uint256)", ["address","address","uint256"], [inp,out,n])
        order = tx(venue, SWAP, ["address","address"] + ["uint256"]*4, [inp,out,n,minimum,end,0])
        calls = (update if hot else []) + funded(inp) + [approve,bal(inp),bal(out),view,order,bal(inp),bal(out)]
        ctl = {"stateOverrides": {OWNER: {"balance": hex(3*10**18)}}, "blockOverrides": block, "calls": calls}
        result = rpc(c, "eth_simulateV1", [{"blockStateCalls":[ctl]},parent])["result"][0]
        if result["parentHash"] != parent["blockHash"]:
            raise ValueError("Другой родитель")
        rows = result["calls"]
        before = rows[-6:-4]
        quote, swap = rows[-4:-2]
        after = rows[-2:]
        if any(r["status"] != "0x1" for r in rows[:-6]+before+after):
            raise ValueError("Не подтверждены собственное финансирование и балансы")
        activated = swap["status"] == "0x1"
        debit = read_amount(before[0])-read_amount(after[0])
        credit = read_amount(after[1])-read_amount(before[1])
        if activated:
            logs = [l for l in swap["logs"] if l["address"] == venue and l["topics"][:1] == [EVENT]]
            assert len(logs)==1 and debit==n and credit>=minimum
            event=logs[0]
            assert ["0x"+t[-40:] for t in event["topics"][1:]] == [OWNER,inp,out]
            x,y,to=decode(["uint256","uint256","address"], bytes.fromhex(event["data"][2:]))
            assert (x,y,to)==(debit,credit,OWNER)
        else:
            assert debit==credit==0
        return {"quote":quote.get("returnData"), "quoteError":quote.get("error"), "calldata":order["data"], "minimum":str(minimum),
                "deadline":end, "status":"paid" if activated else "reverted", "gas":int(swap["gasUsed"],16),
                "before":[str(read_amount(r)) for r in before],"after":[str(read_amount(r)) for r in after],
                "debit":str(debit),"credit":str(credit), "error":swap.get("error"),"logs":swap["logs"]}

    rows=[]
    for a,b in snapshot["directory"]["pairs"]:
        for inp,out in [(a,b),(b,a)]:
            units = {WETH:10**16, WBTC:10**4, USDT:10**6, USDC:10**6}
            for n in [units[inp], 5*units[inp]]:
                ok=attempt(inp,out,n,1,deadline+30,True)
                record={"input":inp,"output":out,"amount":str(n),"withUpdate":ok}
                if ok["status"] == "paid":
                    record["withoutUpdate"]=attempt(inp,out,n,1,deadline+30,False)
                    record["tight"]=attempt(inp,out,n,int(ok["credit"])+1,deadline+30,True)
                    record["expired"]=attempt(inp,out,n,1,deadline-1,True)
                    assert record["tight"]["status"]==record["expired"]["status"]=="reverted"
                rows.append(record)
    return {"chainId":1,"venue":venue,"owner":OWNER,"parent":parent,"height":height,"origin":origin,
            "selector":"0x"+keccak(text=SWAP)[:4].hex(),"rows":rows}

if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args=parser.parse_args()
    snapshot=json.loads((DATA/"stelaxis_ethereum.json").read_bytes())
    for endpoint in ENDPOINTS:
        try:
            d=capture(endpoint,snapshot)
            args.output.write_text(json.dumps(d,separators=(",",":"))+"\n",encoding="utf8")
            break
        except (AssertionError,ValueError,KeyError,RuntimeError) as e:
            print(type(e).__name__,str(e)[:160],flush=True)
    else:
        raise RuntimeError("Не удалось завершить контроль на архивном узле")
