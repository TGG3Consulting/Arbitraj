"""Суммы исполненных заявок #8823 за точные 86400 секунд до канонического H. Денежная единица — атом входного ERC20."""
import argparse
import json
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from eth_abi import decode

from record_pancakex_history import FILL, FILL_DATA, TRANSFER, NETS, Reader, addr, header


def run(net):
    chain,n,_,reactors,_=NETS[net]
    rpc = Reader(("https://base-rpc.publicnode.com","https://mainnet.base.org") if net=="base" else
                 ("https://ethereum-rpc.publicnode.com","https://mainnet.gateway.tenderly.co"))
    def block(n): return rpc.rpc("eth_getBlockByNumber",[hex(n),False])
    head=block(n)
    cutoff=int(head["timestamp"],16)-86400
    window=500_000 if net=="base" else 10_000
    lo=max(n-window,0)
    while int(block(lo)["timestamp"],16)>=cutoff:
        window*=2
        lo=max(n-window,0)
        if not lo: break
    hi=n
    while lo<hi:
        mid=(lo+hi)//2
        if int(block(mid)["timestamp"],16)<cutoff:lo=mid+1
        else:hi=mid
    first=block(lo)
    prior=block(lo-1)
    assert int(prior["timestamp"],16)<cutoff<=int(first["timestamp"],16)
    logs=[]
    for start in range(lo,n+1,40_000):
        logs.extend(rpc.rpc("eth_getLogs",[{"fromBlock":hex(start),"toBlock":hex(min(start+39_999,n)),
                                            "address":reactors,"topics":[FILL]}]))
    txs=sorted({v["transactionHash"] for v in logs})
    def payments(txhash):
        r=rpc.rpc("eth_getTransactionReceipt",[txhash])
        b=block(int(r["blockNumber"],16))
        assert r["blockHash"]==b["hash"] and r["status"]=="0x1" and b["transactions"][int(r["transactionIndex"],16)]==txhash
        events=[v for v in r["logs"] if v["address"].lower() in reactors and v["topics"][0]==FILL]
        assert {v["logIndex"] for v in events}=={v["logIndex"] for v in logs if v["transactionHash"]==txhash}
        used=set();rows=[]
        for v in events:
            _,key,ft,st=v["topics"]; fill=addr(ft); owner=addr(st); index=int(v["logIndex"],16)
            data=[q for q in r["logs"] if q["address"]==v["address"] and q["topics"][:2]==[FILL_DATA,key]]
            assert len(data)<=1
            candidates=[q for q in r["logs"] if q["topics"]==[TRANSFER,st,ft] and int(q["logIndex"],16)<index and q["logIndex"] not in used]
            if data:
                inp,outs=decode(["uint256","(address,uint256,address)[]"],bytes.fromhex(data[0]["data"][2:]))
                token=addr(data[0]["topics"][2])
                candidates=[q for q in candidates if int(q["data"],16)==inp and q["address"].lower()==token]
            # Повторные одинаковые платежи внутри batch требуют отдельной атрибуции.
            record={"tx":txhash,"block":b["hash"],"key":key,"reactor":v["address"],"filler":fill,"owner":owner,"fill_index":index}
            if len(candidates)==1:
                q=candidates[0];used.add(q["logIndex"])
                record.update(token=q["address"],input=str(int(q["data"],16)),transfer_index=int(q["logIndex"],16))
            else:record["candidate_count"]=len(candidates)
            rows.append(record)
        return rows
    with ThreadPoolExecutor(max_workers=3) as workers: rows=[q for group in workers.map(payments,txs) for q in group]
    totals=Counter()
    for v in rows:
        if "input" in v:totals[v["token"]]+=int(v["input"])
    assert block(n)["hash"]==head["hash"] and block(lo)["hash"]==first["hash"]
    return {"chain":chain,"head":header(head),"first":header(first),"previous":header(prior),"seconds":86400,
            "fills":rows,"input_atoms":{token:str(q) for token,q in totals.items()}}


if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("network",choices=NETS);p.add_argument("output",type=Path);args=p.parse_args()
    args.output.write_text(json.dumps(run(args.network),separators=(",",":")))
