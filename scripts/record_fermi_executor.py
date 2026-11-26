"""Replay the Actions-built FermiSlotExecutor on two archived real builder updates. No signatures."""
import json
import sys
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, call, calldata
from record_fermi_ethereum import ACTIVE, ENGINE, WETH, USDT, UNIV2, ARGS, QUOTE, OUT, rpc, selector

ORDER = "(bytes32,bytes32,uint64,uint64,address,address,int256,uint256)"
OTHER = "0x" + keccak(text="Ethereum Fermi independent payer")[-20:].hex()
EXECUTOR = "0x" + keccak(b"\xd6\x94" + bytes.fromhex(OWNER[2:]) + b"\x80")[-20:].hex()


def setup(token, budget, who):
    def tx(to, data, value=0):
        return {**call(to, data, value), "from": who}
    funding = max(10**17, budget) if token == WETH else 10**17
    calls = [tx(WETH, calldata("deposit()"), funding)]
    def approve(token_addr, spender, amount):
        return tx(token_addr, calldata("approve(address,uint256)", ["address","uint256"], [spender,amount]))
    if token != WETH:
        calls.extend([approve(WETH, UNIV2, funding), tx(UNIV2, calldata(
            "swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
            ["uint256","uint256","address[]","address","uint256"], [funding,budget,[WETH,token],who,2**64-1]))])
    return calls + [approve(token, EXECUTOR, budget)]


