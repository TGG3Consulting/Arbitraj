"""Две доли через один Hydrex WETH/USDC внутри одного вызова неизменённого ArbExecutor V5."""
from eth_abi import decode
from eth_utils import keccak

from record_hydrex_integral import EXEC, FOLDER, HOLDERS, RUNTIME, STEP, WETH, calldata, read, rpc, write


def main():
    evidence = read(FOLDER/"base.json")
    sample = read(FOLDER/"hydrex_base.json")
    market = next(p for p in sample["markets"] if WETH in p["tokens"] and p["current_fee"]==375)
    pool = market["pool"]
    topic = "0x"+keccak(text="Swap(address,address,int256,int256,uint160,uint128,int24,uint24,uint24)").hex()
    word = lambda n:"0x"+n.to_bytes(32).hex()
    cases = []
    for prior in market["cases"]:
        token, target, dx = prior["source"],prior["target"],int(prior["dx"])
        # У исходного держателя USDC хватает средств на 100, но не на две доли по 100.
        sender = HOLDERS[token] if token==WETH else "0xb0d0b9b8ab03cbac16cdf9e59bc5a1a2ade55da2"
        if rpc(evidence,"eth_getCode",[sender,evidence["selector"]])!="0x":
            raise ValueError("отправитель не EOA на контрольном блоке")
        zero = token==market["tokens"][0]
        def execute(limit):
            return calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                            ["address","uint256","address","uint256","uint256",STEP+"[]"],
                            [token,dx*2,target,limit,sample["deadline"],
                             [(2,pool,token,target,share,b"") for share in (5000,10000)]])
        overrides = {EXEC:{"code":"0x"+RUNTIME.read_text().strip(),"stateDiff":{
            word(0):word(int(sender,16)),word(1):word(int(WETH,16))}}}
        balance = calldata("balanceOf(address)",["address"],[sender])
        approve = calldata("approve(address,uint256)",["address","uint256"],[EXEC,dx*2])
        def run(entries):
            calls = [{"from":sender,"to":a,"data":d,"gas":hex(3500000)} for _,a,d in entries]
            block, = rpc(evidence,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls,"stateOverrides":overrides}],
                                                    "validation":False},evidence["selector"]])
            if block["parentHash"]!=evidence["selector"]["blockHash"]:
                raise ValueError("другой блок")
            results = {label:r for (label,_,_),r in zip(entries,block["calls"],strict=True)}
            for label,r in results.items():
                if r["status"]!=("0x0" if label=="above" else "0x1"):
                    raise ValueError(f"{pool} {token} {dx} {label} {r.get('error')}")
            return calls,results
        prefix = [("approve",token,approve),("in_before",token,balance),("out_before",target,balance)]
        _, measured = run(prefix+[("exact",EXEC,execute(0))])
        minimum = int(measured["exact"]["returnData"],16)
        calls,res = run(prefix+[("above",EXEC,execute(minimum+1)),("exact",EXEC,execute(minimum)),
                                ("in_after",token,balance),("out_after",target,balance)])
        n = lambda k:int(res[k]["returnData"],16)
        events = []
        for l in res["exact"]["logs"]:
            if l["address"].lower()!=pool or l["topics"][0]!=topic:
                continue
            x,y,price,liq,tick,fee,plugin = decode(["int256","int256","uint160","uint128","int24","uint24","uint24"],
                                                  bytes.fromhex(l["data"][2:]))
            events.append(dict(sender="0x"+l["topics"][1][-40:],delta0=str(x),delta1=str(y),price=str(price),
                               liquidity=str(liq),tick=tick,fee=fee,plugin_fee=plugin))
        main_swaps = [r for r in events if r["sender"]==EXEC]
        outputs = [-int(r["delta1" if zero else "delta0"]) for r in main_swaps]
        if (len(outputs)!=2 or outputs[0]!=int(prior["out"]) or sum(outputs)!=minimum or n("exact")!=minimum
                or any(int(r["delta0" if zero else "delta1"])!=dx for r in main_swaps)
                or n("in_before")-n("in_after")!=2*dx or n("out_after")-n("out_before")!=minimum):
            raise ValueError("доли, событие или балансы расходятся")
        cases.append(dict(source=token,dx=str(dx),zero_for_one=zero,output=list(map(str,outputs)),
                          events=events,calls=calls,results=res))
    write(FOLDER/"hydrex_sequential_base.json",dict(selector=evidence["selector"],endpoint=evidence["endpoint"],
          pool=pool,executor=EXEC,executor_runtime_sha256=sample["runtime_sha256"][EXEC],cases=cases))


if __name__=="__main__":
    main()
