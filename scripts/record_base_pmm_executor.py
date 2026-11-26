"""Архивная проверка общего BasePmmExecutor отдельно для ElfomoFi и LunarBase с тем же собранным runtime."""
import sys
from pathlib import Path
from eth_abi import decode
from base_archive_endpoint import available

from record_elfomo_executor import FOLDER, EXEC, ORDER, TYPES, calldata, capture as elfomo_capture, read, write, signature, simulate
from record_lunarbase import MULTICALL, USDC, ZERO
from record_solidly_v3 import HOLDERS

WETH = "0x4200000000000000000000000000000000000006"


def measure(source, code, case, deadline):
    market = source["markets"][2]["address"]
    a = WETH if case["token_in"]==ZERO else USDC
    b = USDC if a==WETH else WETH
    owner, amount = HOLDERS["base"][a], int(case["amount"])
    bal = lambda addr: calldata("balanceOf(address)", ["address"], [addr])
    allow = lambda payer, recv: calldata("allowance(address,address)", ["address"]*2, [payer,recv])
    approve = lambda n: calldata("approve(address,uint256)", ["address","uint256"], [EXEC,n])
    native = calldata("getEthBalance(address)", ["address"], [EXEC])
    quote = calldata("quoteLunar(address,address,uint256)", ["address","address","uint256"], [a,b,amount])
    order = lambda pool,other,n,end: calldata(ORDER,TYPES,[pool,a,other,amount,n,end])
    prefix = [("wallet_in",a,bal(owner)),("wallet_out",b,bal(owner)),("native_before",MULTICALL,native),
              ("reset",a,approve(0)),("approve",a,approve(amount)),("quote",EXEC,quote)]
    _, before = simulate(source,code,owner,prefix)
    expected = int(before["quote"]["returnData"],16)
    if before["quote"]["status"]!="0x1" or not expected:
        raise ValueError("У модуля нет положительной котировки LunarBase")
    entries = prefix+[("wrong_market",EXEC,order(a,b,expected,deadline)),("wrong_token",EXEC,order(market,market,expected,deadline)),
                      ("expired",EXEC,order(market,b,expected,0)),("above",EXEC,order(market,b,expected+1,deadline)),
                      ("wallet_allow_after_reject",a,allow(owner,EXEC)),("module_allow_after_reject",a,allow(EXEC,market)),
                      ("execute",EXEC,order(market,b,expected,deadline)),("wallet_in_after",a,bal(owner)),
                      ("wallet_out_after",b,bal(owner)),("native_after",MULTICALL,native),("module_in",a,bal(EXEC)),
                      ("module_out",b,bal(EXEC)),("module_allowance",a,allow(EXEC,market)),("wallet_allowance",a,allow(owner,EXEC)),
                      ("repeat",EXEC,order(market,b,expected,deadline))]
    calls, results = simulate(source,code,owner,entries)
    failures = dict(wrong_market="InvalidOrder()",wrong_token="UnsupportedToken()",expired="InvalidOrder()",
                    above="InsufficientOutput()",repeat="UnsupportedToken()")
    for k,row in results.items():
        if k in failures:
            if row["status"]!="0x0" or row["error"].get("data")!=signature(failures[k])[:10]:
                raise ValueError(f"{a}/{amount} {k}: {row}")
        elif row["status"]!="0x1":
            raise ValueError(f"{a}/{amount} {k}: {row.get('error')}")
    n=lambda k:int(results[k]["returnData"],16)
    if (n("wallet_in")-n("wallet_in_after"), n("wallet_out_after")-n("wallet_out"), n("execute"),
        n("quote"), n("wallet_allow_after_reject"), n("native_after")-n("native_before"))!=(amount,expected,expected,expected,amount,0):
        raise ValueError("Не совпали реальные дельты кошелька/нативный остаток")
    if any(n(k) for k in ("module_in","module_out","module_allowance","wallet_allowance","module_allow_after_reject")):
        raise ValueError("Остались средства или разрешение")
    topic=signature("SwapExecuted(address,bool,uint256,uint256,uint256)")
    event,=[r for r in results["execute"]["logs"] if r["address"]==market and r["topics"][0]==topic]
    payee,side,amount_x,amount_y,_fee=decode(["address","bool","uint256","uint256","uint256"],bytes.fromhex(event["data"][2:]))
    spent,paid=(amount_x,amount_y) if side else (amount_y,amount_x)
    if (payee,side,spent,paid)!=(owner if a==WETH else EXEC,a==WETH,amount,expected):
        raise ValueError("Событие пула расходится с балансами, направлением или получателем")
    return dict(owner=owner,input=a,output=b,amount=str(amount),out=str(expected),gas=int(results["execute"]["gasUsed"],16),
                calls=calls,results=results)


if __name__=="__main__":
    compiled=read(Path(sys.argv[1]))
    elfomo=elfomo_capture(compiled)
    write(FOLDER/"base_pmm_executor_elfomo.json",elfomo)
    source=available(read(FOLDER/"lunarbase.json"))
    from record_base_slipstream_executor import rpc
    head=rpc(source["endpoint"],"eth_getBlockByHash",[source["selector"]["blockHash"],False])
    cases=[measure(source,compiled["runtime"],case,int(head["timestamp"],16)+90) for case in source["cases"]]
    if len(cases)!=4 or len({(c["input"],c["amount"]) for c in cases})!=4:
        raise ValueError("Неполная матрица LunarBase")
    write(FOLDER/"base_pmm_executor_lunar.json",dict(chain_id=8453,selector=source["selector"],endpoint=source["endpoint"],
          module=EXEC,runtime_keccak=elfomo["runtime_keccak"],compiler=compiled["solc"],cases=cases))
    from record_baibai_executor import capture as baibai_capture
    write(FOLDER/"base_pmm_executor_baibai.json",baibai_capture(compiled,int(head["timestamp"],16)+90))
