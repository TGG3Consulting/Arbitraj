"""Сверяет менеджеры Ondo, принятую платёжную валюту и две исторические выплаты Ethereum."""
import gzip
import json
import sys
from collections import Counter
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
import requests

FIXTURE=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26/ondo_ethereum_settlement.json.gz"
TRANSFER="0x"+keccak(text="Transfer(address,address,uint256)").hex()

def check(url):
    doc=json.loads(gzip.decompress(FIXTURE.read_bytes()))
    def rpc(method,params):
        reply=requests.post(url,json={"id":1,"jsonrpc":"2.0","method":method,"params":params},timeout=30)
        reply.raise_for_status()
        return reply.json()["result"]
    tag=doc["selector"]
    assert rpc("eth_chainId",[])=="0x1"
    assert rpc("eth_getBlockByNumber",[hex(doc["block"]),False])["hash"]==tag["blockHash"]
    for address,proof in doc["roles"].items():
        code=bytes.fromhex(rpc("eth_getCode",[address,tag])[2:])
        assert len(code)==proof["bytes"] and "0x"+keccak(code).hex()==proof["runtimeKeccak"]
    for row in doc["reads"]:
        assert rpc("eth_call",[{"to":row["to"],"data":row["data"]},tag])==row["result"]
    start,end=doc["window"]
    events=rpc("eth_getLogs",[{"address":doc["gm"],"topics":[doc["topic"]],"fromBlock":hex(start),"toBlock":hex(end)}])
    types=["uint256","uint256","uint256","bytes32","uint8","address","uint256","uint256","uint256","bytes32"]
    sides=Counter(decode(types,bytes.fromhex(e["data"][2:]))[4] for e in events)
    assert len(events)==doc["events"] and len({e["transactionHash"] for e in events})==doc["transactions"]
    assert sides=={0:doc["buys"],1:doc["sells"]}
    for sample in doc["payments"]:
        event=sample["event"]
        tx=rpc("eth_getTransactionReceipt",[event["transactionHash"]])
        assert tx["status"]=="0x1" and (tx["from"],tx["to"],tx["blockHash"])==(sample["sender"],doc["gm"],event["blockHash"])
        assert rpc("eth_getBlockByNumber",[tx["blockNumber"],False])["hash"]==tx["blockHash"]
        assert sum(all(e.get(k)==v for k,v in event.items()) for e in tx["logs"])==1
        owner=tx["from"]; deltas=dict.fromkeys(sample["net"],0); movements=[]
        for e in tx["logs"]:
            if e["address"] in deltas and e["topics"][:1]==[TRANSFER]:
                sender,recipient=["0x"+word[-40:] for word in e["topics"][1:]]
                if owner in (sender,recipient):
                    deltas[e["address"]]+=int(e["data"],16)*((recipient==owner)-(sender==owner))
                    movements.append({k:e[k] for k in ("address","topics","data","logIndex")})
        assert {k:str(v) for k,v in deltas.items()}==sample["net"] and movements==sample["transfers"]
    return doc

if __name__=="__main__":
    for url in ("https://mainnet.gateway.tenderly.co","https://ethereum-rpc.publicnode.com","https://eth.drpc.org"):
        try:
            result=check(url)
            Path(sys.argv[1]).write_text(json.dumps(result,separators=(",",":"))+"\n",encoding="utf8")
            break
        except (requests.RequestException,KeyError) as error:
            print(type(error).__name__,flush=True)
    else:
        raise RuntimeError("Архивный ответ Ethereum недоступен")
