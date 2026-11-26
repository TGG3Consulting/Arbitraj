"""Join relay delivered payloads to every canonical header in the recorded Fermi market window."""
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
from pathlib import Path
import re

import requests

from record_fermi_ethereum import ACTIVE, OUT, ROOT, SWAP_EVENTS, digest, rpc

HEADERS = "https://ethereum-rpc.publicnode.com"
REGISTRIES = {"Titan":"https://docs.titanbuilder.xyz/builder-public-keys.md",
              "BuilderNet":"https://buildernet.org/docs/public-identity"}
RELAYS = ["https://titanrelay.xyz","https://relay.ultrasound.money","https://aestus.live",
          "https://bloxroute.regulated.blxrbdn.com","https://boost-relay.flashbots.net","https://agnostic-relay.net","https://relay.ethgas.com"]
DATA = "/relay/v1/data/bidtraces/proposer_payload_delivered"
FIRST, LAST = 26494352,26494988
OUTPUT = ROOT / "tests/data/first_run_dex_2026-11-26/fermi_builder_coverage.json"


def headers(start):
    nums = list(range(start,min(start+16,LAST+1)))
    batch=[dict(id=n, jsonrpc="2.0", method="eth_getBlockByNumber", params=[hex(n),False]) for n in nums]
    rows=requests.post(HEADERS,json=batch, timeout=30)
    rows.raise_for_status()
    records={}
    for row in rows.json():
        h=row["result"]
        n=int(h["number"],16)
        if n!=row["id"]:raise ValueError("header identity")
        records[n]=[h["hash"],h["parentHash"],int(h["timestamp"],16),h["extraData"]]
    if set(nums)!=set(records):raise ValueError("header missing")
    return records


def relay(url, first_slot,last_slot):
    cursor=last_slot
    selected=[]
    pages=0
    try:
        while cursor>=first_slot:
            r=requests.get(url+DATA,params={"cursor":cursor,"limit":100},timeout=15)
            r.raise_for_status()
            batch=r.json()
            if not isinstance(batch,list):raise ValueError("non-array")
            pages+=1
            if not batch:break
            earliest=min(int(row["slot"]) for row in batch)
            if earliest>cursor:raise ValueError("cursor did not advance")
            for v in batch:
                slot=int(v["slot"])
                if first_slot<=slot<=last_slot:
                    selected.append((int(v["block_number"]),v["block_hash"].lower(),v["builder_pubkey"].lower(),slot))
            cursor=earliest-1
            if len(batch)<100:break
        state="ok"
    except (requests.RequestException,ValueError,KeyError) as e:
        state=type(e).__name__ + (" HTTP "+str(e.response.status_code) if getattr(e,"response",None) is not None else "")
    return url,state,pages,selected


def main():
    canonical={}
    with ThreadPoolExecutor(2) as pool:
        for portion in pool.map(headers,range(FIRST,LAST+1,16)):canonical.update(portion)
    check=json.loads(OUT.read_bytes())["window"]
    if canonical[FIRST][0]!=check["first"]["hash"] or canonical[LAST][0]!=check["last"]["hash"]:
        raise ValueError("anchors changed")
    for n in range(FIRST+1,LAST+1):
        if canonical[n][1]!=canonical[n-1][0]:raise ValueError("canonical chain discontinuity")
    keys={}
    for name,url in REGISTRIES.items():
        r=requests.get(url,timeout=20);r.raise_for_status()
        keys[name]=sorted(set(v.lower() for v in re.findall(r"0x[0-9a-fA-F]{96}(?![0-9a-fA-F])",r.text)))
    if any(not value for value in keys.values()) or set(keys["Titan"]) & set(keys["BuilderNet"]):
        raise ValueError("ambiguous builder identities")
    def slot(n):return (canonical[n][2]-1606824023)//12
    by_number=defaultdict(dict)
    relays={}
    with ThreadPoolExecutor(3) as pool:
        for url,state,pages,records in pool.map(lambda u:relay(u,slot(FIRST),slot(LAST)),RELAYS):
            accepted=0
            for n,h,pubkey,s in records:
                if n in canonical and s==slot(n) and h==canonical[n][0]:
                    by_number[n][url]=pubkey
                    accepted+=1
            relays[url]=dict(status=state,pages=pages,records=len(records),canonical_matches=accepted)
    activity=defaultdict(list)
    topic=digest(SWAP_EVENTS[1].encode())
    for end in range(LAST,FIRST-1,-91):
        logs=rpc("eth_getLogs",[dict(fromBlock=hex(max(FIRST,end-90)),toBlock=hex(end),address=ACTIVE,topics=[topic])])
        for log in logs:
            n=int(log["blockNumber"],16)
            if log["blockHash"]!=canonical[n][0]:raise ValueError("swap hash changed")
            activity[n].append(log["transactionHash"])
    rows=[]
    stats=defaultdict(Counter)
    for n in range(FIRST,LAST+1):
        pubs=set(by_number[n].values())
        label=lambda pub: next((b for b,group in keys.items() if pub in group),"other")
        labels={label(pub) for pub in pubs}
        who=next(iter(labels)) if len(labels)==1 else ("ambiguous" if labels else "unattributed")
        proof=sorted([RELAYS.index(relay_url),pub] for relay_url,pub in by_number[n].items())
        rows.append([n,canonical[n][0],slot(n),who,proof,len(activity[n])])
        stats[who]["blocks"]+=1
        stats[who]["blocks_with_swaps"]+=int(bool(activity[n]))
        stats[who]["swap_events"]+=len(activity[n])
    if sum(v["swap_events"] for v in stats.values())!=check["event_counts"][ACTIVE]:raise ValueError("coverage mismatch")
    iso=lambda n:datetime.fromtimestamp(canonical[n][2],timezone.utc).isoformat()
    output=dict(chain_id=1,first=FIRST,last=LAST,utc=[iso(FIRST),iso(LAST)],slot_range=[slot(FIRST),slot(LAST)],
        canonical_blocks=len(canonical),missed_slots=slot(LAST)-slot(FIRST)+1-len(canonical),
        header_rpc=HEADERS,relay_method=DATA,relay_order=RELAYS,relays=relays,key_sources=REGISTRIES,
        published_bls_keys=keys,summary=stats,columns=["block","canonical_hash","slot","builder",
        "reports_relay_index_and_builder_pubkey","canonical_fermi_events"],blocks=rows)
    OUTPUT.write_text(json.dumps(output,separators=(",",":"),ensure_ascii=False)+"\n",encoding="utf8")
    print(dict(stats),relays)


if __name__=="__main__":
    main()
