"""Сопоставляет выплаты штатного V5 и официального FraxswapRouter в изолированном eth_simulateV1."""
import argparse
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
import requests

from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, STEP, call, calldata, rpc
from record_v2_chain_pools import minimum, swap

ROUTER = "0xc14d550632db8592d1243edc8b95b0ad06703867"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
# Адреса публичных держателей Blockscout. Наличие токенов и перевод проверяются в самом снимке.
HOLDERS = {
    "0x579cea1889991f68acc35ff5c3dd0621ff29b0c9": "0x75f561f56900f1e9a1978dff65e6bdc01202ee4a",
    "0xcacd6fd266af91b8aed52accc382b4e165586e29": "0x84202545b4f7517f288e19eee6404c66f04446e2",
    "0x853d955acef822db058eb8505911ed77f175b99e": "0x5e583b6a1686f7bc09a6bba66e852a7c80d36f00",
}


ADAPTER = "0x" + keccak(text="Arbitraj isolated Fraxswap V5 adapter 2026-11-26")[-20:].hex()


def bound_runtime(artifact, factory, endpoint, selector):
    """Сверяет подстановку solc со всеми байтами постановочного исполнения конструктора."""
    groups = list(artifact["immutableReferences"].values())
    if len(groups) != 1 or not groups[0]:
        raise ValueError("expected precisely one immutable: factory")
    runtime = bytearray.fromhex(artifact["runtime"])
    word = encode(["address"],[factory])
    for ref in groups[0]:
        pos = ref["start"]
        if ref["length"] != 32 or len(runtime[pos:pos+32]) != 32 or any(runtime[pos:pos+32]):
            raise ValueError("invalid factory immutable slot")
        runtime[pos:pos+32] = word
    req = {"jsonrpc":"2.0","id":1,"method":"eth_call","params":[
        {"data":"0x"+artifact["creation"]+word.hex()},selector]}
    response = requests.post(endpoint,json=req,timeout=25)
    response.raise_for_status()
    if response.json().get("result","").lower() != "0x"+runtime.hex():
        raise ValueError("constructor eth_call did not return the factory-bound runtime")
    return runtime.hex()


