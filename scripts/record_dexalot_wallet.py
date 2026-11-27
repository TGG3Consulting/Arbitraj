"""Replay an expired real maker signature through the guarded payer. No live RFQ or signed transaction is sent.
The deployed module code is placed at the historical signed taker in isolated EVM state only.
Original router, settlement, tokens, signature, time-dependent fee table and block parent are unchanged.
"""
import gzip
import hashlib
import json
import sys
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from record_aktionariat_wallet import approve
from record_base_slipstream_executor import OWNER, ROOT
from record_bitswap_ethereum import balance, tx
from record_dexalot_base import ORDER, ROUTER, WETH, USDC, WITNESS, Read

DIR=ROOT/"tests/data/first_run_dex_2026-11-26"
UINT="uint256"
TUPLE="(address,address,uint24,address,uint256,uint256,uint160)"


def acquire(compiled):
    r=Read()
    j=json.loads(gzip.decompress((DIR/"dexalot_base_registry.json.gz").read_bytes()))
    p=j["witness"];head=j["head"]
    original=r("eth_getTransactionByHash",[WITNESS])
    fields,sig=decode([ORDER,"bytes"],bytes.fromhex(original["input"][10:]))
    taker=p["taker"];cash=fields[4]
    assert fields[3]==USDC and fields[2]==WETH and fields[5]==taker
    prep="0x"+compiled["creation"]+encode(["address"]*2,[ROUTER,WETH]).hex()
    create={"from":OWNER,"data":prep,"gas":hex(5_000_000)}
    assert int(r("eth_getTransactionCount",[OWNER,head]),16)==0
    module="0x"+keccak(bytes.fromhex("d694"+OWNER[2:]+"80"))[-20:].hex()
    assert r("eth_getCode",[module,head])=="0x"
    def run(at,env,calls,time=None):
        frame=dict(stateOverrides=env,calls=calls)
        if time is not None:
            frame["blockOverrides"]={"time":hex(time)}
        reply=r("eth_simulateV1",[{"blockStateCalls":[frame]},at])
        assert len(reply)==1 and reply[0]["parentHash"]==at["blockHash"]
        return reply[0]["calls"]
    created,owner,router,wrapped=run(head,{OWNER:{"balance":hex(10**18)}},[create,tx(module,"owner()"),tx(module,"router()"),tx(module,"wrapped()")])
    assert all(x["status"]=="0x1" for x in (created,owner,router,wrapped))
    assert [int(x["returnData"],16) for x in (owner,router,wrapped)]==[int(x,16) for x in (OWNER,ROUTER,WETH)]
    code=created["returnData"]
    manifest=dict(kind="dexalot_rfq",chainId=8453,sourceBlock=head["blockHash"],roles=j["roles"],
                  router=ROUTER,wrapped=WETH,data=prep,gas=int(created["gasUsed"],16),solc=compiled["solc"],
                  runtimeKeccak="0x"+keccak(hexstr=code).hex(),runtimeSha256=hashlib.sha256(bytes.fromhex(code[2:])).hexdigest(),
                  creationSha256=hashlib.sha256(bytes.fromhex(prep[2:])).hexdigest())
    at=dict(blockHash=p["parent_hash"],requireCanonical=True)
    ts=p["timestamp"]
    assert r("eth_getCode",[taker,at])=="0x"
    # The maker signed this address. Relocate only the guarded payer code and its owner's slot to that address.
    # Both original and new runs settle against the actual maker and the original on-chain signature registry.
    env={OWNER:{"balance":hex(2*10**18)},taker:{"code":code,"stateDiff":{"0x"+"00"*32:"0x"+OWNER[2:].zfill(64)}}}
    approved=10**17
    v3="0x2626664c2603336e57b271c5c0b26f421741e481"
    # Preserve its preexisting balances. Reset the former EOA's allowance in this simulation before measuring.
    reset=approve(USDC,ROUTER,0);reset["from"]=taker
    funding=[reset,tx(WETH,"deposit()",value=approved),approve(WETH,v3,approved),
             tx(v3,"exactInputSingle("+TUPLE+")",[TUPLE],[(WETH,USDC,500,OWNER,approved,fields[7],0)])]
    controls=[]
    for partial,amount in [(False,fields[7]),(True,fields[7]//2)]:
        nominal=fields[6]*amount//fields[7]
        net=nominal*(1_000_000-p["slip_ppm"])//1_000_000
        assert net<nominal and net>0
        def guarded(floor,deadline,caller=OWNER):
            f=[ORDER,"bytes",UINT,UINT,"bool",UINT]
            row=tx(taker,"execute("+",".join(f)+")",f,[fields,sig,amount,floor,partial,deadline])
            row["from"]=caller
            return row
        views=[balance(USDC,OWNER),balance(WETH,OWNER),balance(USDC,taker),balance(WETH,taker),
               tx(USDC,"allowance(address,address)",["address","address"],[taker,ROUTER])]
        prefix=funding+[approve(USDC,taker,amount)]
        rejects=[guarded(net+1,ts+1),guarded(net,ts-1),guarded(net,ts+1,cash)]
        sequence=prefix+views+rejects+views+[guarded(net,ts+1),guarded(net,ts+1)]+views
        res=run(at,env,sequence,ts)
        i=len(prefix);k=i+len(views);mid=k+len(rejects);paid=mid+len(views)
        failed=[(n,row.get("error")) for n,row in enumerate(res) if row["status"]!="0x1"]
        if [n for n,_ in failed]!=[*range(k,mid),paid+1]:
            raise ValueError(("Dexalot protected outcomes",partial,failed))
        def balances(n):
            return [int(x["returnData"],16) for x in res[n:n+len(views)]]
        before,held,after=balances(i),balances(mid),balances(paid+2)
        assert before==held and after[2:]==before[2:] and before[-1]==0
        delta=[b-a for a,b in zip(before,after)]
        assert delta==[-amount,net,0,0,0] and decode([UINT,UINT],bytes.fromhex(res[paid]["returnData"][2:]))==(amount,net)
        controls.append(dict(partial=partial,spent=str(amount),nominal=str(nominal),net=str(net),gas=int(res[paid]["gasUsed"],16),
                             refuted=[dict(step=n,reason=e) for n,e in failed],logs=res[paid]["logs"]))
    return dict(head=head,historical_parent=at,tx=WITNESS,mode="isolated guarded wallet at historical signed taker",
                owner=OWNER,fixture_module=taker,manifest=manifest,controls=controls)


if __name__=="__main__":
    report=acquire(json.loads(Path(sys.argv[2]).read_bytes()))
    Path(sys.argv[1]).write_text(json.dumps(report,separators=(",",":")),encoding="utf8")
