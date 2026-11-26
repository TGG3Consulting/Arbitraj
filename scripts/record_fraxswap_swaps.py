"""Сопоставляет выплаты штатного V5 и официального FraxswapRouter в изолированном eth_simulateV1."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, call, calldata, rpc
from record_v2_chain_pools import minimum, swap

ROUTER = "0xc14d550632db8592d1243edc8b95b0ad06703867"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
# Адреса публичных держателей Blockscout. Наличие токенов и перевод проверяются в самом снимке.
HOLDERS = {
    "0x579cea1889991f68acc35ff5c3dd0621ff29b0c9": "0x75f561f56900f1e9a1978dff65e6bdc01202ee4a",
    "0xcacd6fd266af91b8aed52accc382b4e165586e29": "0x84202545b4f7517f288e19eee6404c66f04446e2",
    "0x853d955acef822db058eb8505911ed77f175b99e": "0x5e583b6a1686f7bc09a6bba66e852a7c80d36f00",
}


def run(m, timestamp, capture, url, index, amount):
    tin, tout = m["tokens"][index], m["tokens"][1-index]
    pool = m["pool"]
    pips = int(m["twamm_reserves"][5])*100
    projection = m["projection"][str(timestamp)]["decoded"]
    expected = minimum(amount, int(projection[index]), int(projection[1-index]), pips)
    if expected <= 0:
        raise ValueError("empty output")
    runtime = RUNTIME.read_text().strip().removeprefix("0x")
    def word(n): return "0x"+format(n,"064x")
    overrides = {OWNER:{"balance":hex(100*10**18)},EXEC:{"code":"0x"+runtime,
                 "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    if tin == WETH:
        fund = call(WETH,calldata("deposit()"),amount)
        source = "deposit"
    else:
        source = HOLDERS[tin]
        fund = call(tin,calldata("transfer(address,uint256)", ["address","uint256"],[OWNER,amount]))
        fund["from"] = source
    def balance(token): return call(token,calldata("balanceOf(address)",["address"],[OWNER]))
    def execute(path, floor):
        if path == "v5":
            trade = swap(pool,tin,tout,amount,floor,timestamp+120,pips)
            spender = EXEC
        else:
            spender = ROUTER
            trade = call(ROUTER,calldata("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
                         ["uint256","uint256","address[]","address","uint256"],
                         [amount,floor,[tin,tout],OWNER,timestamp+120]))
        approve = call(tin,calldata("approve(address,uint256)",["address","uint256"],[spender,amount]))
        calls = [fund,approve,balance(tin),balance(tout),trade,balance(tin),balance(tout)]
        block = {"blockOverrides":{"time":hex(timestamp)},"stateOverrides":overrides,"calls":calls}
        output, = rpc(url,"eth_simulateV1",[{"blockStateCalls":[block]},capture["selector"]])
        if int(output["timestamp"],16) != timestamp or output["parentHash"] != capture["selector"]["blockHash"]:
            raise ValueError("simulated timestamp or parent differs")
        actual = output["calls"]
        if len(actual) != len(calls) or any(int(actual[i]["status"],16)!=1 for i in [0,1,2,3,5,6]):
            raise ValueError("funding, allowance or balance read failed")
        value = lambda i: int(actual[i]["returnData"],16)
        return dict(spent=str(value(2)-value(5)),received=str(value(6)-value(3)),status=actual[4]["status"],
                    gas=actual[4]["gasUsed"],error=actual[4].get("error"),floor=str(floor))
    native = execute("router",expected)
    strict = execute("router",expected+1)
    if (native["spent"],native["received"],strict["status"],strict["spent"],strict["received"]) != (str(amount),str(expected),"0x0","0","0"):
        raise ValueError(f"router payout or strict floor differed: {native} {strict}")
    return dict(pool=pool,token_in=tin,token_out=tout,source=source,amount=str(amount),projection=projection,
                predicted=str(expected),native=native,strict=strict,v5=execute("v5",expected))


if __name__ == "__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("directory",type=Path)
    ap.add_argument("output",type=Path)
    ap.add_argument("--endpoint",default="https://ethereum.publicnode.com")
    args=ap.parse_args()
    data=json.loads(args.directory.read_bytes())
    timestamp=data["timestamp"]+12
    cases=[]
    for m in data["markets"]:
        if "projection" not in m: continue
        for i,token in enumerate(m["tokens"]):
            if token not in HOLDERS and token!=WETH:continue
            available=int(m["projection"][str(timestamp)]["decoded"][i])
            for amount in [min(10**15,available//100000), min(10**16,available//1000)]:
                if amount: cases.append(run(m,timestamp,data,args.endpoint,i,amount))
    args.output.write_text(json.dumps(dict(selector=data["selector"],timestamp=timestamp,endpoint=args.endpoint,
        module_hash="0x"+keccak(bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))).hex(),
        router=ROUTER,cases=cases),indent=2)+chr(10),encoding="utf-8")
