"""Reconstruct V3 fee registrations from genesis and verify each value on one canonical block."""
import argparse
import json
import gzip
import time
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT

TOPIC = "0x"+keccak(text="FeeAmountEnabled(uint24,int24)").hex()
SPECS = {
    "base":("base-mainnet",["https://base.drpc.org","https://base.gateway.tenderly.co"]),
    "ethereum":("ethereum-mainnet",["https://eth.drpc.org","https://rpc.mevblocker.io"]),
}
# The others use spacing lists, Algebra plugins, or a different factory/registration event.
KINDS = {"uniswap_v3","pancakeswap_v3","v3_fork","v3_fork_pancake","registered_uni_v3","registered_v3",
         "hello_v3","vvs_v3","base_native_v3"}


def ethereum_events(addresses,head):
    rows={}
    for factory in addresses:
        url="https://api.routescan.io/v2/network/mainnet/evm/1/etherscan/api"
        p=dict(module="logs",action="getLogs",address=factory,fromBlock=0,toBlock=head["block"],topic0=TOPIC,page=1,offset=100)
        records=[]
        while True:
            reply=requests.get(url,params=p,timeout=25)
            reply.raise_for_status()
            body=reply.json()
            batch=body["result"]
            if not isinstance(batch,list) or (body["status"]!="1" and body["message"] not in ("No records found","No logs found")):
                raise ValueError(("fee log archive",factory,body["message"]))
            for log in batch:
                topics=log["topics"]
                if log["address"].lower()!=factory or len(topics)!=3 or topics[0]!=TOPIC or log["data"] not in ("","0x"):
                    raise ValueError("unexpected log returned by the fee archive")
                fee,=decode(["uint24"],bytes.fromhex(topics[1][2:]))
                spacing,=decode(["int24"],bytes.fromhex(topics[2][2:]))
                records.append(dict(block=int(log["blockNumber"],16),hash=log["blockHash"],tx=log["transactionHash"],
                                    log_index=int(log["logIndex"],16),fee=fee,spacing=spacing))
            if len(batch)<p["offset"]:break
            p["page"]+=1
        rows[factory]=records
    return rows


def indexed(network,addresses,head):
    url=f"https://portal.sqd.dev/datasets/{SPECS[network][0]}/finalized-stream"
    start=0
    span=500_000
    rows={address:[] for address in addresses}
    while start<=head["block"]:
        end=min(start+span-1,head["block"])
        q=dict(type="evm",fromBlock=start,toBlock=end,
               fields={"block":{"number":True,"hash":True},"log":{"address":True,"logIndex":True,
                       "transactionHash":True,"topics":True,"data":True}},
               logs=[{"address":sorted(addresses),"topic0":[TOPIC]}])
        errors=[]
        for attempt in range(7):
            try:
                res=requests.post(url,json=q,timeout=35)
                res.raise_for_status()
                blocks=[json.loads(line) for line in res.iter_lines() if line]
                heights=[b["header"]["number"] for b in blocks]
                if not heights or not (start<=heights[0]<=heights[-1]<=end) or heights!=sorted(set(heights)):
                    raise ValueError("incomplete or unordered index page")
                break
            except (requests.RequestException,ValueError) as exc:
                errors.append(str(exc)[:80])
                # Sparse queries may still exceed a public worker's execution allowance.
                # Keep start fixed so a shorter retry cannot skip a block.
                if attempt==2 and end-start>25_000:
                    end=start+(end-start)//4
                    q["toBlock"]=end
                time.sleep(min(2**attempt,10))
        else:
            raise ValueError(("fee event index unavailable",start,end,errors))
        for b in blocks:
            for log in b.get("logs",[]):
                address=log["address"].lower()
                topics=log["topics"]
                if address not in rows or len(topics)!=3 or topics[0]!=TOPIC or log["data"]!="0x":
                    raise ValueError(("unexpected indexed log",address,topics))
                fee,=decode(["uint24"],bytes.fromhex(topics[1][2:]))
                spacing,=decode(["int24"],bytes.fromhex(topics[2][2:]))
                rows[address].append(dict(block=b["header"]["number"],hash=b["header"]["hash"],
                                          tx=log["transactionHash"],log_index=log["logIndex"],fee=fee,spacing=spacing))
        if heights[-1]==head["block"] and blocks[-1]["header"]["hash"]!=head["selector"]["blockHash"]:
            raise ValueError("indexed terminal block differs from archive")
        start=heights[-1]+1
    return rows


