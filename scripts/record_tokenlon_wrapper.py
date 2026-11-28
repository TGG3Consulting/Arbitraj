"""Replay one actual Tokenlon wrapper fill per underlying DEX on its canonical parent without state overrides."""
import json
from pathlib import Path
import sys

from eth_abi import decode
from eth_utils import keccak

from record_tokenlon_ethereum import rpc, calldata, MULTICALL, SLOT
from verify_tokenlon_sources import load, MAKERS, TOPICS, SWAP1, SWAP2, QUOTE, ORDER, LEGACY_WRAPPER

NATIVE="0x"+"00"*20

def flow(entry):
    data=bytes.fromhex(entry["data"][2:])
    topic=TOPICS[entry["topics"][0]]
    if topic==SWAP2:
        quote,order=decode([QUOTE,ORDER],data)
        source,router,src,dst,amount,user,recipient,net=quote[0],order[0],order[1],order[2],order[3],order[5],order[6],quote[2]
    else:
        assert topic==SWAP1
        row=decode(LEGACY_WRAPPER,data)
        source,router,src,dst,amount=row[0],row[3],row[1],row[4],row[2]
        user,recipient,net="0x"+entry["topics"][2][-40:],row[6],row[7]
    assert MAKERS[source]==router
    return source,src,dst,int(amount),user,recipient,int(net)

def balance(token,account):
    abi="getEthBalance(address)" if token==NATIVE else "balanceOf(address)"
    return {"gas":"0x186a0","to":MULTICALL if token==NATIVE else token,"data":calldata(abi,["address"],[account])}

def replay(entry,roles):
    label,src,dst,debit,user,recipient,credit=flow(entry)
    txid=entry["transactionHash"]
    tx=rpc("eth_getTransactionByHash",[txid])
    receipt=rpc("eth_getTransactionReceipt",[txid])
    header=rpc("eth_getBlockByHash",[entry["blockHash"],False])
    assert header["hash"]==tx["blockHash"]==receipt["blockHash"]==entry["blockHash"]
    assert header["transactions"][int(receipt["transactionIndex"],16)]==txid==tx["hash"]==receipt["transactionHash"]
    assert entry["blockNumber"]==header["number"] and receipt["status"]=="0x1"
    assert tx["to"].lower()==roles["user_proxy"]["address"]
    assert tx["input"][:10]=="0x"+keccak(text="toAMM(bytes)")[:4].hex()
    def own(log):
        return log["address"].lower()==entry["address"] and log["topics"][:1]==entry["topics"][:1]
    canonical,=[log for log in receipt["logs"] if own(log) and log["logIndex"]==entry["logIndex"]]
    assert canonical["topics"]==entry["topics"] and canonical["data"]==entry["data"]
    tag={"blockHash":header["parentHash"],"requireCanonical":True}
    for key in ["user_proxy","user_proxy_impl","wrapper"]:
        role=roles[key]
        assert "0x"+keccak(hexstr=rpc("eth_getCode",[role["address"],tag])).hex()==role["runtime"]
    assert rpc("eth_getStorageAt",[tx["to"],SLOT,tag])[-40:]==roles["user_proxy_impl"]["address"][2:]
    payer=tx["from"].lower() if src==NATIVE else user
    reads=[balance(src,payer),balance(dst,recipient)]
    swap={k:tx[k] for k in ["from","to","value","gas"]};swap["data"]=tx["input"]
    calls=[*reads,swap,*reads]
    trial,=rpc("eth_simulateV1",[{"blockStateCalls":[{"blockOverrides":{"time":header["timestamp"]},"calls":calls}]},tag])
    assert trial["parentHash"]==tag["blockHash"] and trial["timestamp"]==header["timestamp"]
    result=trial["calls"]
    out={"source":label,"router":MAKERS[label],"hash":txid,"block":header["hash"],"parent":tag["blockHash"],
         "input":src,"output":dst,"payer":payer,"recipient":recipient,"original_debit":str(debit),"original_credit":str(credit)}
    if result[2]["status"]!="0x1":
        return {**out,"outcome":"reverted on canonical parent","reason":result[2].get("error",{}).get("message","")[:100]}
    def amounts(pair):
        assert all(c["status"]=="0x1" and len(c["returnData"])==66 for c in pair)
        return [int(c["returnData"],16) for c in pair]
    before,after=amounts(result[:2]),amounts(result[3:])
    paid=before[0]-after[0];received=after[1]-before[1]
    emitted,=[flow(log) for log in result[2]["logs"] if own(log)]
    assert emitted[:6]==(label,src,dst,debit,user,recipient)
    assert paid==debit and 0<received==emitted[6]
    return {**out,"outcome":"paid","debit":str(paid),"credit":str(received),"matches_original_credit":received==credit,
            "gas":int(result[2]["gasUsed"],16)}

if __name__=="__main__":
    market=load("tokenlon_ethereum.json")
    assert int(rpc("eth_chainId",[]),16)==1
    raw=load("tokenlon_ethereum_source_events.json.gz")["logs"]["wrapper"]
    first={}
    for row in raw:
        first.setdefault(flow(row)[0],row)
    assert set(first)==MAKERS.keys()
    payments=[replay(row,market["roles"]) for row in first.values()]
    Path(sys.argv[1]).write_text(json.dumps({"samples":payments},indent=2))
