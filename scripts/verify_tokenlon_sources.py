"""Classify the canonical Tokenlon sample using the ABI, the actual emitting address and retained wallet payments."""
from collections import Counter
from pathlib import Path
import gzip
import json
import sys

from eth_abi import decode
from eth_utils import keccak

DATA = Path("tests/data/first_run_dex_2026-11-26")
FILL = "FillOrder(string,bytes32,bytes32,address,address,uint256,address,address,uint256,address,uint256,uint16)"
SWAP1 = "Swapped(string,bytes32,address,address,uint256,address,address,uint256,address,uint256,uint256,uint16,uint16)"
QUOTE = "(string,bytes32,uint256,uint256,uint16,uint16)"
ORDER = "(address,address,address,uint256,uint256,address,address,uint256,uint256)"
SWAP2 = f"Swapped({QUOTE},{ORDER})"
TOPICS = {("0x"+keccak(text=s).hex()): s for s in (FILL,SWAP1,SWAP2)}
# Targets independently confirmed by the verified AMMWrapperWithPath and archived Swapped receipts.
MAKERS = {
    "Uniswap V3":"0xe592427a0aece92de3edee1f18e0157c05861564",
    "Uniswap V2":"0x7a250d5630b4cf539739df2c5dacb4c659f2488d",
    "Curve":"0xbebc44782c7db0a1a60cb6fe97d0b483032ff1c7",
    "SushiSwap":"0xd9e1ce17f2641f24ae83637ab66a2cca9c378b9f",
}
LEGACY_PMM=["string","address","uint256","address","address","uint256","address","uint256","uint16"]
LEGACY_WRAPPER=LEGACY_PMM[:-1]+["uint256","uint16","uint16"]

def load(name):
    path = DATA/name
    return json.loads(gzip.decompress(path.read_bytes()) if path.suffix==".gz" else path.read_bytes())

def classify(events,market,payments):
    assert events["chain"]==market["chain"]==1
    assert events["terminal"]==market["selector"]==payments["selector"]
    assert events["terminal"]["requireCanonical"] is True
    assert (events["first"],events["last"])==(market["window"]["first"],market["window"]["last"])
    assert events["last"]-events["first"]+1==7200
    protocols,makers,topics = Counter(),Counter(),Counter()
    raw_pmm={}
    seen=set()
    for role,logs in events["logs"].items():
        address=market["roles"]["smart_order_strategy" if role=="sor" else role]["address"]
        assert address==events["roles"][role]
        for entry in logs:
            identifier=entry["transactionHash"],int(entry["logIndex"],16)
            assert entry["address"]==address and identifier not in seen
            assert events["first"]<=int(entry["blockNumber"],16)<=events["last"]
            seen.add(identifier)
            event=TOPICS[entry["topics"][0]]
            topics[event]+=1
            data=bytes.fromhex(entry["data"][2:])
            if role=="pmm":
                assert event==FILL and len(entry["topics"])==4
                label,_,debit,maker,_,gross,_,net,_=decode(LEGACY_PMM,data)
                assert label=="0x v2" and debit>0 and 0<net<=gross
                makers[maker]+=1
                raw_pmm[identifier]=(entry["topics"][2],debit,net,entry["blockHash"])
                continue
            assert role=="wrapper"
            if event==SWAP1:
                assert len(entry["topics"])==3
                row=decode(LEGACY_WRAPPER,data)
                label,router=row[0],row[3]
            else:
                assert event==SWAP2 and len(entry["topics"])==1
                quote,order=decode([QUOTE,ORDER],data)
                label,router=quote[0],order[0]
            assert MAKERS[label]==router
            protocols[label]+=1
    assert protocols==Counter({"Uniswap V3":144,"Uniswap V2":1,"SushiSwap":2,"Curve":5})
    assert sorted(makers.values())==[29,72]
    assert [topics[s] for s in (FILL,SWAP1,SWAP2)]==[101,3,149]
    original={r["hash"]:r for r in market["controls"]}
    duplicate=Counter()
    assert len(payments["controls"])==len(original)==8
    assert {p["hash"] for p in payments["controls"]}==original.keys()
    for row in payments["controls"]:
        original_row=original[row["hash"]]
        oid,spent,received,block=raw_pmm[(row["hash"],int(original_row["log_index"]))]
        assert oid==row["order_hash"] and block==row["block"] and row["replay"]=="paid"
        before,after=map(lambda a:list(map(int,a)),(row["before"],row["after"]))
        assert spent==int(row["paid"])==before[0]-after[0]
        assert received==int(row["received"])==after[1]-before[1]
        assert row["receiver"]==original_row["recipient"] and row["maker_signature_bytes"]>0
        duplicate[row["duplicate"]]+=1
    assert duplicate=={"reverted without balance change":6,"insufficient original ETH for a second identical payment":2}
    samples=market["observed_taker"]["sdk_flow"]["samples"]
    assert len(samples)==3
    for sample in samples:
        first,final=sample["result"]
        assert first["step"]=="order" and first["ok"] and first["maker_bytes"]==0
        assert final["step"]=="lastOrder" and not final["ok"] and final["reason"]=="USER_BALANCE_INSUFFICIENT"
        assert not final["maker_bytes"] and not final["payload_bytes"]
    return {"first":events["first"],"last":events["last"],"pmm_by_maker":dict(makers),
            "external_by_source":dict(protocols),"source_events":len(seen),"verified_wallet_payments":len(original),
            "duplicate_outcomes":dict(duplicate),"fresh_firm_sample":len(samples)}

if __name__=="__main__":
    payments=json.loads(Path(sys.argv[1]).read_bytes()) if len(sys.argv)>1 else load("tokenlon_ethereum_wallets.json.gz")
    report=classify(load("tokenlon_ethereum_source_events.json.gz"),load("tokenlon_ethereum.json"),payments)
    print(json.dumps(report,sort_keys=True))
