"""Reconstruct V3 fee registrations from genesis and verify each value on one canonical block."""
import argparse
import json
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


def indexed(network,addresses,head):
    url=f"https://portal.sqd.dev/datasets/{SPECS[network][0]}/finalized-stream"
    start=0
    span=1_000_000
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


def record(network):
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
    events=indexed(network,factories,head)
    errors=[]
    for url in SPECS[network][1]:
        try:
            c=Capture(dict(head,endpoint=url))
            verified={}
            for factory,records in events.items():
                tiers={r["fee"]:r["spacing"] for r in records}
                if not tiers:
                    verified[factory]={"profiles":factories[factory],"events":[],"tiers":{}}
                    continue
                if len(tiers)!=len(records) or any(f<=0 or d<=0 for f,d in tiers.items()):
                    raise ValueError(("ambiguous fee events",factory))
                states=posts(c,[(factory,"feeAmountTickSpacing(uint24)",["uint24"],[fee]) for fee in tiers])
                if [decode(["int24"],row)[0] for row in states]!=list(tiers.values()):
                    raise ValueError(("registry/chain disagreement",factory))
                verified[factory]={"profiles":factories[factory],"events":records,"tiers":tiers}
            break
        except (requests.RequestException,ValueError,RuntimeError,KeyError) as exc:
            errors.append((url,str(exc)[:200]))
    else:
        raise ValueError(errors)
    return dict(network=network,chain_id=head["chain_id"],block=head["block"],selector=head["selector"],
                event=TOPIC,from_block=0,factories=verified)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("network",choices=SPECS)
    p.add_argument("output",type=Path)
    args=p.parse_args()
    args.output.write_bytes(json.dumps(record(args.network),separators=(",",":")).encode())
