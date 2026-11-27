"""Compare Theorem's QuoterV2 with funded original Router and Algebra V5 on the same state."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from record_bitswap_ethereum import tx, balance
from record_concave_ethereum import approval
from record_base_slipstream_executor import EXEC, OWNER, STEP, RUNTIME, simulate_block
from record_ethereum_v2_factories import WETH, STABLES, FUNDING
from record_theorem_ethereum import DATA, QUOTER, ROUTER, directory
from fork_fee_rpc import Reader

QUOTE = "(address,address,address,uint256,uint160)"
ORDER = "(address,address,address,address,uint256,uint256,uint256,uint160)"

def wallets(reader,book):
    tag=reader.tag
    assert book["selector"]==tag and book["state_root"]==reader.header["stateRoot"]
    assert reader.many(("eth_getCode",[a,tag]) for a in [OWNER,EXEC])==["0x","0x"]
    word=lambda n:"0x"+format(n,"064x")
    env={OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
        "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    def rpc(_url,method,params):
        assert method=="eth_simulateV1" and params[-1]==tag
        params[0]["blockStateCalls"][0]["blockOverrides"]={"time":hex(int(reader.header["timestamp"],16)+12)}
        return reader.one(method,params)
    def run(calls):
        return simulate_block(None,tag,env,calls,read_rpc=rpc)
    deadline=int(reader.header["timestamp"],16)+120
    def leg(prefix,market,a,b,size,wire):
        target=EXEC if wire=="v5" else ROUTER
        def swap(floor,end):
            if wire=="v5":
                return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],
                          [a,size,b,floor,end,[(2,market["pool"],a,b,10000,b"")]])
            return tx(ROUTER,"exactInputSingle("+ORDER+")",[ORDER],[(a,b,market["deployer"],OWNER,end,size,floor,0)])
        quote=tx(QUOTER,"quoteExactInputSingle("+QUOTE+")",[QUOTE],[(a,b,market["deployer"],size,0)])
        permit=approval(a,target,size)
        reads=[balance(t,who) for who in (OWNER,EXEC) for t in (a,b)]
        base=prefix+[permit,*reads]
        q=run(prefix+[quote])[-1]
        # Swap quotes via revert internally. Simulate each actual swap on a fresh copy.
        trials=run(base+[swap(1,deadline),*reads])
        at=len(base)
        if any(x["status"]!="0x1" for x in trials[:at]):
            return dict(refusal="funding, approval or balances",errors=[r.get("error") for r in trials[:at] if r["status"]!="0x1"]),None
        quoted=dict(status=q["status"],data=q.get("returnData"),error=q.get("error"))
        if q["status"]=="0x1":
            credit,debit,_,_,gas,fee=decode(["uint256","uint256","uint160","uint32","uint256","uint16"],bytes.fromhex(q["returnData"][2:]))
            quoted.update(spent=str(debit),received=str(credit),fee=fee,gas=gas)
        if trials[at]["status"]!="0x1":
            return dict(refusal="swap",quote=quoted,error=trials[at].get("error")),None
        def values(rows):return [int(v["returnData"],16) for v in rows]
        before=values(trials[at-4:at]);after=values(trials[at+1:]);spent=before[0]-after[0];received=after[1]-before[1]
        assert 0<spent<=size and 0<received and before[2:]==after[2:]==[0,0]
        paid=swap(received,deadline)
        checks=[run(base+[action,*reads]) for action in (swap(received+1,deadline),swap(received,1),paid)]
        above,late,exact=[r[at] for r in checks]
        outcomes=[]
        for scenario in checks:
            assert values(scenario[at-4:at])==before
            final=values(scenario[-4:])
            outcomes.append(dict(status=scenario[at]["status"],spent=str(before[0]-final[0]),
                                 received=str(final[1]-before[1]),error=scenario[at].get("error")))
            if scenario[at]["status"]=="0x0": assert final==before
        if [v["status"] for v in outcomes]!=["0x0","0x0","0x1"] or values(checks[-1][-4:])!=after:
            return dict(refusal="wallet minimum or deadline differs",requested=str(size),loose_received=str(received),
                        input=a,output=b,quote=quoted,controls=outcomes),None
        return dict(input=a,output=b,requested=str(size),spent=str(spent),received=str(received),
                    gas=int(exact["gasUsed"],16),approval_gas=int(checks[-1][len(prefix)]["gasUsed"],16),
                    upper=above["error"],expired=late["error"],transaction=paid,logs=exact["logs"],quote=quoted),prefix+[permit,paid]
    rows=[]
    for m in book["markets"]:
        cash=next((a for a in [WETH,*STABLES] if a in m["tokens"]),None)
        if cash is None:
            rows.append(dict(pool=m["pool"],refusal="no independent cash funding for this pair"));continue
        other=next(a for a in m["tokens"] if a!=cash)
        unit=10**14 if cash==WETH else 10**6
        for amount in [unit,unit*10]:
            if cash==WETH:prefix=[tx(WETH,"deposit()",value=amount)]
            else:prefix=[tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                         ["uint256","address[]","address","uint256"],[amount,[WETH,cash],OWNER,deadline],value=10**17)]
            for wire in ("router","v5"):
                buy,again=leg(prefix,m,cash,other,amount,wire)
                rec=dict(pool=m["pool"],input=cash,requested=str(amount),wire=wire,buy=buy);rows.append(rec)
                if again:
                    rec["sell"],_=leg(again,m,other,cash,int(buy["received"]),wire)
    assert any("received" in r.get("sell",{}) for r in rows)
    return dict(selector=tag,height=book["height"],state_root=book["state_root"],cases=rows,payer=OWNER,executor=EXEC)

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output",type=Path);args=cli.parse_args()
    reader=Reader("ethereum",json.loads((DATA/"ethereum.json").read_bytes()))
    book=directory(reader)
    args.output.write_text(json.dumps({"directory":book,"wallets":wallets(reader,book)},separators=(",",":")),encoding="utf8")
