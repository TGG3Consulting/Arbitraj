"""Реестр CREATE фабрики Stabull, условия пулов и архивные факты Trade на закреплённом Base."""
import argparse
import gzip
from functools import partial
import json
from pathlib import Path

import requests
import rlp
from eth_abi import decode, encode
from eth_utils import keccak

DATA = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"

URL = "https://base.gateway.tenderly.co"
LOG_URL = "https://base.rpc.blxrbdn.com"
FACTORY = "0x86ba17ebf8819f7fd32cf1a43abcaae541a5bebf"
ROUTER = "0x4b82759d385e905c85d1fcf7811ed33b738965a0"
MC = "0xca11bde05977b3631167028862be2a173976ca11"
TRADE = "0x" + keccak(text="Trade(address,address,address,uint256,uint256,int128)").hex()
SOURCE = "https://docs.stabull.finance/amm/contracts"
S = requests.Session()


def rpc(method, params, network="base"):
    import time
    urls = [LOG_URL] if method == "eth_getLogs" else [URL, "https://base.drpc.org", LOG_URL]
    if network == "ethereum":
        urls = ["https://ethereum-rpc.publicnode.com"] if method=="eth_getLogs" else ["https://eth.drpc.org"]
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


def batch(at, calls, network="base"):
    payload = [(address, True, encoded(sig, args)) for address, sig, args, _ in calls]
    data = keccak(text="aggregate3((address,bool,bytes)[])")[:4] + encode(["(address,bool,bytes)[]"],[payload])
    value = rpc("eth_call",[{"to":MC, "data":"0x"+data.hex()},at],network)
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


def capture(network="base"):
    factory_addr,router_addr,chain=(FACTORY,ROUTER,8453) if network=="base" else (
        "0x2e9e34b5af24b66f12721113c1c8ffcbb7bc8051","0x871af97122d08890193e8d6465015f6d9e2889b2",1)
    read=partial(rpc,network=network)
    read_many=partial(batch,network=network)
    base = json.loads((DATA / f"{network}.json").read_bytes())
    height, at = base["block"], base["selector"]
    def code(a):
        b=bytes.fromhex(read("eth_getCode",[a,at])[2:])
        return None if not b else "0x"+keccak(b).hex()
    def block(h):
        return read("eth_getBlockByNumber",[hex(h),False])
    anchor=block(height)
    assert anchor["hash"] == at["blockHash"] and int(read("eth_chainId",[]),16)==chain
    count=int(read("eth_getTransactionCount",[factory_addr,at]),16)
    addresses=["0x"+keccak(rlp.encode([bytes.fromhex(factory_addr[2:]),i]))[-20:].hex() for i in range(1,count)]
    requested=[
        ("router_factory",router_addr,"factory()",(),"address"),
        ("global_frozen",factory_addr,"getGlobalFrozenState()",(),"bool"),
        ("factory_config",factory_addr,"config()",(),"address"),
        ("protocol_fee_share_100000",factory_addr,"getProtocolFee()",(),"int128"),
        ("treasury",factory_addr,"getProtocolTreasury()",(),"address"),
    ]
    def named(fields):
        return dict(zip([f[0] for f in fields],read_many(at,[f[1:] for f in fields])))
    factory=named(requested)
    assert factory["router_factory"]==factory_addr
    factory.update(address=factory_addr, runtime=code(factory_addr), nonce=count, router=router_addr, router_runtime=code(router_addr))
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
        r.update(named([("registered",factory_addr,"getCurve(address,address)",(t0,t1),"address"),
                        ("reverse",factory_addr,"getCurve(address,address)",(t1,t0),"address")]))
        # CurveFactory регистрирует упорядоченную пару base,quote.
        if r["registered"]!=a and r["reverse"]!=a:
            raise ValueError(("Registry mismatch",a))
        fields=[]
        for j,t in enumerate([t0,t1]):
            fields.extend([(f"dp{j}",t,"decimals()",(),"uint8"),(f"balance{j}",t,"balanceOf(address)",(a,),"uint256"),
                           (f"assimilator{j}",a,"assimilator(address)",(t,),"address")])
        r.update(named(fields))
        roles=[(f"{field}{j}",r[f"assimilator{j}"],method,(),typ)
               for j in range(2) for field,method,typ in (
                   ("bound_token","token()","address"),("rate","getRate()","uint256"),("oracle","oracle()","address"))]
        r.update(named(roles))
        quotes=[]
        for j,(ip,op) in enumerate([(t0,t1),(t1,t0)]):
            for n in [1,10]:
                dx=n*10**r[f"dp{j}"];dy=n*10**r[f"dp{1-j}"]
                calls=[(a,"viewOriginSwap(address,address,uint256)",(ip,op,dx),"uint256"),
                       (a,"viewTargetSwap(address,address,uint256)",(ip,op,dy),"uint256")]
                x,y=read_many(at,calls)
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
    log_span=80000 if network=="base" else 1000
    activity_status="complete"
    for h in range(first,height+1,log_span):
        try:
            logs.extend(read("eth_getLogs",[{"address":addresses,"topics":[TRADE],"fromBlock":hex(h),"toBlock":hex(min(h+log_span-1,height))}]))
        except ValueError as error:
            if network!="ethereum" or "Archive requests require a personal token" not in str(error):
                raise
            # Без подтверждённого окна нельзя представить пустую выборку как отсутствие сделок.
            activity_status="historical event access requires an authorized account"
            logs.clear()
            break
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
        receipt=read("eth_getTransactionReceipt",[hsh]);tx=read("eth_getTransactionByHash",[hsh])
        assert receipt["status"]=="0x1" and receipt["blockHash"]==tx["blockHash"]==log["blockHash"]==block(int(log["blockNumber"],16))["hash"]
        assert any(item["logIndex"]==log["logIndex"] and item["data"]==log["data"] for item in receipt["logs"])
        receipts.append({"pool":pool,"input_token":origin,"trade_log":log,
                         "transaction":{k:tx[k] for k in ("from","to","hash","blockHash","blockNumber","value","input")},
                         "receipt":receipt})
    assert block(height)["hash"]==anchor["hash"]
    return {"chain":chain,"anchor":at,"height":height,"start":first,"start_utc":cutoff,"end_utc":int(anchor["timestamp"],16),
            "source":SOURCE,"factory":factory,"pools":list(pairs.values()),"events":len(logs) if activity_status=="complete" else None,"event_coverage":activity_status,"receipts":receipts}


if __name__=="__main__":
    args=argparse.ArgumentParser(description=__doc__); args.add_argument("--network",choices=["base","ethereum"],default="base");args.add_argument("--output",type=Path)
    opt=args.parse_args();record=capture(opt.network)
    file=opt.output or DATA/f"stabull_{opt.network}_directory.json.gz"
    raw=json.dumps(record,separators=(",",":")).encode()
    file.write_bytes(gzip.compress(raw,mtime=0) if file.suffix==".gz" else raw)
    print(record["height"],len(record["pools"]),record["events"],len(record["receipts"]))
