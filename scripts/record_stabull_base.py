"""Реестр CREATE фабрики Stabull, условия пулов и архивные факты Trade на закреплённом Base."""
import gzip
import json
from pathlib import Path

import requests
import rlp
from eth_abi import decode, encode
from eth_utils import keccak

from record_llama_top_contracts import DATA

URL = "https://base.gateway.tenderly.co"
LOG_URL = "https://base.rpc.blxrbdn.com"
FACTORY = "0x86ba17ebf8819f7fd32cf1a43abcaae541a5bebf"
ROUTER = "0x4b82759d385e905c85d1fcf7811ed33b738965a0"
MC = "0xca11bde05977b3631167028862be2a173976ca11"
TRADE = "0x" + keccak(text="Trade(address,address,address,uint256,uint256,int128)").hex()
SOURCE = "https://docs.stabull.finance/amm/contracts"
S = requests.Session()


def rpc(method, params):
    import time
    urls = [LOG_URL] if method == "eth_getLogs" else [URL, "https://base.drpc.org", LOG_URL]
    for url in urls:
        for delay in [0, 1, 3]:
            if delay:
                time.sleep(delay)
            r = S.post(url, json={"jsonrpc":"2.0","id":1,"method":method,"params":params}, timeout=25)
            if r.status_code == 429:
                continue
            if r.ok:
                reply = r.json()
                if "result" in reply:
                    return reply["result"]
            break
    raise ValueError((method, str(params)[:95],r.text[:220]))


def encoded(sig, args=()):
    types = sig.split("(",1)[1][:-1]
    return keccak(text=sig)[:4] + encode(types.split(",") if types else [],args)


def batch(at, calls):
    payload = [(address, True, encoded(sig, args)) for address, sig, args, _ in calls]
    data = keccak(text="aggregate3((address,bool,bytes)[])")[:4] + encode(["(address,bool,bytes)[]"],[payload])
    value = rpc("eth_call",[{"to":MC, "data":"0x"+data.hex()},at])
    rows = decode(["(bool,bytes)[]"],bytes.fromhex(value[2:]))[0]
    assert len(rows)==len(calls)
    results=[]
    for (ok, raw), (_,_,_,out) in zip(rows,calls):
        if not ok:
            results.append(None)
        else:
            decoded=decode(out.split(","),raw)
            results.append(decoded[0] if len(decoded)==1 else decoded)
    return results