def rpc_read(c, method, params):
    if method not in {"eth_chainId","eth_getCode","eth_getBlockByNumber","eth_getLogs","eth_getTransactionReceipt"}:
        raise ValueError("unexpected fee archive method")
    payload = dict(jsonrpc="2.0", id=1, method=method, params=params)
    for attempt in range(5):
        reply = c.session.post(c.endpoint, json=payload, timeout=25)
        if reply.status_code != 429 or attempt == 4:
            break
        time.sleep(2 ** attempt)
    reply.raise_for_status()
    body = reply.json()
    if body.get("error"):
        raise ValueError((method, body["error"]))
    return body["result"]


def deployment_events(c, chain, factory, height):
    # A constructor can register fees even when an explorer silently omits the receipt.
    lo, hi = 0, height
    try:
        response = requests.get(f"https://sourcify.dev/server/v2/contract/{chain}/{factory}",
                                params={"fields":"deployment"},timeout=6)
        if response.ok:
            known = int(response.json()["deployment"]["blockNumber"])
            if 0 < known <= hi:
                if rpc_read(c,"eth_getCode",[factory,hex(known)])!="0x" and rpc_read(c,"eth_getCode",[factory,hex(known-1)])=="0x":
                    lo = hi = known
    except (requests.RequestException,KeyError,TypeError,ValueError):
        pass
    while lo < hi:
        middle = (lo+hi)//2
        if rpc_read(c,"eth_getCode",[factory,hex(middle)])=="0x":
            lo = middle+1
        else:
            hi = middle
    block = rpc_read(c,"eth_getBlockByNumber",[hex(lo),False])
    if rpc_read(c,"eth_getCode",[factory,{"blockHash":block["hash"],"requireCanonical":True}])=="0x":
        raise ValueError(("factory absent",factory,lo))
    query = dict(fromBlock=hex(lo), toBlock=hex(lo), address=factory, topics=[TOPIC])
    logs = rpc_read(c,"eth_getLogs",[query])
    records=[]
    for log in logs:
        t = log["topics"]
        if (log["blockHash"]!=block["hash"] or log["address"].lower()!=factory or
                len(t)!=3 or t[0]!=TOPIC or log["data"]!="0x"):
            raise ValueError(("wrong factory constructor log", factory, lo))
        fee,=decode(["uint24"],bytes.fromhex(t[1][2:]))
        spacing,=decode(["int24"],bytes.fromhex(t[2][2:]))
        records.append(dict(block=lo,hash=block["hash"],tx=log["transactionHash"],
                            log_index=int(log["logIndex"],16),fee=fee,spacing=spacing))
    return lo, records


def candidate_fees(catalog,fixtures):
    candidates = {int(f) for p in catalog for f in p["settings"].get("dex.v3_fee_tiers",[])}
    for network in SPECS:
        file = fixtures / ("v3_fee_events_"+network+".json.gz")
        if file.exists():
            with gzip.open(file, "rt") as stream:
                for row in json.load(stream)["factories"].values():
                    candidates.update(map(int,row["tiers"]))
    return candidates


