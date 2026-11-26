"""Prove Metric or V3 swap results in both directions with the repository executor and controlled ERC-20 balances."""
import argparse
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, OWNER as FIRST_OWNER, ROOT, RUNTIME, STEP, call as message, calldata, rpc
from record_metric_chains import VIRTUAL
from record_v2_chain_pools import NETWORKS


def record(source, owner=FIRST_OWNER):
    def call(target, data, value=0):
        return {**message(target,data,value), "from":owner}
    mode=source.get("kind","metric")
    quote_fn={"metric":"quoteMetric","v3":"quoteExactIn"}[mode]
    step_kind={"metric":13,"v3":1}[mode]
    network=source["network"]
    url,weth=NETWORKS[network]
    tag={"blockHash":source["hash"],"requireCanonical":True}
    header=rpc(url,"eth_getBlockByHash",[source["hash"],False])
    deadline=int(header["timestamp"],16)+300
    runtime=bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))
    virt=(ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x")
    word=lambda v:"0x"+format(v,"064x")
    overrides={owner:{"balance":hex(2*10**18)},VIRTUAL:{"code":"0x"+virt},EXEC:{"code":"0x"+runtime.hex(),"stateDiff":{word(0):word(int(owner,16)),word(1):word(int(weth,16))}}}
    profiles=json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    router=next(r["settings"]["dex.router"] for r in profiles if r["network"]==network and r["settings"]["dex.kind"]=="uniswap_v2")
    def simulate(calls):
        return rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":overrides,"calls":calls}]},tag])[0]["calls"]
    def successful(calls):
        values=simulate(calls)
        for index,row in enumerate(values):
            if int(row["status"],16)!=1:
                raise ValueError(f"call {index}: {row.get('error')}")
        return values
    def approve(token,to,amount):
        return call(token,calldata("approve(address,uint256)",["address","uint256"],[to,amount]))
    def balance(token,owner):
        return call(token,calldata("balanceOf(address)",["address"],[owner]))
    result={"network":network,"block":source["block"],"hash":source["hash"],"factory":source["contracts"]["factory"],
            "executor_runtime_keccak":"0x"+keccak(runtime).hex(),"sender":owner,"kind":mode,"virtual_quoter_keccak":source["quoter_runtime_keccak"],"pools":[]}
    for pool in source["pools"]:
        quotes=pool["quotes"]
        if mode=="metric" and (len(quotes)!=4 or not all(int(q["out"]) and q["input"]==q["used"] for q in quotes)):
            continue
        addr=pool["pool"]
        tokens=pool["tokens"]
        result_row={"idx":pool["idx"],"pool":addr,"tokens":tokens,"immutables":pool.get("immutables"),"prices":pool.get("prices"),"cases":[]}
        result["pools"].append(result_row)
        for q in (quotes[::2] if mode=="metric" else quotes):
            side=q["side"]
            cash,coin=tokens[side],tokens[1-side]
            amount=int(q["input"])
            try:
                deposit=amount if cash==weth else int(pool.get("funding_weth",10**16))
                prefix=[call(weth,calldata("deposit()"),deposit)]
                if cash!=weth:
                    prefix.extend([approve(weth,router,deposit),call(router,calldata("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",["uint256","uint256","address[]","address","uint256"],[deposit,amount,[weth,cash],owner,deadline]))])
                def preview(side,amount):
                    return call(VIRTUAL,calldata(quote_fn+"(address,bool,uint256)",["address","bool","uint256"],[addr,side==0,amount]))
                def execute(side,x,minimum):
                    data=calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[tokens[side],x,tokens[1-side],minimum,deadline,[(step_kind,addr,tokens[side],tokens[1-side],10000,encode(["bool"],[side==0]) if mode=="metric" else b"")]])
                    return call(EXEC,data)
                def quote(calls,amount):
                    raw=bytes.fromhex(successful(calls)[-1]["returnData"][2:])
                    if len(raw)!=64 or int.from_bytes(raw[:32],"big")!=amount or not int.from_bytes(raw[32:],"big"):
                        raise ValueError("partial or zero quote")
                    return int.from_bytes(raw[32:],"big")
                prefix.extend([approve(cash,EXEC,amount),balance(cash,owner)])
                bought=quote(prefix+[preview(side,amount)],amount)
                buy=execute(side,amount,bought)
                legs=prefix+[buy,balance(coin,owner),approve(coin,EXEC,bought)]
                sold=quote(legs+[preview(1-side,bought)],bought)
                full=successful(legs+[execute(1-side,bought,sold),balance(cash,owner),balance(coin,owner),balance(coin,EXEC),balance(cash,EXEC)])
                def val(i):return int(full[i]["returnData"],16)
                assert val(len(prefix)+1)==bought
                assert val(-4)==val(len(prefix)-1)-amount+sold and [val(-3),val(-2),val(-1)]==[0,0,0]
                for calls in [prefix+[execute(side,amount,bought+1)],legs+[execute(1-side,bought,sold+1)]]:
                    if int(simulate(calls)[-1]["status"],16)!=0:raise ValueError("minOut+1 passed")
                result_row["cases"].append({"side":side,"input":str(amount),"bought":str(bought),"sold":str(sold),
                    "cash_before":str(val(len(prefix)-1)),"cash_after":str(val(-4)),"after_coin_owner":str(val(-3)),
                    "after_coin_executor":str(val(-2)),"after_cash_executor":str(val(-1)),
                    "min_out_plus_one_buy_reverted":True,"min_out_plus_one_sell_reverted":True})
            except (ValueError,KeyError) as error:result_row["cases"].append({"side":side,"input":str(amount),"reason":str(error)[:160]})
    return result


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("capture",type=Path)
    p.add_argument("output",type=Path)
    p.add_argument("--second-sender",action="store_true")
    args=p.parse_args()
    owner = "0x"+keccak(text="second independent executor owner for verification")[-20:].hex() if args.second_sender else FIRST_OWNER
    sample=record(json.loads(args.capture.read_bytes()),owner)
    args.output.write_text(json.dumps(sample,indent=2)+"\n",encoding="utf8")
    print(sample["network"],[(p["idx"],len([x for x in p["cases"] if "sold" in x])) for p in sample["pools"]])
