"""Реестры двух опубликованных подграфов Treble на каноническом Base блоке.
Nonce отдельного AlgebraPoolDeployer считает успешные создания, включая CustomPool.
Диапазоны с одинаковым счётчиком пропускаются. Каждая запись подтверждается getter фабрики.
"""
import argparse
import gzip
import json
import os
import time
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT

DEPLOYMENTS = {
    "5659": ("0xac900f12fb25d514e3ccfe8572b153a9991ca4e7", 25118579,
             "8rV9EcKW8J8u6rt7t9vdCf2gCieiaEM1TiKWYxVTxa4i", "QmVnzA9qydbobhWe62qZhvBdy9G3j7VU9hkVqb98dLjG3R"),
    "7587": ("0x6e606cf94a4ddc01aeed2fce16d1b4f5b33e0a31", 39029383,
             "3sThy2UsWd9X3D2M6MUQWzNUYrs8snMMhQKHSg9kUEAd", "QmdkprafsPH2jahhaacyVdqUGuGTxvQ2jnDE9cQUVHAgCK"),
}
LOGS = "https://base.api.pocket.network"
STATE = "https://base.drpc.org"
TOPICS = ["0x" + keccak(text=m).hex() for m in ("Pool(address,address,address)", "CustomPool(address,address,address,address)")]
PAGE = 9000
ZERO = "0x" + "00" * 20
MC = "0xca11bde05977b3631167028862be2a173976ca11"
# Узел Base из секрета Actions BASE_ARCHIVE_RPC (если задан и не пуст) — первым; LOGS/STATE остаются запасными.
ARCHIVE = os.environ.get("BASE_ARCHIVE_RPC", "").strip()
SKIP = set()  # методы, на которых узел из переменной уже отказал: дальше сразу прежний узел


def hide(text):
    """Принимает текст вывода; возвращает его с адресом из BASE_ARCHIVE_RPC, заменённым меткой. Ошибок нет."""
    return text.replace(ARCHIVE, "BASE_ARCHIVE_RPC") if ARCHIVE else text

def call(sig, types=(), values=()):
    return "0x" + (keccak(text=sig)[:4] + encode(types,values)).hex()

def batch(session, method, args):
    output = []
    size = 1 if method == "eth_getLogs" else 3
    for start in range(0,len(args),size):
        group = args[start:start+size]
        payload = [dict(id=i,jsonrpc="2.0",method=method,params=p) for i,p in enumerate(group)]
        time.sleep(0.5)
        primary = LOGS if method == 'eth_getLogs' else STATE
        for endpoint in [ARCHIVE]*(bool(ARCHIVE) and method not in SKIP)+[primary]:
            resp = None
            try:
                resp = session.post(endpoint,json=payload[0] if size==1 else payload,timeout=22)
                resp.raise_for_status()
                body = resp.json()
                replies = body if isinstance(body,list) else [body]
                if endpoint != primary and any("error" in row for row in replies): raise ValueError(replies)
                break
            except (requests.RequestException, ValueError) as exc:
                if endpoint == primary: raise
                SKIP.add(method)
                print("BASE_ARCHIVE_RPC", method, hide(str(exc))[:300], hide(resp.text if resp is not None else "")[:300], flush=True)
        rows = {row["id"]:row for row in replies}
        rejected = next((row["error"] for row in replies if "error" in row), None)
        if rejected: raise ValueError((method,start,rejected))
        output.extend(rows[i]["result"] for i in range(len(group)))
    return output

def views(session,tag,requests_):
    results=[]
    for i in range(0,len(requests_),45):
        pack=requests_[i:i+45]
        aggregate=call("aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],
                       [[(a,False,bytes.fromhex(data[2:])) for a,data in pack]])
        raw,=batch(session,"eth_call",[[{"to":MC,"data":aggregate},tag]])
        pairs,=decode(["(bool,bytes)[]"],bytes.fromhex(raw[2:]))
        if len(pairs)!=len(pack) or not all(ok for ok,_ in pairs):raise ValueError("неполное чтение рынка")
        results.extend(data for _,data in pairs)
    return results

def address(raw):
    return decode(["address"],raw)[0]