def capture():
    base = json.loads((DATA / "base.json").read_bytes())
    height, at = base["block"], base["selector"]
    def code(a):
        b=bytes.fromhex(rpc("eth_getCode",[a,at])[2:])
        return None if not b else "0x"+keccak(b).hex()
    def block(h):
        return rpc("eth_getBlockByNumber",[hex(h),False])
    anchor=block(height)
    assert anchor["hash"] == at["blockHash"] and int(rpc("eth_chainId",[]),16)==8453
    count=int(rpc("eth_getTransactionCount",[FACTORY,at]),16)
    addresses=["0x"+keccak(rlp.encode([bytes.fromhex(FACTORY[2:]),i]))[-20:].hex() for i in range(1,count)]
    requested=[
        ("router_factory",ROUTER,"factory()",(),"address"),
        ("global_frozen",FACTORY,"getGlobalFrozenState()",(),"bool"),
        ("factory_config",FACTORY,"config()",(),"address"),
        ("protocol_fee_share_100000",FACTORY,"getProtocolFee()",(),"int128"),
        ("treasury",FACTORY,"getProtocolTreasury()",(),"address"),
    ]
    def named(fields):
        return dict(zip([f[0] for f in fields],batch(at,[f[1:] for f in fields])))
    factory=named(requested)
    assert factory["router_factory"]==FACTORY
    factory.update(address=FACTORY, runtime=code(FACTORY), nonce=count, router=ROUTER, router_runtime=code(ROUTER))
    pairs={}
    for i,a in enumerate(addresses,1):
        fields=[("pool_name",a,"name()",(),"string"),("frozen",a,"frozen()",(),"bool"),
                ("emergency",a,"emergency()",(),"bool"),("liquidity",a,"liquidity()",(),"uint256,uint256[]"),
                ("curve",a,"viewCurve()",(),"uint256,uint256,uint256,uint256,uint256")]
        for j in range(2):
            fields.append((f"token{j}",a,"derivatives(uint256)",(j,),"address"))
        r=named(fields);r.update(address=a,nonce=i,runtime=code(a))
        t0,t1=r["token0"],r["token1"]
        if not (t0 and t1):
            raise ValueError(("Unknown CREATE product",a))
        r.update(named([("registered",FACTORY,"getCurve(address,address)",(t0,t1),"address"),
                        ("reverse",FACTORY,"getCurve(address,address)",(t1,t0),"address")]))
        # CurveFactory регистрирует упорядоченную пару base,quote.
        if r["registered"]!=a and r["reverse"]!=a:
            raise ValueError(("Registry mismatch",a))
        fields=[]
        for j,t in enumerate([t0,t1]):
            fields.extend([(f"dp{j}",t,"decimals()",(),"uint8"),(f"balance{j}",t,"balanceOf(address)",(a,),"uint256"),
                           (f"assimilator{j}",a,"assimilator(address)",(t,),"address")])
        r.update(named(fields))
        quotes=[]
        for j,(ip,op) in enumerate([(t0,t1),(t1,t0)]):
            for n in [1,10]:
                dx=n*10**r[f"dp{j}"];dy=n*10**r[f"dp{1-j}"]
                calls=[(a,"viewOriginSwap(address,address,uint256)",(ip,op,dx),"uint256"),
                       (a,"viewTargetSwap(address,address,uint256)",(ip,op,dy),"uint256")]
                x,y=batch(at,calls)
                quotes.append({"input":ip,"output":op,"offer":str(dx),"received":x,"want":str(dy),"cost":y})
        r["quotes"]=quotes; pairs[a]=r
    cutoff=int(anchor["timestamp"],16)-86400
    low,high=height-600000,height
    assert int(block(low)["timestamp"],16)<cutoff
    while low+1<high:
        middle=(low+high)//2
        if int(block(middle)["timestamp"],16)<cutoff:
            low=middle
        else:
            high=middle
    first=high
    logs=[]
    for h in range(first,height+1,80000):
        logs.extend(rpc("eth_getLogs",[{"address":addresses,"topics":[TRADE],"fromBlock":hex(h),"toBlock":hex(min(h+79999,height))}]))
    samples={}
    for log in logs:
        pool=log["address"].lower()
        assert pool in pairs and log["topics"][0]==TRADE and not log.get("removed")
        addr=lambda topic:"0x"+topic[-40:]
        origin,target=map(addr,log["topics"][2:4])
        assert {origin,target}=={pairs[pool]["token0"],pairs[pool]["token1"]}
        qty,got,fee=decode(["uint256","uint256","int128"],bytes.fromhex(log["data"][2:]))
        pairs[pool].setdefault("activity",{}).setdefault(origin,{"events":0,"paid_raw":0,"received_raw":0})
        agg=pairs[pool]["activity"][origin];agg["events"]+=1;agg["paid_raw"]+=qty;agg["received_raw"]+=got
        samples[(pool,origin)]=log
    receipts=[]
    for (pool,origin),log in samples.items():
        hsh=log["transactionHash"]
        receipt=rpc("eth_getTransactionReceipt",[hsh]);tx=rpc("eth_getTransactionByHash",[hsh])
        assert receipt["status"]=="0x1" and receipt["blockHash"]==tx["blockHash"]==log["blockHash"]==block(int(log["blockNumber"],16))["hash"]
        assert any(item["logIndex"]==log["logIndex"] and item["data"]==log["data"] for item in receipt["logs"])
        receipts.append({"pool":pool,"input_token":origin,"trade_log":log,
                         "transaction":{k:tx[k] for k in ("from","to","hash","blockHash","blockNumber","value","input")},
                         "receipt":receipt})
    assert block(height)["hash"]==anchor["hash"]
    return {"chain":8453,"anchor":at,"height":height,"start":first,"start_utc":cutoff,"end_utc":int(anchor["timestamp"],16),
            "source":SOURCE,"factory":factory,"pools":list(pairs.values()),"events":len(logs),"receipts":receipts}


if __name__=="__main__":
    record=capture()
    file=DATA/"stabull_base_directory.json.gz"
    file.write_bytes(gzip.compress(json.dumps(record,separators=(",",":")).encode(),mtime=0))
    print(record["height"],len(record["pools"]),record["events"],len(record["receipts"]))
