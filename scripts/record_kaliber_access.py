"""Kaliber Base: обе кассы, оригинальный вызов и расход своего плательщика на обычном входе."""
import argparse
import json
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

from eth_abi import decode
from record_e3_base import OWNER, USDC, WETH, UNI, tx, ok, result
from record_kaliber_base import EVENT, VAULTS, Reader

AAPL="0xb200000000000000000000c2e324d24d7eecd1fb"
WITNESS="0x3fa51d338eaef69d0f94593fc4b6cbec9553dd221d18993aeb18de02c5c33220"
SWAP="swap(bytes32,address,address,uint256,uint256,uint256)"
TYPES=["bytes32","address","address","uint256","uint256","uint256"]


def days(r):
    first,last=51882127,52357326
    head=lambda i:r.rpc("eth_getBlockByNumber",[hex(i),False])
    bounds=[int(head(i)["timestamp"],16) for i in (first-1,first,last,last+1)]
    assert bounds[0]<1790553600<=bounds[1] and bounds[2]<1791504000<=bounds[3],bounds
    for start in range(first,last+1,43200):
        timestamp=bounds[1]+2*(start-first)
        before=int(head(start-1)["timestamp"],16)
        current=int(head(start)["timestamp"],16)
        assert before<timestamp<=current and current//86400==timestamp//86400
    counts=Counter(); stable=Counter();examples={}
    for lower in range(first,last+1,10000):
        logs=r.rpc("eth_getLogs",[dict(address=VAULTS,topics=[EVENT],fromBlock=hex(lower),toBlock=hex(min(lower+9999,last)))])
        for l in logs:
            height=int(l["blockNumber"],16)
            # На этом интервале Base выпускает блок раз в две секунды.
            stamp=bounds[1]+2*(height-first)
            date=datetime.fromtimestamp(stamp,timezone.utc).date().isoformat()
            src,dst,amount,received,_=decode(["address","address","uint256","uint256","uint256"],bytes.fromhex(l["data"][2:]))
            key=(date,l["address"],l["topics"][1],src,dst,"0x"+l["topics"][2][-40:])
            counts[key]+=1
            stable[key]+=amount if src==USDC else received if dst==USDC else 0
            examples.setdefault((l["address"],l["topics"][1],src,dst),l["transactionHash"])
    return dict(first=first,last=last,timestamps=bounds,
                rows=[dict(date=k[0],cash=k[1],pair=k[2],src=k[3],dst=k[4],trader=k[5],count=n,usdc=stable[k],sample=examples[k[1:5]]) for k,n in counts.items()])


def cash_frames(node):
    if node.get("to","").lower() in VAULTS:
        yield dict(sender=node["from"],cash=node["to"],input=node["input"],output=node.get("output"),error=node.get("error"))
    for child in node.get("calls",[]):
        yield from cash_frames(child)


def own(r,tag,vault,pair):
    block=r.rpc("eth_getBlockByHash",[tag["blockHash"],False])
    stamp=int(block["timestamp"],16)+2
    seed=tx(UNI,"exactOutputSingle((address,address,uint24,address,uint256,uint256,uint160))",
            ["(address,address,uint24,address,uint256,uint256,uint160)"],[(WETH,USDC,500,OWNER,10**6,10**17,0)],10**17)
    balance=lambda t:tx(t,"balanceOf(address)",["address"],[OWNER])
    order=tx(vault,SWAP,TYPES,[bytes.fromhex(pair[2:]),USDC,AAPL,10**6,1,stamp+60])
    calls=[seed,tx(USDC,"approve(address,uint256)",["address","uint256"],[vault,10**6]),balance(USDC),balance(AAPL),order,balance(USDC),balance(AAPL)]
    frame=dict(blockOverrides={"time":hex(stamp)},stateOverrides={OWNER:{"balance":hex(10**18)}},calls=calls)
    b,=r.rpc("eth_simulateV1",[dict(blockStateCalls=[frame],validation=False),tag])
    assert b["parentHash"]==tag["blockHash"]
    a=b["calls"]
    assert all(ok(a[i]) for i in (1,2,3,5,6))
    before=[int.from_bytes(result(a[i])) for i in (2,3)]
    after=[int.from_bytes(result(a[i])) for i in (5,6)]
    spent,received=before[0]-after[0],after[1]-before[1]
    if ok(a[4]):assert ok(a[0]) and spent==10**6 and received>=1
    else:assert spent==received==0
    return dict(cash=vault,tag=tag,funding=ok(a[0]),selector=order["data"][:10],before=before,after=after,success=ok(a[4]),error=a[4].get("error"),gas=int(a[4]["gasUsed"],16))


def capture():
    r=Reader("base")
    report=days(r)
    rec=r.rpc("eth_getTransactionReceipt",[WITNESS])
    assert rec["status"]=="0x1"
    swap,=[l for l in rec["logs"] if l["address"] in VAULTS and l["topics"][0]==EVENT]
    pair=swap["topics"][1]
    try:
        trace=r.rpc("debug_traceTransaction",[WITNESS,{"tracer":"callTracer"}])
        frames=list(cash_frames(trace))
    except ValueError as e:
        frames=[];trace_error=str(e)[:240]
    else:trace_error=None
    hist={"blockHash":rec["blockHash"],"requireCanonical":True}
    reference=tx(VAULTS[0],SWAP,TYPES,[bytes.fromhex(pair[2:]),USDC,AAPL,10**6,1,1])["data"][:10]
    verified=[f for f in frames if f["input"].startswith(reference) and not f["error"]]
    methods=r.many([(AAPL,"symbol()",(),"string"),(AAPL,"decimals()",(),"uint8")])
    trials=[]
    for v in VAULTS:
        # Старая касса приводится к фактическому Base входу из оплаченной квитанции,
        # вторая реализует байткод кассы Ethereum с тем же обычным ABI.
        if v==VAULTS[0] and not verified:continue
        for tag in (hist,r.tag):trials.append(own(r,tag,v,pair))
    return dict(history=report,roles={v:r.code_hash(v) for v in VAULTS},witness=WITNESS,token=[AAPL,*methods],payer=OWNER,
                trader="0x"+swap["topics"][2][-40:],frames=frames,trace_error=trace_error,observations=trials)


if __name__=="__main__":
    a=argparse.ArgumentParser(description=__doc__);a.add_argument("output",type=Path)
    a.parse_args().output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

