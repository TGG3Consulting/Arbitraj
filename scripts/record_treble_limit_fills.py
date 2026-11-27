"""Trace each Treble limit market to the physical Algebra pool and the transaction that filled an epoch."""
import argparse
import gzip
import json
from pathlib import Path
import time

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_base_launch_hooks import DATA


def topic(s):
    return "0x"+keccak(text=s).hex()


# Located by bisecting epochInfos(epoch).filled. Each boundary is checked on both canonical block hashes.
BOUNDARIES = {268:52529532, 3:42028259, 120:46288988, 5:42074628, 273:58413283}
ENDPOINTS = ("https://mainnet.base.org","https://base-rpc.publicnode.com","https://base.gateway.tenderly.co","https://base.drpc.org")

def capture():
    with gzip.open(DATA/"treble_limit_epochs.json.gz") as f:
        inventory=json.load(f)
    with gzip.open(DATA/"base_algebra_treble-v4.json.gz") as f:
        accepted=json.load(f)
    manager=inventory["manager"]
    session=requests.Session()
    def rpc(m,params):
        last=None
        for attempt in range(3):
            for url in ENDPOINTS:
                try:
                    result=session.post(url,json={"id":1,"jsonrpc":"2.0","method":m,"params":params},timeout=15)
                    result.raise_for_status()
                    data=result.json()
                    if data.get("result") is None:raise ValueError(str(data.get("error"))[:160])
                    return data["result"]
                except (requests.RequestException,ValueError) as err:
                    last=err
            time.sleep(2**attempt)
        raise RuntimeError(f"{m}: {last}")
    snapshot=rpc("eth_getBlockByHash",[inventory["selector"]["blockHash"],False])
    head=int(snapshot["number"],16)
    if rpc("eth_chainId",[])!="0x2105" or snapshot["stateRoot"]!=accepted["state_root"]:
        raise ValueError("mismatched Base history")
    result=[]
    for market in inventory["markets"]:
        group=[e for e in inventory["epochs"] if (e["x"],e["y"],e["deployer"])==(market["x"],market["y"],market["deployer"])]
        if market["indexer_id"]!="treble-v4" or not group:
            raise ValueError("unbound pool or foreign indexer")
        completed=[e for e in group if e["filled"]]
        row=dict(**market,epochs=len(group),filled_count=len(completed))
        result.append(row)
        if not completed:
            continue
        epoch=completed[-1]
        call={"to":manager,"data":topic("epochInfos(uint232)")[:10]+encode(["uint232"],[epoch["id"]]).hex()}
        hi=BOUNDARIES[epoch["id"]]
        header=rpc("eth_getBlockByNumber",[hex(hi),False])
        if hi > head:raise ValueError("fill after inventory")
        def filled(block_hash):
            raw=rpc("eth_call",[call,{"blockHash":block_hash,"requireCanonical":True}])
            return raw!="0x" and decode(["bool"],bytes.fromhex(raw[2:]))[0]
        if filled(header["parentHash"]) or not filled(header["hash"]):raise ValueError("epoch boundary")
        logs=rpc("eth_getLogs",[{"blockHash":header["hash"],"address":manager,
                      "topics":[topic("Fill(uint232,int24,bool)"),"0x"+encode(["uint232"],[epoch["id"]]).hex()]}])
        if len(logs)!=1:raise ValueError((epoch["id"],hi,"Fill count",len(logs)))
        event=logs[0]
        receipt=rpc("eth_getTransactionReceipt",[event["transactionHash"]])
        pool=market["pool"]
        pool_logs=[e for e in receipt["logs"] if e["address"]==pool]
        swap=topic("Swap(address,address,int256,int256,uint160,uint128,int24)")
        burn=topic("Burn(address,int24,int24,uint128,uint256,uint256)")
        fill_tick,direction=decode(["int24","bool"],bytes.fromhex(event["data"][2:]))
        burns=[e for e in pool_logs if e["topics"][0]==burn and len(e["topics"])==4 and e["topics"][1][-40:]==manager[2:]
               and int.from_bytes(bytes.fromhex(e["topics"][2][2:]),signed=True)==epoch["tick_lower"]
               and int.from_bytes(bytes.fromhex(e["topics"][3][2:]),signed=True)==epoch["tick_upper"]]
        swaps=[e for e in pool_logs if e["topics"][0]==swap]
        if receipt["blockHash"]!=header["hash"] or receipt["status"]!="0x1" or fill_tick!=epoch["tick_lower"] or not burns or not swaps:
            raise ValueError((epoch["id"],hi,"pool/manager attribution",len(burns),len(swaps),[e["topics"][0] for e in pool_logs]))
        row["settlement"]=dict(epoch=epoch["id"],block=hi,parent=header["parentHash"],block_hash=header["hash"],
                state_root=header["stateRoot"],tx=receipt["transactionHash"],direction=direction,fill=event,burns=burns,swaps=swaps)
    return {"chain":8453,"factory":inventory["factory"],"manager":manager,"selector":inventory["selector"],"markets":result}


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("out",type=Path)
    args=p.parse_args()
    data=capture()
    args.out.write_text(json.dumps(data,separators=(",",":")),encoding="utf8")
