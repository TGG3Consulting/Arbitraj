"""Снимает WooPP и выполняет read-only симуляцию V5 на двух парах официального IntegrationHelper Base."""
import hashlib
import re

import requests
from eth_utils import keccak

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_curve_ng_deployments import code
from record_base_slipstream_executor import EXEC, RUNTIME, STEP, calldata

DOC = "https://learn.woo.org/woofi-docs/woofi-dev-docs/references/readme/base.md"
FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"
SLOT = "0x" + (int.from_bytes(keccak(text="eip1967.proxy.implementation")) - 1).to_bytes(32).hex()
WETH = "0x4200000000000000000000000000000000000006"
# Владельцы из событий Transfer шести предшествующих блоков. Исходные балансы и код проверяются по blockHash.
HOLDERS = {
    WETH: "0x682718efd1bd558afa17d02b1cd328bc705b5cef",
    "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf": "0x682718efd1bd558afa17d02b1cd328bc705b5cef",
    "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913": "0x179c94e8cb6e30f2f74e1dd467502aa92d21298e",
}


def rpc(e, method, params):
    payload = dict(jsonrpc="2.0", id=1, method=method, params=params)
    response = requests.post(e["endpoint"],json=payload,timeout=25)
    response.raise_for_status()
    result=response.json()
    if "result" not in result:
        raise ValueError(result["error"])
    return result["result"]


def simulation(e, pool, source, target, dx, deadline, runtime, expected=None):
    sender = HOLDERS[source]
    balance = lambda who: calldata("balanceOf(address)",["address"],[who])
    query = calldata("query(address,address,uint256)",["address","address","uint256"],[source,target,dx])
    approve = calldata("approve(address,uint256)",["address","uint256"],[EXEC,dx])
    def execute(minimum):
        return calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                        ["address","uint256","address","uint256","uint256",STEP+"[]"],
                        [source,dx,target,minimum,deadline,[(6,pool,source,target,10000,b"")]])
    prepared = [("approve",source,approve),("quote",pool,query),("in_before",source,balance(sender)),
                ("out_before",target,balance(sender))]
    if expected is not None:
        prepared.append(("above",EXEC,execute(expected+1)))
    prepared.extend([("exact",EXEC,execute(expected or 0)),("in_after",source,balance(sender)),
                     ("out_after",target,balance(sender)),("executor_in",source,balance(EXEC)),
                     ("executor_out",target,balance(EXEC))])
    calls = [{"from":sender,"to":to,"data":data} for _,to,data in prepared]
    word = lambda x: "0x"+x.to_bytes(32).hex()
    overrides={EXEC:{"code":"0x"+runtime.hex(),"stateDiff":{word(0):word(int(sender,16)),word(1):word(int(WETH,16))}}}
    params = [{"blockStateCalls":[{"calls":calls,"stateOverrides":overrides}],"validation":False},e["selector"]]
    result,=rpc(e,"eth_simulateV1",params)
    if result["parentHash"]!=e["selector"]["blockHash"]:
        raise ValueError("иной блок симуляции")
    results={name:row for (name,_,_),row in zip(prepared,result["calls"],strict=True)}
    for name,row in results.items():
        if row["status"] != ("0x0" if name=="above" else "0x1"):
            raise ValueError(f"{pool} {source} {dx} {name} {row}")
    n=lambda name:int(results[name]["returnData"],16)
    if (n("in_before")-n("in_after")!=dx or n("out_after")-n("out_before")!=n("exact")
            or n("exact")!=n("quote") or n("quote")==0 or n("executor_in") or n("executor_out")):
        raise ValueError("расхождение квотера, списания, выплаты или остатков V5")
    if expected is not None and n("exact")!=expected:
        raise ValueError("повторная котировка дала другой результат")
    return n("quote"), dict(source=source,target=target,input=str(dx),received=str(n("exact")),sender=sender,
                            parent_hash=result["parentHash"],calls=calls,results=results)


def main():
    e=read(FOLDER/"base.json")
    c=Capture(e)
    page=requests.get(DOC,timeout=20)
    page.raise_for_status()
    addresses={label:addr.lower() for addr,label in re.findall(r"<td>(0x[\da-fA-F]{40})</td><td>([^<]+)</td>",page.text)}
    pool=addresses["WooPPV2.2"]
    helper=addresses["IntegrationHelper(token info)"]
    oracle,=c.one(pool,"wooracle()",returns=["address"])
    quote,=c.one(pool,"quoteToken()",returns=["address"])
    paused,=c.one(pool,"paused()",returns=["bool"])
    official_quote,tokens=c.one(helper,"getSupportTokens()",returns=["address","address[]"])
    if quote!=official_quote or paused:
        raise ValueError("пауза WooPP либо другой quoteToken в справочнике")
    raw_slot=rpc(e,"eth_getStorageAt",[pool,SLOT,e["selector"]])
    implementation="0x"+raw_slot[-40:]
    for address in [pool,helper,oracle,implementation]:
        code(e,address)
    runtime=bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))
    header=rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    if rpc(e,"eth_getCode",[EXEC,e["selector"]])!="0x":
        raise ValueError("адрес виртуального исполнителя занят")
    deadline=int(header["timestamp"],16)+300
    cases,markets=[],[]
    for token in tokens:
        price,spread,coeff,feasible=c.one(oracle,"state(address)",["address"],[token],
                                          ["uint128","uint64","uint64","bool"])
        if not price or not feasible:
            raise ValueError("у официального токена неактивная котировка")
        markets.append(dict(token=token,price=str(price),spread=str(spread),coeff=str(coeff),feasible=feasible))
        for source,target in [(token,quote),(quote,token)]:
            if rpc(e,"eth_getCode",[HOLDERS[source],e["selector"]])!="0x":
                raise ValueError("адрес из события Transfer не является EOA")
            sizes = (10**14,10**16) if source==WETH else ((100,10000) if source==token else (10**6,10**8))
            for dx in sizes:
                expected,_=simulation(e,pool,source,target,dx,deadline,runtime)
                _,case=simulation(e,pool,source,target,dx,deadline,runtime,expected)
                cases.append(case)
    write(FOLDER/"woofi_base.json",dict(source=DOC,chain_id=e["chain_id"],selector=e["selector"],endpoint=c.endpoint,
          woo=pool,helper=helper,oracle=oracle,documented_oracle=addresses["WooracleV2.1"],implementation=implementation,
          implementation_slot=SLOT,raw_slot=raw_slot,quote=quote,paused=paused,markets=markets,view_calls=c.calls,
          executor=EXEC,runtime_sha256=hashlib.sha256(runtime).hexdigest(),runtime_file="contracts/out/ArbExecutor.bin-runtime",
          runtime_file_sha256=hashlib.sha256(RUNTIME.read_text().encode()).hexdigest(),
          override_slots={"0":"sender","1":WETH},deadline=deadline,cases=cases))
    write(FOLDER/"base.json",e)
    path=ROOT/"config/first_run_dex.json"
    rows=[p for p in read(path) if (p["network"],p["id"])!=("base","woofi-swap")]
    rows.append(dict(network="base",id="woofi-swap",chain_id=e["chain_id"],source=DOC,
                     settings={"dex.kind":"woofi_v2","dex.pmm_pool":pool,"dex.oracle":oracle,
                               "dex.pool_implementation":implementation}))
    write(path,rows)


if __name__=="__main__":
    main()