def registry(session, tag, head, spec):
    factory, first, graph, cid = spec
    url="https://ipfs.network.thegraph.com/ipfs/"+cid
    manifest = session.get(url,timeout=18)
    manifest.raise_for_status()
    if factory not in manifest.text.lower() or f"startBlock: {first}" not in manifest.text:
        raise ValueError("не совпала привязка опубликованного подграфа")
    deployer=address(views(session,tag,[(factory,call("poolDeployer()"))])[0])
    def counts(heights):
        nums=batch(session,"eth_getTransactionCount",[[deployer,tag if h==head else hex(h)] for h in heights])
        return [max(0,int(n,16)-1) for n in nums]
    [head_count]=counts([head])
    key=os.environ.get("TREBLE_GRAPH_API_KEY")
    hints=None
    if key:
        from treble_index import placements
        hints=placements(session,graph,key,tag["blockHash"])
        if len(hints)!=head_count or any(not first<=h[0]<=head for h in hints.values()):
            raise ValueError("Число размещений подграфа отличается от канонического CREATE")
        created={}
        for h,_,_ in hints.values():created[h]=created.get(h,0)+1
        pages=[(h-1,h) for h in sorted(created)]
        expected={(h-1,h):n for h,n in created.items()}
    else:
        bounds=[first-1,*range(first+500000,head,500000),head]
        at=dict(zip(bounds,counts(bounds),strict=True))
        if at[first-1]:raise ValueError("до первого события уже есть рынки")
        pending=[(a,b) for a,b in zip(bounds,bounds[1:]) if at[a]!=at[b]]
        pages=[]
        while pending:
            long=[]
            for a,b in pending:
                if b-a<=PAGE: pages.append((a,b))
                else:long.append((a,b))
            mid=[(a+b)//2 for a,b in long]
            at.update(zip(mid,counts(mid),strict=True))
            pending=[(a,b) for (lo,hi),m in zip(long,mid,strict=True) for a,b in [(lo,m),(m,hi)] if at[a]!=at[b]]
        expected={(a,b):at[b]-at[a] for a,b in pages}
    filters=[[dict(fromBlock=hex(a+1),toBlock=hex(b),address=factory,topics=[TOPICS])] for a,b in pages]
    records=[]
    for (a,b),logs in zip(pages,batch(session,"eth_getLogs",filters),strict=True):
        if len(logs)!=expected[(a,b)]:raise ValueError(("CREATE2 / события",a,b,expected[(a,b)],len(logs)))
        for log in logs:
            ts=log["topics"];custom=ts[0]==TOPICS[1]
            if len(ts)!=3+int(custom) or log["address"].lower()!=factory or log.get("removed"):
                raise ValueError("неожиданное событие рынка")
            x,y=["0x"+t[-40:] for t in ts[-2:]]
            records.append(dict(pool=address(bytes.fromhex(log["data"][2:])),tokens=[x,y],
                                deployer="0x"+ts[1][-40:] if custom else ZERO,block=int(log["blockNumber"],16),
                                hash=log["blockHash"],tx=log["transactionHash"],log=int(log["logIndex"],16)))
    if hints is not None and any(hints.get(r["pool"])!=(r["block"],r["deployer"],r["tokens"]) for r in records):
        raise ValueError("Размещение подграфа не совпало с исходным событием фабрики")
    if len({r["pool"] for r in records})!=head_count:raise ValueError("неполный реестр размещений")
    requests_=[]
    for r in records:
        x,y=r["tokens"];d=r["deployer"];p=r["pool"]
        getter=call("poolByPair(address,address)",["address"]*2,[x,y]) if d==ZERO else call(
            "customPoolByPair(address,address,address)",["address"]*3,[d,x,y])
        requests_.extend([(factory,getter)]+[(p,call(sig)) for sig in ("factory()","token0()","token1()","plugin()","liquidity()","globalState()")])
    checked=iter(views(session,tag,requests_))
    for r in records:
        pool,owner,x,y,plugin=[address(next(checked)) for _ in range(5)]
        if (pool,owner,[x,y])!=(r["pool"],factory,r["tokens"]):raise ValueError("ссылка расходится с реестром")
        r["plugin"]=plugin;r["liquidity"]=str(decode(["uint128"],next(checked))[0])
        r["state"]=list(decode(["uint160","int24","uint16","uint8","uint16","bool"],next(checked)))
    return dict(factory=factory,pool_deployer=deployer,subgraph=graph,manifest=cid,
                from_block=first,creations=head_count,rows=sorted(records,key=lambda r:(r["block"],r["log"])))

def collect():
    snapshot=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    tag=snapshot["selector"];head=snapshot["block"];session=requests.Session()
    if int(batch(session,"eth_chainId",[[]])[0],16)!=8453:raise ValueError("ожидается Base")
    header=batch(session,"eth_getBlockByNumber",[[hex(head),False]])[0]
    if header["hash"]!=tag["blockHash"]:raise ValueError("изменился архивный блок")
    return dict(chain=8453,block=head,selector=tag,markets={k:registry(session,tag,head,v) for k,v in DEPLOYMENTS.items()})

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("out",type=Path)
    args=parser.parse_args()
    raw = json.dumps(collect(),separators=(",",":")).encode()
    args.out.write_bytes(gzip.compress(raw,mtime=0) if args.out.suffix == '.gz' else raw)