def run(m, timestamp, capture, url, index, amount, artifact=None):
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
    if artifact:
        overrides[ADAPTER] = {"code":"0x"+artifact["bound_runtime"]}
    if tin == WETH:
        fund = call(WETH,calldata("deposit()"),amount)
        source = "deposit"
    else:
        source = HOLDERS[tin]
        fund = call(tin,calldata("transfer(address,uint256)", ["address","uint256"],[OWNER,amount]))
        fund["from"] = source
    def balance(token): return call(token,calldata("balanceOf(address)",["address"],[OWNER]))
    def execute(path, floor):
        if path == "adapter":
            spender = EXEC
            args = [tin,amount,tout,floor,timestamp+120,[(29,ADAPTER,tin,tout,10000,b"")]]
            trade = call(EXEC,calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                         ["address","uint256","address","uint256","uint256",STEP+"[]"],args))
        elif path == "v5":
            trade = swap(pool,tin,tout,amount,floor,timestamp+120,pips)
            spender = EXEC
        else:
            spender = ROUTER
            name = "swapExactTokensForTokensSupportingFeeOnTransferTokens" if path=="recipient" else "swapExactTokensForTokens"
            trade = call(ROUTER,calldata(name+"(uint256,uint256,address[],address,uint256)",
                         ["uint256","uint256","address[]","address","uint256"],
                         [amount,floor,[tin,tout],OWNER,timestamp+120]))
        approve = call(tin,calldata("approve(address,uint256)",["address","uint256"],[spender,amount]))
        calls = [fund,approve,balance(tin),balance(tout),trade,balance(tin),balance(tout)]
        if path == "adapter":
            for account in (ADAPTER,EXEC):
                for token in (tin,tout):
                    calls.append(call(token,calldata("balanceOf(address)",["address"],[account])))
            calls.append(call(tin,calldata("allowance(address,address)",["address","address"],[EXEC,ADAPTER])))
            calls.append(call(ADAPTER,calldata("factory()")))
        block = {"blockOverrides":{"time":hex(timestamp)},"stateOverrides":overrides,"calls":calls}
        output, = rpc(url,"eth_simulateV1",[{"blockStateCalls":[block]},capture["selector"]])
        if int(output["timestamp"],16) != timestamp or output["parentHash"] != capture["selector"]["blockHash"]:
            raise ValueError("simulated timestamp or parent differs")
        actual = output["calls"]
        if len(actual) != len(calls) or any(int(actual[i]["status"],16)!=1 for i in [0,1,2,3,5,6]):
            raise ValueError("funding, allowance or balance read failed")
        value = lambda i: int(actual[i]["returnData"],16)
        if path == "adapter" and (any(int(v["status"],16)!=1 for v in actual[7:])
                or any(value(i)!=0 for i in range(7,12)) or value(12)!=int(capture["factory"],16)):
            raise ValueError("unexpected adapter factory, residue or allowance")
        return dict(spent=str(value(2)-value(5)),received=str(value(6)-value(3)),status=actual[4]["status"],
                    approval_gas=actual[1]["gasUsed"],gas=actual[4]["gasUsed"],error=actual[4].get("error"),floor=str(floor))
    native = execute("router",expected)
    strict = execute("router",expected+1)
    if (native["spent"],native["received"],strict["status"],strict["spent"],strict["received"]) != (str(amount),str(expected),"0x0","0","0"):
        raise ValueError(f"router payout or strict floor differed: {native} {strict}")
    recipient=execute("recipient",expected)
    recipient_strict=execute("recipient",expected+1)
    if (recipient["status"],recipient["spent"],recipient["received"],recipient_strict["status"],
        recipient_strict["spent"],recipient_strict["received"]) != ("0x1",str(amount),str(expected),"0x0","0","0"):
        raise ValueError(f"recipient-checked native route differs: {recipient} {recipient_strict}")
    measured = {}
    if artifact:
        passed = execute("adapter",expected)
        rejected = execute("adapter",expected+1)
        if (passed["status"],passed["spent"],passed["received"],rejected["status"],
            rejected["spent"],rejected["received"]) != ("0x1",str(amount),str(expected),"0x0","0","0"):
            raise ValueError(f"V5 adapter differs from the official router: {passed} {rejected}")
        measured = dict(adapter=passed,adapter_strict=rejected,adapter_dust=0,v5_dust=0,v5_to_adapter_allowance=0)
    return dict(**measured,native_recipient=recipient,native_recipient_strict=recipient_strict,pool=pool,token_in=tin,token_out=tout,source=source,amount=str(amount),projection=projection,
                predicted=str(expected),native=native,strict=strict,v5=execute("v5",expected))


if __name__ == "__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("directory",type=Path)
    ap.add_argument("output",type=Path)
    ap.add_argument("--adapter",type=Path,help="Артефакт FraxswapAdapter из GitHub Actions")
    ap.add_argument("--endpoint",default="https://ethereum.publicnode.com")
    args=ap.parse_args()
    data=json.loads(args.directory.read_bytes())
    artifact = json.loads(args.adapter.read_bytes()) if args.adapter else None
    if artifact:
        artifact["bound_runtime"] = bound_runtime(artifact,data["factory"],args.endpoint,data["selector"])
    timestamp=data["timestamp"]+12
    cases=[]
    for m in data["markets"]:
        if "projection" not in m: continue
        for i,token in enumerate(m["tokens"]):
            if token not in HOLDERS and token!=WETH:continue
            available=int(m["projection"][str(timestamp)]["decoded"][i])
            for amount in [min(10**15,available//100000), min(10**16,available//1000)]:
                if amount: cases.append(run(m,timestamp,data,args.endpoint,i,amount,artifact))
    args.output.write_text(json.dumps(dict(selector=data["selector"],timestamp=timestamp,endpoint=args.endpoint,
        module_hash="0x"+keccak(bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))).hex(),
        router=ROUTER,adapter_runtime=("0x"+keccak(bytes.fromhex(artifact["bound_runtime"])).hex()) if artifact else None,cases=cases),indent=2)+chr(10),encoding="utf-8")