def record(network, full_index=False):
    fixtures=ROOT/"tests/data/first_run_dex_2026-11-26"
    head=json.loads((fixtures/(network+".json")).read_bytes())
    catalog=json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    factories={}
    for p in catalog:
        s=p["settings"]
        if p["network"]==network and s["dex.kind"] in KINDS and s.get("dex.factory"):
            factories.setdefault(s["dex.factory"].lower(),[]).append(p["id"])
    if not factories:
        raise ValueError("empty factory selection")
    coverage = "constructor receipts and registered fee getter at pinned block"
    events = {address: [] for address in factories}
    if network=="ethereum":
        events=ethereum_events(factories,head)
        coverage += "; historical explorer logs checked against original receipts"
    elif full_index:
        events=indexed(network,factories,head)
        coverage += "; historical SQD logs checked against original receipts"
    probes = candidate_fees(catalog,fixtures)
    errors=[]
    for url in SPECS[network][1]:
        try:
            c=Capture(dict(head,endpoint=url))
            if int(rpc_read(c,"eth_chainId",[]),16)!=head["chain_id"] or rpc_read(c,"eth_getBlockByNumber",[hex(head["block"]),False])["hash"]!=head["selector"]["blockHash"]:
                raise ValueError("incorrect network or canonical block")
            verified={}
            for factory,historical in events.items():
                creation, original = deployment_events(c,head["chain_id"],factory,head["block"])
                unique = {(r["tx"],r["log_index"]):r for r in historical}
                for row in original:
                    key = row["tx"],row["log_index"]
                    if key in unique and unique[key]!=row:
                        raise ValueError(("explorer/constructor disagree",key))
                    unique[key] = row
                records = sorted(unique.values(),key=lambda r:(r["block"],r["log_index"]))
                tiers={r["fee"]:r["spacing"] for r in records}
                if len(tiers)!=len(records) or any(d<=0 for d in tiers.values()):
                    raise ValueError(("ambiguous fee events",factory))
                candidates = sorted(set(tiers) | probes)
                states=posts(c,[(factory,"feeAmountTickSpacing(uint24)",["uint24"],[fee]) for fee in candidates])
                observed = {fee: decode(["int24"],buf)[0] for fee,buf in zip(candidates,states,strict=True)}
                if any(observed[fee]!=space for fee,space in tiers.items()):
                    raise ValueError(("registry/chain disagreement",factory))
                for record in records:
                    receipt=rpc_read(c,"eth_getTransactionReceipt",[record["tx"]])
                    hits=[l for l in receipt["logs"] if int(l["logIndex"],16)==record["log_index"] and l["address"].lower()==factory]
                    if receipt["blockHash"]!=record["hash"] or int(receipt["blockNumber"],16)!=record["block"] or len(hits)!=1:
                        raise ValueError(("foreign fee event",factory,record["tx"]))
                    h=hits[0]["topics"]
                    if hits[0]["data"]!="0x" or len(h)!=3 or h[0]!=TOPIC or int(h[1],16)!=record["fee"] or int(h[2],16)!=record["spacing"]:
                        raise ValueError(("wrong original log",factory,record["tx"]))
                verified[factory]={"profiles":factories[factory],"creation_block":creation,
                                   "events":records,"tiers":{fee:delta for fee,delta in observed.items() if delta>0},
                                   "getter_only":[fee for fee,delta in observed.items() if delta>0 and fee not in tiers]}
            break
        except (requests.RequestException,ValueError,RuntimeError,KeyError) as exc:
            errors.append((url,str(exc)[:200]))
    else:
        raise ValueError(errors)
    return dict(network=network,chain_id=head["chain_id"],block=head["block"],selector=head["selector"],
                event=TOPIC,coverage=coverage,probed_fees=sorted(probes),factories=verified)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("network",choices=SPECS)
    p.add_argument("output",type=Path)
    p.add_argument("--full-index",action="store_true",help="Also scan historical Base registration events via the public SQD archive")
    args=p.parse_args()
    args.output.write_bytes(json.dumps(record(args.network,args.full_index),separators=(",",":")).encode())
