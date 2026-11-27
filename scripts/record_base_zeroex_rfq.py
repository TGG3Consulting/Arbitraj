"""Attribute a Base maker fill to the registered 0x Settler and to two actual transfers."""
import argparse
import json
from pathlib import Path
from eth_utils import keccak
from fork_fee_rpc import Reader
from record_base_slipstream_executor import ROOT
from record_llama_base_cl import CanonicalCalls
from zeroex_rfq_format import PATTERN, REGISTRY

TRANSFER="0x"+keccak(text="Transfer(address,address,uint256)").hex()

def amount(log):
    return int(log["data"],16)
def sender(log):
    return "0x"+log["topics"][1][-40:]
def receiver(log):
    return "0x"+log["topics"][2][-40:]

def identify(tx,receipt,settler):
    markers=[l for l in receipt["logs"] if l["address"]==settler and not l["topics"] and len(l["data"])==98]
    orders=list(PATTERN.finditer(tx["input"]))
    if len(markers)!=1 or len(orders)!=1:return None
    m,order=markers[0],orders[0]
    filled=int(m["data"][-32:],16)
    coin="0x"+(order[1] or order[3])
    if not 0<filled<=int(order[2] or order[4],16):return None
    transfers=[l for l in receipt["logs"] if len(l["topics"])==3 and l["topics"][0]==TRANSFER]
    paid=[l for l in transfers if l["address"]==coin and receiver(l)==settler and amount(l)==filled]
    if len(paid)!=1:return None
    maker=sender(paid[0])
    funded=[l for l in transfers if l["address"]!=coin and receiver(l)==maker and sender(l)==settler and amount(l)>0]
    if len(funded)!=1:return None
    return {"transaction":tx["hash"],"block":int(receipt["blockNumber"],16),"blockHash":receipt["blockHash"],
            "settler":settler,"maker":maker,"marker":m,"makerInput":funded[0],"makerOutput":paid[0],
            "matchedCalldata":order[0],"remainingEvents":len(receipt["logs"])-3}

def capture():
    head=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    reader=Reader("base",head);cap=CanonicalCalls(reader)
    deployed={str(i):cap.one(REGISTRY,"ownerOf(uint256)",["uint256"],[i],["address"])[0] for i in (2,3,4)}
    checked=set()
    witnesses=[]
    # The target is a payment witness, so stop on the first unambiguous transaction.
    for offset in range(0,24000,1200):
        end=head["block"]-offset
        logs=reader.one("eth_getLogs",[{"address":list(deployed.values()),"fromBlock":hex(end-1199),"toBlock":hex(end)}])
        for log in reversed(logs):
            h=log["transactionHash"]
            if log["topics"] or len(log["data"])!=98 or h in checked:continue
            checked.add(h)
            tx,receipt=reader.many([("eth_getTransactionByHash",[h]),("eth_getTransactionReceipt",[h])])
            if tx["blockHash"]!=receipt["blockHash"] or receipt["status"]!="0x1":continue
            proof=identify(tx,receipt,log["address"])
            if proof:
                block=reader.one("eth_getBlockByNumber",[receipt["blockNumber"],False])
                if block["hash"]!=receipt["blockHash"]:raise ValueError("replaced fill")
                tag={"blockHash":block["hash"],"requireCanonical":True}
                original=cap.selector
                try:
                    cap.selector=tag
                    feature=next(int(key) for key,value in deployed.items() if value==log["address"])
                    registered=cap.one(REGISTRY,"ownerOf(uint256)",["uint256"],[feature],["address"])[0]
                finally:cap.selector=original
                if registered!=log["address"]:continue
                proof["feature"]=feature
                witnesses.append(proof);break
            if len(checked)>=100:break
        if witnesses or len(checked)>=100:break
    return {"chain":8453,"anchor":reader.tag,"registry":REGISTRY,"features":deployed,
            "transactionsExamined":len(checked),"fromBlock":end-1199,"toBlock":head["block"],"witnesses":witnesses,
            "source":"https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/0x-rfq.ts"}
if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("output",type=Path);args=p.parse_args()
    args.output.write_bytes(json.dumps(capture(),separators=(",",":")).encode())
