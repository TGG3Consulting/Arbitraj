"""Все опубликованные на Ethereum поколения Raindex. Обнаружение по AddOrder;
orderExists, денежный vault и собственный quote повторно сверяются с каноническим H.
"""
import argparse
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import requests
from eth_abi import decode, encode
from eth_utils import event_abi_to_log_topic, keccak
from eth_utils.abi import collapse_if_tuple
from record_raindex_quotes import DATA, PAYER, data

SHA="0352173773372ab0b8f3f779586bec446d4eca99"
GITHUB="https://raw.githubusercontent.com/rainlanguage/raindex/"+SHA+"/"
ETHRPC=["https://eth.drpc.org","https://mainnet.gateway.tenderly.co","https://ethereum.public.blockpi.network/v1/rpc/public"]
LEGACY="https://sourcify.dev/server/v2/contract/1/"


def get(url,**kwargs):
    r=requests.get(url,timeout=30,**kwargs);r.raise_for_status();return r.json()


def rpc(method,args):
    if method not in {"eth_call","eth_getCode","eth_getBlockByNumber","eth_chainId"}:raise ValueError(method)
    why=None
    for url in ETHRPC:
        try:
            ans=requests.post(url,json={"jsonrpc":"2.0","id":1,"method":method,"params":args},timeout=15).json()
            if "result" in ans or "revert" in ans.get("error",{}).get("message","").lower():
                return ans
            why=ans.get("error")
        except requests.RequestException as e:why=str(e)
    raise ValueError((method,why))


def capture(history=None):
    net=json.loads((DATA/"ethereum.json").read_bytes());tag=net["selector"];height=net["block"]
    if int(rpc("eth_chainId",[])["result"],16)!=1 or rpc("eth_getBlockByNumber",[hex(height),False])["result"]["hash"]!=tag["blockHash"]:
        raise ValueError("Ethereum anchor")
    manifest=get(GITHUB+"deployments/raindex.json")
    def market(meta):
        book=meta["address"].lower();version=meta["interfaceVersion"]
        abi=(get(GITHUB+f"subgraph/v{version}/abis/Raindex.json") if version>=6 else get(LEGACY+book,params={"fields":"abi"})["abi"])
        if isinstance(abi,dict):abi=abi["abi"]
        sigs={x["name"]:x for x in abi if x.get("name")}
        events={"0x"+event_abi_to_log_topic(x).hex():x for x in abi if x["type"]=="event"}
        cursor={};events_seen={};seeds={};seen=set()
        url=f"https://eth.blockscout.com/api/v2/addresses/{book}/logs"
        while True:
            page=({"items":json.loads(gzip.decompress((history/f"ethereum-v{version}-history.json.gz").read_bytes())),"next_page_params":None}
                  if history else get(url,params=cursor))
            for row in page["items"]:
                if not meta["startBlock"]<=row["block_number"]<=height:continue
                key=(row["block_hash"],row["index"])
                if key in seen:raise ValueError("duplicate log")
                seen.add(key)
                event=events.get(row["topics"][0]);name=event["name"] if event else row["topics"][0]
                events_seen[name]=events_seen.get(name,0)+1
                if not name.startswith("AddOrder"):continue
                args=event["inputs"];values=decode([collapse_if_tuple(x) for x in args],bytes.fromhex(row["data"][2:]))
                fields={x["name"]:(collapse_if_tuple(x),v) for x,v in zip(args,values)}
                ty,o=fields["order"];raw=encode([ty],[o]);digest=fields["orderHash"][1]
                if keccak(raw)!=digest:raise ValueError("hash")
                seeds[digest]=(o,raw,row["transaction_hash"])
            nxt=page["next_page_params"]
            if not nxt:break
            if nxt==cursor:raise ValueError("non-advancing pagination")
            cursor=nxt
        def call(name,args):
            t=[collapse_if_tuple(x) for x in sigs[name]["inputs"]]
            return rpc("eth_call",[{"from":PAYER,"to":book,"data":data(name,t,args)},tag])
        def check(kv):
            h,(o,blob,origin)=kv
            if not decode(["bool"],bytes.fromhex(call("orderExists",[h])["result"][2:]))[0]:return None
            ins,outs=o[3:5] if version==3 else o[2:4]
            legs=[]
            for j,out in enumerate(outs):
                cash=call("vaultBalance2" if version>=5 else "vaultBalance",[o[0],out[0],out[-1]])["result"]
                if not int(cash,16) & ((1<<224)-1 if version>=5 else (1<<256)-1):continue
                for i,src in enumerate(ins):
                    if src[0]==out[0]:continue
                    rec={"i":i,"j":j,"pay":src[0],"receive":out[0],"vault":cash}
                    if version!=3:
                        name={4:"quote",6:"quote2",7:"quote3"}[version]
                        q=call(name,[(o,i,j,[])])
                        if "result" in q:
                            types=[collapse_if_tuple(f) for f in sigs[name]["outputs"]]
                            live,cap,ratio=decode(types,bytes.fromhex(q["result"][2:]))
                            rec.update(live=live,cap=hex(cap) if isinstance(cap,int) else "0x"+cap.hex(),ratio=hex(ratio) if isinstance(ratio,int) else "0x"+ratio.hex())
                        else:rec["revert"]=(str(q["error"].get("data",""))[:10],q["error"].get("message","")[:100])
                    legs.append(rec)
            return {"hash":"0x"+h.hex(),"order":"0x"+blob.hex(),"owner":o[0],"added":origin,"legs":legs}
        with ThreadPoolExecutor(3) as pool:orders=[x for x in pool.map(check,seeds.items()) if x]
        code=bytes.fromhex(rpc("eth_getCode",[book,tag])["result"][2:]);return {"book":book,"version":version,"start":meta["startBlock"],"runtime":"0x"+keccak(code).hex(),"events":events_seen,"added":len(seeds),"live":orders}
    return {"anchor":tag,"height":height,"payer":PAYER,"manifest":GITHUB+"deployments/raindex.json","books":[market(m) for m in manifest if m["network"]=="ethereum" and m["startBlock"]<=height]}


if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("out",type=Path);p.add_argument("--history",type=Path);a=p.parse_args()
    a.out.write_bytes(gzip.compress(json.dumps(capture(a.history),separators=(",",":")).encode(),mtime=0))