def capture(compiled):
    f = json.loads(OUT.read_bytes())
    report = {}
    deploy = {"from":OWNER, "data":"0x"+compiled["creation"], "gas":hex(4_000_000)}
    for name,frame in f["frames"].items():
        parent = f["simulations"][name]["parent"]
        tag = selector(parent)
        if any(rpc("eth_getCode", [a, tag]) != "0x" for a in (OWNER,OTHER,EXECUTOR)):
            raise ValueError("simulation address occupied")
        target, timestamp = frame["target"], f["simulations"][name]["timestamp"]
        settings = {OWNER:{"balance":hex(10**20),"nonce":"0x0"}, OTHER:{"balance":hex(10**20),"nonce":"0x0"},
                    ENGINE:{"stateDiff":frame["stateDiff"]}}
        def simulate(calls):
            body = {"blockStateCalls":[{"blockOverrides":{"time":hex(timestamp)},"stateOverrides":settings,"calls":calls}]}
            b = rpc("eth_simulateV1", [body, tag])[0]
            if b["parentHash"] != parent["hash"] or int(b["number"],16) != target or int(b["timestamp"],16) != timestamp:
                raise ValueError("wrong block for replay")
            return b["calls"]
        def success(row):
            if row["status"] != "0x1":
                raise ValueError(row.get("error","failed"))
            return bytes.fromhex(row["returnData"][2:])
        def exchange(order, who=OWNER):
            return {**call(EXECUTOR,calldata("execute("+ORDER+")",[ORDER],[order])), "from":who}
        def balance(token, who):
            return call(token,calldata("balanceOf(address)",["address"],[who]))
        def completed(who, leg):
            return call(EXECUTOR,calldata("completed(address,bytes32)",["address","bytes32"],[who,leg]))
        # Price the modeled gas at an observed effective price from the *canonical target block*.
        # This is a reference tariff. These simulated calls were never included/mined.
        target_header = rpc("eth_getBlockByNumber",[hex(target),False])
        ref = rpc("eth_getTransactionReceipt",[target_header["transactions"][0]])
        if ref["blockHash"] != target_header["hash"]: raise ValueError("reference tx was reorganized")
        price = int(ref["effectiveGasPrice"],16)
        cases=[]
        for source,dest,qty in [(WETH,USDT,10**15),(WETH,USDT,-1_000_000),(USDT,WETH,1_000_000),(USDT,WETH,-10**15)]:
            quote = {**call(ACTIVE,calldata(QUOTE,ARGS,[source,dest,qty])),"from":EXECUTOR}
            raw = simulate([deploy,quote])
            if success(raw[0]).hex() != compiled["runtime"]: raise ValueError("executed bytecode mismatch")
            spent, received = decode(["uint256","uint256"],success(raw[1]))
            if min(spent,received) <= 0 or (qty>0 and spent!=qty) or (qty<0 and received!=-qty):
                raise ValueError("exact amount missing")
            limit = received if qty > 0 else spent+100
            budget = spent if qty>0 else limit
            leg = keccak(text=f"{name}/{source}/{qty}")
            args = [leg,bytes.fromhex(parent["hash"][2:]),target,timestamp,source,dest,qty,limit]
            bad_price = args.copy()
            bad_price[-1] = received+1 if qty>0 else spent-1
            wrong_parent = args.copy(); wrong_parent[1] = bytes(32)
            wrong_number = args.copy(); wrong_number[2] -= 1
            wrong_time = args.copy(); wrong_time[3] += 12
            prefix = [deploy]+setup(source,budget,OWNER)
            model = [balance(source,OWNER),balance(dest,OWNER),exchange(bad_price),
                     completed(OWNER,leg),exchange(wrong_parent),exchange(wrong_number),exchange(wrong_time),
                     exchange(args),balance(source,OWNER),balance(dest,OWNER),completed(OWNER,leg),
                     exchange(args),completed(OTHER,leg)]
            rows = simulate(prefix+model)
            for row in rows[:len(prefix)]: success(row)
            approval_gas = int(rows[len(prefix)-1]["gasUsed"],16)
            r = rows[len(prefix):]
            val=lambda i:int.from_bytes(success(r[i]),"big")
            for i in [2,4,5,6,11]:
                if r[i]["status"] != "0x0" or r[i].get("logs"): raise ValueError(("expected atomic revert",i))
            if val(3) or val(10) != 1 or val(12) or decode(["uint256","uint256"],success(r[7])) != (spent,received):
                raise ValueError("leg reuse or incorrect execution result")
            if val(0)-val(8) != spent or val(9)-val(1) != received: raise ValueError("payer deltas differ")
            swap_gas=int(r[7]["gasUsed"],16)
            # A separate payer can use the same public leg id with their own funds at the same slot.
            # Run each payer sequentially. A fresh quote avoids assuming the first swap did not move the price.
            other_args = list(args)
            other_args[-1] = 1 if qty>0 else budget*2
            both = simulate(prefix+[exchange(args)]+setup(source,budget if qty>0 else budget*2,OTHER)+
                            [exchange(other_args,OTHER),completed(OTHER,leg)])
            for row in both: success(row)
            if int.from_bytes(success(both[-1]),"big")!=1: raise ValueError("independent leg missing")
            cases.append(dict(input=source,output=dest,amount_specified=str(qty),limit=str(limit),spent=str(spent),
                              received=str(received),refunded=str(budget-spent),approval_gas=approval_gas,
                              swap_gas=swap_gas,modeled_swap_wei=str(price*swap_gas),
                              modeled_first_approval_wei=str(price*approval_gas),exchange=r[7], min_or_max_reject=r[2],
                              replay_reject=r[11], wrong_parent=r[4],wrong_target=r[5],wrong_timestamp=r[6],
                              second_payer=both[-2]))
        report[name]=dict(slot=frame["slot"],target=target,parent_hash=parent["hash"],target_time=timestamp,
                         reference_price_tx=ref["transactionHash"],reference_effective_gas_price_wei=str(price),cases=cases)
    return dict(isolated=True,chain_id=1,executor=EXECUTOR,runtime_keccak="0x"+keccak(bytes.fromhex(compiled["runtime"])).hex(),
                solc=compiled["solc"],builders=report)


if __name__=="__main__":
    compiled, output = map(Path,sys.argv[1:])
    output.write_text(json.dumps(capture(json.loads(compiled.read_bytes())),indent=2)+"\n")
