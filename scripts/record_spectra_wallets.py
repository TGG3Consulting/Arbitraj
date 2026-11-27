"""Измеряет чистые выплаты IBT и PT через сам Curve и через V5 с настоящим финансированием базовым активом."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import encode
from record_base_launch_wallets import PAYER, word
from record_base_slipstream_executor import EXEC, RUNTIME, STEP, calldata, rpc
from record_omni_v2_base import ENDPOINTS
from record_woo_base import WETH


def wallet(endpoint, snapshot, market, budget, mode, funding):
    asset, ibt, pt = (market[k] for k in ("underlying", "coin0", "coin1"))
    method, owner = funding
    pool = market["pool"]
    curve_index = "int128" if market["kind"] == "sng" else "uint256"
    at = snapshot["selector"]
    expiry = snapshot["timestamp"] + 180
    empty, runtime = "0x", "0x" + RUNTIME.read_text().strip().removeprefix("0x")
    if rpc(endpoint, "eth_getCode", [owner, at]) != empty or rpc(endpoint, "eth_getCode", [EXEC, at]) != empty:
        return {"stage": "плательщик не EOA или занят адрес архивного V5"}

    def tx(to, sig, ts, args):
        return {"from": owner, "to": to, "data": calldata(sig, ts, args), "gas": "0xc00000"}

    def simulate(calls):
        frame = {"calls": calls}
        if mode == "v5":
            frame["stateOverrides"] = {EXEC: {"code": runtime, "stateDiff": {word(0): word(int(owner,16)), word(1): word(int(WETH,16))}}}
        b, = rpc(endpoint, "eth_simulateV1", [{"blockStateCalls": [frame], "validation": False}, at])
        if b["parentHash"] != at["blockHash"] or len(b["calls"]) != len(calls):
            raise ValueError("подменён родитель или число ответов")
        return b["calls"]

    def approve(token, spender, amount):
        return tx(token, "approve(address,uint256)", ["address","uint256"], [spender,amount])

    def balance(token, address=owner):
        return tx(token, "balanceOf(address)", ["address"], [address])

    def amount(result):
        return int(result["returnData"],16)

    def good(result):
        return result["status"] == "0x1"

    if method=="owned_ibt":
        init=[balance(ibt)]
    else:
        init = [balance(asset), balance(ibt), approve(asset,ibt,budget),
                tx(ibt,"deposit(uint256,address)",["uint256","address"],[budget,owner]),balance(ibt),balance(asset)]
    result = simulate(init)
    record = dict(owner=owner, mode=mode, underlying=asset, budget=str(budget), funding={"method":method,"calls":init,"results":result})
    if not all(map(good,result)):
        return dict(record,stage="funding")
    if method=="owned_ibt":
        if amount(result[0])<budget:return dict(record,stage="funding")
        qty,prefix=budget,[]
    else:
        if amount(result[0])-amount(result[-1])!=budget:return dict(record,stage="funding")
        qty = amount(result[-2]) - amount(result[1])
        if qty <= 0:return dict(record,stage="zero shares")
        prefix = [init[2], init[3]]
    record["legs"] = []
    for i, (token, out) in enumerate(((ibt,pt),(pt,ibt))):
        spender = {"v5": EXEC, "router": snapshot["router"], "pool": pool}[mode]
        base = prefix + [approve(token, spender, qty), balance(token), balance(out)]
        args = [curve_index,curve_index,"uint256"]
        quote = tx(pool, f"get_dy({','.join(args)})",args,[i,1-i,qty])
        initial = simulate(base + [quote])
        if not all(map(good,initial)) or not amount(initial[-1]):
            record["legs"].append(dict(stage="quote",calls=base+[quote],results=initial))
            break
        gross = amount(initial[-1])
        def trade(floor, deadline):
            if mode == "pool":
                full = args + ["uint256"]
                return tx(pool,f"exchange({','.join(full)})",full,[i,1-i,qty,floor])
            if mode == "router":
                actions = bytes([0, 30 if curve_index=="int128" else 3, 15])
                body = [encode(["address","uint256"], [token,qty]),
                        encode(["address","uint256","uint256","uint256","uint256","address"], [pool,i,1-i,qty,floor,owner]),
                        encode(["address","address","uint256"], [out,owner,amount(initial[-2])+floor])]
                return tx(spender,"execute(bytes,bytes[],uint256)",["bytes","bytes[]","uint256"],[actions,body,deadline])
            kind = 4 if curve_index=="int128" else 9
            step = (kind,pool,token,out,10000,encode([curve_index,curve_index],[i,1-i]))
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                      ["address","uint256","address","uint256","uint256",STEP+"[]"],[token,qty,out,floor,deadline,[step]])
        trial = [trade(gross+1,expiry)]
        if mode != "pool":
            trial.append(trade(gross,0))
        real = trade(gross,expiry)
        calls = base + trial + [real,balance(token),balance(out),balance(token,EXEC),balance(out,EXEC)]
        results = simulate(calls)
        debit = amount(initial[-3])-amount(results[-4])
        net = amount(results[-3])-amount(initial[-2])
        if any(map(good,results[len(base):len(base)+len(trial)])):
            raise ValueError("строгий минимум или срок пропустил обмен")
        accepted = good(results[-5]) and debit==qty and net==gross and (mode!="v5" or not any(map(amount,results[-2:])))
        record["legs"].append(dict(stage="accepted" if accepted else "payout",i=i,paid=str(debit),net=str(net),
                                   qty=str(qty),gross=str(gross),gas=int(results[-5]["gasUsed"],16),calls=calls,results=results))
        if not accepted:
            break
        prefix += [base[-3],real]
        qty = net
    return dict(record,stage="completed")


def replay(endpoint):
    directory = Path("tests/data/first_run_dex_2026-11-26/spectra_base_directory.json.gz")
    snapshot = json.loads(gzip.decompress(directory.read_bytes()))
    head = rpc(endpoint,"eth_getBlockByHash",[snapshot["selector"]["blockHash"],False])
    if head["stateRoot"] != snapshot["state_root"]:
        raise ValueError("другой корень состояния")
    holders=json.loads((directory.parent/"spectra_base_funding.json").read_bytes())
    if holders["selector"]!=snapshot["selector"]:raise ValueError("держатели с другого блока")
    funds={**{r["token"]:r["owner"] for r in holders["holders"] if "owner" in r},**PAYER}
    rows = []
    for r in snapshot["markets"]:
        pool = dict(pool=r["pool"],template=r["template"],kind=r["kind"],maturity=r["maturity"],wallets=[])
        rows.append(pool)
        for method,token,decimals in [("owned_ibt",r["coin0"],r["decimals0"]),("deposit",r["underlying"],r["underlying_decimals"])]:
            if token not in funds:continue
            for denominator in (10000,1000):
                for mode in ("pool","v5","router"):
                    w=wallet(endpoint,snapshot,r,max(10**decimals//denominator,1),mode,(method,funds[token]))
                    pool["wallets"].append(w)
            if all(w.get("legs") and len(w["legs"])==2 and all(l["stage"]=="accepted" for l in w["legs"])
                   for w in pool["wallets"][-6:]):
                break
    return dict(selector=snapshot["selector"],state_root=snapshot["state_root"],endpoint=endpoint,markets=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    import requests
    for endpoint in ENDPOINTS:
        try:
            result=replay(endpoint)
        except (requests.RequestException,ValueError) as err:
            if endpoint==ENDPOINTS[-1]:
                raise
            print(str(err)[:160],flush=True)
        else:
            args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
            break
