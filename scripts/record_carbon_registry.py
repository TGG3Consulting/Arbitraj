"""Verify Carbon v6 proxy, pair storage, both native/ERC20 representations and recent on-chain strategy events."""
import argparse
import gzip
import json
from pathlib import Path
from eth_abi import decode, encode
from eth_utils import keccak
import requests
import time
from record_base_slipstream_executor import calldata

ARCHIVE=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz"

class Snapshot:
    def __init__(self,source):
        self.http=requests.Session();self.height=source["height"];self.tag=source["selector"];self.urls=[]
        for url in ["https://ethereum-rpc.publicnode.com","https://gateway.tenderly.co/public/mainnet","https://mainnet.gateway.tenderly.co","https://ethereum.drpc.org"]:
            try:
                h=self.query(url,"eth_getBlockByHash",[self.tag["blockHash"],False]); chain=self.query(url,"eth_chainId",[])
                if chain=="0x1" and h["hash"]==self.tag["blockHash"] and h["stateRoot"]==source["state_root"] and int(h["number"],16)==self.height:
                    self.header=h;self.urls.append(url)
            except (ValueError,KeyError,OSError):continue
        if not self.urls:raise ValueError("no canonical Ethereum reader")

    def query(self,url,method,params):
        r=self.http.post(url,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=18);r.raise_for_status();j=r.json()
        if "error" in j:raise ValueError(str(j["error"])[:200])
        return j["result"]

    def rpc(self,method,params):
        if method not in {"eth_call","eth_getBlockByNumber","eth_getStorageAt","eth_getCode","eth_getLogs"}:raise ValueError("unsupported reader")
        errors=[]
        for attempt in range(3):
            for url in self.urls:
                try:return self.query(url,method,params)
                except (ValueError,KeyError,OSError) as err:errors.append(str(err)[:150])
            time.sleep(2**attempt)
        raise ValueError((method,errors[-len(self.urls):]))

    def code(self,address):return bytes.fromhex(self.rpc("eth_getCode",[address,self.tag])[2:])

def capture():
    src=json.loads(gzip.decompress(ARCHIVE.read_bytes()))
    chain=Snapshot(src)
    if chain.tag!=src["selector"] or chain.header["stateRoot"]!=src["state_root"]:raise ValueError("anchor mismatch")
    ctrl,impl=src["roles"]
    def read(name,types,args):return chain.rpc("eth_call",[{"to":ctrl,"data":calldata(name,types,args)},chain.tag])
    def slot(s):return chain.rpc("eth_getStorageAt",[ctrl,hex(s),chain.tag])
    code={}
    for addr,h in zip(src["roles"],src["hashes"],strict=True):
        raw=chain.code(addr); code[addr]={"bytes":len(raw),"keccak256":"0x"+keccak(raw).hex()}
        if code[addr]["keccak256"]!=h:raise ValueError("code mismatch")
    role=slot(int.from_bytes(keccak(text="eip1967.proxy.implementation"),"big")-1)
    if role[-40:]!=impl[2:]:raise ValueError("upgraded proxy")
    reg=src["directory"]; pairs=[1,5,21,38,296,492,602,len(reg)]; rows=[]
    for pair_id in pairs:
        row=reg[pair_id-1];tokens=row["tokens"];base=int.from_bytes(keccak(encode(["uint128","uint256"],[pair_id,53])),"big")
        state=[slot(base+i) for i in range(2)]
        actual=read("pair(address,address)",["address","address"],tokens)
        reversed_=read("pair(address,address)",["address","address"],list(reversed(tokens)))
        if actual!=reversed_ or ["0x"+s[-40:] for s in state]!=tokens or decode(["(uint128,address[2])"],bytes.fromhex(actual[2:]))[0]!=(pair_id,tuple(tokens)):raise ValueError("pair binding failed")
        rows.append({"id":pair_id,"tokens":tokens,"slots":[hex(base),hex(base+1)],"words":state,"pair":actual,"reverse":reversed_,"count":row["count"]})
    order="(uint128,uint128,uint64,uint64)"; events=["StrategyCreated(uint256,address,address,address,"+order+","+order+")","StrategyUpdated(uint256,address,address,"+order+","+order+",uint8)","StrategyDeleted(uint256,address,address,address,"+order+","+order+")","TradingFeePPMUpdated(uint32,uint32)","PairTradingFeePPMUpdated(address,address,uint32,uint32)","Upgraded(address)","PairCreated(uint128,address,address)","TokensTraded(address,address,address,uint256,uint256,uint128,bool)"]
    topic=["0x"+keccak(text=x).hex() for x in events];begin=chain.height-7199;counts=dict.fromkeys(events,0);samples={};market_volume={};trade_txs=set()
    for n in range(begin,chain.height+1,450):
        logs=chain.rpc("eth_getLogs",[{"address":ctrl,"fromBlock":hex(n),"toBlock":hex(min(n+449,chain.height)),"topics":[topic]}]);
        for l in logs:
            if l.get("removed") or l["address"].lower()!=ctrl:raise ValueError("stale log")
            t=events[topic.index(l["topics"][0])];counts[t]+=1
            if t.startswith("TokensTraded("):
                a,b=("0x"+k[-40:] for k in l["topics"][2:4]);paid,out,_,_=decode(["uint256","uint256","uint128","bool"],bytes.fromhex(l["data"][2:]));pair=tuple(sorted([a,b]));rec=market_volume.setdefault(pair,[0,0,0]);rec[0]+=1;rec[1 if a==pair[0] else 2]+=paid;rec[1 if b==pair[0] else 2]+=out;trade_txs.add(l["transactionHash"])
            if t not in samples:
                h=chain.rpc("eth_getBlockByNumber",[l["blockNumber"],False])
                if h["hash"]!=l["blockHash"]:raise ValueError("hash mismatch")
                samples[t]=l
    if chain.rpc("eth_getBlockByNumber",[hex(chain.height),False])["hash"]!=chain.tag["blockHash"]:raise ValueError("anchor no longer canonical")
    return {"chain_id":1,"height":chain.height,"selector":chain.tag,"state_root":src["state_root"],"contracts":code,"implementation_slot":role,"version":src["version"],"storage_layout":"contracts/carbon/Pairs.sol:_pairsStorage slot 53","pairs":rows,"event_window":[begin,chain.height],"counts":counts,"samples":samples,"trade_txs":len(trade_txs),"markets":[{"tokens":p,"swaps":r[0],"amount0":str(r[1]),"amount1":str(r[2])} for p,r in sorted(market_volume.items())]}

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);args=p.parse_args();args.output.write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8",newline="\n")
