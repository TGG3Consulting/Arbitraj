"""Кассы Kaliber Base, журнал полных суток и исходные денежные события исторического дня."""
import argparse
import json
from collections import Counter
from pathlib import Path
from eth_utils import keccak
from eth_abi import decode
from record_eulerswap_directory import Reader

VAULTS=["0x5ebcee186821704aeac480d51b539ba4ede1fdf9","0xf639cf213b63f7e77d699ff686d591c0ba55fc63"]
LAMBDA_ETH="0x4ddf368080cd7946db5b459ad591c350158175e1"
EVENT="0x"+keccak(text="Swap(bytes32,address,address,address,uint256,uint256,uint256)").hex()
TRANSFER="0x"+keccak(text="Transfer(address,address,uint256)").hex()
RANGES=[("canonical",61830757,62262756,1795556526,1795642926),("2026-10-08",52314127,52357326,1791417600,1791503999)]

def capture():
    r=Reader("base")
    def head(i):return r.rpc("eth_getBlockByNumber",[hex(i),False])
    roles={a:r.code_hash(a) for a in VAULTS}
    assert r.rpc("eth_getCode",[LAMBDA_ETH,r.tag])=="0x"
    windows=[]
    for name,start,end,first,final in RANGES:
        threshold=first+(name=="canonical")
        # L2 после смены интервала может поместить несколько последовательных блоков в одну секунду.
        for _ in range(20):
            if int(head(start-1)["timestamp"],16)>=threshold:start-=1
            elif int(head(start)["timestamp"],16)<threshold:start+=1
            else:break
        else:raise ValueError("Граница первых суток не подтверждена")
        if name!="canonical":
            for _ in range(20):
                if int(head(end+1)["timestamp"],16)<=final:end+=1
                elif int(head(end)["timestamp"],16)>final:end-=1
                else:break
            else:raise ValueError("Граница последних суток не подтверждена")
        lower,upper=head(start),head(end)
        assert int(lower["timestamp"],16)>=threshold and int(upper["timestamp"],16)<=final
        if name=="canonical":assert upper["hash"]==r.tag["blockHash"]
        events=[]
        for i in range(start,end+1,10000):
            events+=r.rpc("eth_getLogs",[{"address":VAULTS,"fromBlock":hex(i),"toBlock":hex(min(i+9999,end)),"topics":[EVENT]}])
        txs={};quotes=Counter();pairs={}
        for e in events:
            src,dst,amount,received,timing=decode(["address","address","uint256","uint256","uint256"],bytes.fromhex(e["data"][2:]))
            market=e["topics"][1];trader="0x"+e["topics"][2][-40:];vault=e["address"]
            agg=(vault,market,src,dst);quotes[agg]+=1;pairs[agg]=(amount,received)
            txid=e["transactionHash"]
            if txid in txs:continue
            tx=r.rpc("eth_getTransactionByHash",[txid]);rec=r.rpc("eth_getTransactionReceipt",[txid])
            assert tx["blockHash"]==rec["blockHash"]==e["blockHash"] and rec["status"]=="0x1"
            transfers=[]
            for l in rec["logs"]:
                h=l["topics"]
                if len(h)!=3 or h[0]!=TRANSFER or l["address"] not in (src,dst):continue
                transfers.append(dict(token=l["address"],sender="0x"+h[1][-40:],recipient="0x"+h[2][-40:],amount=int(l["data"],16),index=int(l["logIndex"],16)))
            txs[txid]=dict(block=e["blockHash"],height=int(e["blockNumber"],16),index=int(tx["transactionIndex"],16),sender=tx["from"],
                           target=tx["to"],selector=tx["input"][:10],cash=vault,trader=trader,logs=transfers)
        windows.append(dict(name=name,start=start,end=end,first=lower["hash"],last=upper["hash"],events=events,receipts=txs,
                            pairs=[dict(vault=k[0],market=k[1],src=k[2],dst=k[3],count=v,last_debit=pairs[k][0],last_credit=pairs[k][1]) for k,v in quotes.items()]))
    return dict(chain_id=8453,selector=r.tag,roles=roles,lambda_eth=LAMBDA_ETH,windows=windows)
if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path)
    ap.parse_args().output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

