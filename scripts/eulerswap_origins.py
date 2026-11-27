"""Расположение успешных CREATE2 EulerSwap V2 по nonce фабрики; исходные события в блоках создания."""
import gzip
import json
from eth_abi import decode


def deployment(row,address,event):
    assert not row.get("removed") and row["address"].lower()==address and row["topics"][0]==event
    pool,*config=decode(["address"]*7,bytes.fromhex(row["data"][2:]))
    assets=["0x"+topic[-40:] for topic in row["topics"][1:3]]
    assert config[4]=="0x"+row["topics"][3][-40:]
    return dict(pool=pool,tokens=assets,config=config,block_hash=row["blockHash"],
                block=int(row["blockNumber"],16),tx=row["transactionHash"],index=int(row["logIndex"],16))


def from_receipts(reader,factory,final_nonce,path):
    source=json.loads(gzip.decompress(path.read_bytes()))
    logs=[]
    for candidate in source["events"]:
        block=reader.rpc("eth_getBlockByNumber",[hex(candidate["block"]),False])
        assert block["hash"]==candidate["blockHash"] and candidate["block"]<=reader.height
        if candidate["tx"]:
            receipt=reader.rpc("eth_getTransactionReceipt",[candidate["tx"]])
            assert receipt["status"]=="0x1" and receipt["blockHash"]==block["hash"] and receipt["transactionHash"]==candidate["tx"]
            rows=receipt["logs"]
        else:
            rows=reader.rpc("eth_getLogs",[{"blockHash":block["hash"],"address":factory["address"],"topics":[factory["event"]]}])
        row,=[r for r in rows if int(r["logIndex"],16)==candidate["index"]]
        assert row["blockHash"]==block["hash"] and row["topics"]==candidate["topics"] and row["data"]==candidate["data"]
        assert len(row["transactionHash"])==66
        logs.append(deployment(row,factory["address"],factory["event"]))
    assert len({x["pool"] for x in logs})==len(logs)==final_nonce-1
    return sorted(logs,key=lambda row:(row["block"],row["index"]))


def origins(reader, factory, final_nonce):
    address = factory["address"]
    first = factory["block"]
    def nonce(height):
        return int(reader.rpc("eth_getTransactionCount", [address, hex(height)]), 16)
    if nonce(first) != 1:
        raise ValueError("Начальный nonce EulerSwap не соответствует размещению фабрики")
    ranges = [(first, 1, reader.height, final_nonce)]
    logs = []
    while ranges:
        low, left, high, right = ranges.pop()
        if left == right:
            continue
        if high-low > 1000:
            mid = (low+high)//2
            seen = nonce(mid)
            if not left <= seen <= right:
                raise ValueError("Нарушена последовательность CREATE2 фабрики EulerSwap")
            ranges.extend([(low,left,mid,seen),(mid,seen,high,right)])
            continue
        rows = reader.rpc("eth_getLogs", [{"fromBlock":hex(low+1),"toBlock":hex(high),"address":address,"topics":[factory["event"]]}])
        if len(rows) != right-left:
            raise ValueError(("EulerSwap PoolDeployed",low,high,right-left,len(rows)))
        for row in rows:
            logs.append(deployment(row,address,factory["event"]))
    assert len({log["pool"] for log in logs}) == len(logs) == final_nonce-1
    return sorted(logs,key=lambda row:(row["block"],row["index"]))

