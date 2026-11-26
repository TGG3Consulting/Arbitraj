"""Проверяет собранный в GitHub Actions SolidlyV3Executor через eth_simulateV1 на двух закреплённых блоках."""
import sys
from pathlib import Path

from eth_utils import keccak

from base_archive_endpoint import available
from record_balancer_deployments import ROOT, read, write
from record_base_slipstream_executor import EXEC, calldata, rpc

FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"

ORDER = "execute(address,address,address,uint256,uint256,uint256)"
TYPES = ["address","address","address","uint256","uint256","uint256"]


def capture(compiled, net):
    source = read(FOLDER / f"solidly_v3_{net}.json")
    e = {k:source[k] for k in ("endpoint","selector")}
    if source["chain_id"] == 8453:
        e = available(e)
    runtime = compiled["runtime"]
    if not runtime or rpc(e["endpoint"],"eth_getCode",[EXEC,e["selector"]]) != "0x":
        raise ValueError("ошибка байткода или адрес симуляции занят")
    block = rpc(e["endpoint"],"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    deadline = int(block["timestamp"],16) + 300
    cases = []
    for market in source["markets"]:
        pool,tokens = market["pool"],market["tokens"]
        for fill in market["cases"]:
            if "results" not in fill:
                continue
            side,dx,out,full = fill["side"],int(fill["dx"]),int(fill["out"]),fill["dx"]==fill["taken"]
            sender,token_in,token_out = fill["sender"],tokens[side],tokens[1-side]
            balance = lambda holder:calldata("balanceOf(address)",["address"],[holder])
            approve = lambda n:calldata("approve(address,uint256)",["address","uint256"],[EXEC,n])
            allowance = calldata("allowance(address,address)",["address","address"],[EXEC,pool])
            order = lambda m,a,b,n,d:calldata(ORDER,TYPES,[m,a,b,dx,n,d])
            entries = [
                ("original_in",token_in,balance(sender)),("original_out",token_out,balance(sender)),
                ("reset",token_in,approve(0)),("approve",token_in,approve(dx)),
                ("unregistered",EXEC,order(tokens[0],token_in,token_out,out,deadline)),
                ("bad_tokens",EXEC,order(pool,token_in,source["factory"],out,deadline)),
                ("expired",EXEC,order(pool,token_in,token_out,out,0)),
                ("too_high",EXEC,order(pool,token_in,token_out,out+1,deadline)),
                ("execute",EXEC,order(pool,token_in,token_out,out,deadline)),
                ("in_after",token_in,balance(sender)),("out_after",token_out,balance(sender)),
                ("module_input",token_in,balance(EXEC)),("module_output",token_out,balance(EXEC)),
                ("pool_allowance",token_in,allowance),
            ]
            native_gas = int(fill["results"]["exact"]["gasUsed"],16)
            calls = [{"from":sender,"to":to,"data":data,"gas":hex(min(2**24,native_gas*2+300000)
                     if key in ("execute","too_high") else 150000)} for key,to,data in entries]
            override = {EXEC:{"code":"0x"+runtime}}
            stage, = rpc(e["endpoint"],"eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":override,"calls":calls}],
                                              "validation":False},e["selector"]])
            if stage["parentHash"] != e["selector"]["blockHash"]:
                raise ValueError("хеш блока изменился")
            results = {k:r for (k,_,_),r in zip(entries,stage["calls"],strict=True)}
            bad = {"unregistered","bad_tokens","expired","too_high"} | (set() if full else {"execute"})
            for k,r in results.items():
                if r["status"] != ("0x0" if k in bad else "0x1"):
                    raise ValueError(f"{pool} {side} {dx} {k}: {r.get('error')}")
            n = lambda key:int(results[key]["returnData"],16)
            spent = n("original_in")-n("in_after")
            credited = n("out_after")-n("original_out")
            if ((spent,credited) != ((dx,out) if full else (0,0)) or
                any(n(k) for k in ("module_input","module_output","pool_allowance"))):
                raise ValueError("изменились дополнительные балансы, полномочия или полная сумма")
            if full:
                if n("execute")!=out:
                    raise ValueError("возврат модуля расходится с выручкой")
            elif results["execute"]["error"]["data"] != "0x"+keccak(text="PartialFill()")[:4].hex():
                raise ValueError("не проверен отказ неполного входа")
            cases.append(dict(pool=pool,side=side,sender=sender,dx=str(dx),out=str(out),full=full,
                              spent=str(spent),credited=str(credited),calls=calls,results=results))
    return dict(chain_id=source["chain_id"],selector=e["selector"],endpoint=e["endpoint"],
                executor=EXEC,runtime_keccak="0x"+keccak(bytes.fromhex(runtime)).hex(),
                compiler=compiled["solc"],cases=cases)


if __name__=="__main__":
    build,net = Path(sys.argv[1]),sys.argv[2]
    result = capture(read(build),net)
    full = sum(c["full"] for c in result["cases"])
    partial = len(result["cases"]) - full
    expected = {"base":(12,0),"ethereum":(11,1)}
    if (full,partial)!=expected[net] or len({c["pool"] for c in result["cases"]})!=3:
        raise ValueError("неполная матрица приёмки модуля")
    write(FOLDER / f"solidly_v3_executor_{net}.json",result)
    # В проверяемом дереве сборки сохраняет только приёмку фактически исполненного runtime этой сети.
    if "--record-acceptance" in sys.argv[3:]:
        profiles = ROOT/"config/first_run_dex.json"
        rows = read(profiles)
        profile, = [r for r in rows if r["network"]==net and r["settings"].get("dex.kind")=="solidly_v3"]
        profile["verified_module"] = dict(runtime_keccak=result["runtime_keccak"],block_hash=result["selector"]["blockHash"],
                                          full=full,partial_rejected=partial)
        write(profiles,rows)
    print(net,full,partial,result["runtime_keccak"])
